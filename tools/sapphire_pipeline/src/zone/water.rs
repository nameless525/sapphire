//! A zone's drawn water (M6 W3a): each lake's and river's surface, its mesh and its info texture, cooked with the
//! zone into a `.swater` of its own (`town.water.<id>.swater`), which the `.szone` names outside its digest.
//! [`sapphire_zone::water::drawn`] says what a mesh and an info texture hold; this module makes them.
//!
//! # Per body
//!
//! A body is one asset whatever cells it spans, as it's one draw: a lake's outline and a river's ribbon are
//! single shapes the renderer culls by their bounds, an info texture is read across the whole of one with no
//! seam, and a zone holds at most sixteen. The sea has none: its grid follows the camera and its info texture
//! covers the coast (H1b).
//!
//! Its bed is the document's `World` colliders ([`ColliderGround`]), not the set G1b joins its trunks into: a
//! tree that a foliage rule stands in a lake, one that doesn't keep off water, reads as water, with no shore
//! round it.
//!
//! # Sizes
//!
//! Each cook has its own (the desktop cook's for Desktop-High and Desktop-Low, the mobile cook's for the phones):
//!
//! - **The mesh's square** is [`spacing`] (2 m, 4 m on mobile), doubled until the body has at most
//!   [`MAX_QUADS`] of them: a lake's squares over its drawn bounds, a river's sections times its columns; then,
//!   while the zone's meshes come to more than [`mesh_budget`] (12 MiB, 3 MiB on mobile) at [`QUAD_BYTES`] a
//!   square, the body with the most squares (the first of them on a tie) doubles its own.
//! - **The info texture's texel** is [`texel`] (1 m, 2 m on mobile), doubled until neither side passes
//!   [`MAX_INFO_SIDE`] texels; then, while the zone's info textures together pass [`info_budget`] (spec §3.4's
//!   4 MiB, 1 MiB on mobile), the body with the most texels (the first of them on a tie) doubles its own. So a
//!   body's sizes are the zone's shapes' to say, before anything cooks, and each body keys alone.
//!
//! # Determinism
//!
//! Every byte comes from `f64` sums, products, quotients and square roots of `f32` values (each correctly
//! rounded), comparisons, `floor` and `round`, and integer arithmetic: a grown outline's corners and the
//! ribbon's frames are those sums and roots; the triangulation and the grid's cut decide on orientations of
//! `f32` points, which `f64` computes exactly; the bed is [`ColliderGround`]'s sums and products; the shore's
//! distance is an exact integer distance transform (Meijster, Roerdink and Hesselink's) and one root. No
//! transcendental, no hash order, no thread count: the same bytes on every machine.
//!
//! # Refusals
//!
//! A body the cook can't draw fails the zone's cook by its field: a lake whose outline grown by its margin
//! meets itself (`water[i].shore_margin`), a river that turns back on itself or turns so tightly for its width
//! that its inner bank folds over its stretch (`water[i].shape.points[j]`).

use std::collections::BTreeMap;

use sapphire_zone::water::check_outline_points;
use sapphire_zone::water::drawn::{
    DEPTH_STEP, InfoTexture, MAX_INFO_SIDE, SHORE_STEP, SIGNED_ZERO, WaterMesh, WaterVertex,
};
use sapphire_zone::{RiverPoint, WaterBody, WaterKind, WaterShape};
use serde::Serialize;

use super::scatter::ColliderGround;
use crate::cache::Stage;
use crate::cook::Cook;

/// The `.swater` writer. Bump the version whenever it changes output for the same body, sizes and colliders.
pub const WATER_COOKER: Stage = Stage { name: "swater", version: 1 };

/// The most squares a body's mesh is cut into (see the [module docs](self#sizes)).
pub const MAX_QUADS: u64 = 1 << 16;

/// How far a lake's grown outline mitres a corner before it bevels it, as a share of its margin.
const MITRE_LIMIT: f64 = 2.0;

/// The mesh's square on `cook`, before a large body doubles it, in metres.
pub const fn spacing(cook: Cook) -> f32 {
    match cook {
        Cook::Desktop => 2.0,
        Cook::Mobile => 4.0,
    }
}

/// The info texture's texel on `cook`, before a large body or a full zone doubles it, in metres.
pub const fn texel(cook: Cook) -> f32 {
    match cook {
        Cook::Desktop => 1.0,
        Cook::Mobile => 2.0,
    }
}

/// What a zone's info textures may hold together on `cook`, in bytes (spec §3.4).
pub const fn info_budget(cook: Cook) -> u64 {
    match cook {
        Cook::Desktop => 4 << 20,
        Cook::Mobile => 1 << 20,
    }
}

/// What a zone's meshes may hold together on `cook`, in bytes, as [`QUAD_BYTES`] counts them: with
/// [`info_budget`], the asset server's budget for water meshes.
pub const fn mesh_budget(cook: Cook) -> u64 {
    match cook {
        Cook::Desktop => 12 << 20,
        Cook::Mobile => 3 << 20,
    }
}

/// What a square of a mesh costs, as [`sizes`] counts it: a vertex of 20 bytes and two triangles of 12. A cut
/// square's extra pieces are on top, which the budgets' rounding leaves room for.
pub const QUAD_BYTES: u64 = 20 + 2 * 12;

/// What a body is cooked at: its mesh's square and its info texture's texel, in metres.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Sizes {
    pub spacing: f32,
    pub texel: f32,
}

/// What keys a body's drawn surface in place of the zone's document: its ID (the surface holds it, so a renamed
/// body, or another of the same outline, never takes a cached surface naming the wrong one), its shape, its
/// margin, the `sizes` the zone's water gives it and the colliders' triangulation ([`crate::nav::TRIANGULATION`]),
/// as its bed lies on their triangles; the colliders' key is its dependency.
pub fn recipe(body: &WaterBody, sizes: Option<Sizes>) -> blake3::Hash {
    recipe_on(body, sizes, crate::nav::TRIANGULATION)
}

/// [`recipe`] on the colliders' triangulation of version `triangulation`.
fn recipe_on(body: &WaterBody, sizes: Option<Sizes>, triangulation: u32) -> blake3::Hash {
    let recipe =
        (sapphire_core::Name::new(&body.name).as_u64(), &body.shape, body.shore_margin, sizes, triangulation);
    blake3::hash(&sapphire_serialize::postcard::to_allocvec(&recipe).expect("a water recipe always encodes"))
}

/// Each body's sizes on `cook`, in the document's order: none for the sea (see the [module docs](self#sizes)).
pub fn sizes(bodies: &[WaterBody], cook: Cook) -> Vec<Option<Sizes>> {
    let mut sizes: Vec<Option<Sizes>> = bodies
        .iter()
        .map(|body| {
            if body.shape.kind() == WaterKind::Ocean {
                return None;
            }
            let mut spacing = spacing(cook);
            while quads(body, spacing) > MAX_QUADS {
                spacing *= 2.0;
            }
            let mut texel = texel(cook);
            while rect(body, texel).sides().iter().any(|&side| side > MAX_INFO_SIDE) {
                texel *= 2.0;
            }
            Some(Sizes { spacing, texel })
        })
        .collect();
    let squares =
        |sizes: &Option<Sizes>, body: &WaterBody| sizes.map_or(0, |sizes| quads(body, sizes.spacing));
    loop {
        let total: u64 =
            sizes.iter().zip(bodies).map(|(sizes, body)| squares(sizes, body) * QUAD_BYTES).sum();
        if total <= mesh_budget(cook) {
            break;
        }
        let mut largest = 0;
        for index in 1..bodies.len() {
            if squares(&sizes[index], &bodies[index]) > squares(&sizes[largest], &bodies[largest]) {
                largest = index;
            }
        }
        let sizes = sizes[largest].as_mut().expect("the sea has no squares, so it's never the largest");
        sizes.spacing *= 2.0;
    }
    let texels =
        |sizes: &Option<Sizes>, body: &WaterBody| sizes.map_or(0, |sizes| rect(body, sizes.texel).texels());
    loop {
        let total: u64 = sizes.iter().zip(bodies).map(|(sizes, body)| texels(sizes, body) * 4).sum();
        if total <= info_budget(cook) {
            return sizes;
        }
        let mut largest = 0;
        for index in 1..bodies.len() {
            if texels(&sizes[index], &bodies[index]) > texels(&sizes[largest], &bodies[largest]) {
                largest = index;
            }
        }
        let sizes = sizes[largest].as_mut().expect("the sea has no texels, so it's never the largest");
        sizes.texel *= 2.0;
    }
}

/// Cooks a body's drawn surface at `sizes`, its bed on `ground` if the zone has colliders: the mesh, or what's
/// wrong with the body, by its field under `water[i]`.
pub fn cook(
    body: &WaterBody,
    ground: Option<&ColliderGround>,
    sizes: Sizes,
) -> Result<WaterMesh, (String, String)> {
    let (vertices, indices) = match &body.shape {
        WaterShape::Lake { outline, height } => lake(outline, *height, body.shore_margin, sizes.spacing)?,
        WaterShape::River { points } => river(points, body.shore_margin, sizes.spacing)?,
        WaterShape::Ocean { .. } => unreachable!("the sea has no sizes"),
    };
    let mut bounds_min = [f32::INFINITY; 3];
    let mut bounds_max = [f32::NEG_INFINITY; 3];
    for vertex in &vertices {
        for axis in 0..3 {
            bounds_min[axis] = bounds_min[axis].min(vertex.position[axis]);
            bounds_max[axis] = bounds_max[axis].max(vertex.position[axis]);
        }
    }
    Ok(WaterMesh {
        body: sapphire_core::Name::new(&body.name).as_u64(),
        kind: body.shape.kind(),
        vertices,
        indices,
        bounds_min,
        bounds_max,
        info: info(body, ground, sizes.texel),
    })
}

/// A body's drawn bounds on the ground plane: around its grown outline's corners, or its ribbon's banks' and its
/// volume's.
fn drawn(body: &WaterBody) -> ([f64; 2], [f64; 2]) {
    let corners: Vec<[f64; 2]> = match &body.shape {
        WaterShape::Lake { outline, .. } => {
            grow(outline, body.shore_margin).iter().map(|corner| corner.map(f64::from)).collect()
        }
        // And its volume's, whose round ends reach half a width past its first and last points, which its info
        // texture covers.
        WaterShape::River { points } => {
            let (min, max) = body.shape.bounds();
            let mut corners = bank_corners(points, body.shore_margin);
            corners.extend([min, max].map(|corner| corner.map(f64::from)));
            corners
        }
        WaterShape::Ocean { .. } => unreachable!("the sea isn't drawn"),
    };
    corners.iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(low, high), corner| {
        ([low[0].min(corner[0]), low[1].min(corner[1])], [high[0].max(corner[0]), high[1].max(corner[1])])
    })
}

/// The squares of side `size` a span of the ground covers, along one axis: the first and the last.
fn span(min: f64, max: f64, size: f64) -> (i64, i64) {
    ((min / size).floor() as i64, (max / size).floor() as i64)
}

/// How many squares a body's mesh is cut into at `spacing`.
fn quads(body: &WaterBody, spacing: f32) -> u64 {
    let size = f64::from(spacing);
    match &body.shape {
        WaterShape::River { points } => {
            let sections: u64 = points
                .windows(2)
                .map(|pair| (length(pair[0].at, pair[1].at) / size).ceil().max(1.0) as u64)
                .sum();
            let ends = if body.shore_margin > 0.0 { 2 } else { 0 };
            sections.saturating_add(ends).saturating_mul(columns(points, body.shore_margin, spacing))
        }
        _ => {
            let (min, max) = drawn(body);
            let (x0, x1) = span(min[0], max[0], size);
            let (z0, z1) = span(min[1], max[1], size);
            ((x1 - x0 + 1) as u64).saturating_mul((z1 - z0 + 1) as u64)
        }
    }
}

/// An info texture's place: its first texel's corner, in texels of `texel`, and its sides.
#[derive(Clone, Copy, Debug)]
struct Rect {
    first: [i64; 2],
    size: [i64; 2],
    texel: f64,
}

impl Rect {
    fn sides(self) -> [u32; 2] {
        self.size.map(|side| u32::try_from(side).unwrap_or(u32::MAX))
    }

    fn texels(self) -> u64 {
        self.size[0] as u64 * self.size[1] as u64
    }

    /// Texel `(i, j)`'s centre: exact, a texel being a power of two.
    fn centre(self, i: i64, j: i64) -> [f32; 2] {
        [(self.first[0] + i) as f64, (self.first[1] + j) as f64]
            .map(|corner| ((corner + 0.5) * self.texel) as f32)
    }
}

/// The info texture's place over a body's drawn bounds, a texel past them each way.
fn rect(body: &WaterBody, texel: f32) -> Rect {
    let (min, max) = drawn(body);
    let size = f64::from(texel);
    let (x0, x1) = span(min[0], max[0], size);
    let (z0, z1) = span(min[1], max[1], size);
    Rect { first: [x0 - 1, z0 - 1], size: [x1 - x0 + 3, z1 - z0 + 3], texel: size }
}

/// The distance across the ground between two of a river's points.
fn length(from: [f32; 3], to: [f32; 3]) -> f64 {
    let (dx, dz) = (f64::from(to[0]) - f64::from(from[0]), f64::from(to[2]) - f64::from(from[2]));
    (dx * dx + dz * dz).sqrt()
}

/// How many columns a river's ribbon is cut into across at `spacing`: as many as its widest point needs.
fn columns(points: &[RiverPoint], margin: f32, spacing: f32) -> u64 {
    let widest =
        points.iter().map(|point| f64::from(point.width) + 2.0 * f64::from(margin)).fold(0.0, f64::max);
    (widest / f64::from(spacing)).ceil().max(1.0) as u64
}

/// Twice the signed area of `a`, `b`, `c` on the ground plane, `(x, z)`: positive when they run anticlockwise
/// as `(x, z)` reads (which faces down, `+y` being `z × x`). Exact for `f32` inputs: each difference and product
/// is, and the one subtraction rounds without crossing zero.
fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// A mesh being made: vertices by their `(x, z)`'s bits, triangles facing up.
struct Builder {
    vertices: Vec<WaterVertex>,
    indices: Vec<u32>,
    by_place: BTreeMap<[u32; 2], u32>,
}

impl Builder {
    fn new() -> Self {
        Self { vertices: Vec::new(), indices: Vec::new(), by_place: BTreeMap::new() }
    }

    /// A lake's vertex at `(x, z)`, its UV its place, made once.
    fn lake_vertex(&mut self, [x, z]: [f32; 2], height: f32) -> u32 {
        let next = self.vertices.len() as u32;
        let index = *self.by_place.entry([x.to_bits(), z.to_bits()]).or_insert(next);
        if index == next {
            self.vertices.push(WaterVertex { position: [x, height, z], uv: [x, z] });
        }
        index
    }

    /// A triangle of three lake points in anticlockwise order as `(x, z)` reads them, unless it has no area once
    /// its corners are `f32`, wound to face up.
    fn lake_triangle(&mut self, corners: [[f64; 2]; 3], height: f32) {
        let corners = corners.map(|[x, z]| [x as f32, z as f32]);
        let wide = corners.map(|corner| corner.map(f64::from));
        if orient(wide[0], wide[1], wide[2]) <= 0.0 {
            return;
        }
        let [a, b, c] = corners.map(|corner| self.lake_vertex(corner, height));
        self.indices.extend([a, c, b]);
    }
}

/// A lake's mesh: its outline grown by `margin`, triangulated and cut by the grid of `spacing` (see
/// [`sapphire_zone::water::drawn`]).
fn lake(
    outline: &[[f32; 2]],
    height: f32,
    margin: f32,
    spacing: f32,
) -> Result<(Vec<WaterVertex>, Vec<u32>), (String, String)> {
    let grown = grow(outline, margin);
    check_outline_points(&grown).map_err(|problem| {
        let problem =
            format!("the outline grown by it meets itself, so its surface can't be drawn: {problem}");
        ("shore_margin".to_owned(), problem)
    })?;
    let points: Vec<[f64; 2]> = grown.iter().map(|point| point.map(f64::from)).collect();
    // The check above rounds as the document's does; the clipper's orientations are exact, so an outline grown
    // to only just meet itself can pass the one and fail the other, and it's the margin's still.
    let triangles = ear_clip(&points).ok_or_else(|| {
        let problem = "the outline grown by it only just meets itself, so its surface can't be drawn";
        ("shore_margin".to_owned(), problem.to_owned())
    })?;

    let size = f64::from(spacing);
    let (low, high) =
        points.iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(low, high), point| {
            ([low[0].min(point[0]), low[1].min(point[1])], [high[0].max(point[0]), high[1].max(point[1])])
        });
    let (x0, x1) = span(low[0], high[0], size);
    let (z0, z1) = span(low[1], high[1], size);
    let square = |x: i64, z: i64| {
        let min = [x as f64 * size, z as f64 * size];
        (min, [(x + 1) as f64 * size, (z + 1) as f64 * size])
    };

    // The squares an edge of a triangle touches, closed, the outline's and the diagonals between: only there is
    // a square cut, and there each triangle over it is clipped to it. A square none touches lies wholly in one
    // triangle or out of them all, so its corners are its neighbours' too and the mesh has no T-junction.
    let mut cut: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    let edges = triangles.iter().flat_map(|triangle| (0..3).map(|k| (triangle[k], triangle[(k + 1) % 3])));
    for (from, to) in edges {
        let (a, b) = (points[from], points[to]);
        let (ex0, ex1) = span(a[0].min(b[0]), a[0].max(b[0]), size);
        let (ez0, ez1) = span(a[1].min(b[1]), a[1].max(b[1]), size);
        for z in ez0..=ez1 {
            for x in ex0..=ex1 {
                let (min, max) = square(x, z);
                let sides = [min, [max[0], min[1]], max, [min[0], max[1]]].map(|corner| orient(a, b, corner));
                if !(sides.iter().all(|&side| side > 0.0) || sides.iter().all(|&side| side < 0.0)) {
                    cut.entry((x, z)).or_default();
                }
            }
        }
    }
    for (index, triangle) in triangles.iter().enumerate() {
        let corners = triangle.map(|corner| points[corner]);
        let (low, high) =
            corners.iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(low, high), c| {
                ([low[0].min(c[0]), low[1].min(c[1])], [high[0].max(c[0]), high[1].max(c[1])])
            });
        let (tx0, tx1) = span(low[0], high[0], size);
        let (tz0, tz1) = span(low[1], high[1], size);
        for z in tz0..=tz1 {
            for x in tx0..=tx1 {
                if let Some(over) = cut.get_mut(&(x, z)) {
                    over.push(index);
                }
            }
        }
    }

    let mut mesh = Builder::new();
    for z in z0..=z1 {
        for x in x0..=x1 {
            let (min, max) = square(x, z);
            let corners = [min, [max[0], min[1]], max, [min[0], max[1]]];
            match cut.get(&(x, z)) {
                // A square no edge touches is wholly in or wholly out: its middle says which.
                None => {
                    if inside(&points, [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5]) {
                        mesh.lake_triangle([corners[0], corners[1], corners[2]], height);
                        mesh.lake_triangle([corners[0], corners[2], corners[3]], height);
                    }
                }
                Some(over) => {
                    for &triangle in over {
                        let clipped = clip(triangles[triangle].map(|corner| points[corner]), min, max);
                        for piece in fan(&clipped) {
                            mesh.lake_triangle(piece, height);
                        }
                    }
                }
            }
        }
    }
    Ok((mesh.vertices, mesh.indices))
}

/// An outline grown outward by `margin`, anticlockwise as `(x, z)` reads it whichever way it ran: each edge
/// moved out by `margin`, a convex corner mitred up to [`MITRE_LIMIT`] times the margin and bevelled past it,
/// a concave one mitred. With no margin, the outline itself.
fn grow(outline: &[[f32; 2]], margin: f32) -> Vec<[f32; 2]> {
    let count = outline.len();
    let point = |index: usize| outline[index % count].map(f64::from);
    let twice_area: f64 = (0..count)
        .map(|index| {
            let (a, b) = (point(index), point(index + 1));
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    let ordered: Vec<[f64; 2]> = if twice_area >= 0.0 {
        (0..count).map(point).collect()
    } else {
        (0..count).rev().map(point).collect()
    };
    if margin == 0.0 {
        return ordered.iter().map(|point| point.map(|value| value as f32)).collect();
    }
    let margin = f64::from(margin);
    let direction = |from: [f64; 2], to: [f64; 2]| {
        let (dx, dz) = (to[0] - from[0], to[1] - from[1]);
        let length = (dx * dx + dz * dz).sqrt();
        [dx / length, dz / length]
    };
    // Out of an anticlockwise outline: to the right of each edge as it runs.
    let outward = |[dx, dz]: [f64; 2]| [dz, -dx];
    let mut grown = Vec::with_capacity(count * 2);
    for index in 0..count {
        let (before, at, after) =
            (ordered[(index + count - 1) % count], ordered[index], ordered[(index + 1) % count]);
        let (into, out) = (direction(before, at), direction(at, after));
        let (n0, n1) = (outward(into), outward(out));
        let dot = n0[0] * n1[0] + n0[1] * n1[1];
        let convex = into[0] * out[1] - into[1] * out[0] > 0.0;
        // A mitre's length is the margin over the cosine of half the turn, `sqrt(2 / (1 + dot))`.
        if convex && (1.0 + dot) * MITRE_LIMIT * MITRE_LIMIT < 2.0 {
            grown.push([at[0] + n0[0] * margin, at[1] + n0[1] * margin]);
            grown.push([at[0] + n1[0] * margin, at[1] + n1[1] * margin]);
        } else {
            let scale = margin / (1.0 + dot);
            grown.push([at[0] + (n0[0] + n1[0]) * scale, at[1] + (n0[1] + n1[1]) * scale]);
        }
    }
    grown.iter().map(|point| point.map(|value| value as f32)).collect()
}

/// An anticlockwise simple polygon's triangles, by ear clipping from its first corner each time, each
/// anticlockwise; a corner in line with its neighbours goes without one. `None` only if no ear is found, which
/// exact orientations rule out for a simple polygon.
fn ear_clip(points: &[[f64; 2]]) -> Option<Vec<[usize; 3]>> {
    let mut ring: Vec<usize> = (0..points.len()).collect();
    let mut triangles = Vec::with_capacity(points.len());
    while ring.len() > 3 {
        let count = ring.len();
        let ear = (0..count).find_map(|k| {
            let (a, b, c) = (ring[(k + count - 1) % count], ring[k], ring[(k + 1) % count]);
            let turn = orient(points[a], points[b], points[c]);
            if turn == 0.0 {
                return Some((k, None));
            }
            let blocked = turn < 0.0
                || ring.iter().any(|&other| {
                    other != a
                        && other != b
                        && other != c
                        && within(points[other], [points[a], points[b], points[c]])
                });
            (!blocked).then_some((k, Some([a, b, c])))
        })?;
        triangles.extend(ear.1);
        ring.remove(ear.0);
    }
    if orient(points[ring[0]], points[ring[1]], points[ring[2]]) > 0.0 {
        triangles.push([ring[0], ring[1], ring[2]]);
    }
    Some(triangles)
}

/// Whether `point` lies in an anticlockwise triangle, its edges included.
fn within(point: [f64; 2], [a, b, c]: [[f64; 2]; 3]) -> bool {
    orient(a, b, point) >= 0.0 && orient(b, c, point) >= 0.0 && orient(c, a, point) >= 0.0
}

/// Whether `point` lies inside a polygon, by the crossings of a ray toward +X, as the water query counts them.
fn inside(points: &[[f64; 2]], [x, z]: [f64; 2]) -> bool {
    let mut odd = false;
    let mut previous = points[points.len() - 1];
    for &next in points {
        if (previous[1] > z) != (next[1] > z) {
            let crossing =
                previous[0] + (z - previous[1]) * (next[0] - previous[0]) / (next[1] - previous[1]);
            if x < crossing {
                odd = !odd;
            }
        }
        previous = next;
    }
    odd
}

/// What a clipped polygon's edge runs along: one of its triangle's edges (from corner `k` to the next), or a
/// side of the square (where `axis` is `at`).
#[derive(Clone, Copy)]
enum Along {
    Edge(usize),
    Side(usize, f64),
}

/// Where the edge from `a` to `b` crosses the line where `axis` is `bound`, its other coordinate rounded to
/// `f32` as a vertex's is: from its ends in a fixed order, so the squares either side of the line and the
/// triangles either side of a diagonal find the same point, bit for bit.
fn crossing(a: [f64; 2], b: [f64; 2], axis: usize, bound: f64) -> [f64; 2] {
    let (a, b) = if (a[0], a[1]) <= (b[0], b[1]) { (a, b) } else { (b, a) };
    let other = 1 - axis;
    let t = (bound - a[axis]) / (b[axis] - a[axis]);
    let mut point = [0.0; 2];
    point[axis] = bound;
    point[other] = f64::from((a[other] + (b[other] - a[other]) * t) as f32);
    point
}

/// An anticlockwise triangle clipped to the square from `min` to `max`: a polygon, anticlockwise, of none to
/// seven corners, each a corner of the triangle or of the square or where an edge of the triangle crosses a
/// side, computed from the triangle's own edge ([`crossing`]), never from an edge already clipped.
fn clip(triangle: [[f64; 2]; 3], min: [f64; 2], max: [f64; 2]) -> Vec<[f64; 2]> {
    let mut polygon: Vec<([f64; 2], Along)> = (0..3).map(|k| (triangle[k], Along::Edge(k))).collect();
    // Each side as (axis, bound, whether inside is above it).
    for (axis, bound, above) in [(0, min[0], true), (0, max[0], false), (1, min[1], true), (1, max[1], false)]
    {
        if polygon.is_empty() {
            break;
        }
        let keep = |point: [f64; 2]| if above { point[axis] >= bound } else { point[axis] <= bound };
        let mut next = Vec::with_capacity(polygon.len() + 1);
        for index in 0..polygon.len() {
            let ((from, along), to) = (polygon[index], polygon[(index + 1) % polygon.len()].0);
            let cross = || match along {
                Along::Edge(k) => crossing(triangle[k], triangle[(k + 1) % 3], axis, bound),
                Along::Side(side, at) => {
                    let mut corner = [0.0; 2];
                    corner[axis] = bound;
                    corner[side] = at;
                    corner
                }
            };
            match (keep(from), keep(to)) {
                (true, true) => next.push((from, along)),
                (true, false) => {
                    next.push((from, along));
                    next.push((cross(), Along::Side(axis, bound)));
                }
                (false, true) => next.push((cross(), along)),
                (false, false) => {}
            }
        }
        polygon = next;
    }
    let mut corners: Vec<[f64; 2]> = Vec::with_capacity(polygon.len());
    for (point, _) in polygon {
        if corners.last() != Some(&point) {
            corners.push(point);
        }
    }
    if corners.len() > 1 && corners.first() == corners.last() {
        corners.pop();
    }
    corners
}

/// A clipped polygon's triangles, each anticlockwise: ears cut from a corner that turns, never one that would
/// leave the rest in a line. So every corner on a side, of the square or of a diagonal, stays a corner of a
/// triangle there, as the polygon beside it has it.
fn fan(polygon: &[[f64; 2]]) -> Vec<[[f64; 2]; 3]> {
    let mut ring = polygon.to_vec();
    let mut pieces = Vec::with_capacity(ring.len());
    while ring.len() > 3 {
        let count = ring.len();
        let ear = (0..count).find(|&k| {
            let (a, b, c) = (ring[(k + count - 1) % count], ring[k], ring[(k + 1) % count]);
            orient(a, b, c) > 0.0 && ring.iter().any(|&other| orient(a, c, other) != 0.0 && other != b)
        });
        let Some(k) = ear else { return pieces };
        pieces.push([ring[(k + count - 1) % count], ring[k], ring[(k + 1) % count]]);
        ring.remove(k);
    }
    if ring.len() == 3 && orient(ring[0], ring[1], ring[2]) > 0.0 {
        pieces.push([ring[0], ring[1], ring[2]]);
    }
    pieces
}

/// One cross-section of a river's ribbon: its middle, the way across it (to the right, looking downstream),
/// how far its banks reach along that way and as the UV counts it, its surface and how far along it is.
#[derive(Clone, Copy)]
struct Section {
    middle: [f64; 2],
    across: [f64; 2],
    reach: f64,
    half: f64,
    height: f64,
    along: f64,
}

/// A river point's `(x, z)`.
pub(super) fn xz(points: &[RiverPoint], index: usize) -> [f64; 2] {
    [f64::from(points[index].at[0]), f64::from(points[index].at[2])]
}

/// How far a river's drawn banks lie from its middle at a point, as it runs straight: half its width and its
/// margin.
fn half(points: &[RiverPoint], index: usize, margin: f32) -> f64 {
    f64::from(points[index].width) * 0.5 + f64::from(margin)
}

/// The way to the right of `way`, looking along it from above.
pub(super) fn right([dx, dz]: [f64; 2]) -> [f64; 2] {
    [-dz, dx]
}

/// Each stretch of a river: the way it runs, a unit vector across the ground, and its length.
pub(super) fn stretches(points: &[RiverPoint]) -> Vec<([f64; 2], f64)> {
    (0..points.len() - 1)
        .map(|index| {
            let (from, to) = (xz(points, index), xz(points, index + 1));
            let length = length(points[index].at, points[index + 1].at);
            ([(to[0] - from[0]) / length, (to[1] - from[1]) / length], length)
        })
        .collect()
}

/// A river's cross-section at one of its points.
#[derive(Clone, Copy)]
struct Frame {
    /// The way across, to the right looking downstream, halfway between its two stretches' at a bend.
    across: [f64; 2],
    /// How much farther its banks lie along `across` than half its width, so each stays that far from both
    /// stretches: one over the cosine of half the turn.
    mitre: f64,
    /// How far the inner bank's corner falls back along each stretch, as a share of its half width: the
    /// tangent of half the turn, 0 at its ends.
    fold: f64,
    /// Which bank is the inner one: 1 for the right (the river turning right), -1 for the left, 0 at its ends.
    inner: f64,
}

/// Each point's frame, or the point at which the river turns back on itself.
fn frames(points: &[RiverPoint], stretches: &[([f64; 2], f64)]) -> Result<Vec<Frame>, usize> {
    let count = points.len();
    (0..count)
        .map(|index| {
            if index == 0 || index == count - 1 {
                let across = right(stretches[index.min(count - 2)].0);
                return Ok(Frame { across, mitre: 1.0, fold: 0.0, inner: 0.0 });
            }
            let (into, out) = (stretches[index - 1].0, stretches[index].0);
            let dot = into[0] * out[0] + into[1] * out[1];
            if dot <= -0.999_999 {
                return Err(index);
            }
            let sum = [into[0] + out[0], into[1] + out[1]];
            let length = (sum[0] * sum[0] + sum[1] * sum[1]).sqrt();
            let middle = [sum[0] / length, sum[1] / length];
            let cosine = middle[0] * out[0] + middle[1] * out[1];
            let turn = into[0] * out[1] - into[1] * out[0];
            let fold = turn.abs() / (1.0 + dot);
            // `right` of +X is +Z, so a turn toward it, anticlockwise as `(x, z)` reads, has the right bank inside.
            let inner = if turn > 0.0 {
                1.0
            } else if turn < 0.0 {
                -1.0
            } else {
                0.0
            };
            Ok(Frame { across: right(middle), mitre: 1.0 / cosine, fold, inner })
        })
        .collect()
}

/// The corners of a river's drawn banks, around which its whole ribbon lies: each point's mitred banks, each
/// stretch's banks square to it at both ends, and its ends grown by `margin`.
fn bank_corners(points: &[RiverPoint], margin: f32) -> Vec<[f64; 2]> {
    let stretches = stretches(points);
    let Ok(frames) = frames(points, &stretches) else {
        let (min, max) = WaterShape::River { points: points.to_vec() }.bounds();
        let grow = f64::from(margin);
        return vec![min.map(|value| f64::from(value) - grow), max.map(|value| f64::from(value) + grow)];
    };
    let banks = |middle: [f64; 2], across: [f64; 2], reach: f64| {
        [-1.0, 1.0].map(|side| [middle[0] + across[0] * reach * side, middle[1] + across[1] * reach * side])
    };
    let mut corners = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        corners.extend(banks(xz(points, index), frame.across, half(points, index, margin) * frame.mitre));
    }
    for (index, &(way, _)) in stretches.iter().enumerate() {
        for end in [index, index + 1] {
            corners.extend(banks(xz(points, end), right(way), half(points, end, margin)));
        }
    }
    let grow = f64::from(margin);
    for (end, way) in
        [(0, stretches[0].0.map(|value| -value)), (points.len() - 1, stretches[stretches.len() - 1].0)]
    {
        let at = xz(points, end);
        let middle = [at[0] + way[0] * grow, at[1] + way[1] * grow];
        corners.extend(banks(middle, frames[end].across, half(points, end, margin)));
    }
    corners
}

/// A river's mesh: its ribbon, its banks grown by `margin`, cut by `spacing` (see
/// [`sapphire_zone::water::drawn`]).
fn river(
    points: &[RiverPoint],
    margin: f32,
    spacing: f32,
) -> Result<(Vec<WaterVertex>, Vec<u32>), (String, String)> {
    let count = points.len();
    let xz = |index: usize| xz(points, index);
    let half = |index: usize| half(points, index, margin);
    let columns = columns(points, margin, spacing) as u32;
    let margin = f64::from(margin);
    let stretches = stretches(points);
    let field = |index: usize| format!("shape.points[{index}]");
    let frames = frames(points, &stretches)
        .map_err(|index| (field(index), "the river turns back on itself here".to_owned()))?;
    for (index, &(_, length)) in stretches.iter().enumerate() {
        let (start, end) = (half(index) * frames[index].fold, half(index + 1) * frames[index + 1].fold);
        // The two ends' inner corners fold over each other only on the same bank: an S's lie on opposite ones.
        let same_bank = frames[index].inner == frames[index + 1].inner;
        if start.max(end) > length || (same_bank && start + end > length) {
            let at = if frames[index + 1].fold > 0.0 { index + 1 } else { index };
            let problem = format!(
                "the river turns too tightly here for its width: its inner bank folds back over the {length:.2} m \
                 stretch beside it; ease the turn, narrow it or lengthen the stretch"
            );
            return Err((field(at), problem));
        }
    }
    // Two stretches that aren't neighbours mustn't meet: the ribbon would lie over itself there.
    let banks = |index: usize, end: usize| {
        let (way, _) = stretches[index];
        let grown = if margin > 0.0 && (end == 0 || end == count - 1) { margin } else { 0.0 };
        let toward = if end == index { -grown } else { grown };
        let at = xz(end);
        let middle = [at[0] + way[0] * toward, at[1] + way[1] * toward];
        let (across, reach) = (frames[end].across, half(end) * frames[end].mitre);
        [-1.0, 1.0].map(|side| [middle[0] + across[0] * reach * side, middle[1] + across[1] * reach * side])
    };
    let outlines: Vec<[[f64; 2]; 4]> = (0..stretches.len())
        .map(|index| {
            let ([start_left, start_right], [end_left, end_right]) =
                (banks(index, index), banks(index, index + 1));
            [start_left, start_right, end_right, end_left]
        })
        .collect();
    for later in 2..outlines.len() {
        if let Some(earlier) = (0..later - 1).find(|&earlier| overlap(&outlines[earlier], &outlines[later])) {
            let problem = format!(
                "the river's stretch from here meets its stretch from point {earlier}: its banks would lie over \
                 each other, so its surface would be drawn twice there; move them apart or narrow it"
            );
            return Err((field(later), problem));
        }
    }

    let size = f64::from(spacing);
    let height = |index: usize| f64::from(points[index].at[1]);
    let mut sections = Vec::new();
    let joint = |index: usize, along: f64| Section {
        middle: xz(index),
        across: frames[index].across,
        reach: half(index) * frames[index].mitre,
        half: half(index),
        height: height(index),
        along,
    };
    if margin > 0.0 {
        let (way, _) = stretches[0];
        let start = xz(0);
        sections.push(Section {
            middle: [start[0] - way[0] * margin, start[1] - way[1] * margin],
            along: -margin,
            ..joint(0, 0.0)
        });
    }
    let mut along = 0.0;
    for (index, &(way, length)) in stretches.iter().enumerate() {
        // A bend's inner bank falls back along the stretch by its fold, so the sections between stand clear of
        // both ends' (the check above leaves room), or a square there would turn over.
        let (start, end) = (half(index) * frames[index].fold, half(index + 1) * frames[index + 1].fold);
        let clear = length - start - end;
        let cuts = (clear / size).ceil().max(1.0) as u32;
        sections.push(joint(index, along));
        let (from, to) = (xz(index), xz(index + 1));
        for cut in 1..cuts {
            let t = (start + clear * f64::from(cut) / f64::from(cuts)) / length;
            let half = half(index) + (half(index + 1) - half(index)) * t;
            sections.push(Section {
                middle: [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t],
                across: right(way),
                reach: half,
                half,
                height: height(index) + (height(index + 1) - height(index)) * t,
                along: along + length * t,
            });
        }
        along += length;
    }
    sections.push(joint(count - 1, along));
    if margin > 0.0 {
        let (way, _) = stretches[count - 2];
        let end = xz(count - 1);
        sections.push(Section {
            middle: [end[0] + way[0] * margin, end[1] + way[1] * margin],
            along: along + margin,
            ..joint(count - 1, along)
        });
    }

    let mut vertices = Vec::with_capacity(sections.len() * (columns as usize + 1));
    for section in &sections {
        for column in 0..=columns {
            let share = f64::from(column) / f64::from(columns) * 2.0 - 1.0;
            let out = share * section.reach;
            let position = [
                (section.middle[0] + section.across[0] * out) as f32,
                section.height as f32,
                (section.middle[1] + section.across[1] * out) as f32,
            ];
            let uv = [(share * section.half) as f32, section.along as f32];
            vertices.push(WaterVertex { position, uv });
        }
    }
    let row = columns + 1;
    let mut indices = Vec::with_capacity((sections.len() - 1) * columns as usize * 6);
    for section in 0..sections.len() as u32 - 1 {
        for column in 0..columns {
            let here = section * row + column;
            let (next_column, next_section) = (here + 1, here + row);
            indices.extend([here, next_column, next_section, next_column, next_section + 1, next_section]);
        }
    }
    Ok((vertices, indices))
}

/// Whether two quadrilaterals on the ground plane meet, edges included: an edge of one crosses or touches an
/// edge of the other, or a corner of one lies inside the other.
fn overlap(one: &[[f64; 2]; 4], other: &[[f64; 2]; 4]) -> bool {
    let meet = |p: [f64; 2], q: [f64; 2], r: [f64; 2], s: [f64; 2]| {
        let (a, b) = (orient(p, q, r), orient(p, q, s));
        let (c, d) = (orient(r, s, p), orient(r, s, q));
        if a == 0.0 && b == 0.0 && c == 0.0 && d == 0.0 {
            // In one line: whether their spans along it meet.
            return (0..2).all(|axis| {
                p[axis].min(q[axis]) <= r[axis].max(s[axis]) && r[axis].min(s[axis]) <= p[axis].max(q[axis])
            });
        }
        a * b <= 0.0 && c * d <= 0.0
    };
    fn edges(quad: &[[f64; 2]; 4]) -> impl Iterator<Item = ([f64; 2], [f64; 2])> + '_ {
        (0..4).map(move |k| (quad[k], quad[(k + 1) % 4]))
    }
    if edges(one).any(|(p, q)| edges(other).any(|(r, s)| meet(p, q, r, s))) {
        return true;
    }
    inside(&one[..], other[0]) || inside(&other[..], one[0])
}

/// A body's info texture at `texel` (see [`sapphire_zone::water::drawn`]): its bed on `ground`, or none
/// anywhere with no colliders.
fn info(body: &WaterBody, ground: Option<&ColliderGround>, texel: f32) -> InfoTexture {
    let rect = rect(body, texel);
    let [width, height] = rect.size.map(|side| side as usize);
    let mut wet = vec![false; width * height];
    let mut depth = vec![0u8; width * height];
    let mut flow = vec![[0.0f32; 2]; width * height];
    for j in 0..height {
        for i in 0..width {
            let [x, z] = rect.centre(i as i64, j as i64);
            let Some((surface, current)) = body.shape.column(x, z) else { continue };
            let (bed, covered) = ground.map_or((None, false), |ground| ground.under(x, z, surface));
            let surface = f64::from(surface);
            let here = j * width + i;
            match bed {
                Some(bed) if bed < surface => {
                    wet[here] = true;
                    depth[here] = ((surface - bed) / f64::from(DEPTH_STEP)).round().clamp(0.0, 255.0) as u8;
                }
                None if !covered => {
                    wet[here] = true;
                    depth[here] = u8::MAX;
                }
                _ => continue,
            }
            flow[here] = current;
        }
    }
    let to_shore = distances(&wet, false, width, height);
    let to_water = distances(&wet, true, width, height);
    let flow_scale = match &body.shape {
        WaterShape::River { points } => points.iter().map(|point| point.speed).fold(0.0, f32::max),
        _ => 0.0,
    };
    let texel_size = f64::from(texel);
    let signed = |value: f64, step: f64, low: f64| {
        (value / step).round().clamp(low, 127.0) as i32 + i32::from(SIGNED_ZERO)
    };
    let mut texels = Vec::with_capacity(width * height);
    for here in 0..width * height {
        // From a texel's centre to the nearest texel's of the other side, less half a texel.
        let (nearest, sign) = if wet[here] { (to_shore[here], 1.0) } else { (to_water[here], -1.0) };
        let shore = nearest.map_or(sign * f64::MAX, |(squared, _)| {
            sign * ((squared as f64).sqrt() * texel_size - texel_size * 0.5)
        });
        let current = match (wet[here], to_water[here]) {
            (true, _) => flow[here],
            (false, Some((_, nearest))) => flow[nearest],
            (false, None) => [0.0; 2],
        };
        let flow_byte = |value: f32| {
            if flow_scale == 0.0 {
                i32::from(SIGNED_ZERO)
            } else {
                signed(f64::from(value) / f64::from(flow_scale) * 127.0, 1.0, -127.0)
            }
        };
        texels.push([
            depth[here],
            signed(shore, f64::from(SHORE_STEP), -128.0) as u8,
            flow_byte(current[0]) as u8,
            flow_byte(current[1]) as u8,
        ]);
    }
    InfoTexture {
        origin: [rect.first[0] as f64 * texel_size, rect.first[1] as f64 * texel_size]
            .map(|value| value as f32),
        texel,
        width: width as u32,
        height: height as u32,
        flow_scale,
        texels,
    }
}

/// For each texel of a `width × height` grid, the squared distance in texels to the nearest texel whose `wet`
/// is `sites`, and which that is: the first in a fixed scan of those as near; `None` where there's none at all.
/// Meijster, Roerdink and Hesselink's exact Euclidean transform, in integers: a column's pass, then a row's lower
/// envelope of parabolas.
fn distances(wet: &[bool], sites: bool, width: usize, height: usize) -> Vec<Option<(i64, usize)>> {
    if !wet.contains(&sites) {
        return vec![None; width * height];
    }
    // Past any real distance: a column with no site stands this far from every row.
    let far = (width + height) as i64;
    let mut column = vec![(far, 0usize); width * height];
    for i in 0..width {
        let mut last: Option<usize> = None;
        for j in 0..height {
            if wet[j * width + i] == sites {
                last = Some(j);
            }
            column[j * width + i] = last.map_or((far, 0), |row| ((j - row) as i64, row));
        }
        for j in (0..height.saturating_sub(1)).rev() {
            let (below, row) = column[(j + 1) * width + i];
            if below + 1 < column[j * width + i].0 {
                column[j * width + i] = (below + 1, row);
            }
        }
    }
    let mut out = vec![None; width * height];
    let (mut starts, mut owners) = (vec![0i64; width], vec![0usize; width]);
    for j in 0..height {
        let g = |i: usize| column[j * width + i].0;
        let f = |x: i64, i: usize| (x - i as i64) * (x - i as i64) + g(i) * g(i);
        let separation = |i: usize, u: usize| {
            let (i, u) = (i as i64, u as i64);
            (u * u - i * i + g(u as usize) * g(u as usize) - g(i as usize) * g(i as usize))
                .div_euclid(2 * (u - i))
        };
        let mut q = 0usize;
        owners[0] = 0;
        starts[0] = 0;
        for u in 1..width {
            loop {
                if f(starts[q], owners[q]) <= f(starts[q], u) {
                    let next = 1 + separation(owners[q], u);
                    if next < width as i64 {
                        q += 1;
                        owners[q] = u;
                        starts[q] = next;
                    }
                    break;
                }
                if q == 0 {
                    owners[0] = u;
                    break;
                }
                q -= 1;
            }
        }
        for u in (0..width).rev() {
            let owner = owners[q];
            let (_, row) = column[j * width + owner];
            out[j * width + u] = Some((f(u as i64, owner), row * width + owner));
            if u as i64 == starts[q] && q > 0 {
                q -= 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use sapphire_zone::WaterSurface;
    use sapphire_zone::water::drawn::ArchivedInfoTexture;

    use super::*;
    use crate::nav::Triangle;

    const SIZES: Sizes = Sizes { spacing: 2.0, texel: 1.0 };

    fn body(name: &str, shape: WaterShape, margin: f32) -> WaterBody {
        WaterBody {
            name: name.to_owned(),
            shape,
            surface: WaterSurface::Liquid,
            material: None,
            swim: true,
            probe: None,
            shore_margin: margin,
        }
    }

    fn lake(outline: &[[f32; 2]], height: f32, margin: f32) -> WaterBody {
        body("pond", WaterShape::Lake { outline: outline.to_vec(), height }, margin)
    }

    fn river(points: &[([f32; 3], f32, f32)], margin: f32) -> WaterBody {
        let points = points.iter().map(|&(at, width, speed)| RiverPoint { at, width, speed }).collect();
        body("race", WaterShape::River { points }, margin)
    }

    /// Falls 1 m along its first 20 m stretch, then turns left a quarter for 20 m more, widening from 4 m to 6.
    fn race() -> WaterBody {
        river(
            &[([0.0, 2.0, 0.0], 4.0, 2.0), ([20.0, 1.0, 0.0], 6.0, 2.0), ([20.0, 1.0, 20.0], 6.0, 1.0)],
            1.0,
        )
    }

    /// An L of 64 m², concave at (4, 4).
    const ELL: [[f32; 2]; 6] = [[0.0, 0.0], [10.0, 0.0], [10.0, 4.0], [4.0, 4.0], [4.0, 10.0], [0.0, 10.0]];

    /// A rectangle's two faces at `y`, facing up.
    fn rectangle(min: [f32; 2], max: [f32; 2], y: f32) -> [Triangle; 2] {
        [
            [[min[0], y, min[1]], [min[0], y, max[1]], [max[0], y, max[1]]],
            [[min[0], y, min[1]], [max[0], y, max[1]], [max[0], y, min[1]]],
        ]
    }

    /// A bed 1 m down west of x = 8, and a bank 3 m up east of it, with nothing under the bank.
    fn bed_and_bank() -> ColliderGround {
        let bed = rectangle([-50.0, -50.0], [8.0, 50.0], -1.0);
        let faces: Vec<Triangle> =
            bed.into_iter().chain(rectangle([8.0, -50.0], [50.0, 50.0], 3.0)).collect();
        ColliderGround::new(&faces).expect("small")
    }

    /// Twice each triangle's area as it faces up: positive when it faces up.
    fn facing(mesh: &WaterMesh) -> Vec<f64> {
        mesh.indices
            .chunks(3)
            .map(|triangle| {
                let [a, b, c] =
                    [0, 1, 2].map(|k| mesh.vertices[triangle[k] as usize].position.map(f64::from));
                let (ab, ac) = ([b[0] - a[0], b[2] - a[2]], [c[0] - a[0], c[2] - a[2]]);
                ab[1] * ac[0] - ab[0] * ac[1]
            })
            .collect()
    }

    fn twice_area(points: &[[f32; 2]]) -> f64 {
        let count = points.len();
        (0..count)
            .map(|index| {
                let (a, b) = (points[index].map(f64::from), points[(index + 1) % count].map(f64::from));
                a[0] * b[1] - b[0] * a[1]
            })
            .sum::<f64>()
            .abs()
    }

    /// A lake's mesh covers its outline grown by its margin and nothing else, every triangle facing up within
    /// one of the grid's squares, its UV its place and its height the lake's, each vertex once.
    #[test]
    fn a_lake_is_its_grown_outline_cut_by_the_grid_facing_up() {
        for margin in [0.0, 1.0, 2.5] {
            let mesh = cook(&lake(&ELL, 1.5, margin), None, SIZES).expect("cooks");
            let grown = grow(&ELL, margin);
            let faces = facing(&mesh);
            assert!(faces.iter().all(|&twice| twice > 0.0), "every triangle faces up");
            let covered: f64 = faces.iter().sum();
            assert!((covered - twice_area(&grown)).abs() < 1e-3, "{covered} against {}", twice_area(&grown));
            if margin == 0.0 {
                assert!((covered - 128.0).abs() < 1e-6, "the L itself: {covered}");
            }
            let points: Vec<[f64; 2]> = grown.iter().map(|point| point.map(f64::from)).collect();
            for triangle in mesh.indices.chunks(3) {
                let corners = [0, 1, 2].map(|k| mesh.vertices[triangle[k] as usize].position);
                let middle = [0, 2].map(|axis| corners.iter().map(|c| f64::from(c[axis])).sum::<f64>() / 3.0);
                assert!(inside(&points, middle), "{middle:?} is in the grown outline");
                let square = middle.map(|value| (value / 2.0).floor() * 2.0);
                for corner in corners {
                    for (axis, at) in [(0, corner[0]), (1, corner[2])] {
                        let at = f64::from(at);
                        assert!(
                            square[axis] <= at && at <= square[axis] + 2.0,
                            "{corner:?} within its square"
                        );
                    }
                }
            }
            let mut places = BTreeMap::new();
            for vertex in &mesh.vertices {
                assert_eq!(vertex.position[1], 1.5);
                assert_eq!(vertex.uv, [vertex.position[0], vertex.position[2]]);
                let place = [vertex.position[0].to_bits(), vertex.position[2].to_bits()];
                assert!(places.insert(place, ()).is_none(), "each place once");
            }
            assert_eq!(mesh.bounds_min[1], 1.5);
            WaterMesh::read(&mesh.to_cooked().expect("writes")).expect("reads back");

            // No T-junction: every edge inside is two triangles', and those only one's run round the grown outline,
            // as long as it. A vertex partway along another triangle's edge would leave that edge one triangle's.
            let mut edges: BTreeMap<(u32, u32), u32> = BTreeMap::new();
            for triangle in mesh.indices.chunks(3) {
                for k in 0..3 {
                    let (a, b) = (triangle[k], triangle[(k + 1) % 3]);
                    *edges.entry((a.min(b), a.max(b))).or_default() += 1;
                }
            }
            assert!(edges.values().all(|&count| count <= 2), "no edge is three triangles'");
            let length = |(a, b): (u32, u32)| {
                let [a, b] = [a, b].map(|index| mesh.vertices[index as usize].position.map(f64::from));
                ((b[0] - a[0]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
            };
            let open: f64 =
                edges.iter().filter(|(_, count)| **count == 1).map(|(&edge, _)| length(edge)).sum();
            let perimeter: f64 = (0..grown.len())
                .map(|k| {
                    let (a, b) = (grown[k].map(f64::from), grown[(k + 1) % grown.len()].map(f64::from));
                    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
                })
                .sum();
            assert!(
                (open - perimeter).abs() < 1e-3,
                "edges of one triangle: {open} m against {perimeter} m round"
            );
        }
        // Either way round, the same mesh.
        let backwards: Vec<[f32; 2]> = ELL.iter().rev().copied().collect();
        let one = cook(&lake(&ELL, 1.5, 1.0), None, SIZES).expect("cooks");
        let other = cook(&lake(&backwards, 1.5, 1.0), None, SIZES).expect("cooks");
        assert_eq!(one.vertices, other.vertices);
        assert_eq!(one.indices, other.indices, "and the same triangles, wound the same way");
    }

    /// A U whose arms stand 2 m apart, grown by more than half that, meets itself: refused by its margin.
    #[test]
    fn a_lake_grown_into_itself_is_refused_by_its_margin() {
        let you = [
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [6.0, 10.0],
            [6.0, 2.0],
            [4.0, 2.0],
            [4.0, 10.0],
            [0.0, 10.0],
        ];
        assert!(cook(&lake(&you, 0.0, 0.5), None, SIZES).is_ok());
        let (field, problem) = cook(&lake(&you, 0.0, 1.5), None, SIZES).expect_err("refused");
        assert_eq!(field, "shore_margin");
        assert!(problem.contains("meets itself"), "{problem}");
    }

    /// A river's ribbon: rows across it down its length, its UV's v the metres downstream (from minus its margin
    /// to its length and its margin) and its u the metres across (left negative), every triangle facing up, each
    /// row's middle downstream of the last's along the current.
    #[test]
    fn a_rivers_ribbon_runs_its_uv_with_its_flow() {
        let body = race();
        let mesh = cook(&body, None, SIZES).expect("cooks");
        assert!(facing(&mesh).iter().all(|&twice| twice > 0.0), "every triangle faces up");
        let row = 5; // ceil((6 + 2) / 2) columns
        assert_eq!(mesh.vertices.len() % row, 0);
        let rows: Vec<&[WaterVertex]> = mesh.vertices.chunks(row).collect();
        assert_eq!(rows[0][0].uv, [-3.0, -1.0], "half the first width and the margin, a margin upstream");
        assert_eq!(rows[rows.len() - 1][row - 1].uv, [4.0, 41.0]);
        assert_eq!((rows[0][0].position[1], rows[rows.len() - 1][0].position[1]), (2.0, 1.0));
        for pair in rows.windows(2) {
            assert!(pair[1][0].uv[1] > pair[0][0].uv[1], "v grows downstream");
            let (here, next) = (pair[0][row / 2].position, pair[1][row / 2].position);
            let between = [(here[0] + next[0]) * 0.5, (here[2] + next[2]) * 0.5];
            if let Some((_, flow)) = body.shape.column(between[0], between[1]) {
                let way = [next[0] - here[0], next[2] - here[2]];
                assert!(way[0] * flow[0] + way[1] * flow[1] > 0.0, "with the current at {between:?}");
            }
        }
        for vertices in &rows {
            assert!(vertices.iter().all(|vertex| vertex.uv[1] == vertices[0].uv[1]), "a row is one v");
            assert!(vertices.windows(2).all(|pair| pair[1].uv[0] > pair[0].uv[0]), "u grows across");
            assert_eq!(vertices[row / 2].uv[0], 0.0, "the middle");
        }
        // On the first stretch, running +X, left is -Z.
        let first = rows[2];
        assert!(first[0].position[2] < first[row - 1].position[2] && first[0].uv[0] < 0.0);
    }

    /// A river that turns back on itself, or so tightly for its width that its inner bank folds over the stretch
    /// beside it, is refused at that point.
    #[test]
    fn a_river_that_folds_or_turns_back_is_refused_at_its_point() {
        let back =
            river(&[([0.0; 3], 4.0, 1.0), ([20.0, 0.0, 0.0], 4.0, 1.0), ([10.0, 0.0, 0.0], 4.0, 1.0)], 1.0);
        let (field, problem) = cook(&back, None, SIZES).expect_err("refused");
        assert_eq!(field, "shape.points[1]");
        assert!(problem.contains("turns back"), "{problem}");

        let tight =
            river(&[([0.0; 3], 4.0, 1.0), ([20.0, 0.0, 0.0], 4.0, 1.0), ([0.0, 0.0, 3.5], 4.0, 1.0)], 1.0);
        let (field, problem) = cook(&tight, None, SIZES).expect_err("refused");
        assert_eq!(field, "shape.points[1]");
        assert!(problem.contains("folds back"), "{problem}");
        // Eased, the same turn draws.
        let eased =
            river(&[([0.0; 3], 4.0, 1.0), ([20.0, 0.0, 0.0], 4.0, 1.0), ([10.0, 0.0, 20.0], 4.0, 1.0)], 1.0);
        assert!(cook(&eased, None, SIZES).is_ok());

        // A horseshoe whose two arms' banks meet, 12 m apart and each 6 m to its bank.
        let shoe = [([0.0; 3], 10.0, 1.0), ([30.0, 0.0, 0.0], 10.0, 1.0)];
        let shoe =
            river(&[shoe[0], shoe[1], ([30.0, 0.0, 12.0], 10.0, 1.0), ([0.0, 0.0, 12.0], 10.0, 1.0)], 1.0);
        let (field, problem) = cook(&shoe, None, SIZES).expect_err("refused");
        assert_eq!(field, "shape.points[2]");
        assert!(problem.contains("from point 0"), "{problem}");

        // An S's two bends fold onto opposite banks, so a stretch shorter than their folds together draws.
        let ess = [([0.0; 3], 10.0, 1.0), ([10.0, 0.0, 0.0], 10.0, 1.0), ([13.0, 0.0, 3.0], 10.0, 1.0)];
        let ess = river(&[ess[0], ess[1], ess[2], ([23.0, 0.0, 3.0], 10.0, 1.0)], 1.0);
        let mesh = cook(&ess, None, SIZES).expect("an S draws");
        assert!(facing(&mesh).iter().all(|&twice| twice > 0.0), "every triangle faces up");
    }

    fn texel(info: &ArchivedInfoTexture, at: [f32; 2]) -> [u8; 4] {
        let texel = info.texel.to_native();
        let [i, j] = [0, 1].map(|axis| ((at[axis] - info.origin[axis].to_native()) / texel).floor() as u32);
        info.texel(i, j)
    }

    /// The info texture: R the depth to the colliders' bed under the surface (0 on land, the bank standing over
    /// it with nothing under), G the signed distance to the shore, B and A the current, each texel at its centre.
    #[test]
    fn the_info_texture_holds_the_bed_the_shore_and_the_flow() {
        let ground = bed_and_bank();
        let pond = lake(&[[0.0, 0.0], [16.0, 0.0], [16.0, 16.0], [0.0, 16.0]], 1.0, 1.0);
        let bytes = cook(&pond, Some(&ground), SIZES).expect("cooks").to_cooked().expect("writes");
        let mesh = WaterMesh::read(&bytes).expect("reads");
        let info = &mesh.info;
        let texel_size = info.texel.to_native();
        assert_eq!(texel_size, 1.0);
        assert!(info.origin.iter().all(|value| value.to_native() % texel_size == 0.0), "on the texel grid");
        assert_eq!(info.flow_scale.to_native(), 0.0, "a lake is still");
        let (width, height) = (info.width.to_native(), info.height.to_native());
        let mut wet = 0;
        for j in 0..height {
            for i in 0..width {
                let centre = [i, j].map(|k| k as f32 + 0.5);
                let [x, z] = [0, 1].map(|axis| info.origin[axis].to_native() + centre[axis] * texel_size);
                let [depth, shore, along, across] = info.texel(i, j);
                assert_eq!([along, across], [SIGNED_ZERO; 2]);
                let expected = pond.shape.column(x, z).and_then(|(surface, _)| {
                    let (bed, _) = ground.under(x, z, surface);
                    bed.filter(|&bed| bed < f64::from(surface))
                        .map(|bed| ((f64::from(surface) - bed) / f64::from(DEPTH_STEP)).round() as u8)
                });
                assert_eq!(depth, expected.unwrap_or(0), "the depth at ({x}, {z})");
                assert_eq!(shore >= SIGNED_ZERO, expected.is_some(), "the shore's side at ({x}, {z})");
                wet += usize::from(expected.is_some());
            }
        }
        assert_eq!(wet, 8 * 16, "west of the bank");
        assert_eq!(texel(info, [4.5, 8.5])[0], 20, "2 m deep");
        assert_eq!(ArchivedInfoTexture::shore(texel(info, [4.5, 8.5])[1]), 3.5, "the bank's 3.5 m east");
        assert_eq!(
            ArchivedInfoTexture::shore(texel(info, [0.5, 8.5])[1]),
            0.5,
            "a texel from the edge, less half"
        );
        assert_eq!(ArchivedInfoTexture::shore(texel(info, [10.5, 8.5])[1]), -2.5, "on the bank");
        assert_eq!(texel(info, [12.5, 8.5])[0], 0, "land");

        // With no colliders, no floor anywhere: as deep as R says.
        let mesh = cook(&pond, None, SIZES).expect("cooks");
        let bytes = mesh.to_cooked().expect("writes");
        let info = &WaterMesh::read(&bytes).expect("reads").info;
        assert_eq!(texel(info, [12.5, 8.5])[0], u8::MAX);

        // A river's current, each way scaled by its fastest.
        let body = race();
        let bytes = cook(&body, None, SIZES).expect("cooks").to_cooked().expect("writes");
        let info = &WaterMesh::read(&bytes).expect("reads").info;
        assert_eq!(info.flow_scale.to_native(), 2.0);
        for at in [[10.5, 0.5], [20.5, 12.5], [-0.5, 0.5]] {
            let [_, _, along, across] = texel(info, at);
            let (_, flow) = body.shape.column(at[0], at[1]).unwrap_or_else(|| {
                // Off the water: the nearest wet texel's, which a texel downstream holds.
                body.shape.column(at[0] + 1.0, at[1]).expect("by the water")
            });
            for (byte, flow) in [(along, flow[0]), (across, flow[1])] {
                let read = info.flow(byte);
                assert!((read - flow).abs() <= 2.0 / 127.0, "{at:?}: {read} against {flow}");
            }
        }
    }

    /// A rock standing on the bed through the surface is land: no depth there, and the shore's distance runs to
    /// it.
    #[test]
    fn a_rock_standing_through_the_surface_is_land() {
        let pond = lake(&[[0.0, 0.0], [16.0, 0.0], [16.0, 16.0], [0.0, 16.0]], 1.0, 1.0);
        let rock: Vec<Triangle> = rectangle([2.0, 2.0], [4.0, 4.0], 2.0)
            .into_iter()
            .chain(rectangle([2.0, 2.0], [4.0, 4.0], -2.0))
            .collect();
        let bed = rectangle([-50.0, -50.0], [8.0, 50.0], -1.0);
        let faces: Vec<Triangle> = bed
            .into_iter()
            .chain(rectangle([8.0, -50.0], [50.0, 50.0], 3.0))
            .chain(rock.iter().copied())
            .collect();
        let ground = ColliderGround::with_solids(&faces, &[rock]).expect("small");
        let bytes = cook(&pond, Some(&ground), SIZES).expect("cooks").to_cooked().expect("writes");
        let info = &WaterMesh::read(&bytes).expect("reads").info;
        let [depth, shore, ..] = texel(info, [3.5, 3.5]);
        assert_eq!(depth, 0, "the rock is land");
        assert!(shore < SIGNED_ZERO, "on the rock's side of the shore");
        assert_eq!(ArchivedInfoTexture::shore(texel(info, [5.5, 3.5])[1]), 1.5, "the rock's 1.5 m west");
        assert_eq!(texel(info, [5.5, 3.5])[0], 20, "beside it, the bed");
        // Without it as a solid, its top over the bed is a bridge: water under it.
        let bridge = ColliderGround::new(&faces).expect("small");
        let bytes = cook(&pond, Some(&bridge), SIZES).expect("cooks").to_cooked().expect("writes");
        assert_eq!(
            texel(&WaterMesh::read(&bytes).expect("reads").info, [3.5, 3.5])[0],
            20,
            "the bed under it"
        );
    }

    /// Meijster's transform finds the same nearest distances as looking at every texel.
    #[test]
    fn the_distance_transform_is_exact() {
        let mut seed = 0x2545_f491_u64;
        for (width, height) in [(1, 1), (1, 7), (7, 1), (13, 9), (32, 17), (5, 40)] {
            let wet: Vec<bool> = (0..width * height)
                .map(|_| {
                    seed =
                        seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                    (seed >> 33) % 5 < 2
                })
                .collect();
            for sites in [false, true] {
                let found = distances(&wet, sites, width, height);
                for (here, &found) in found.iter().enumerate() {
                    let (x, z) = ((here % width) as i64, (here / width) as i64);
                    let brute = (0..width * height)
                        .filter(|&other| wet[other] == sites)
                        .map(|other| {
                            let (ox, oz) = ((other % width) as i64, (other / width) as i64);
                            (x - ox) * (x - ox) + (z - oz) * (z - oz)
                        })
                        .min();
                    assert_eq!(found.map(|(squared, _)| squared), brute, "{width}×{height} at {here}");
                    if let Some((squared, site)) = found {
                        let (ox, oz) = ((site % width) as i64, (site / width) as i64);
                        assert_eq!(wet[site], sites);
                        assert_eq!((x - ox) * (x - ox) + (z - oz) * (z - oz), squared, "it names that site");
                    }
                }
            }
        }
    }

    /// A body too large for its squares or its texels doubles them; a zone past its info budget doubles its
    /// largest body's texel until it fits; the sea has no sizes.
    #[test]
    fn sizes_double_for_a_large_body_and_a_full_zone() {
        let wide = lake(&[[0.0, 0.0], [2000.0, 0.0], [2000.0, 2000.0], [0.0, 2000.0]], 0.0, 1.0);
        let sea = body("sea", WaterShape::Ocean { height: 0.0 }, 1.0);
        let small = Some(Sizes { spacing: 2.0, texel: 1.0 });
        assert_eq!(sizes(&[race(), sea.clone()], Cook::Desktop), [small, None]);
        assert_eq!(sizes(&[race()], Cook::Mobile), [Some(Sizes { spacing: 4.0, texel: 2.0 })]);
        let wide4 = Some(Sizes { spacing: 8.0, texel: 4.0 });
        assert_eq!(sizes(std::slice::from_ref(&wide), Cook::Desktop), [wide4]);
        assert_eq!(sizes(std::slice::from_ref(&wide), Cook::Mobile), [wide4]);
        // Sixteen of them pass the desktops' 12 MiB of meshes: each doubles its squares until they fit.
        let many = vec![wide.clone(); 16];
        let total: u64 = (sizes(&many, Cook::Desktop).iter().zip(&many))
            .map(|(sizes, body)| quads(body, sizes.expect("a lake's").spacing) * QUAD_BYTES)
            .sum();
        assert!(total <= mesh_budget(Cook::Desktop), "{total}");
        assert!(sizes(&many, Cook::Desktop).iter().all(|sizes| sizes.expect("a lake's").spacing == 16.0));
        // Two of them pass the phones' 3 MiB of meshes and 1 MiB of texels: the first doubles its squares and its
        // texels, then the second, now the larger.
        let two = [wide.clone(), sea, wide];
        let wide8 = Some(Sizes { spacing: 16.0, texel: 8.0 });
        assert_eq!(sizes(&two, Cook::Mobile), [wide8, None, wide8]);
        assert_eq!(sizes(&two, Cook::Desktop), [wide4, None, wide4], "the desktops' 4 MiB holds both");
        for cook in [Cook::Desktop, Cook::Mobile] {
            let sizes = sizes(&two, cook);
            let total: u64 = (sizes.iter().zip(&two))
                .filter_map(|(sizes, body)| sizes.map(|sizes| rect(body, sizes.texel).texels() * 4))
                .sum();
            assert!(total <= info_budget(cook), "{total}");
            let mesh = super::cook(&two[0], None, sizes[0].expect("a lake's")).expect("cooks");
            assert!(mesh.indices.len() as u64 <= MAX_QUADS * 2 * 3 * 2, "{} indices", mesh.indices.len());
            assert!(mesh.info.width <= MAX_INFO_SIDE && mesh.info.height <= MAX_INFO_SIDE);
        }
    }

    /// The same body cooks to the same bytes every time, pinned: they're sums, products, quotients and roots of
    /// its values, never an order a hash or a thread chose. It moves when the cook's output does; bump
    /// [`WATER_COOKER`] with it.
    #[test]
    fn a_body_cooks_to_the_same_bytes_and_is_pinned() {
        let ground = bed_and_bank();
        let digest = |body: &WaterBody| {
            let bytes = cook(body, Some(&ground), SIZES).expect("cooks").to_cooked().expect("writes");
            blake3::hash(&bytes).to_hex().to_string()
        };
        let pond = lake(&ELL, 1.5, 1.0);
        for body in [&pond, &race()] {
            assert_eq!(digest(body), digest(body));
        }
        assert_eq!(
            digest(&pond),
            "e7f58b12568fe15518087fa2ae3f959fc1d0b4602954f75bd974734a6d01b0ef",
            "the pond's bytes moved"
        );
        assert_eq!(
            digest(&race()),
            "c26d2014cd3ab5985f01d9335d9d9c18c74b0e611a2480d5a9fe12c70eca0436",
            "the race's bytes moved"
        );
    }

    /// A surface's key holds its body's ID, which its bytes hold, the colliders' triangulation (G1b's rule for
    /// what stands on their triangles), and its sizes.
    #[test]
    fn a_surfaces_key_holds_its_id_the_triangulation_and_its_sizes() {
        let body = race();
        let sizes = sizes(std::slice::from_ref(&body), Cook::Desktop)[0];
        let key = recipe(&body, sizes);
        assert_eq!(key, recipe_on(&body, sizes, crate::nav::TRIANGULATION));
        assert_ne!(
            key,
            recipe_on(&body, sizes, crate::nav::TRIANGULATION + 1),
            "a new triangulation recooks it"
        );
        let coarser = sizes.map(|sizes| Sizes { spacing: sizes.spacing * 2.0, ..sizes });
        assert_ne!(key, recipe(&body, coarser), "so do new sizes");
        let renamed = WaterBody { name: format!("{}_2", body.name), ..body.clone() };
        assert_ne!(key, recipe(&renamed, sizes), "a renamed body, or a second of its outline, recooks");
    }
}
