//! The screen effects' GPU side (M1 P3): how [`PostEffects`] become the fused pass's constants,
//! and what a tier spends on them. See the post module's docs, "Screen effects".

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use glam::Vec2;
use serde::{Deserialize, Serialize};

use super::layers::{encoded, finite, finite_color};
use super::params::{EffectCoverage, PostEffects};

/// The names of `post.wgsl`'s pipeline-overridable constants for the effects: the fused pass's
/// variants with the lens effects (radial blur, chromatic aberration) and with the color effects
/// (inversion, duotone, speed lines), and the radial blur's taps per pixel (the tier's).
pub(crate) const LENS_EFFECTS_CONSTANT: &str = "POST_LENS_EFFECTS";
pub(crate) const COLOR_EFFECTS_CONSTANT: &str = "POST_COLOR_EFFECTS";
pub(crate) const RADIAL_TAPS_CONSTANT: &str = "POST_RADIAL_TAPS";

/// The most taps per pixel a radial blur takes.
pub const MAX_RADIAL_BLUR_TAPS: u32 = 16;

/// What the screen effects cost on a quality tier (`tiers.ron`,
/// [`QualityTier`](crate::quality::QualityTier)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(deny_unknown_fields)]
pub struct PostEffectSettings {
    /// Taps per pixel along a radial blur's streak: 4, 8, 12 or 16. They are unrolled in groups
    /// of four, and a pixel's taps start at a per-pixel offset, so fewer read as finer grain
    /// rather than as copies of the frame.
    pub radial_blur_taps: u32,
}

impl Default for PostEffectSettings {
    /// Desktop-Low's, which the golden images render with.
    fn default() -> Self {
        Self { radial_blur_taps: 12 }
    }
}

impl PostEffectSettings {
    /// Checks the taps, returning the field and what is wrong.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        let taps = self.radial_blur_taps;
        if !(taps.is_multiple_of(4) && (4..=MAX_RADIAL_BLUR_TAPS).contains(&taps)) {
            return Err(("post_effects.radial_blur_taps", "must be 4, 8, 12 or 16"));
        }
        Ok(())
    }
}

/// Which screen effects post drew in the last prepared frame (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawnEffects {
    pub radial_blur: bool,
    pub chromatic_aberration: bool,
    pub inversion: bool,
    pub duotone: bool,
    pub speed_lines: bool,
    /// An effect read the character mask: it covers the world and characters differently.
    pub character_mask: bool,
}

impl DrawnEffects {
    /// How many effects drew.
    pub fn count(&self) -> u32 {
        [self.radial_blur, self.chromatic_aberration, self.inversion, self.duotone, self.speed_lines]
            .into_iter()
            .map(u32::from)
            .sum()
    }

    /// Whether any effect drew.
    pub fn any(&self) -> bool {
        self.count() > 0
    }

    /// Whether a lens effect drew, which reads the scene along a ray or split: the fused pass
    /// then draws a variant with the lens effects (`POST_LENS_EFFECTS`).
    pub fn lens(&self) -> bool {
        self.radial_blur || self.chromatic_aberration
    }

    /// Whether a color effect drew, over the display values: the fused pass then draws a variant
    /// with the color effects (`POST_COLOR_EFFECTS`).
    pub fn color(&self) -> bool {
        self.inversion || self.duotone || self.speed_lines
    }
}

/// The effects' share of the post constants (WGSL `PostEffects`), appended to `PostUniforms`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuEffects {
    /// xy: the radial blur's center; z: its strength (0: none); w: chromatic aberration's
    /// strength (0: none).
    pub lens: [f32; 4],
    /// xy: chromatic aberration's center.
    pub chromatic: [f32; 4],
    /// x: inversion's amount; y: its weight on the world; z: on characters.
    pub inversion: [f32; 4],
    /// x: the duotone's amount; y: its weight on the world; z: on characters; w: the display
    /// luminance where its ramp starts.
    pub duotone: [f32; 4],
    /// rgb: the duotone's dark color, sRGB-encoded; w: 1 / its ramp's width.
    pub duotone_dark: [f32; 4],
    /// rgb: its light color, sRGB-encoded.
    pub duotone_light: [f32; 4],
    /// x: speed lines' opacity; y: their weight on the world; z: on characters; w: lines around
    /// the center.
    pub lines: [f32; 4],
    /// xy: the lines' center; z: a line's width at its outer end, as a share of its slot; w: the
    /// clear radius, in frame heights.
    pub lines_shape: [f32; 4],
    /// rgb: the lines' color, sRGB-encoded; w: how far past the clear radius their tips scatter.
    pub lines_color: [f32; 4],
    /// x: the lines' seed; y: 1 when an effect reads the character mask.
    pub flags: [u32; 4],
}

impl GpuEffects {
    /// `effects` as the fused pass reads them, clamped to working ranges so no input can produce
    /// NaN, and which of them draw.
    pub(crate) fn new(effects: &PostEffects) -> (Self, DrawnEffects) {
        let unit = |value: f32| finite(value, 0.0).clamp(0.0, 1.0);
        // Hit points may lie off screen; far off, nothing changes on it.
        let point = |center: Vec2| {
            if center.is_finite() {
                center.clamp(Vec2::splat(-1.0), Vec2::splat(2.0))
            } else {
                Vec2::splat(0.5)
            }
        };
        let coverage = |coverage: EffectCoverage| [unit(coverage.world), unit(coverage.characters)];
        let shows = |amount: f32, [world, characters]: [f32; 2]| amount > 0.0 && world.max(characters) > 0.0;
        let masked = |[world, characters]: [f32; 2]| world != characters;

        let blur = &effects.radial_blur;
        let blur_center = point(blur.center);
        let blur_strength = unit(blur.strength);
        let aberration = &effects.chromatic_aberration;
        let aberration_center = point(aberration.center);
        let aberration_strength = finite(aberration.strength, 0.0).clamp(0.0, 0.25);

        let inversion = &effects.inversion;
        let inversion_amount = unit(inversion.amount);
        let inversion_coverage = coverage(inversion.coverage);

        let duotone = &effects.duotone;
        let duotone_amount = unit(duotone.amount);
        let duotone_coverage = coverage(duotone.coverage);
        let softness = finite(duotone.softness, 0.0).clamp(1e-3, 1.0);
        let ramp_start = unit(duotone.threshold) - 0.5 * softness;

        let lines = &effects.speed_lines;
        let lines_opacity = unit(lines.opacity);
        let lines_coverage = coverage(lines.coverage);
        let lines_center = point(lines.center);
        let lines_width = unit(lines.width);

        let drawn_inversion = shows(inversion_amount, inversion_coverage);
        let drawn_duotone = shows(duotone_amount, duotone_coverage);
        let drawn_lines = shows(lines_opacity, lines_coverage) && lines_width > 0.0;
        let character_mask = (drawn_inversion && masked(inversion_coverage))
            || (drawn_duotone && masked(duotone_coverage))
            || (drawn_lines && masked(lines_coverage));
        let drawn = DrawnEffects {
            radial_blur: blur_strength > 0.0,
            chromatic_aberration: aberration_strength > 0.0,
            inversion: drawn_inversion,
            duotone: drawn_duotone,
            speed_lines: drawn_lines,
            character_mask,
        };
        let gpu = Self {
            lens: [blur_center.x, blur_center.y, blur_strength, aberration_strength],
            chromatic: [aberration_center.x, aberration_center.y, 0.0, 0.0],
            inversion: [
                if drawn_inversion { inversion_amount } else { 0.0 },
                inversion_coverage[0],
                inversion_coverage[1],
                0.0,
            ],
            duotone: [
                if drawn_duotone { duotone_amount } else { 0.0 },
                duotone_coverage[0],
                duotone_coverage[1],
                ramp_start,
            ],
            duotone_dark: encoded(finite_color(duotone.dark)).extend(1.0 / softness).to_array(),
            duotone_light: encoded(finite_color(duotone.light)).extend(0.0).to_array(),
            lines: [
                if drawn_lines { lines_opacity } else { 0.0 },
                lines_coverage[0],
                lines_coverage[1],
                lines.count.clamp(1, 1024) as f32,
            ],
            lines_shape: [
                lines_center.x,
                lines_center.y,
                lines_width,
                finite(lines.inner_radius, 0.0).clamp(0.0, 4.0),
            ],
            lines_color: encoded(finite_color(lines.color))
                .extend(finite(lines.spread, 0.0).clamp(0.0, 4.0))
                .to_array(),
            flags: [lines.seed, u32::from(character_mask), 0, 0],
        };
        (gpu, drawn)
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::post::params::{ChromaticAberration, Duotone, Inversion, RadialBlur, SpeedLines};

    #[test]
    fn no_effects_draw_nothing_and_read_no_mask() {
        let (gpu, drawn) = GpuEffects::new(&PostEffects::NONE);
        assert_eq!(drawn, DrawnEffects::default());
        assert!(!drawn.any());
        assert_eq!(
            (gpu.lens[2], gpu.lens[3], gpu.inversion[0], gpu.duotone[0], gpu.lines[0]),
            (0.0, 0.0, 0.0, 0.0, 0.0)
        );
        // An amount without anywhere to show, or lines without width, draw nothing either.
        let hidden = PostEffects {
            inversion: Inversion { amount: 1.0, coverage: EffectCoverage { world: 0.0, characters: 0.0 } },
            speed_lines: SpeedLines { opacity: 1.0, width: 0.0, ..SpeedLines::NONE },
            ..PostEffects::NONE
        };
        assert!(!GpuEffects::new(&hidden).1.any());
    }

    #[test]
    fn effects_clamp_every_parameter_and_say_which_draw() {
        let effects = PostEffects {
            radial_blur: RadialBlur { center: Vec2::new(f32::NAN, 0.2), strength: 3.0 },
            chromatic_aberration: ChromaticAberration { center: Vec2::new(9.0, -9.0), strength: 1.0 },
            inversion: Inversion { amount: 0.5, coverage: EffectCoverage::EVERYWHERE },
            duotone: Duotone {
                amount: 1.0,
                dark: Vec3::splat(f32::INFINITY),
                light: Vec3::new(1.0, 0.0, 0.0),
                threshold: 0.4,
                softness: 0.0,
                coverage: EffectCoverage::WORLD,
            },
            speed_lines: SpeedLines { opacity: 2.0, count: 5000, spread: f32::NAN, ..SpeedLines::NONE },
        };
        let (gpu, drawn) = GpuEffects::new(&effects);
        assert_eq!(drawn.count(), 5);
        assert!(drawn.character_mask, "the duotone keeps characters colored");
        assert_eq!(gpu.flags[1], 1);
        assert_eq!(gpu.lens, [0.5, 0.5, 1.0, 0.25], "a non-finite center is the middle");
        assert_eq!(&gpu.chromatic[..2], &[2.0, -1.0], "far off screen is far enough");
        assert_eq!(gpu.duotone_dark[..3], [0.0; 3], "a non-finite color is black");
        let (start, slope) = (gpu.duotone[3], gpu.duotone_dark[3]);
        assert!((start - 0.3995).abs() < 1e-6 && (slope - 1000.0).abs() < 0.01, "the hardest cut");
        assert_eq!((gpu.lines[0], gpu.lines[3], gpu.lines_color[3]), (1.0, 1024.0, 0.0));
        let all = bytemuck::cast_slice::<GpuEffects, u32>(std::slice::from_ref(&gpu));
        assert!(all[..36].iter().all(|bits| f32::from_bits(*bits).is_finite()));
        // Everywhere alike reads no mask.
        let plain = PostEffects {
            duotone: Duotone { coverage: EffectCoverage::EVERYWHERE, ..effects.duotone },
            ..effects
        };
        assert!(!GpuEffects::new(&plain).1.character_mask);
    }

    #[test]
    fn radial_taps_are_whole_groups_of_four() {
        for taps in [4, 8, 12, 16] {
            assert!(PostEffectSettings { radial_blur_taps: taps }.validate().is_ok());
        }
        for taps in [0, 6, 20] {
            assert!(PostEffectSettings { radial_blur_taps: taps }.validate().is_err(), "{taps}");
        }
        assert!(PostEffectSettings::default().validate().is_ok());
    }
}
