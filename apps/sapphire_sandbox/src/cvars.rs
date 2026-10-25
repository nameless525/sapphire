//! The sandbox's cvars, tuned live from the debug overlay (`dev` builds).
//!
//! Post-processing's exposure, bloom, grade and vignette come from time of day's keys (M1 W1),
//! which replaced the look-dev cvars that stood in for them; the overlay's time-of-day panel
//! scrubs them. What remains is the player's setting on top of time of day.
//!
//! Each default is exactly the scene's look, so a build without the overlay, which never sets a
//! cvar, renders the same frame as before them (and goldens hold). A change to the filter
//! strength re-bakes the grading LUT, a few milliseconds of CPU.

use sapphire_core::cvar::Cvar;

static FILTER_STRENGTH: Cvar<f32> = Cvar::with_range(
    "post.filter_strength",
    1.0,
    0.0,
    1.0,
    "The player's color-filter strength: 0 is the tone curve alone (re-bakes the LUT).",
);

/// Every sandbox cvar, for the app's registry.
#[cfg(feature = "dev")]
pub(crate) static CVARS: [&dyn sapphire_core::cvar::AnyCvar; 1] = [&FILTER_STRENGTH];

/// The player's color-filter strength, 0 to 1 (`PostParams::filter_strength`), until a settings
/// screen (M3).
pub(crate) fn filter_strength() -> f32 {
    FILTER_STRENGTH.get()
}

#[cfg(test)]
mod tests {
    use sapphire_render::PostParams;

    use super::*;

    #[test]
    fn defaults_are_the_scene_look() {
        // Goldens render without the overlay; the cvars' defaults must leave them unchanged.
        assert_eq!(filter_strength(), PostParams::default().filter_strength);
    }
}
