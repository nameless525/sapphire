//! The screen effects' GPU side (M1 P3): how [`PostEffects`] become the fused pass's constants,
//! and what a tier spends on them. See the post module's docs, "Screen effects".

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

use super::layers::{encoded, finite, finite_color};
use sapphire_look::post::{EffectCoverage, PostEffects};

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
    /// The effects pack's (M5 L9).
    pub desaturation: bool,
    pub halftone: bool,
    pub flash: bool,
    pub glitch: bool,
    pub crt: bool,
    /// An effect read the character mask: it covers the world and characters differently.
    pub character_mask: bool,
}

impl DrawnEffects {
    /// How many effects drew.
    pub fn count(&self) -> u32 {
        [
            self.radial_blur,
            self.chromatic_aberration,
            self.inversion,
            self.duotone,
            self.speed_lines,
            self.desaturation,
            self.halftone,
            self.flash,
            self.glitch,
            self.crt,
        ]
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
    /// with the color effects (`POST_COLOR_EFFECTS`). The effects pack (M5 L9) is all in it, the glitch
    /// too: its shifts are whole pixels of the scene the pass reads anyway.
    pub fn color(&self) -> bool {
        self.inversion
            || self.duotone
            || self.speed_lines
            || self.desaturation
            || self.halftone
            || self.flash
            || self.glitch
            || self.crt
    }
}

/// Relative luminance's weights, as `post.wgsl`'s `LUMA`.
const LUMA: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);

/// A linear `tint` over its relative luminance, so multiplying light by it keeps the light's; white if it has
/// none to keep.
fn luminance_kept(tint: Vec3) -> Vec3 {
    let luminance = tint.dot(LUMA);
    if luminance > 1e-4 { tint / luminance } else { Vec3::ONE }
}

/// The effects' share of the post constants (WGSL `PostEffects`), appended to `PostUniforms`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuEffects {
    /// xy: the radial blur's center; z: its strength (0: none); w: chromatic aberration's
    /// strength (0: none).
    pub lens: [f32; 4],
    /// xy: chromatic aberration's center; z: the most the effects may move a pixel's relative luminance
    /// (M4 A3), 1 for no hold.
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
    /// x: the lines' seed; y: 1 when an effect reads the character mask; z: the glitch's seed.
    pub flags: [u32; 4],
    // The effects pack's (M5 L9), appended.
    /// x: the desaturation's amount; y: its weight on the world; z: on characters.
    pub desaturation: [f32; 4],
    /// rgb: what the grey's light is multiplied by, linear, over its own relative luminance, so the tint
    /// keeps the grey's.
    pub desaturation_tint: [f32; 4],
    /// x: the halftone's amount; y: its weight on the world; z: on characters; w: its cell, in frame heights.
    pub halftone: [f32; 4],
    /// rgb: its ink, sRGB-encoded.
    pub halftone_ink: [f32; 4],
    /// xy: the grid's turn, its cosine and sine.
    pub halftone_turn: [f32; 4],
    /// x: the flash's amount; y: its weight on the world; z: on characters.
    pub flash: [f32; 4],
    /// rgb: its color, sRGB-encoded.
    pub flash_color: [f32; 4],
    /// x: the glitch's amount; y: bands per frame height; z: the share displaced; w: the farthest shift, in
    /// frame widths.
    pub glitch: [f32; 4],
    /// x: the red and blue split, in frame widths.
    pub glitch_split: [f32; 4],
    /// x: the CRT's amount; y: scanlines per frame height; z: their gaps' depth; w: the grille's.
    pub crt: [f32; 4],
    /// x: the corners' radius, in frame heights.
    pub crt_corner: [f32; 4],
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

        let desaturation = &effects.desaturation;
        let desaturation_amount = unit(desaturation.amount);
        let desaturation_coverage = coverage(desaturation.coverage);

        let halftone = &effects.halftone;
        let halftone_amount = unit(halftone.amount);
        let halftone_coverage = coverage(halftone.coverage);
        let halftone_cell = finite(halftone.cell, 0.0).clamp(0.0, 0.25);
        let (sine, cosine) = finite(halftone.angle, 0.0).sin_cos();

        let flash = &effects.flash;
        let flash_amount = unit(flash.amount);
        let flash_coverage = coverage(flash.coverage);

        let glitch = &effects.glitch;
        let glitch_amount = unit(glitch.amount);
        let glitch_share = unit(glitch.share);
        let glitch_shift = finite(glitch.shift, 0.0).clamp(0.0, 0.5);
        let glitch_split = finite(glitch.split, 0.0).clamp(0.0, 0.1);

        let crt = &effects.crt;
        let crt_amount = unit(crt.amount);
        let crt_depth = unit(crt.depth);
        let crt_grille = unit(crt.grille);
        let crt_corner = finite(crt.corner, 0.0).clamp(0.0, 0.5);

        let drawn_inversion = shows(inversion_amount, inversion_coverage);
        let drawn_duotone = shows(duotone_amount, duotone_coverage);
        let drawn_lines = shows(lines_opacity, lines_coverage) && lines_width > 0.0;
        let drawn_desaturation = shows(desaturation_amount, desaturation_coverage);
        // A cell under about a thousandth of a frame is finer than any output's pixels: nothing to print.
        let drawn_halftone = shows(halftone_amount, halftone_coverage) && halftone_cell >= 1e-3;
        let drawn_flash = shows(flash_amount, flash_coverage);
        let drawn_glitch =
            glitch_amount > 0.0 && ((glitch_share > 0.0 && glitch_shift > 0.0) || glitch_split > 0.0);
        let drawn_crt = crt_amount > 0.0 && (crt_depth > 0.0 || crt_grille > 0.0 || crt_corner > 0.0);
        let character_mask = (drawn_inversion && masked(inversion_coverage))
            || (drawn_duotone && masked(duotone_coverage))
            || (drawn_lines && masked(lines_coverage))
            || (drawn_desaturation && masked(desaturation_coverage))
            || (drawn_halftone && masked(halftone_coverage))
            || (drawn_flash && masked(flash_coverage));
        let drawn = DrawnEffects {
            radial_blur: blur_strength > 0.0,
            chromatic_aberration: aberration_strength > 0.0,
            inversion: drawn_inversion,
            duotone: drawn_duotone,
            speed_lines: drawn_lines,
            desaturation: drawn_desaturation,
            halftone: drawn_halftone,
            flash: drawn_flash,
            glitch: drawn_glitch,
            crt: drawn_crt,
            character_mask,
        };
        // An effect that doesn't draw has amount 0 here, which is what the pass branches on.
        let amount = |drawn: bool, amount: f32| if drawn { amount } else { 0.0 };
        let gpu = Self {
            lens: [blur_center.x, blur_center.y, blur_strength, aberration_strength],
            chromatic: [aberration_center.x, aberration_center.y, unit(effects.swing), 0.0],
            inversion: [
                amount(drawn_inversion, inversion_amount),
                inversion_coverage[0],
                inversion_coverage[1],
                0.0,
            ],
            duotone: [
                amount(drawn_duotone, duotone_amount),
                duotone_coverage[0],
                duotone_coverage[1],
                ramp_start,
            ],
            duotone_dark: encoded(finite_color(duotone.dark)).extend(1.0 / softness).to_array(),
            duotone_light: encoded(finite_color(duotone.light)).extend(0.0).to_array(),
            lines: [
                amount(drawn_lines, lines_opacity),
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
            flags: [lines.seed, u32::from(character_mask), glitch.seed, 0],
            desaturation: [
                amount(drawn_desaturation, desaturation_amount),
                desaturation_coverage[0],
                desaturation_coverage[1],
                0.0,
            ],
            desaturation_tint: luminance_kept(finite_color(desaturation.tint)).extend(0.0).to_array(),
            halftone: [
                amount(drawn_halftone, halftone_amount),
                halftone_coverage[0],
                halftone_coverage[1],
                halftone_cell.max(1e-3),
            ],
            halftone_ink: encoded(finite_color(halftone.ink)).extend(0.0).to_array(),
            halftone_turn: [cosine, sine, 0.0, 0.0],
            flash: [amount(drawn_flash, flash_amount), flash_coverage[0], flash_coverage[1], 0.0],
            flash_color: encoded(finite_color(flash.color)).extend(0.0).to_array(),
            glitch: [
                amount(drawn_glitch, glitch_amount),
                glitch.bands.clamp(1, 512) as f32,
                glitch_share,
                glitch_shift,
            ],
            glitch_split: [glitch_split, 0.0, 0.0, 0.0],
            crt: [
                amount(drawn_crt, crt_amount),
                finite(crt.lines, 0.0).clamp(1.0, 4096.0),
                crt_depth,
                crt_grille,
            ],
            crt_corner: [crt_corner, 0.0, 0.0, 0.0],
        };
        (gpu, drawn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sapphire_look::post::{
        ChromaticAberration, Crt, Desaturation, Duotone, Flash, Glitch, Halftone, Inversion, RadialBlur,
        SpeedLines,
    };

    #[test]
    fn no_effects_draw_nothing_and_read_no_mask() {
        let (gpu, drawn) = GpuEffects::new(&PostEffects::NONE);
        assert_eq!(drawn, DrawnEffects::default());
        assert!(!drawn.any());
        assert_eq!(
            (gpu.lens[2], gpu.lens[3], gpu.inversion[0], gpu.duotone[0], gpu.lines[0]),
            (0.0, 0.0, 0.0, 0.0, 0.0)
        );
        assert_eq!(gpu.chromatic[2], 1.0, "and holds nothing");
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
            swing: f32::NAN,
            ..PostEffects::NONE
        };
        let (gpu, drawn) = GpuEffects::new(&effects);
        assert_eq!(drawn.count(), 5);
        assert!(drawn.character_mask, "the duotone keeps characters colored");
        assert_eq!(gpu.flags[1], 1);
        assert_eq!(gpu.lens, [0.5, 0.5, 1.0, 0.25], "a non-finite center is the middle");
        assert_eq!(&gpu.chromatic[..2], &[2.0, -1.0], "far off screen is far enough");
        assert_eq!(gpu.chromatic[2], 0.0, "a swing that isn't a number holds them all");
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

    /// The effects pack (M5 L9): each draws in the color variant, held to its working range, and only what has
    /// somewhere to show draws; a desaturation of the world alone reads the mask.
    #[test]
    fn the_effects_pack_clamps_and_says_which_draw() {
        let pack = PostEffects {
            desaturation: Desaturation {
                amount: 4.0,
                tint: Vec3::new(f32::NAN, 1.0, 1.0),
                coverage: EffectCoverage::WORLD,
            },
            halftone: Halftone { amount: 1.0, cell: 9.0, angle: f32::INFINITY, ..Halftone::NONE },
            flash: Flash { amount: -1.0, ..Flash::NONE },
            glitch: Glitch { amount: 1.0, bands: 0, share: 2.0, shift: f32::NAN, split: 1.0, seed: 41 },
            crt: Crt { amount: 0.5, lines: f32::INFINITY, ..Crt::NONE },
            ..PostEffects::NONE
        };
        let (gpu, drawn) = GpuEffects::new(&pack);
        assert_eq!(
            (drawn.desaturation, drawn.halftone, drawn.flash, drawn.glitch, drawn.crt),
            (true, true, false, true, true),
            "a flash under 0 is none"
        );
        assert!(drawn.color() && !drawn.lens() && drawn.character_mask);
        assert_eq!(gpu.desaturation, [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(gpu.desaturation_tint[0], 0.0, "a channel that isn't a number is black");
        assert_eq!((gpu.halftone[3], gpu.halftone_turn[..2].to_vec()), (0.25, vec![1.0, 0.0]));
        assert_eq!(gpu.flash[0], 0.0);
        assert_eq!(gpu.glitch, [1.0, 1.0, 1.0, 0.0], "one band at least, all of them, no shift");
        assert_eq!((gpu.glitch_split[0], gpu.flags[2]), (0.1, 41), "the split alone still glitches");
        assert_eq!((gpu.crt[0], gpu.crt[1]), (0.5, 1.0));
        let all = bytemuck::cast_slice::<GpuEffects, f32>(std::slice::from_ref(&gpu));
        let flags = std::mem::offset_of!(GpuEffects, flags) / 4;
        assert!(
            all.iter().enumerate().all(|(at, value)| (flags..flags + 4).contains(&at) || value.is_finite())
        );

        // Nothing to show draws nothing: no cell, a CRT of nothing, a glitch that moves nothing.
        let silent = PostEffects {
            halftone: Halftone { amount: 1.0, cell: 0.0, ..Halftone::NONE },
            glitch: Glitch { amount: 1.0, share: 0.0, split: 0.0, ..Glitch::NONE },
            crt: Crt { amount: 1.0, depth: 0.0, grille: 0.0, corner: 0.0, ..Crt::NONE },
            desaturation: Desaturation {
                amount: 1.0,
                coverage: EffectCoverage::CHARACTERS,
                ..Desaturation::NONE
            },
            ..PostEffects::NONE
        };
        let (_, drawn) = GpuEffects::new(&silent);
        assert_eq!(drawn.count(), 1, "only the characters' desaturation: {drawn:?}");
        assert!(drawn.character_mask);
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
