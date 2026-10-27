//! Parametric builtins (M1 X1 part 2): East Asian architecture an assembly builds from numbers,
//! sized per building, where a kit would ship fixed pieces.
//!
//! A placement's `shape` (in place of a `piece`) is one of [`Shape`]'s, each a builtin whose
//! parameters say its size and form:
//!
//! - `#roof` ([`Roof`]): hip, hip-and-gable, gable and skirt roofs with a concave slope, corners
//!   that sweep up at the eaves, modeled rows of tiles, a ridge with end ornaments, hip ridges,
//!   timber soffits and corner beams.
//! - `#hall` ([`Hall`]): a building's walls: plaster on a stone plinth, timber posts and beams,
//!   and openings (lattice windows, moon windows, doors, moon gates) recessed into it.
//! - `#wall` ([`Wall`]): a freestanding wall with a tiled coping, its openings through it.
//! - `#column` ([`Column`]): a round timber column on a stone drum, a block on its top.
//! - `#lantern` ([`Lantern`]): paper lanterns, round, drum or square, hanging or on a post.
//!
//! Every shape is built in its own meters about its origin (a building's walls stand on it, a
//! roof sits on its walls' tops), so a placement normally places it unscaled; a scale scales its
//! geometry and its texture coordinates alike. Shapes draw in **parts** ([`Part`]): a part is
//! what a glTF piece's material is to surface rules, so `(pieces: ["#roof"], material:
//! Some("tiles"), slot: "roof_tile")` sends a roof's tiles to a slot, variants included, and a
//! placement's `slot` still takes everything. Faces nobody sees from outside (undersides on the
//! ground, the insides of walls) aren't built. A shape's faces never overlap one another in one
//! plane; between placements the cook's check holds as for any piece.
//!
//! The desktop cook builds shapes in full detail and the mobile cook in a lighter one
//! ([`Detail`]), about half the triangles; both cook clean under the coplanar check.
//!
//! Surfaces that curve (roof tiles, soffits, lanterns, columns) carry texture coordinates of
//! their own, in meters along and down them ([`ShapeUv`]); flat faces are projected by their
//! slot as kit pieces are. Each corner may carry occlusion of the shape's own (tile channels,
//! soffits toward the wall, reveals toward their back), multiplied into the assembly's.

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

use super::{PieceTriangle, ShapeUv};

mod polygon;
mod props;
mod roof;
#[cfg(test)]
mod tests;
mod walls;

pub use props::{Column, Lantern, LanternKind, Mount};
pub use roof::{Ridge, Roof, RoofKind, Tiles};
pub use walls::{Beam, Coping, Hall, Opening, OpeningKind, Plinth, Posts, Side, Wall};

/// Meters across one repeat of plaster's map along a wall: plaster runs once up each wall, from
/// under its eaves (V 0) to its plinth (V 1), so the map's weathering (streaks under the eaves,
/// grime over the plinth) fits every wall's height ([`Part::Plaster`]).
pub const PLASTER_REPEAT: f32 = 3.2;

/// Tile periods across one repeat of the tiles' map, and meters down the slope in one: tiles run
/// in periods across the slope, so the map's painted channels and cover tiles lie on the modeled
/// ones ([`Part::Tiles`]).
pub const TILE_PERIODS: f32 = 4.0;
pub const TILE_COURSES: f32 = 1.2;

/// How finely shapes are built: the desktop cook's full detail, or the mobile cook's lighter one
/// (tile rows of two faces each and 40% wider apart, fewer rows up roofs, fewer samples along
/// ridges, at most 8 sides round columns and 6 round lanterns), about half the triangles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Detail {
    #[default]
    Full,
    Light,
}

/// A parametric builtin, placed by a placement's `shape`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Roof(Roof),
    Hall(Hall),
    Wall(Wall),
    Column(Column),
    Lantern(Lantern),
}

impl Shape {
    /// The builtin's name, as rules and allowances name its placements.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Roof(_) => "#roof",
            Self::Hall(_) => "#hall",
            Self::Wall(_) => "#wall",
            Self::Column(_) => "#column",
            Self::Lantern(_) => "#lantern",
        }
    }

    /// Checks its parameters: sizes positive and the parts fitting together.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Roof(roof) => roof.validate(),
            Self::Hall(hall) => hall.validate(),
            Self::Wall(wall) => wall.validate(),
            Self::Column(column) => column.validate(),
            Self::Lantern(lantern) => lantern.validate(),
        }
    }

    /// Its triangles in its own space, each in a part ([`Part::ALL`]'s index as its material).
    pub(super) fn triangles(&self, detail: Detail) -> Result<Vec<PieceTriangle>, String> {
        self.validate()?;
        let mut mesh = Mesh::default();
        match self {
            Self::Roof(roof) => roof.build(&mut mesh, Vec3::ZERO, detail)?,
            Self::Hall(hall) => hall.build(&mut mesh)?,
            Self::Wall(wall) => wall.build(&mut mesh, detail)?,
            Self::Column(column) => column.build(&mut mesh, detail),
            Self::Lantern(lantern) => lantern.build(&mut mesh, detail),
        }
        Ok(mesh.triangles)
    }
}

/// What a shape's triangle is made of, for surface rules to send to a slot by its name (rules'
/// `material`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    /// Walls' plaster.
    Plaster,
    /// The stone base walls stand on, and columns' drums.
    Plinth,
    /// Stone rings and frames around moon windows and moon gates.
    Trim,
    /// A gable's face under its roof.
    Gable,
    /// Roof tiles and the tile ends along the eaves.
    Tiles,
    /// Ridges, their end ornaments and finials.
    Ridge,
    /// Roofs' undersides: soffits, fascia boards and corner beams.
    Eaves,
    /// Posts, beams, columns and frames.
    Timber,
    /// Window lattices.
    Lattice,
    /// Windows' paper (or glass) behind their lattices.
    Window,
    /// Door leaves and the dark backs of doorways.
    Door,
    /// Lanterns' paper.
    Paper,
    /// Lanterns' caps, frames and cords.
    Frame,
}

impl Part {
    pub const ALL: [Self; 13] = [
        Self::Plaster,
        Self::Plinth,
        Self::Trim,
        Self::Gable,
        Self::Tiles,
        Self::Ridge,
        Self::Eaves,
        Self::Timber,
        Self::Lattice,
        Self::Window,
        Self::Door,
        Self::Paper,
        Self::Frame,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Plaster => "plaster",
            Self::Plinth => "plinth",
            Self::Trim => "trim",
            Self::Gable => "gable",
            Self::Tiles => "tiles",
            Self::Ridge => "ridge",
            Self::Eaves => "eaves",
            Self::Timber => "timber",
            Self::Lattice => "lattice",
            Self::Window => "window",
            Self::Door => "door",
            Self::Paper => "paper",
            Self::Frame => "frame",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&part| part == self).expect("every part is listed")
    }
}

/// A plane in a shape's space with a 2D frame on it: `right` and `up` unit and square to each
/// other, its front facing `right × up`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Frame {
    pub origin: Vec3,
    pub right: Vec3,
    pub up: Vec3,
}

impl Frame {
    pub fn at(&self, p: Vec2) -> Vec3 {
        self.origin + self.right * p.x + self.up * p.y
    }

    pub fn normal(&self) -> Vec3 {
        self.right.cross(self.up)
    }

    /// The same frame moved `distance` along its normal.
    pub fn offset(&self, distance: f32) -> Self {
        Self { origin: self.origin + self.normal() * distance, ..*self }
    }
}

/// A shape's triangles as they are built.
#[derive(Default)]
pub(super) struct Mesh {
    pub triangles: Vec<PieceTriangle>,
}

impl Mesh {
    /// A triangle as given: counter-clockwise seen from its front, normals per corner (zero takes
    /// the face's).
    pub fn triangle(
        &mut self,
        part: Part,
        positions: [Vec3; 3],
        normals: [Vec3; 3],
        uv: ShapeUv,
        occlusion: [f32; 3],
    ) {
        self.triangles.push(PieceTriangle {
            positions,
            normals,
            cell: [0, 0],
            down: [0.0; 3],
            material: part.index(),
            uv,
            occlusion,
        });
    }

    /// A flat triangle facing `out` (wound to face it), its texture coordinates projected by its
    /// slot.
    pub fn flat(&mut self, part: Part, mut positions: [Vec3; 3], out: Vec3, mut occlusion: [f32; 3]) {
        if (positions[1] - positions[0]).cross(positions[2] - positions[0]).dot(out) < 0.0 {
            positions.swap(1, 2);
            occlusion.swap(1, 2);
        }
        self.triangle(part, positions, [Vec3::ZERO; 3], ShapeUv::Project, occlusion);
    }

    /// A flat quad `a b c d` (in order round it) facing `out`.
    pub fn quad(&mut self, part: Part, corners: [Vec3; 4], out: Vec3, occlusion: [f32; 4]) {
        let [a, b, c, d] = corners;
        let [oa, ob, oc, od] = occlusion;
        self.flat(part, [a, b, c], out, [oa, ob, oc]);
        self.flat(part, [a, c, d], out, [oa, oc, od]);
    }

    /// The flat polygon `outer` less `holes` on `frame` (2D points in its plane), facing its
    /// front, its occlusion from each corner's 2D point; its texture coordinates projected by its
    /// slot.
    pub fn fill(
        &mut self,
        part: Part,
        frame: &Frame,
        outer: &[Vec2],
        holes: &[Vec<Vec2>],
        occlusion: &dyn Fn(Vec2) -> f32,
    ) -> Result<(), String> {
        self.fill_mapped(part, frame, outer, holes, occlusion, None)
    }

    /// [`Mesh::fill`], with texture coordinates in tiles from each corner's 2D point when `uv`
    /// is given (as plaster's run once up a wall).
    pub fn fill_mapped(
        &mut self,
        part: Part,
        frame: &Frame,
        outer: &[Vec2],
        holes: &[Vec<Vec2>],
        occlusion: &dyn Fn(Vec2) -> f32,
        uv: Option<&dyn Fn(Vec2) -> Vec2>,
    ) -> Result<(), String> {
        let triangles = polygon::triangulate(outer, holes)?;
        let points: Vec<Vec2> = outer.iter().chain(holes.iter().flatten()).copied().collect();
        let out = frame.normal();
        for [a, b, c] in triangles {
            let mut corners = [points[a], points[b], points[c]];
            let mut positions = corners.map(|p| frame.at(p));
            if (positions[1] - positions[0]).cross(positions[2] - positions[0]).dot(out) < 0.0 {
                positions.swap(1, 2);
                corners.swap(1, 2);
            }
            let mapped = match uv {
                Some(uv) => ShapeUv::Tiles(corners.map(uv)),
                None => ShapeUv::Project,
            };
            self.triangle(part, positions, [Vec3::ZERO; 3], mapped, corners.map(occlusion));
        }
        Ok(())
    }

    /// Triangles between two runs side by side, `a` and `b`, each point with its place along the
    /// run (increasing along both) and its position, facing `out`; texture coordinates are the
    /// place along the run (U) and the depth below `a`'s first point (V), in meters.
    pub fn zipper(
        &mut self,
        part: Part,
        a: &[(f32, Vec3)],
        b: &[(f32, Vec3)],
        out: Vec3,
        occlusion: [f32; 2],
    ) {
        if a.len() < 2 && b.len() < 2 {
            return;
        }
        let top = a.first().map_or(0.0, |(_, p)| p.y);
        let uv = |(t, p): (f32, Vec3)| Vec2::new(t, top - p.y);
        let (mut i, mut j) = (0, 0);
        while i + 1 < a.len() || j + 1 < b.len() {
            let advance_a = j + 1 >= b.len() || (i + 1 < a.len() && a[i + 1].0 <= b[j + 1].0);
            let (corners, from) = if advance_a {
                i += 1;
                ([a[i - 1], a[i], b[j]], [0, 0, 1])
            } else {
                j += 1;
                ([a[i], b[j], b[j - 1]], [0, 1, 1])
            };
            let mut positions = corners.map(|(_, p)| p);
            let mut uvs = corners.map(uv);
            let mut shade = from.map(|side| occlusion[side]);
            if (positions[1] - positions[0]).cross(positions[2] - positions[0]).dot(out) < 0.0 {
                positions.swap(1, 2);
                uvs.swap(1, 2);
                shade.swap(1, 2);
            }
            self.triangle(part, positions, [Vec3::ZERO; 3], ShapeUv::Meters(uvs), shade);
        }
    }

    /// A box section swept along `path` (its bottom middle): `width` across, from `below` under
    /// the path to `height` over it, its sides upright and square to the path's heading. Both
    /// ends are capped; `underside` adds its bottom.
    pub fn sweep(&mut self, part: Part, path: &[Vec3], width: f32, height: f32, below: f32, underside: bool) {
        if path.len() < 2 {
            return;
        }
        let n = path.len();
        let heading = |k: usize| {
            let (a, b) = (path[k.saturating_sub(1)], path[(k + 1).min(n - 1)]);
            let d = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
            d.try_normalize().unwrap_or(Vec3::X)
        };
        // Each point's section: left and right at the bottom and top.
        let sections: Vec<[Vec3; 4]> = (0..n)
            .map(|k| {
                let side = heading(k).cross(Vec3::Y) * (0.5 * width);
                let (low, high) = (path[k] - Vec3::Y * below, path[k] + Vec3::Y * height);
                [low - side, low + side, high + side, high - side]
            })
            .collect();
        for k in 0..n - 1 {
            let (s, t) = (sections[k], sections[k + 1]);
            let side = heading(k).cross(Vec3::Y);
            self.quad(part, [s[1], t[1], t[2], s[2]], side, [1.0; 4]);
            self.quad(part, [s[0], s[3], t[3], t[0]], -side, [1.0; 4]);
            self.quad(part, [s[3], s[2], t[2], t[3]], Vec3::Y, [1.0; 4]);
            if underside {
                self.quad(part, [s[0], t[0], t[1], s[1]], -Vec3::Y, [0.8; 4]);
            }
        }
        let first = path[0] - path[1];
        let last = path[n - 1] - path[n - 2];
        self.quad(part, sections[0], first, [1.0; 4]);
        self.quad(part, sections[n - 1], last, [1.0; 4]);
    }

    /// An axis-aligned box from `min` to `max`, without the faces `skip` names (unit axes: `-Y`
    /// for its bottom).
    pub fn cuboid(&mut self, part: Part, min: Vec3, max: Vec3, skip: &[Vec3]) {
        let corner = |x: bool, y: bool, z: bool| {
            Vec3::new(
                if x { max.x } else { min.x },
                if y { max.y } else { min.y },
                if z { max.z } else { min.z },
            )
        };
        let faces = [
            (
                Vec3::X,
                [
                    corner(true, false, false),
                    corner(true, true, false),
                    corner(true, true, true),
                    corner(true, false, true),
                ],
            ),
            (
                -Vec3::X,
                [
                    corner(false, false, false),
                    corner(false, false, true),
                    corner(false, true, true),
                    corner(false, true, false),
                ],
            ),
            (
                Vec3::Y,
                [
                    corner(false, true, false),
                    corner(false, true, true),
                    corner(true, true, true),
                    corner(true, true, false),
                ],
            ),
            (
                -Vec3::Y,
                [
                    corner(false, false, false),
                    corner(true, false, false),
                    corner(true, false, true),
                    corner(false, false, true),
                ],
            ),
            (
                Vec3::Z,
                [
                    corner(false, false, true),
                    corner(true, false, true),
                    corner(true, true, true),
                    corner(false, true, true),
                ],
            ),
            (
                -Vec3::Z,
                [
                    corner(false, false, false),
                    corner(false, true, false),
                    corner(true, true, false),
                    corner(true, false, false),
                ],
            ),
        ];
        for (out, corners) in faces {
            if !skip.contains(&out) {
                self.quad(part, corners, out, [1.0; 4]);
            }
        }
    }

    /// `profile` ((radius, height) pairs from the bottom up) turned about the +Y axis through
    /// `center` in `sides` faces, a band of quads between each two of its points, smooth round
    /// the axis and creased along it; `caps` closes its bottom and top with flat discs.
    /// Texture coordinates are meters round its widest point and down its profile, or with
    /// `ribs` in tiles: one per side around, and one down the whole profile.
    pub fn lathe(
        &mut self,
        part: Part,
        center: Vec3,
        profile: &[(f32, f32)],
        sides: u32,
        caps: [bool; 2],
        ribs: bool,
    ) {
        let sides = sides.max(3);
        let directions: Vec<Vec3> = (0..=sides)
            .map(|k| {
                let angle = std::f32::consts::TAU * (k % sides) as f32 / sides as f32;
                Vec3::new(angle.cos(), 0.0, -angle.sin())
            })
            .collect();
        let widest = profile.iter().map(|&(r, _)| r).fold(0.0, f32::max);
        let around = std::f32::consts::TAU * widest / sides as f32;
        let mut along = 0.0;
        let length: f32 = profile
            .windows(2)
            .map(|w| Vec2::new(w[1].0 - w[0].0, w[1].1 - w[0].1).length())
            .sum::<f32>()
            .max(1e-6);
        for w in profile.windows(2) {
            let ((r0, y0), (r1, y1)) = (w[0], w[1]);
            let step = Vec2::new(r1 - r0, y1 - y0).length();
            // The band's outward normal in the (radius, height) plane.
            let n = Vec2::new(y1 - y0, -(r1 - r0)).normalize_or_zero();
            let (v0, v1) = (1.0 - along / length, 1.0 - (along + step) / length);
            along += step;
            if step <= 0.0 {
                continue;
            }
            for k in 0..sides as usize {
                let (d0, d1) = (directions[k], directions[k + 1]);
                let corners = [
                    center + d0 * r0 + Vec3::Y * y0,
                    center + d1 * r0 + Vec3::Y * y0,
                    center + d1 * r1 + Vec3::Y * y1,
                    center + d0 * r1 + Vec3::Y * y1,
                ];
                let normal = |d: Vec3| (d * n.x + Vec3::Y * n.y).normalize_or_zero();
                let normals = [normal(d0), normal(d1), normal(d1), normal(d0)];
                let (u0, u1) = (k as f32, k as f32 + 1.0);
                let mut uv = [Vec2::new(u0, v0), Vec2::new(u1, v0), Vec2::new(u1, v1), Vec2::new(u0, v1)];
                if !ribs {
                    // Meters: round the widest point, and down from the profile's top.
                    uv = uv.map(|uv| Vec2::new(uv.x * around, uv.y * length));
                }
                let out = normal(0.5 * (d0 + d1));
                // A band shrinking to a point has one triangle.
                let mut order: Vec<usize> = vec![0, 1, 2, 3];
                if r0 <= 0.0 {
                    order = vec![0, 2, 3];
                } else if r1 <= 0.0 {
                    order = vec![0, 1, 2];
                }
                let mut push = |i: usize, j: usize, l: usize| {
                    let mut tri = [i, j, l];
                    let p = tri.map(|m| corners[m]);
                    if (p[1] - p[0]).cross(p[2] - p[0]).dot(out) < 0.0 {
                        tri.swap(1, 2);
                    }
                    let uv = tri.map(|m| uv[m]);
                    let uv = if ribs { ShapeUv::Tiles(uv) } else { ShapeUv::Meters(uv) };
                    self.triangle(part, tri.map(|m| corners[m]), tri.map(|m| normals[m]), uv, [1.0; 3]);
                };
                push(order[0], order[1], order[2]);
                if order.len() == 4 {
                    push(order[0], order[2], order[3]);
                }
            }
        }
        for (cap, &(radius, height), out) in
            [(caps[0], profile.first(), -Vec3::Y), (caps[1], profile.last(), Vec3::Y)]
                .into_iter()
                .filter_map(|(cap, point, out)| point.map(|point| (cap, point, out)))
        {
            if !cap || radius <= 0.0 {
                continue;
            }
            let middle = center + Vec3::Y * height;
            for k in 0..sides as usize {
                let (a, b) = (middle + directions[k] * radius, middle + directions[k + 1] * radius);
                self.flat(part, [middle, a, b], out, [1.0; 3]);
            }
        }
    }

    /// The 2D outline `profile` on `frame`, extruded from `back` to `front` along the frame's
    /// normal: both faces and the sides, flat.
    pub fn prism(
        &mut self,
        part: Part,
        frame: &Frame,
        profile: &[Vec2],
        back: f32,
        front: f32,
    ) -> Result<(), String> {
        let front_frame = frame.offset(front);
        let back_frame = frame.offset(back);
        self.fill(part, &front_frame, profile, &[], &|_| 1.0)?;
        let mirrored = Frame { origin: back_frame.origin, right: -frame.right, up: frame.up };
        let flipped: Vec<Vec2> = profile.iter().map(|p| Vec2::new(-p.x, p.y)).collect();
        self.fill(part, &mirrored, &flipped, &[], &|_| 1.0)?;
        let winding = polygon::signed_area(profile).signum();
        for k in 0..profile.len() {
            let (a, b) = (profile[k], profile[(k + 1) % profile.len()]);
            let edge = b - a;
            let out2 = Vec2::new(edge.y, -edge.x) * winding;
            let out = frame.right * out2.x + frame.up * out2.y;
            self.quad(
                part,
                [back_frame.at(a), back_frame.at(b), front_frame.at(b), front_frame.at(a)],
                out,
                [1.0; 4],
            );
        }
        Ok(())
    }
}

/// Points round a circle of `radius` about `center`, `count` of them counter-clockwise from its
/// right, in 2D.
pub(super) fn circle(center: Vec2, radius: f32, count: usize) -> Vec<Vec2> {
    (0..count)
        .map(|k| {
            let angle = std::f32::consts::TAU * k as f32 / count as f32;
            center + Vec2::new(angle.cos(), angle.sin()) * radius
        })
        .collect()
}

pub(super) fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A parameter check's failure message, if `ok` fails.
pub(super) fn check(ok: bool, message: impl FnOnce() -> String) -> Result<(), String> {
    if ok { Ok(()) } else { Err(message()) }
}

/// Whether every number is finite.
pub(super) fn finite(values: &[f32]) -> bool {
    values.iter().all(|value| value.is_finite())
}
