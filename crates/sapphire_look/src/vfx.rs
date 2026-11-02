//! The frame's effects as plain data (rule R4, spec §5.16): the sprites and ribbon points
//! `sapphire_vfx` resolves each frame and the renderer draws, and the shapes of the sprites' map,
//! generated in code. Beneath both, so neither depends on the other: the renderer never sees an
//! emitter, a curve or an owner's clock, and the simulation links no GPU code.
//!
//! - **Colors** are linear. A [`VfxLight::Emissive`] color is relative to the display: the renderer
//!   divides it by the frame's exposure, so 1.0 reaches post's tone curve as 1.0 at noon and at
//!   midnight, and a spark's core stays what its author set whatever the hour. A [`VfxLight::Lit`]
//!   color is an albedo the frame's ambient and key light fall on: dust is dim at night.
//! - **Blending** is premultiplied, with one number between its two ends: [`VfxSprite::additive`]
//!   is 1 for pure light over what's behind, 0 for a surface that covers it, and anything between
//!   for light with some body, which is what keeps a tinted edge readable over white plaster.
//!
//! The shapes' coverage is a pure function ([`VfxShape::coverage`]); the renderer rasterizes it
//! into a texture array at first use, so no third-party art is involved.

use std::time::Duration;

use glam::Vec3;

/// What a sprite shows: a layer of the generated map (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VfxShape {
    /// A round dot, soft to its edge: dust, motes.
    #[default]
    SoftDot,
    /// A thin stroke along the quad's length, a point at its tail and a rounded head: sparks.
    Streak,
    /// A four-pointed star with concave sides and a crisp outline: a hit's flash.
    Star,
    /// A thin ring, crisp outside and soft inside: a hit's shock.
    Ring,
}

impl VfxShape {
    pub const ALL: [Self; 4] = [Self::SoftDot, Self::Streak, Self::Star, Self::Ring];

    /// Its layer of the map.
    pub const fn layer(self) -> u32 {
        self as u32
    }

    /// How much of the quad the shape covers at `(u, v)` in 0 to 1, a quad's whole width and
    /// length, its length along `v`: 0 at the quad's edges, so no quad shows its border.
    pub fn coverage(self, u: f32, v: f32) -> f32 {
        let (x, y) = (2.0 * u - 1.0, 2.0 * v - 1.0);
        let r = (x * x + y * y).sqrt();
        match self {
            Self::SoftDot => {
                let falloff = (1.0 - r * r).max(0.0);
                falloff * falloff
            }
            Self::Streak => {
                let across = (1.0 - x * x).max(0.0);
                // A long point toward the tail (v = 0), and a short round head.
                across * across * smoothstep(0.0, 0.8, v) * (1.0 - smoothstep(0.9, 1.0, v))
            }
            Self::Star => {
                // An astroid: √|x| + √|y| = 1, whose sides curve in to four sharp points. Its
                // outline is crisp, with a small glow at the center that bloom picks up.
                let astroid = x.abs().sqrt() + y.abs().sqrt();
                let body = 1.0 - smoothstep(0.82, 1.0, astroid);
                let center = (1.0 - (r / 0.45).powi(2)).max(0.0);
                body.max(center * center) * (1.0 - smoothstep(0.92, 1.0, r))
            }
            // A crisp outer edge, and a short fall inside it.
            Self::Ring => smoothstep(0.66, 0.9, r) * (1.0 - smoothstep(0.9, 0.95, r)),
        }
    }
}

fn smoothstep(start: f32, end: f32, x: f32) -> f32 {
    let t = ((x - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Where a sprite's light comes from (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VfxLight {
    /// Its color is its own, relative to the display: sparks, flashes, trails.
    #[default]
    Emissive,
    /// Its color is an albedo under the frame's ambient and key light: dust.
    Lit,
}

/// A quad the renderer draws (rule R4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VfxSprite {
    pub position: Vec3,
    /// The world direction the quad's length lies along, which it turns about to face the camera;
    /// any length. Zero faces the camera whole, turned by [`Self::rotation`].
    pub axis: Vec3,
    /// Radians about the view axis, where [`Self::axis`] is zero.
    pub rotation: f32,
    /// Half its width, meters.
    pub half_width: f32,
    /// Half its length, meters.
    pub half_length: f32,
    pub color: Vec3,
    /// Its opacity, over the shape's coverage.
    pub alpha: f32,
    /// How much of it is light added over what's behind (see the module docs): 0 to 1.
    pub additive: f32,
    /// Over how many meters it fades into the surface behind it, where the frame has depth to
    /// read (the renderer's `vfx` module).
    pub soft: f32,
    pub shape: VfxShape,
    pub light: VfxLight,
}

/// A point along a ribbon: two rails, and how the cross-section between them looks there. A
/// ribbon's points are consecutive in [`VfxFrame::ribbon_points`], oldest first, its last one
/// marked [`Self::ends`], and it draws as a strip from rail to rail, always
/// [emissive](VfxLight::Emissive).
///
/// Across the strip (0 at `base`, 1 at `tip`) a crisp bright band, the **core**, sits at
/// [`Self::core_position`], and a soft glow falls away from it over [`Self::glow_reach`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VfxRibbonPoint {
    pub base: Vec3,
    pub tip: Vec3,
    /// The glow's color.
    pub color: Vec3,
    /// Its opacity, over the cross-section's coverage.
    pub alpha: f32,
    /// The core's color.
    pub core: Vec3,
    /// Where the core's middle sits across the strip, 0 to 1.
    pub core_position: f32,
    /// Half the core's width, as a share of the strip's.
    pub core_half_width: f32,
    /// How far the glow reaches from the core, as a share of the strip's width.
    pub glow_reach: f32,
    /// As [`VfxSprite::additive`].
    pub additive: f32,
    /// As [`VfxSprite::soft`].
    pub soft: f32,
    /// The ribbon's last point: no strip joins it to the next.
    pub ends: bool,
}

/// What the simulation holds and what it dropped (rule R9), carried with the frame so the
/// renderer's stats, and the overlay that shows them, need no edge to `sapphire_vfx`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VfxStats {
    /// Effects playing.
    pub effects: u32,
    /// Live particles.
    pub particles: u32,
    /// Ribbons drawn, and their points after smoothing.
    pub ribbons: u32,
    pub ribbon_points: u32,
    /// Particles not spawned because the budget's cap was full, since the simulation began.
    pub dropped_particles: u64,
    /// Ribbon points not drawn because the budget's cap was full, since the simulation began. Each
    /// ribbon loses its oldest first.
    pub dropped_ribbon_points: u64,
    /// Effects not spawned because the budget's cap was full, since the simulation began.
    pub dropped_effects: u64,
    /// Effects that spawned without their trail because the budget's cap was full, since the
    /// simulation began.
    pub dropped_trails: u64,
    /// The app's measure of the last simulation step and frame write, against spec §3.2's budget
    /// (0.3 ms on Desktop-Low, 0.6 on Mobile-Low). Zero until an app measures it: the simulation
    /// can't read the clock itself (rule R2).
    pub simulation: Duration,
}

/// One frame's effects (rule R4): `FramePacket::vfx`. Empty costs the renderer nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VfxFrame {
    pub sprites: Vec<VfxSprite>,
    pub ribbon_points: Vec<VfxRibbonPoint>,
    pub stats: VfxStats,
}

impl VfxFrame {
    /// Empties it, keeping its vectors' capacity.
    pub fn clear(&mut self) {
        self.sprites.clear();
        self.ribbon_points.clear();
        self.stats = VfxStats::default();
    }

    /// Whether it draws nothing.
    pub fn is_empty(&self) -> bool {
        self.sprites.is_empty() && self.ribbon_points.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples at the quad's border: none may show.
    fn border() -> impl Iterator<Item = (f32, f32)> {
        (0..=64).flat_map(|i| {
            let t = i as f32 / 64.0;
            [(t, 0.0), (t, 1.0), (0.0, t), (1.0, t)]
        })
    }

    #[test]
    fn no_shape_shows_its_quads_border() {
        for shape in VfxShape::ALL {
            for (u, v) in border() {
                let coverage = shape.coverage(u, v);
                assert!(coverage < 0.005, "{shape:?} at ({u}, {v}): {coverage}");
            }
        }
    }

    #[test]
    fn coverage_stays_between_zero_and_one() {
        for shape in VfxShape::ALL {
            let mut most = 0.0_f32;
            for j in 0..128 {
                for i in 0..128 {
                    let coverage = shape.coverage((i as f32 + 0.5) / 128.0, (j as f32 + 0.5) / 128.0);
                    assert!((0.0..=1.0).contains(&coverage), "{shape:?}: {coverage}");
                    most = most.max(coverage);
                }
            }
            assert!(most > 0.95, "{shape:?} reaches {most}");
        }
    }

    #[test]
    fn the_dot_and_the_star_fill_their_centers_and_the_ring_leaves_its_empty() {
        assert_eq!(VfxShape::SoftDot.coverage(0.5, 0.5), 1.0);
        assert_eq!(VfxShape::Star.coverage(0.5, 0.5), 1.0);
        assert_eq!(VfxShape::Ring.coverage(0.5, 0.5), 0.0);
        assert!(VfxShape::Ring.coverage(0.93, 0.5) > 0.9, "on the ring's crest");
    }

    #[test]
    fn the_star_points_along_its_axes_not_its_diagonals() {
        let (axis, diagonal) = (VfxShape::Star.coverage(0.85, 0.5), VfxShape::Star.coverage(0.75, 0.75));
        assert!(axis > 0.9 && diagonal < 0.1, "axis {axis}, diagonal {diagonal}");
    }

    #[test]
    fn a_streak_lies_along_its_length_with_its_head_forward() {
        let (along, across) = (VfxShape::Streak.coverage(0.5, 0.8), VfxShape::Streak.coverage(0.95, 0.8));
        assert!(along > 0.9 && across < 0.1, "along {along}, across {across}");
        assert!(
            VfxShape::Streak.coverage(0.5, 0.2) < VfxShape::Streak.coverage(0.5, 0.8),
            "the tail is thinner"
        );
    }

    #[test]
    fn layers_are_the_shapes_places() {
        for (index, shape) in VfxShape::ALL.into_iter().enumerate() {
            assert_eq!(shape.layer(), index as u32);
        }
    }

    #[test]
    fn clearing_keeps_capacity_and_is_empty() {
        let mut frame = VfxFrame::default();
        assert!(frame.is_empty());
        frame.sprites.reserve(8);
        frame.stats.particles = 3;
        frame.clear();
        assert!(frame.is_empty() && frame.sprites.capacity() >= 8);
        assert_eq!(frame.stats, VfxStats::default());
    }
}
