//! A cell's levels of detail (M5 D6): what each level locks, and that two cells cooked apart meet at
//! every two levels with no crack.

use std::collections::BTreeSet;

use sapphire_assets::mesh::{VERTEX_COLOR_NEUTRAL, VertexFlags};

use super::*;
use crate::bake;
use crate::optimize::LodLevel;
use crate::test_util::material;

/// The test grid's side, in metres.
const SIDE: f32 = 16.0;
const GRID: Grid = Grid { cell_size: SIDE };
/// Where the two test cells meet: cell (0, 0) runs up to it in X, cell (1, 0) on from it.
const SEAM: f32 = SIDE;
const GROUND: u32 = 0;
const WALLS: u32 = 1;

/// The district's chain, as its document asks (`bench/district/district.ron`).
fn settings() -> ZoneSettings {
    let level = |triangle_ratio, max_error| LodLevel { triangle_ratio, max_error };
    let levels = vec![level(0.5, 0.0005), level(0.25, 0.002), level(0.125, 0.006)];
    ZoneSettings { lods: LodSettings::new(levels, 0).unwrap(), ..ZoneSettings::default() }
}

/// A test cell's look, drawn from a seed.
#[derive(Clone, Copy, Debug)]
struct Look {
    seed: u32,
    /// Ground quads along a cell's side: neighbours share it, so their level 0s meet vertex for vertex.
    quads: u32,
    /// How far the ground rises and falls, in metres.
    hills: f32,
    /// Where the wall cut at the seam stands, in Z, and how high it is.
    wall: (f32, f32),
    /// Where the pipe cut at the seam runs, in Z, and its radius.
    pipe: (f32, f32),
}

impl Look {
    fn new(seed: u32) -> Self {
        let pick = |salt: u32| unit(hash(&[seed, salt]));
        Self {
            seed,
            quads: [16, 32][(hash(&[seed, 0]) % 2) as usize],
            hills: 0.05 + 0.4 * pick(1),
            wall: (2.0 + 5.0 * pick(2), 1.0 + 3.0 * pick(3)),
            pipe: (9.0 + 5.0 * pick(4), 0.3 + 0.4 * pick(5)),
        }
    }

    /// The ground's height at `(x, z)` in the zone's space: the same function in every cell, so the cells
    /// agree where they meet.
    fn height(&self, x: f32, z: f32) -> f32 {
        let phase = self.seed as f32 * 0.37;
        self.hills * (det::sin(0.31 * x + phase) * det::cos(0.23 * z - phase) + 0.3 * det::sin(0.9 * (x + z)))
    }
}

fn hash(values: &[u32]) -> u32 {
    values.iter().fold(0x9e37_79b9_u32, |hash, &value| {
        let mixed = (hash ^ value).wrapping_mul(0x85eb_ca6b);
        (mixed ^ (mixed >> 13)).wrapping_mul(0xc2b2_ae35) ^ (mixed >> 16)
    })
}

fn unit(hash: u32) -> f32 {
    (hash >> 8) as f32 / (1u32 << 24) as f32
}

/// A mesh being built by material, as a zone's merge writes them.
struct Builder {
    vertices: Vec<Vertex>,
    by_material: [Vec<u32>; 2],
}

impl Builder {
    fn new() -> Self {
        Self { vertices: Vec::new(), by_material: [Vec::new(), Vec::new()] }
    }

    fn vertex(&mut self, position: Vec3, normal: Vec3) -> u32 {
        self.vertices.push(Vertex {
            position: position.to_array(),
            normal: normal.to_array(),
            outline_normal: normal.to_array(),
            uv: [position.x / 4.0, (position.z + position.y) / 4.0],
            color: VERTEX_COLOR_NEUTRAL,
            flags: VertexFlags::NONE,
        });
        (self.vertices.len() - 1) as u32
    }

    /// A quad `a b c d` round its normal's way, of its own four vertices (a hard edge all round).
    fn face(&mut self, material: u32, corners: [Vec3; 4]) {
        let normal = (corners[1] - corners[0]).cross(corners[3] - corners[0]).normalize();
        let [a, b, c, d] = corners.map(|corner| self.vertex(corner, normal));
        self.by_material[material as usize].extend([a, b, c, a, c, d]);
    }

    /// A box from `min` to `max`, its long side along X cut into `segments`, each face of its own vertices.
    fn wall(&mut self, min: Vec3, max: Vec3, segments: u32) {
        let x = |k: u32| min.x + (max.x - min.x) * k as f32 / segments as f32;
        for k in 0..segments {
            let (x0, x1) = (x(k), x(k + 1));
            let v = |x, y, z| Vec3::new(x, y, z);
            // Top, front (+Z), back (-Z) and bottom, each a strip.
            self.face(
                WALLS,
                [v(x0, max.y, min.z), v(x0, max.y, max.z), v(x1, max.y, max.z), v(x1, max.y, min.z)],
            );
            self.face(
                WALLS,
                [v(x0, min.y, max.z), v(x1, min.y, max.z), v(x1, max.y, max.z), v(x0, max.y, max.z)],
            );
            self.face(
                WALLS,
                [v(x1, min.y, min.z), v(x0, min.y, min.z), v(x0, max.y, min.z), v(x1, max.y, min.z)],
            );
            self.face(
                WALLS,
                [v(x0, min.y, min.z), v(x1, min.y, min.z), v(x1, min.y, max.z), v(x0, min.y, max.z)],
            );
        }
        let v = |x, y, z| Vec3::new(x, y, z);
        let (x0, x1) = (min.x, max.x);
        self.face(
            WALLS,
            [v(x0, min.y, min.z), v(x0, min.y, max.z), v(x0, max.y, max.z), v(x0, max.y, min.z)],
        );
        self.face(
            WALLS,
            [v(x1, min.y, max.z), v(x1, min.y, min.z), v(x1, max.y, min.z), v(x1, max.y, max.z)],
        );
    }

    /// A pipe of `radius` along X from `from` to `to` at `(y, z)`, smooth round its side, which is cut into
    /// `segments` along it, and closed by a flat cap at each end: its caps' rims are seams that may slide along
    /// themselves, as a round piece's foot or end does.
    fn pipe(&mut self, from: f32, to: f32, (y, z): (f32, f32), radius: f32, segments: u32) {
        const SIDES: u32 = 24;
        let round = |side: u32| {
            let (sin, cos) = det::sin_cos(std::f32::consts::TAU * side as f32 / SIDES as f32);
            Vec3::new(0.0, cos, sin)
        };
        let x = |k: u32| from + (to - from) * k as f32 / segments as f32;
        let first = self.vertices.len() as u32;
        for k in 0..=segments {
            for side in 0..SIDES {
                let normal = round(side);
                self.vertex(Vec3::new(x(k), y, z) + normal * radius, normal);
            }
        }
        for k in 0..segments {
            for side in 0..SIDES {
                let at = |k: u32, side: u32| first + k * SIDES + side % SIDES;
                let (a, b, c, d) = (at(k, side), at(k, side + 1), at(k + 1, side), at(k + 1, side + 1));
                self.by_material[WALLS as usize].extend([a, b, c, b, d, c]);
            }
        }
        for (end, facing) in [(from, -1.0), (to, 1.0)] {
            let normal = Vec3::X * facing;
            let middle = self.vertex(Vec3::new(end, y, z), normal);
            let rim: Vec<u32> = (0..SIDES)
                .map(|side| self.vertex(Vec3::new(end, y, z) + round(side) * radius, normal))
                .collect();
            for side in 0..SIDES as usize {
                let (a, b) = (rim[side], rim[(side + 1) % SIDES as usize]);
                let corners = if facing > 0.0 { [middle, a, b] } else { [middle, b, a] };
                self.by_material[WALLS as usize].extend(corners);
            }
        }
    }

    fn finish(self) -> ImportedMesh {
        let mut mesh = ImportedMesh {
            materials: vec![material("ground"), material("walls")],
            ..ImportedMesh::default()
        };
        let mut submeshes = Vec::new();
        for (material, indices) in self.by_material.into_iter().enumerate() {
            let first_index = mesh.indices.len() as u32;
            mesh.indices.extend(indices);
            let index_count = mesh.indices.len() as u32 - first_index;
            submeshes.push(Submesh { first_index, index_count, material: material as u32 });
        }
        mesh.authored_outline = vec![false; self.vertices.len()];
        mesh.lods = vec![Lod { vertex_count: self.vertices.len() as u32, error: 0.0, submeshes }];
        mesh.vertices = self.vertices;
        mesh
    }
}

/// Cell `(cell, 0)` of `look`, merged: its ground, the halves of a wall and of a pipe cut at the seam that
/// stand in it, and, in cell 0, a box reaching across the seam into cell 1. Coordinates come from whole steps of
/// powers of two, so the cells' shared vertices are the same bits.
fn cell(look: &Look, cell: u32) -> ImportedMesh {
    let mut builder = Builder::new();
    let step = SIDE / look.quads as f32;
    let first = cell * look.quads;
    let mut row = Vec::new();
    for k in 0..=look.quads {
        for i in first..=first + look.quads {
            let (x, z) = (i as f32 * step, k as f32 * step);
            let e = 0.01;
            let dx = (look.height(x + e, z) - look.height(x - e, z)) / (2.0 * e);
            let dz = (look.height(x, z + e) - look.height(x, z - e)) / (2.0 * e);
            let normal = Vec3::new(-dx, 1.0, -dz).normalize();
            row.push(builder.vertex(Vec3::new(x, look.height(x, z), z), normal));
        }
    }
    let stride = look.quads + 1;
    for k in 0..look.quads {
        for i in 0..look.quads {
            let a = row[(k * stride + i) as usize];
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            builder.by_material[GROUND as usize].extend([a, b, c, c, b, d]);
        }
    }
    let (z, height) = look.wall;
    let reach = 8.0;
    let (from, to) = if cell == 0 { (SEAM - reach, SEAM) } else { (SEAM, SEAM + reach) };
    builder.wall(Vec3::new(from, 0.0, z - 0.25), Vec3::new(to, height, z + 0.25), 16);
    let (along, radius) = look.pipe;
    builder.pipe(from, to, (2.0, along), radius, 16);
    if cell == 0 {
        let z = SIDE - z;
        builder.wall(Vec3::new(SEAM - 2.0, 0.0, z - 0.5), Vec3::new(SEAM + 1.5, 1.0, z + 0.5), 8);
    }
    builder.finish()
}

/// `mesh` cooked as a cell's is, with its zone's grid.
fn cooked(mut mesh: ImportedMesh, settings: &ZoneSettings) -> ImportedMesh {
    bake::outline_normals(&mut mesh, &settings.outline);
    optimize_place(&mut mesh, settings, Some(GRID), Cook::Desktop);
    mesh
}

type Bits = [u32; 3];

fn bits(position: [f32; 3]) -> Bits {
    position.map(f32::to_bits)
}

/// Level `lod`'s triangles in `material`, as their corners' positions.
fn triangles(mesh: &ImportedMesh, lod: usize, material: u32) -> Vec<[[f32; 3]; 3]> {
    let lod = &mesh.lods[lod];
    let submeshes = lod.submeshes.iter().filter(|submesh| submesh.material == material);
    submeshes
        .flat_map(|submesh| {
            let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
            mesh.indices[range]
                .as_chunks::<3>()
                .0
                .iter()
                .map(|corners| corners.map(|index| mesh.vertices[index as usize].position))
        })
        .collect()
}

/// The positions level `lod` draws.
fn drawn(mesh: &ImportedMesh, lod: usize) -> BTreeSet<Bits> {
    let corners = [GROUND, WALLS].into_iter().flat_map(|material| triangles(mesh, lod, material));
    corners.flatten().map(bits).collect()
}

/// The edges of level `lod`'s triangles in `material` that only one triangle has, by position.
fn open_edges(mesh: &ImportedMesh, lod: usize, material: u32) -> Vec<[Bits; 2]> {
    let mut count = BTreeMap::<[Bits; 2], u32>::new();
    for corners in triangles(mesh, lod, material) {
        for edge in 0..3 {
            let mut ends = [bits(corners[edge]), bits(corners[(edge + 1) % 3])];
            ends.sort_unstable();
            *count.entry(ends).or_default() += 1;
        }
    }
    count.into_iter().filter(|(_, count)| *count == 1).map(|(edge, _)| edge).collect()
}

/// Level 0's positions no level may move: its open border, by material, and whatever lies on a line of
/// the grid.
fn locked(mesh: &ImportedMesh) -> BTreeSet<Bits> {
    let border = [GROUND, WALLS].into_iter().flat_map(|material| open_edges(mesh, 0, material)).flatten();
    let on_line = |value: f32| (value - (value / SIDE).round() * SIDE).abs() <= GRID_LINE_TOLERANCE;
    let lines = drawn(mesh, 0).into_iter().filter(|bits| {
        let position = bits.map(f32::from_bits);
        on_line(position[0]) || on_line(position[2])
    });
    border.chain(lines).collect()
}

/// The ground's open edges on the seam's plane, at level `lod`, as `(z, y)` pairs, least `z` first.
fn seam_edges(mesh: &ImportedMesh, lod: usize) -> Vec<[[f32; 2]; 2]> {
    let on_seam = |bits: &Bits| bits[0] == SEAM.to_bits();
    open_edges(mesh, lod, GROUND)
        .into_iter()
        .filter(|ends| ends.iter().all(on_seam))
        .map(|ends| {
            let mut ends = ends.map(|bits| [f32::from_bits(bits[2]), f32::from_bits(bits[1])]);
            ends.sort_by(|a, b| a[0].total_cmp(&b[0]));
            ends
        })
        .collect()
}

/// Whether `ours`' every edge is an edge or a chain of edges of `theirs` with the same ends: no end of
/// theirs falls inside one of ours (a T-junction, where a crack opens), and each of our ends is one of theirs.
fn meets(ours: &[[[f32; 2]; 2]], theirs: &[[[f32; 2]; 2]]) -> Result<(), String> {
    let ends: BTreeSet<[u32; 2]> = theirs.iter().flatten().map(|end| end.map(f32::to_bits)).collect();
    for [a, b] in ours {
        for end in [a, b] {
            if !ends.contains(&end.map(f32::to_bits)) {
                return Err(format!("our end {end:?} isn't one of theirs"));
            }
        }
        if let Some(inside) = theirs.iter().flatten().find(|end| end[0] > a[0] && end[0] < b[0]) {
            return Err(format!("their end {inside:?} lies inside our edge {a:?}–{b:?}: a T-junction"));
        }
    }
    Ok(())
}

/// The walls' positions on the seam's plane at level `lod`.
fn wall_ends(mesh: &ImportedMesh, lod: usize) -> BTreeSet<Bits> {
    let corners = triangles(mesh, lod, WALLS).into_iter().flatten().map(bits);
    corners.filter(|bits| bits[0] == SEAM.to_bits()).collect()
}

#[test]
fn a_cells_border_and_the_grids_lines_stay_where_they_are_at_every_level() {
    let settings = settings();
    for seed in 0..12 {
        let look = Look::new(seed);
        for index in 0..2 {
            let mesh = cooked(cell(&look, index), &settings);
            assert!(mesh.lods.len() >= 2, "{look:?}, cell {index}: {} levels", mesh.lods.len());
            let locked = locked(&mesh);
            assert!(locked.len() > 4 * look.quads as usize, "the ground's border at least");
            for lod in 1..mesh.lods.len() {
                let drawn = drawn(&mesh, lod);
                let missing: Vec<_> = locked.difference(&drawn).collect();
                assert!(missing.is_empty(), "{look:?}, cell {index}, level {lod} lost {missing:?}");
            }
            // A level may simplify only the pieces where the ground's hills are steeper than its error, but
            // the coarsest simplifies the ground too: the lock holds its border, not its inside.
            let coarsest = mesh.lods.len() - 1;
            let (first, last) = (triangles(&mesh, 0, GROUND).len(), triangles(&mesh, coarsest, GROUND).len());
            assert!(
                last < first,
                "{look:?}, cell {index}, level {coarsest}: the ground went from {first} to {last}"
            );
        }
    }
}

#[test]
fn two_cells_cooked_apart_meet_at_every_two_levels() {
    let settings = settings();
    for seed in 0..12 {
        let look = Look::new(seed);
        let [left, right] = [0, 1].map(|index| cooked(cell(&look, index), &settings));
        assert_eq!(
            seam_edges(&left, 0).len(),
            look.quads as usize,
            "{look:?}: the seam is the ground's border"
        );
        for i in 0..left.lods.len() {
            for j in 0..right.lods.len() {
                let (ours, theirs) = (seam_edges(&left, i), seam_edges(&right, j));
                let pair = format!("{look:?}, levels {i} and {j}");
                meets(&ours, &theirs).unwrap_or_else(|problem| panic!("{pair}: {problem}"));
                meets(&theirs, &ours).unwrap_or_else(|problem| panic!("{pair}, the other way: {problem}"));
                assert_eq!(wall_ends(&left, i), wall_ends(&right, j), "{pair}: the wall cut at the seam");
            }
        }
    }
}

/// Without the grid's lines, a cell's ground still keeps its border (meshoptimizer's `LockBorder`), and a box
/// its corners, but a round piece cut at the seam is free to slide its cap's rim along itself, so its end no
/// longer matches the next cell's: what the lines are for. (Closed, it leaves no hole, only a step.)
#[test]
fn the_grids_lines_hold_what_the_border_alone_doesnt() {
    let settings = settings();
    let look = Look { pipe: (12.0, 0.6), ..Look::new(3) };
    let mut free = cell(&look, 1);
    bake::outline_normals(&mut free, &settings.outline);
    optimize_place(&mut free, &settings, None, Cook::Desktop);
    let held = cooked(cell(&look, 1), &settings);
    let last = |mesh: &ImportedMesh| mesh.lods.len() - 1;
    assert_eq!(wall_ends(&held, last(&held)), wall_ends(&held, 0));
    assert_ne!(wall_ends(&free, last(&free)), wall_ends(&free, 0), "the pipe's end moves without them");
    assert_eq!(seam_edges(&free, last(&free)), seam_edges(&free, 0), "the border holds without them");
}

/// A zone that asks for no levels cooks a cell with one, as before levels: nothing is locked, and nothing
/// would be different if it were.
#[test]
fn a_cell_without_levels_has_one() {
    let mesh = cooked(cell(&Look::new(0), 0), &ZoneSettings::default());
    assert_eq!(mesh.lods.len(), 1);
    assert_eq!(mesh.lods[0].error, 0.0);
}
