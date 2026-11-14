//! Ground decals (M5 L7): what a frame's [`VfxDecal`]s are, flat on the ground under a cue, each a signed
//! distance shape ([`VfxDecalShape`]) the renderer evaluates per pixel on a quad lying in the ground's plane.
//! A foe's telegraph and a heavy blow's scorch are both one of these.
//!
//! - **Flat, not projected.** A decal is a quad at its placement's height, turned to the ground's normal
//!   there: on flat or evenly sloped ground it lies on the surface, but it doesn't wrap over a step, a kerb or
//!   a wall. Projected decals are a later package's (m5-plan §3, "Effects").
//! - **Its own frame:** local +Y is the ground's normal and +Z the way its placement faces; shapes lie in the
//!   local XZ plane, in metres, a cone and a lane reaching along +Z.
//! - **Outline, body and fill.** The outline, a band just inside the shape's edge, shows the whole reach at
//!   once; the body inside it shows at `body` of the decal's opacity; and the fill grows over it from the
//!   centre outward (along a lane from its foot) as [`VfxDecal::fill`] runs from 0 to 1, so a telegraph's fill
//!   says how long is left. A scorch is all body, filled, with a broken edge ([`VfxDecal::noise`]).
//! - **Unlit**, its color relative to the display as an emissive sprite's ([`VfxDecalBlend`]).
//! - **Pure functions** ([`VfxDecalShape::distance`], [`VfxDecalShape::progress`], [`edge_noise`],
//!   [`VfxDecal::opacity`]): `vfx_decal.wgsl` evaluates the same ones, and their tests here are its reference.

use glam::{Quat, Vec2, Vec3};

/// A decal's shape in its local XZ plane (see the module docs), as `(x, z)` points. Sizes in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VfxDecalShape {
    /// A disc round the origin.
    Disc { radius: f32 },
    /// A band round the origin from `inner` to `outer`: a blow that spares the ground at its feet.
    Ring { inner: f32, outer: f32 },
    /// A cone along +Z, `angle` degrees across in all (360 a whole disc), from `inner` out to `radius`.
    Sector { radius: f32, angle: f32, inner: f32 },
    /// A lane `width` across reaching `length` along +Z from the origin: a lunge's path.
    Lane { width: f32, length: f32 },
}

/// How a decal meets the ground under it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VfxDecalBlend {
    /// Light added over the ground, × 1/exposure as an emissive sprite's, bright enough for bloom to take: a
    /// telegraph.
    #[default]
    Light,
    /// The ground darkened toward its color, `1` leaving it as it is: a scorch.
    Darken,
}

/// The cells of the edge's noise, metres a side: the coarse octave, and the fine one at a third of it.
const NOISE_CELL: f32 = 0.45;
const NOISE_FINE: f32 = NOISE_CELL / 3.0;

/// The noise's share of the fine octave.
const NOISE_FINE_WEIGHT: f32 = 0.35;

/// A 32-bit integer hash (lowbias32), as `vfx_decal.wgsl` has it.
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

/// The lattice point `(x, y)`'s value, -1 to 1.
fn lattice(x: i32, y: i32, seed: u32) -> f32 {
    let h = hash((x as u32).wrapping_mul(0x9e37_79b1) ^ hash((y as u32) ^ seed));
    (h >> 8) as f32 / (1 << 23) as f32 - 1.0
}

/// Value noise over a lattice of `cell` metres, -1 to 1, smooth between its points.
fn value_noise(point: Vec2, cell: f32, seed: u32) -> f32 {
    let at = point / cell;
    let (x0, y0) = (at.x.floor(), at.y.floor());
    let (fx, fy) = (at.x - x0, at.y - y0);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (ix, iy) = (x0 as i32, y0 as i32);
    let top = lattice(ix, iy, seed) + (lattice(ix + 1, iy, seed) - lattice(ix, iy, seed)) * sx;
    let bottom = lattice(ix, iy + 1, seed) + (lattice(ix + 1, iy + 1, seed) - lattice(ix, iy + 1, seed)) * sx;
    top + (bottom - top) * sy
}

/// The edge's break-up at a local point, -1 to 1: two octaves of value noise, a coarse wander and a fine
/// fray, so a scorch's edge reads burnt rather than drawn. Pure, so every machine gets the same edge.
#[must_use]
pub fn edge_noise(point: Vec2) -> f32 {
    let coarse = value_noise(point, NOISE_CELL, 0x5c07);
    let fine = value_noise(point, NOISE_FINE, 0x1f3d);
    ((1.0 - NOISE_FINE_WEIGHT) * coarse + NOISE_FINE_WEIGHT * fine).clamp(-1.0, 1.0)
}

impl VfxDecalShape {
    /// How far `point` is from the shape's edge, in metres: negative inside.
    #[must_use]
    pub fn distance(self, point: Vec2) -> f32 {
        let r = point.length();
        match self {
            Self::Disc { radius } => r - radius,
            Self::Ring { inner, outer } => (r - outer).max(inner - r),
            Self::Sector { radius, angle, inner } => {
                let half = (angle.clamp(0.0, 360.0) * 0.5).to_radians();
                // A pie's distance (`(sin, cos)` of its half angle along +Z): |x| folds it onto one side.
                let (sin, cos) = half.sin_cos();
                let folded = Vec2::new(point.x.abs(), point.y);
                let edge = Vec2::new(sin, cos);
                let outside_arc = r - radius;
                let to_side = (folded - edge * folded.dot(edge).clamp(0.0, radius)).length();
                let side = if cos * folded.x - sin * folded.y > 0.0 { to_side } else { -to_side };
                outside_arc.max(side).max(inner - r)
            }
            Self::Lane { width, length } => {
                let half = Vec2::new(width * 0.5, length * 0.5);
                let q = (point - Vec2::new(0.0, length * 0.5)).abs() - half;
                q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0)
            }
        }
    }

    /// How far into the shape's reach `point` lies, 0 at its start to 1 at its far edge: from the inner edge
    /// outward round the origin, and from the lane's foot along it. Where the fill has reached once
    /// [`VfxDecal::fill`] passes it.
    #[must_use]
    pub fn progress(self, point: Vec2) -> f32 {
        let r = point.length();
        let (from, to) = match self {
            Self::Disc { radius } => (0.0, radius),
            Self::Ring { inner, outer } => (inner, outer),
            Self::Sector { radius, inner, .. } => (inner, radius),
            Self::Lane { length, .. } => return point.y / length.max(1e-6),
        };
        (r - from) / (to - from).max(1e-6)
    }

    /// The local rectangle, `(min, max)` in XZ, that holds the whole shape.
    #[must_use]
    pub fn bounds(self) -> (Vec2, Vec2) {
        match self {
            Self::Disc { radius } | Self::Ring { outer: radius, .. } => {
                (Vec2::splat(-radius), Vec2::splat(radius))
            }
            Self::Sector { radius, angle, .. } => {
                let half = (angle.clamp(0.0, 360.0) * 0.5).to_radians();
                if half <= std::f32::consts::FRAC_PI_2 {
                    let across = radius * half.sin();
                    (Vec2::new(-across, 0.0), Vec2::new(across, radius))
                } else {
                    (Vec2::new(-radius, radius * half.cos()), Vec2::splat(radius))
                }
            }
            Self::Lane { width, length } => (Vec2::new(-width * 0.5, 0.0), Vec2::new(width * 0.5, length)),
        }
    }

    /// Its largest size across, metres.
    #[must_use]
    pub fn size(self) -> f32 {
        let (min, max) = self.bounds();
        (max - min).max_element()
    }
}

/// A ground decal the renderer draws (rule R4), as `sapphire_vfx` resolved it for the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VfxDecal {
    pub shape: VfxDecalShape,
    /// Where its local origin lies on the ground.
    pub position: Vec3,
    /// Its local +Y to the ground's normal, its +Z the way its placement faces.
    pub rotation: Quat,
    /// Linear, relative to the display: the light it adds, or what it darkens toward.
    pub color: Vec3,
    /// Its opacity, 0 to 1, over its outline, body and fill.
    pub alpha: f32,
    pub blend: VfxDecalBlend,
    /// How far the fill has grown over its reach, 0 to 1 ([`VfxDecalShape::progress`]).
    pub fill: f32,
    /// The outline's width inside the edge, metres: 0 has none.
    pub outline: f32,
    /// The share of its opacity the body shows where the fill hasn't reached, 0 to 1.
    pub body: f32,
    /// How far the edge wanders either way with [`edge_noise`], metres: 0 is a clean edge.
    pub noise: f32,
}

impl VfxDecal {
    /// The share of a pixel `footprint` metres across that covers `distance` from an edge (negative inside):
    /// linear across one footprint, as `vfx_decal.wgsl`'s `decal_cover`.
    #[must_use]
    pub fn cover(distance: f32, footprint: f32) -> f32 {
        (0.5 - distance / footprint.max(1e-6)).clamp(0.0, 1.0)
    }

    /// How opaque it is at local `point`, for a pixel `footprint` metres across there and the progress
    /// changing by `progress_footprint` across it: the shader's sum of outline, body and fill, without the
    /// fog.
    #[must_use]
    pub fn opacity(&self, point: Vec2, footprint: f32, progress_footprint: f32) -> f32 {
        let distance = self.shape.distance(point) - self.noise * edge_noise(point);
        let inside = Self::cover(distance, footprint);
        let inner = Self::cover(distance + self.outline, footprint);
        let band = (inside - inner).max(0.0);
        let filled = if self.fill >= 1.0 {
            1.0
        } else if self.fill <= 0.0 {
            0.0
        } else {
            Self::cover(self.shape.progress(point) - self.fill, progress_footprint)
        };
        self.alpha.clamp(0.0, 1.0) * (band + inner * (self.body + (1.0 - self.body) * filled))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPES: [VfxDecalShape; 5] = [
        VfxDecalShape::Disc { radius: 1.5 },
        VfxDecalShape::Ring { inner: 0.8, outer: 2.0 },
        VfxDecalShape::Sector { radius: 3.0, angle: 90.0, inner: 0.0 },
        VfxDecalShape::Sector { radius: 2.0, angle: 240.0, inner: 0.5 },
        VfxDecalShape::Lane { width: 1.2, length: 4.0 },
    ];

    fn grid(min: Vec2, max: Vec2, side: u32) -> impl Iterator<Item = Vec2> {
        (0..side * side).map(move |index| {
            let t = Vec2::new((index % side) as f32 + 0.5, (index / side) as f32 + 0.5) / side as f32;
            min + (max - min) * t
        })
    }

    /// Inside, outside and on the edge, each shape's distance says so, and it's a distance: moving a point by
    /// `step` changes it by at most `step`.
    #[test]
    fn each_shapes_distance_is_signed_and_one_lipschitz() {
        let inside = [
            (SHAPES[0], Vec2::ZERO),
            (SHAPES[1], Vec2::new(1.4, 0.0)),
            (SHAPES[2], Vec2::new(0.0, 2.0)),
            (SHAPES[3], Vec2::new(-1.0, 0.2)),
            (SHAPES[4], Vec2::new(0.3, 3.5)),
        ];
        let outside = [
            (SHAPES[0], Vec2::new(1.6, 0.0)),
            (SHAPES[1], Vec2::ZERO),
            (SHAPES[2], Vec2::new(2.0, 0.5)),
            (SHAPES[3], Vec2::new(0.0, -1.0)),
            (SHAPES[4], Vec2::new(0.0, -0.1)),
        ];
        for (shape, point) in inside {
            assert!(shape.distance(point) < 0.0, "{shape:?} at {point}: {}", shape.distance(point));
        }
        for (shape, point) in outside {
            assert!(shape.distance(point) > 0.0, "{shape:?} at {point}: {}", shape.distance(point));
        }
        assert!(SHAPES[0].distance(Vec2::new(0.0, 1.5)).abs() < 1e-6);
        assert!((SHAPES[4].distance(Vec2::new(0.6, 2.0))).abs() < 1e-6, "the lane's side");
        assert!((SHAPES[2].distance(Vec2::new(0.0, -1.0)) - 1.0).abs() < 1e-5, "behind the cone's point");
        let step = 0.01;
        for shape in SHAPES {
            let (min, max) = shape.bounds();
            for point in grid(min - 1.0, max + 1.0, 48) {
                for offset in [Vec2::X, Vec2::Y, Vec2::new(0.6, -0.8)] {
                    let change = (shape.distance(point + offset * step) - shape.distance(point)).abs();
                    assert!(change <= step * 1.001 + 1e-6, "{shape:?} at {point}: {change}");
                }
            }
        }
    }

    /// Everything inside lies within the bounds, and the bounds aren't loose by much.
    #[test]
    fn bounds_hold_the_whole_shape() {
        for shape in SHAPES {
            let (min, max) = shape.bounds();
            let (wide_min, wide_max) = (min - 2.0, max + 2.0);
            let mut reach_min = Vec2::splat(f32::MAX);
            let mut reach_max = Vec2::splat(f32::MIN);
            for point in grid(wide_min, wide_max, 160) {
                if shape.distance(point) < 0.0 {
                    assert!(
                        point.cmpge(min - 1e-4).all() && point.cmple(max + 1e-4).all(),
                        "{shape:?}: {point}"
                    );
                    reach_min = reach_min.min(point);
                    reach_max = reach_max.max(point);
                }
            }
            let cell = (wide_max - wide_min) / 160.0;
            assert!((reach_min - min).cmple(cell * 2.0).all(), "{shape:?}: {reach_min} {min}");
            assert!((max - reach_max).cmple(cell * 2.0).all(), "{shape:?}: {reach_max} {max}");
        }
    }

    /// The fill runs from the centre (the inner edge, the lane's foot) at 0 to the far edge at 1.
    #[test]
    fn progress_runs_from_the_start_of_the_reach_to_its_end() {
        assert_eq!(SHAPES[0].progress(Vec2::ZERO), 0.0);
        assert!((SHAPES[0].progress(Vec2::new(0.0, 1.5)) - 1.0).abs() < 1e-6);
        assert!(SHAPES[1].progress(Vec2::new(0.8, 0.0)).abs() < 1e-6);
        assert!((SHAPES[1].progress(Vec2::new(0.0, 1.4)) - 0.5).abs() < 1e-6);
        assert!((SHAPES[4].progress(Vec2::new(0.5, 1.0)) - 0.25).abs() < 1e-6, "along a lane, not across");
        assert_eq!(SHAPES[4].progress(Vec2::new(-0.5, 1.0)), SHAPES[4].progress(Vec2::new(0.5, 1.0)));
    }

    fn telegraph(fill: f32) -> VfxDecal {
        VfxDecal {
            shape: VfxDecalShape::Disc { radius: 2.0 },
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            color: Vec3::ONE,
            alpha: 1.0,
            blend: VfxDecalBlend::Light,
            fill,
            outline: 0.1,
            body: 0.25,
            noise: 0.0,
        }
    }

    /// The outline shows the whole reach at once; inside it the body is faint where the fill hasn't reached
    /// and whole where it has; past the edge, nothing.
    #[test]
    fn the_outline_shows_the_reach_and_the_fill_the_time() {
        let footprint = 0.01;
        let decal = telegraph(0.5);
        let at = |x: f32| decal.opacity(Vec2::new(x, 0.0), footprint, footprint / 2.0);
        assert!((at(1.95) - 1.0).abs() < 1e-5, "on the outline: {}", at(1.95));
        assert!((at(0.5) - 1.0).abs() < 1e-5, "filled: {}", at(0.5));
        assert!((at(1.5) - 0.25).abs() < 1e-5, "the body ahead of the fill: {}", at(1.5));
        assert_eq!(at(2.1), 0.0, "past the edge");
        // At the fill's front, half a pixel either way.
        assert!((at(1.0) - 0.625).abs() < 1e-3, "{}", at(1.0));
        assert!((telegraph(1.0).opacity(Vec2::new(1.5, 0.0), footprint, 0.0) - 1.0).abs() < 1e-5);
        assert!((telegraph(0.0).opacity(Vec2::ZERO, footprint, footprint) - 0.25).abs() < 1e-5);
    }

    /// One pixel's footprint softens the edge, no more: a pixel wholly inside is covered, wholly outside not.
    #[test]
    fn the_edge_is_smoothed_over_one_footprint() {
        assert_eq!(VfxDecal::cover(-0.5, 1.0), 1.0);
        assert_eq!(VfxDecal::cover(0.5, 1.0), 0.0);
        assert_eq!(VfxDecal::cover(0.0, 1.0), 0.5);
        assert_eq!(VfxDecal::cover(0.0, 0.0), 0.5);
    }

    /// The noise stays within its range, moves the edge by at most its amplitude, and is the same each time.
    #[test]
    fn the_noise_breaks_the_edge_within_its_reach() {
        let mut low = 0.0_f32;
        let mut high = 0.0_f32;
        for point in grid(Vec2::splat(-4.0), Vec2::splat(4.0), 128) {
            let value = edge_noise(point);
            assert!((-1.0..=1.0).contains(&value), "{point}: {value}");
            assert_eq!(value, edge_noise(point));
            low = low.min(value);
            high = high.max(value);
        }
        assert!(low < -0.4 && high > 0.4, "it spreads: {low} {high}");
        let scorch = VfxDecal { noise: 0.2, outline: 0.0, body: 1.0, fill: 1.0, ..telegraph(1.0) };
        let footprint = 0.01;
        for point in grid(Vec2::splat(-2.5), Vec2::splat(2.5), 96) {
            let opacity = scorch.opacity(point, footprint, footprint);
            let distance = scorch.shape.distance(point);
            if distance < -0.2 - footprint {
                assert_eq!(opacity, 1.0, "{point}");
            } else if distance > 0.2 + footprint {
                assert_eq!(opacity, 0.0, "{point}");
            }
        }
    }
}
