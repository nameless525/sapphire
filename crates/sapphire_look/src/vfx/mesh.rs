//! Effect meshes (M5 L5): what a frame's [`VfxMesh`]es are, the shapes they take ([`VfxMeshShape`]) and the
//! patterns they show ([`VfxPattern`]), all generated in code, so no mesh or texture is ever downloaded or
//! drawn by hand. Both are pure functions: the renderer builds the shapes' buffers and rasterizes the patterns
//! into a texture array when the first mesh draws, as it does the sprites' map.
//!
//! - **Unlit.** A mesh's color is its own, relative to the display as an emissive sprite's is
//!   ([`VfxLight::Emissive`](super::VfxLight::Emissive)): the scene's light never falls on it, so it reads the
//!   same in every zone and at every hour.
//! - **Its UVs** run along a shape's sweep (`u`, wrapping where the shape closes) and across its band (`v`), so a
//!   pattern scrolled along `u` travels the way a slash or a ring moves.
//! - **Its own soft edges:** each vertex carries a `fade` that is 0 on the shape's open borders, so no mesh shows
//!   a hard rim, as no sprite shows its quad's border.
//! - **Deterministic:** the shapes' angles go through `sapphire_math::det`, and the patterns are integer hashes
//!   and polynomials, so every machine builds the same buffers and the same texels.

use bytemuck::{Pod, Zeroable};
use glam::{Quat, Vec2, Vec3};
use sapphire_math::det;

/// A generated shape (see the module docs). Each is about a metre across, its middle at the origin of its own
/// space, for a layer to scale, turn and place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VfxMeshShape {
    /// A crescent in the XZ plane, on a circle of radius 1 round the origin: from its tail at -X through +Z to
    /// its head at +X, pointed at both ends and broadest two thirds of the way along, with its outer edge
    /// brightest. `u` runs tail to head, `v` from the inner edge out. A swing round the body.
    #[default]
    SlashArc,
    /// A flat band round the origin in the XZ plane, radii 0.8 to 1: a shockwave on the ground. `u` runs round
    /// it, `v` from the inner edge out.
    Ring,
    /// An open column rising from a circle of radius 1 at y 0 to one of 0.7 at y 1, brightest low down and gone
    /// at the top: a pillar of light. `u` runs round it, `v` up it.
    Column,
    /// A hemisphere of radius 1 over the XZ plane, soft at its rim: a burst's shell. `u` runs round it, `v` from
    /// the rim up to the pole.
    Dome,
    /// A flat square a metre a side, facing +Z: a flipbook's card. `u` runs left to right, `v` top to bottom.
    Card,
}

/// A vertex of a shape (`VfxMeshVertex` in `vfx_mesh.wgsl`), as it uploads.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct VfxMeshVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    /// How much of the layer shows here, 0 to 1: 0 on the shape's open borders.
    pub fade: f32,
}

/// A shape's triangles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VfxGeometry {
    pub vertices: Vec<VfxMeshVertex>,
    pub indices: Vec<u16>,
}

/// A band's profile across it, `(v, fade)`: soft inside, broad in the middle, crisp but not hard outside.
const BAND: [(f32, f32); 4] = [(0.0, 0.0), (0.55, 1.0), (0.85, 1.0), (1.0, 0.0)];
/// The column's up its height: soft at its foot, bright low down, gone at the top.
const COLUMN: [(f32, f32); 4] = [(0.0, 0.0), (0.08, 1.0), (0.45, 0.7), (1.0, 0.0)];
/// The slash arc's sweep, radians: half a turn.
const ARC_SWEEP: f32 = std::f32::consts::PI;
/// The slash arc's broadest width, as a share of its radius.
const ARC_WIDTH: f32 = 0.4;
/// The ring's inner radius.
const RING_INNER: f32 = 0.8;
/// The column's radius at its top.
const COLUMN_TOP: f32 = 0.7;
/// Segments along a shape's sweep or round it, and the dome's rings.
const ARC_SEGMENTS: u16 = 48;
const ROUND_SEGMENTS: u16 = 64;
const DOME_RINGS: u16 = 12;

impl VfxMeshShape {
    pub const ALL: [Self; 5] = [Self::SlashArc, Self::Ring, Self::Column, Self::Dome, Self::Card];

    /// Its place among [`Self::ALL`].
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Its triangles, generated (see the module docs). Pure: the same bytes on every machine.
    #[must_use]
    pub fn geometry(self) -> VfxGeometry {
        match self {
            Self::SlashArc => grid(ARC_SEGMENTS, &BAND, |u, v| {
                // Pointed at both ends, broadest two thirds along: 27/4 u² (1 − u) peaks at 1 there.
                let width = ARC_WIDTH * 6.75 * u * u * (1.0 - u);
                let angle = (u - 0.5) * ARC_SWEEP;
                round(angle, 1.0 - width * (1.0 - v), 0.0)
            }),
            Self::Ring => grid(ROUND_SEGMENTS, &BAND, |u, v| {
                round(u * std::f32::consts::TAU, RING_INNER + (1.0 - RING_INNER) * v, 0.0)
            }),
            Self::Column => grid(ROUND_SEGMENTS, &COLUMN, |u, v| {
                round(u * std::f32::consts::TAU, 1.0 + (COLUMN_TOP - 1.0) * v, v)
            }),
            Self::Dome => {
                let rings: Vec<(f32, f32)> = (0..=DOME_RINGS)
                    .map(|ring| {
                        let v = f32::from(ring) / f32::from(DOME_RINGS);
                        (v, (v * 6.0).min(1.0))
                    })
                    .collect();
                grid(ROUND_SEGMENTS, &rings, |u, v| {
                    let (rise, across) = det::sin_cos(v * std::f32::consts::FRAC_PI_2);
                    round(u * std::f32::consts::TAU, across, rise)
                })
            }
            Self::Card => {
                let corner = |x: f32, y: f32| VfxMeshVertex {
                    position: [x - 0.5, 0.5 - y, 0.0],
                    uv: [x, y],
                    fade: 1.0,
                };
                VfxGeometry {
                    vertices: vec![corner(0.0, 0.0), corner(1.0, 0.0), corner(0.0, 1.0), corner(1.0, 1.0)],
                    indices: vec![0, 2, 1, 1, 2, 3],
                }
            }
        }
    }
}

/// A point `radius` from the Y axis at `angle` from +Z toward +X, `height` up it.
fn round(angle: f32, radius: f32, height: f32) -> [f32; 3] {
    let (sin, cos) = det::sin_cos(angle);
    [radius * sin, height, radius * cos]
}

/// A strip of quads `segments` long along `u`, its rows across at `rows`' `(v, fade)`, each vertex placed by
/// `place(u, v)`.
fn grid(segments: u16, rows: &[(f32, f32)], place: impl Fn(f32, f32) -> [f32; 3]) -> VfxGeometry {
    let columns = segments + 1;
    let mut vertices = Vec::with_capacity(usize::from(columns) * rows.len());
    for &(v, fade) in rows {
        for column in 0..columns {
            let u = f32::from(column) / f32::from(segments);
            vertices.push(VfxMeshVertex { position: place(u, v), uv: [u, v], fade });
        }
    }
    let mut indices = Vec::with_capacity(usize::from(segments) * (rows.len() - 1) * 6);
    for row in 0..rows.len() as u16 - 1 {
        for column in 0..segments {
            let at = |r: u16, c: u16| r * columns + c;
            let (a, b, c, d) =
                (at(row, column), at(row, column + 1), at(row + 1, column), at(row + 1, column + 1));
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    VfxGeometry { vertices, indices }
}

/// A generated pattern a mesh shows, a layer of the renderer's pattern array: a coverage from 0 to 1 that
/// tiles across `u` and `v`, so scrolling it never shows a seam.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VfxPattern {
    /// Everywhere 1: the shape's own fades and the layer's color alone.
    #[default]
    Solid,
    /// Lanes of streaks running along `u`, each a long tail rising to a bright head toward `+u`, over a faint
    /// floor: what a slash's sweep or a ring's spin carries, scrolled toward `+u`.
    Streaks,
    /// Soft value noise in four octaves, spread across 0 to 1: an erosion's mask, or smoke.
    Noise,
    /// A flipbook of a burst, 4 by 4 frames in reading order (`Self::grid`): a core that flares and fades, a
    /// ring opening and thinning, and eight rays reaching out and going. Every frame is empty at its cell's
    /// border, so neighbours never bleed into each other, even down the mips.
    Burst,
}

/// A pattern's side in texels as the renderer rasterizes it: a burst's frame is 64 a side.
pub const PATTERN_SIZE: u32 = 256;

impl VfxPattern {
    pub const ALL: [Self; 4] = [Self::Solid, Self::Streaks, Self::Noise, Self::Burst];

    /// Its layer of the renderer's pattern array.
    pub const fn layer(self) -> u32 {
        self as u32
    }

    /// The flipbook it holds, columns by rows, if it's a sheet of frames.
    pub const fn grid(self) -> Option<(u32, u32)> {
        match self {
            Self::Burst => Some((4, 4)),
            Self::Solid | Self::Streaks | Self::Noise => None,
        }
    }

    /// Its coverage at `(u, v)`, each from 0 to 1 (see the type's docs). Pure.
    #[must_use]
    pub fn coverage(self, u: f32, v: f32) -> f32 {
        match self {
            Self::Solid => 1.0,
            Self::Streaks => streaks(u, v),
            Self::Noise => noise(u, v),
            Self::Burst => burst(u, v),
        }
    }
}

/// A 32-bit integer hash (lowbias32): what the patterns' randomness is, so it's the same everywhere.
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

/// `hash` as a share from 0 to 1.
fn unit(x: u32) -> f32 {
    (hash(x) >> 8) as f32 / (1 << 24) as f32
}

fn smoothstep(start: f32, end: f32, x: f32) -> f32 {
    let t = ((x - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The lanes across `v`.
const STREAK_LANES: f32 = 12.0;

fn streaks(u: f32, v: f32) -> f32 {
    let lane_v = v * STREAK_LANES;
    let lane = lane_v.floor() as i32 as u32 % STREAK_LANES as u32;
    let across = 2.0 * lane_v.fract() - 1.0;
    let profile = (1.0 - across * across).max(0.0);
    // Whole repeats along `u`, so the pattern tiles there.
    let repeats = 1.0 + (hash(lane.wrapping_add(0x51)) % 3) as f32;
    let head = 0.6 + 0.25 * unit(lane.wrapping_add(0x9e));
    let phase = unit(lane.wrapping_add(0x3c));
    let strength = 0.55 + 0.45 * unit(lane.wrapping_add(0x77));
    let x = (u * repeats + phase).fract();
    // A long tail rising to the head, then a short fall past it: 0 at both ends of the period.
    let dash = if x < head {
        let rise = x / head;
        rise * rise * rise
    } else {
        let fall = 1.0 - (x - head) / (1.0 - head);
        fall * fall
    };
    (0.18 + 0.82 * profile * profile * dash * strength).clamp(0.0, 1.0)
}

/// The value at a lattice point of `cells` a side (a power of two): the lattice's points shuffled by a bijection
/// of their index, so they hold each of `cells`² evenly spaced values once. Hashed values on a lattice this small
/// lean high or low as a whole; these average exactly a half.
fn lattice(cx: u32, cy: u32, cells: u32, seed: u32) -> f32 {
    let count = cells * cells;
    let mask = count - 1;
    let shift = (count.trailing_zeros() / 2).max(1);
    let mut x = cy * cells + cx;
    for round in 0..3 {
        // An odd multiplier, an addition and a right xorshift are each one to one within the mask's bits.
        x = x.wrapping_mul(0x2c1b_3c6d).wrapping_add(hash(seed.wrapping_add(round))) & mask;
        x ^= x >> shift;
    }
    (x as f32 + 0.5) / count as f32
}

/// Value noise on a lattice of `cells` a side (a power of two) that wraps, so it tiles.
fn value_noise(u: f32, v: f32, cells: u32, seed: u32) -> f32 {
    let (x, y) = (u * cells as f32, v * cells as f32);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let wrap = |c: f32| (c as i32).rem_euclid(cells as i32) as u32;
    let at = |cx: u32, cy: u32| lattice(cx, cy, cells, seed);
    let (ix0, iy0) = (wrap(x0), wrap(y0));
    let (ix1, iy1) = ((ix0 + 1) % cells, (iy0 + 1) % cells);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let top = at(ix0, iy0) + (at(ix1, iy0) - at(ix0, iy0)) * sx;
    let bottom = at(ix0, iy1) + (at(ix1, iy1) - at(ix0, iy1)) * sx;
    top + (bottom - top) * sy
}

fn noise(u: f32, v: f32) -> f32 {
    let octaves = [(4, 0.5), (8, 0.25), (16, 0.125), (32, 0.0625)];
    let (sum, weight) =
        octaves.iter().enumerate().fold((0.0, 0.0), |(sum, weight), (octave, &(cells, w))| {
            (sum + w * value_noise(u, v, cells, 0x1000 * octave as u32), weight + w)
        });
    // Octaves pile up toward the middle: spread the result back across 0 to 1, so an erosion's threshold
    // moving at an even rate eats an even share.
    ((sum / weight - 0.5) * 2.2 + 0.5).clamp(0.0, 1.0)
}

fn burst(u: f32, v: f32) -> f32 {
    let (columns, rows) = (4.0, 4.0);
    let (cell_u, cell_v) = (u * columns, v * rows);
    let frame = (cell_v.floor().clamp(0.0, rows - 1.0)) * columns + cell_u.floor().clamp(0.0, columns - 1.0);
    let t = frame / (columns * rows - 1.0);
    let (x, y) = (2.0 * cell_u.fract() - 1.0, 2.0 * cell_v.fract() - 1.0);
    let r2 = x * x + y * y;
    let r = r2.sqrt();
    // The core flares and fades in the first frames.
    let core = (1.0 - r / (0.45 - 0.2 * t)).max(0.0);
    let core = core * core * (1.0 - t) * (1.0 - t);
    // A ring opening and thinning.
    let radius = 0.15 + 0.62 * t;
    let thickness = 0.16 * (1.0 - 0.6 * t);
    let ring = (1.0 - smoothstep(0.0, thickness, (r - radius).abs())) * (1.0 - 0.6 * t);
    // Eight rays: cos 8θ from cos 4θ = (x⁴ − 6x²y² + y⁴) / r⁴, with no angle taken.
    let cos4 =
        if r2 > 1e-12 { (x * x * x * x - 6.0 * x * x * y * y + y * y * y * y) / (r2 * r2) } else { 1.0 };
    let cos8 = 2.0 * cos4 * cos4 - 1.0;
    let ray = cos8.max(0.0);
    let ray = ray * ray * ray * ray;
    let reach = 0.3 + 0.55 * t;
    let rays = ray * (1.0 - smoothstep(reach * 0.5, reach, r)) * (1.0 - t * t);
    // Empty at the cell's border, so frames never bleed into each other.
    let border = (1.0 - smoothstep(0.82, 0.92, r)) * (1.0 - smoothstep(0.88, 0.96, x.abs().max(y.abs())));
    core.max(ring).max(rays).clamp(0.0, 1.0) * border
}

/// An erosion: the layer shows only where its mask passes a threshold that moves over its life, an edge of
/// its own color where the mask has only just passed it (`VfxMesh::erosion`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VfxErosion {
    /// What it erodes by.
    pub mask: VfxPattern,
    /// The mask's repeats across the shape's UVs, unscrolled.
    pub tiling: Vec2,
    /// How far it has eroded, 0 (whole) to 1 (gone).
    pub threshold: f32,
    /// The edge's width in the mask's values, 0 to 1: 0 has none.
    pub edge: f32,
    /// The edge's color, relative to the display as the mesh's own is.
    pub edge_color: Vec3,
}

/// The frames a flipbook shows this frame (`VfxMesh::flipbook`): its pattern's grid, the frame and the one
/// after it in reading order, and how far toward the next it blends (0 with blending off).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VfxFlipbook {
    pub columns: u32,
    pub rows: u32,
    pub frame: u32,
    pub next: u32,
    pub blend: f32,
}

/// An effect mesh the renderer draws (rule R4): a generated shape, placed, colored and patterned as
/// `sapphire_vfx` resolved it for the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VfxMesh {
    pub shape: VfxMeshShape,
    pub position: Vec3,
    pub rotation: Quat,
    /// Metres along the shape's own axes.
    pub scale: Vec3,
    /// Linear, relative to the display (see the module docs).
    pub color: Vec3,
    /// Its opacity, over its pattern's coverage and its shape's fades.
    pub alpha: f32,
    /// As [`VfxSprite::additive`](super::VfxSprite::additive): 1 pure light, 0 a surface that covers.
    pub additive: f32,
    /// Over how many metres it fades into the surface behind it, where the frame has depth to read.
    pub soft: f32,
    pub pattern: VfxPattern,
    /// The pattern's repeats across the shape's UVs.
    pub tiling: Vec2,
    /// How far the pattern has scrolled toward `+u` and `+v`, in UVs: a share of a repeat, 0 to 1 on each axis.
    pub scroll: Vec2,
    pub erosion: Option<VfxErosion>,
    pub flipbook: Option<VfxFlipbook>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shapes_indices_name_its_vertices_and_its_uvs_and_fades_are_shares() {
        for shape in VfxMeshShape::ALL {
            let geometry = shape.geometry();
            assert!(geometry.indices.len().is_multiple_of(3) && !geometry.indices.is_empty(), "{shape:?}");
            assert!(
                geometry.indices.iter().all(|&index| usize::from(index) < geometry.vertices.len()),
                "{shape:?}"
            );
            for vertex in &geometry.vertices {
                assert!(vertex.position.iter().all(|c| c.is_finite() && c.abs() <= 1.0 + 1e-6), "{shape:?}");
                assert!(vertex.uv.iter().chain([&vertex.fade]).all(|c| (0.0..=1.0).contains(c)), "{shape:?}");
            }
        }
    }

    /// The same bytes on every machine: the angles go through `det`, and a test can pin a shape's sum.
    #[test]
    fn shapes_are_generated_the_same_every_time() {
        for shape in VfxMeshShape::ALL {
            assert_eq!(shape.geometry(), shape.geometry());
        }
        let arc = VfxMeshShape::SlashArc.geometry();
        // Its tail and head are points: the band has no width there.
        let row = usize::from(ARC_SEGMENTS) + 1;
        for end in [0, row - 1] {
            let inner = arc.vertices[end].position;
            let outer = arc.vertices[3 * row + end].position;
            assert!((Vec3::from(inner) - Vec3::from(outer)).length() < 1e-6, "{inner:?} {outer:?}");
        }
        // Its middle faces +Z.
        let middle = arc.vertices[3 * row + row / 2].position;
        assert!((Vec3::from(middle) - Vec3::Z).length() < 1e-5, "{middle:?}");
    }

    #[test]
    fn open_borders_fade_to_nothing_and_the_ring_closes_on_itself() {
        let ring = VfxMeshShape::Ring.geometry();
        let row = usize::from(ROUND_SEGMENTS) + 1;
        assert!(ring.vertices[..row].iter().all(|vertex| vertex.fade == 0.0), "its inner edge");
        assert!(ring.vertices[3 * row..].iter().all(|vertex| vertex.fade == 0.0), "its outer edge");
        let (first, last) = (ring.vertices[row].position, ring.vertices[2 * row - 1].position);
        assert!((Vec3::from(first) - Vec3::from(last)).length() < 1e-5, "the seam meets");
        let column = VfxMeshShape::Column.geometry();
        assert!(column.vertices[column.vertices.len() - 1].fade == 0.0, "the column's top");
        let dome = VfxMeshShape::Dome.geometry();
        assert!(dome.vertices[..row].iter().all(|vertex| vertex.fade == 0.0), "the dome's rim");
        assert!(dome.vertices.last().is_some_and(|top| (top.position[1] - 1.0).abs() < 1e-6), "its pole");
    }

    #[test]
    fn patterns_stay_between_zero_and_one_and_tile() {
        let side = 64;
        for pattern in VfxPattern::ALL {
            for j in 0..side {
                for i in 0..side {
                    let (u, v) = ((i as f32 + 0.5) / side as f32, (j as f32 + 0.5) / side as f32);
                    let coverage = pattern.coverage(u, v);
                    assert!((0.0..=1.0).contains(&coverage), "{pattern:?} at ({u}, {v}): {coverage}");
                }
            }
        }
        // Across the wrap, the tiling patterns run on: one texel either side of the edge differs as little as
        // two texels inside do.
        for pattern in [VfxPattern::Streaks, VfxPattern::Noise] {
            let step = 1.0 / 256.0;
            for k in 0..32 {
                let v = (k as f32 + 0.5) / 32.0;
                let across = (pattern.coverage(1.0 - step * 0.5, v) - pattern.coverage(step * 0.5, v)).abs();
                assert!(across < 0.2, "{pattern:?} across u's wrap at v {v}: {across}");
                let down = (pattern.coverage(v, 1.0 - step * 0.5) - pattern.coverage(v, step * 0.5)).abs();
                assert!(down < 0.2, "{pattern:?} across v's wrap at u {v}: {down}");
            }
        }
    }

    #[test]
    fn each_noise_lattice_holds_every_value_once() {
        for cells in [4, 8, 16, 32] {
            let mut values: Vec<u32> = (0..cells * cells)
                .map(|index| {
                    (lattice(index % cells, index / cells, cells, 0x1000) * (cells * cells) as f32) as u32
                })
                .collect();
            values.sort_unstable();
            assert!(values.iter().copied().eq(0..cells * cells), "{cells} a side");
        }
    }

    #[test]
    fn noise_spreads_across_its_range() {
        let side = 128;
        let values: Vec<f32> = (0..side * side)
            .map(|index| {
                let (i, j) = (index % side, index / side);
                VfxPattern::Noise.coverage((i as f32 + 0.5) / side as f32, (j as f32 + 0.5) / side as f32)
            })
            .collect();
        let below = |threshold: f32| {
            values.iter().filter(|&&value| value < threshold).count() as f32 / values.len() as f32
        };
        // An erosion's threshold moving evenly eats roughly evenly.
        assert!((0.3..0.7).contains(&below(0.5)), "half: {}", below(0.5));
        assert!(below(0.1) < 0.2 && below(0.9) > 0.8, "{} {}", below(0.1), below(0.9));
    }

    /// Each of the burst's frames is empty at its cell's border, and they differ.
    #[test]
    fn the_bursts_frames_are_apart_and_differ() {
        let (columns, rows) = VfxPattern::Burst.grid().expect("a sheet");
        let mut sums = Vec::new();
        for frame in 0..columns * rows {
            let (c, r) = ((frame % columns) as f32, (frame / columns) as f32);
            let at =
                |x: f32, y: f32| VfxPattern::Burst.coverage((c + x) / columns as f32, (r + y) / rows as f32);
            for k in 0..=16 {
                let t = k as f32 / 16.0;
                for (x, y) in [(t, 0.01), (t, 0.99), (0.01, t), (0.99, t)] {
                    assert!(at(x, y) < 0.005, "frame {frame} at its border ({x}, {y}): {}", at(x, y));
                }
            }
            let sum: f32 = (0..32 * 32).map(|i| at((i % 32) as f32 / 32.0, (i / 32) as f32 / 32.0)).sum();
            sums.push(sum);
        }
        for pair in sums.windows(2) {
            assert!((pair[0] - pair[1]).abs() > 1.0, "neighbouring frames differ: {sums:?}");
        }
    }
}
