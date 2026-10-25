//! The sandbox's cvars: look-dev knobs on the scene's post-processing, tuned live from the
//! debug overlay (`dev` builds) until time of day (W1) drives these parameters from its keys.
//!
//! Each default is exactly the scene's look ([`PostParams::default`]), so a build without the
//! overlay, which never sets a cvar, renders the same frame as before them (and goldens hold).
//! A change to the saturation or the filter strength re-bakes the grading LUT, a few
//! milliseconds of CPU; the rest are per-frame uniforms.

use sapphire_core::cvar::Cvar;
use sapphire_render::{BloomParams, ColorGrade, PostParams, Vignette};

static EXPOSURE: Cvar<f32> =
    Cvar::with_range("post.exposure", 0.0, -6.0, 6.0, "Exposure in stops, before bloom and the tone curve.");
static BLOOM_INTENSITY: Cvar<f32> =
    Cvar::with_range("post.bloom.intensity", 0.25, 0.0, 2.0, "How much bloom is added; 0 skips bloom.");
static BLOOM_THRESHOLD: Cvar<f32> = Cvar::with_range(
    "post.bloom.threshold",
    1.0,
    0.0,
    4.0,
    "Brightness where bloom starts (lit white is 1).",
);
static SATURATION: Cvar<f32> = Cvar::with_range(
    "post.saturation",
    1.15,
    0.0,
    2.0,
    "The grade's saturation: 1 is unchanged (re-bakes the LUT).",
);
static FILTER_STRENGTH: Cvar<f32> = Cvar::with_range(
    "post.filter_strength",
    1.0,
    0.0,
    1.0,
    "The player's color-filter strength: 0 is the tone curve alone (re-bakes the LUT).",
);
static VIGNETTE: Cvar<f32> =
    Cvar::with_range("post.vignette", 0.18, 0.0, 1.0, "Darkening at the corners: 0 is none, 1 black.");

/// Every sandbox cvar, for the app's registry.
#[cfg(feature = "dev")]
pub(crate) static CVARS: [&dyn sapphire_core::cvar::AnyCvar; 6] =
    [&EXPOSURE, &BLOOM_INTENSITY, &BLOOM_THRESHOLD, &SATURATION, &FILTER_STRENGTH, &VIGNETTE];

/// The scene's post-processing, with the cvars applied.
pub(crate) fn post_params() -> PostParams {
    let look = PostParams::default();
    let saturation = SATURATION.get();
    PostParams {
        exposure: EXPOSURE.get(),
        bloom: BloomParams {
            intensity: BLOOM_INTENSITY.get(),
            threshold: BLOOM_THRESHOLD.get(),
            ..look.bloom
        },
        grade_a: ColorGrade { saturation, ..look.grade_a },
        grade_b: ColorGrade { saturation, ..look.grade_b },
        filter_strength: FILTER_STRENGTH.get(),
        vignette: Vignette { intensity: VIGNETTE.get(), ..look.vignette },
        ..look
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_scene_look() {
        // Goldens render without the overlay; the cvars' defaults must leave them unchanged.
        assert_eq!(post_params(), PostParams::default());
    }
}
