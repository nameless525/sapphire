//! What each golden scene costs on the CPU rasterizers CI shards, so `--shard` can split a
//! family's scenes into slices that take about as long (review BC-107).
//!
//! A cost is a scene's median wall time in seconds on CI's `main` runs: the sandbox's start, its
//! capture and the comparison, as the runner's `ok` lines time them. Each scene is a fresh process
//! that compiles its own pipelines, so on lavapipe and WARP the costs range from under a second
//! to two minutes, and dealing the scenes out in turn left one slice minutes longer than the
//! others.
//!
//! The table is keyed by scene name. A scene missing from it counts as the family's costliest, so
//! a new scene never lands on an already full slice; a test fails on a name that is no longer a
//! scene. To refresh it, take the medians of at least three recent CI runs on the current code,
//! on `main` or on a PR that is about to land: each golden job uploads `timings.json` with its
//! captures (`gh run download <run> -p 'golden-dx12-warp-*' -p 'golden-vulkan-lavapipe-*'`).
//! Runners vary too much for one run: these five's lavapipe totals were 1558 to 2214 s and WARP's 3169
//! to 3845 s. The values below are the medians of runs 41546183771, 41556276807, 41562253351, 41566294022
//! and 41579437613 (7ee1583, 106ee2f and c74d964, 142 scenes). Against those runs, a table from the other
//! four left lavapipe's slower shard 1.00 to 1.03 times its share and WARP's slowest 1.03 to 1.07 times,
//! where the table before took 1.01 to 1.19 and 1.02 to 1.24.

/// A backend family CI splits across runners, and so has costs here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    /// `vulkan-lavapipe`: CI's Linux runners, Mesa 26.1.3.
    Lavapipe,
    /// `dx12-warp`: CI's Windows runners, WARP 1.0.21.
    Warp,
}

impl Family {
    /// The family a run will render with, from the backend it forces (`SAPPHIRE_GPU_BACKEND`, as
    /// CI's golden jobs set it): CI's only Vulkan is lavapipe and its only D3D12 is WARP. `None`
    /// for anything else, whose scenes all cost the same, which deals them out in turn.
    pub(crate) fn from_backend(backend: &str) -> Option<Self> {
        match backend.trim().to_ascii_lowercase().as_str() {
            "vulkan" => Some(Self::Lavapipe),
            "dx12" => Some(Self::Warp),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Lavapipe => "vulkan-lavapipe",
            Self::Warp => "dx12-warp",
        }
    }

    /// `scene`'s cost in tenths of a second: its measured median, or the costliest scene's when
    /// it has none yet.
    pub(crate) fn cost(self, scene: &str) -> u32 {
        let seconds = |cost: &Cost| match self {
            Self::Lavapipe => cost.lavapipe,
            Self::Warp => cost.warp,
        };
        let seconds = match COSTS.iter().find(|cost| cost.scene == scene) {
            Some(cost) => seconds(cost),
            None => COSTS.iter().map(seconds).fold(0.0, f32::max),
        };
        // Tenths: whole numbers keep the split exact and the same on every host.
        (seconds * 10.0).round() as u32
    }
}

/// One scene's median seconds per family.
pub(crate) struct Cost {
    pub scene: &'static str,
    pub lavapipe: f32,
    pub warp: f32,
}

/// In the order of `scenes()` when last refreshed.
pub(crate) const COSTS: &[Cost] = &[
    Cost { scene: "turntable-start", lavapipe: 5.0, warp: 1.9 },
    Cost { scene: "turntable", lavapipe: 4.9, warp: 1.4 },
    Cost { scene: "turntable-msaa-off", lavapipe: 0.8, warp: 1.5 },
    Cost { scene: "turntable-lights", lavapipe: 0.5, warp: 1.7 },
    Cost { scene: "turntable-render-scale", lavapipe: 0.5, warp: 1.4 },
    Cost { scene: "turntable-skinned", lavapipe: 0.6, warp: 1.6 },
    Cost { scene: "turntable-sunset", lavapipe: 0.4, warp: 1.7 },
    Cost { scene: "turntable-swatches", lavapipe: 0.5, warp: 2.1 },
    Cost { scene: "turntable-night", lavapipe: 0.4, warp: 1.5 },
    Cost { scene: "faces", lavapipe: 2.6, warp: 4.5 },
    Cost { scene: "sky-day", lavapipe: 1.2, warp: 2.1 },
    Cost { scene: "sky-sunset", lavapipe: 0.9, warp: 2.2 },
    Cost { scene: "sky-night", lavapipe: 1.3, warp: 2.0 },
    Cost { scene: "turntable-taa", lavapipe: 1.5, warp: 6.9 },
    Cost { scene: "turntable-taa-motion", lavapipe: 0.9, warp: 6.8 },
    Cost { scene: "turntable-taa-upsampled", lavapipe: 1.0, warp: 4.8 },
    Cost { scene: "environment-day", lavapipe: 4.8, warp: 11.4 },
    Cost { scene: "environment-sunset", lavapipe: 4.4, warp: 11.8 },
    Cost { scene: "environment-night", lavapipe: 6.6, warp: 12.9 },
    Cost { scene: "post-day", lavapipe: 3.7, warp: 7.8 },
    Cost { scene: "post-sunset", lavapipe: 1.1, warp: 2.2 },
    Cost { scene: "post-flare", lavapipe: 1.1, warp: 2.3 },
    Cost { scene: "outlines-distance", lavapipe: 0.5, warp: 1.5 },
    Cost { scene: "outlines-telephoto", lavapipe: 0.5, warp: 1.5 },
    Cost { scene: "outlines-hard-edges", lavapipe: 0.4, warp: 1.4 },
    Cost { scene: "outlines-turning-taa", lavapipe: 1.1, warp: 6.0 },
    Cost { scene: "outlines-turning-taa-upsampled", lavapipe: 1.1, warp: 6.0 },
    Cost { scene: "shadows-contact", lavapipe: 2.6, warp: 10.5 },
    Cost { scene: "shadows-pillar", lavapipe: 2.3, warp: 11.5 },
    Cost { scene: "shadows-faces", lavapipe: 11.3, warp: 31.5 },
    Cost { scene: "shadows-courtyard", lavapipe: 5.4, warp: 13.1 },
    Cost { scene: "shadows-sunset", lavapipe: 5.5, warp: 16.7 },
    Cost { scene: "shadows-capsule", lavapipe: 1.0, warp: 3.2 },
    Cost { scene: "effects-blur-lines", lavapipe: 3.4, warp: 10.2 },
    Cost { scene: "effects-impact-frame", lavapipe: 3.8, warp: 11.9 },
    Cost { scene: "effects-chromatic-inversion", lavapipe: 1.7, warp: 4.3 },
    Cost { scene: "fog-day", lavapipe: 5.2, warp: 12.3 },
    Cost { scene: "fog-sunset", lavapipe: 5.2, warp: 12.3 },
    Cost { scene: "fog-night", lavapipe: 5.4, warp: 12.0 },
    Cost { scene: "fog-valley", lavapipe: 4.3, warp: 12.1 },
    Cost { scene: "fog-cards", lavapipe: 4.5, warp: 12.4 },
    Cost { scene: "fog-cards-mobile", lavapipe: 1.0, warp: 2.1 },
    Cost { scene: "fog-characters", lavapipe: 3.7, warp: 11.2 },
    Cost { scene: "ambient-day", lavapipe: 6.8, warp: 17.1 },
    Cost { scene: "ambient-sunset", lavapipe: 7.9, warp: 23.7 },
    Cost { scene: "ambient-night", lavapipe: 6.0, warp: 23.7 },
    Cost { scene: "ambient-gtao", lavapipe: 6.8, warp: 18.3 },
    Cost { scene: "ambient-capsule", lavapipe: 2.0, warp: 13.6 },
    Cost { scene: "ambient-mobile", lavapipe: 1.3, warp: 5.5 },
    Cost { scene: "rim-angles", lavapipe: 0.6, warp: 1.9 },
    Cost { scene: "rim-backlit", lavapipe: 0.7, warp: 1.9 },
    Cost { scene: "hair-highlights", lavapipe: 2.2, warp: 4.0 },
    Cost { scene: "hair-shadow", lavapipe: 1.1, warp: 4.1 },
    Cost { scene: "hair-shadow-mobile", lavapipe: 1.0, warp: 4.2 },
    Cost { scene: "eyes-through-hair", lavapipe: 1.3, warp: 4.4 },
    Cost { scene: "foliage-day", lavapipe: 54.2, warp: 71.7 },
    Cost { scene: "foliage-backlit", lavapipe: 11.4, warp: 23.4 },
    Cost { scene: "foliage-ground", lavapipe: 75.3, warp: 125.6 },
    Cost { scene: "foliage-mobile", lavapipe: 1.5, warp: 4.1 },
    Cost { scene: "foliage-wind", lavapipe: 55.6, warp: 75.0 },
    Cost { scene: "night-character", lavapipe: 7.0, warp: 19.0 },
    Cost { scene: "lookdev-town-day", lavapipe: 63.9, warp: 96.9 },
    Cost { scene: "lookdev-town-hero", lavapipe: 33.6, warp: 61.1 },
    Cost { scene: "lookdev-town-golden", lavapipe: 56.5, warp: 88.6 },
    Cost { scene: "lookdev-town-blue", lavapipe: 58.3, warp: 97.5 },
    Cost { scene: "lookdev-town-night", lavapipe: 45.6, warp: 67.7 },
    Cost { scene: "lookdev-street-night", lavapipe: 48.9, warp: 99.5 },
    Cost { scene: "gi-golden-hour", lavapipe: 1.4, warp: 4.7 },
    Cost { scene: "gi-clamped", lavapipe: 1.2, warp: 4.4 },
    Cost { scene: "eyes-front", lavapipe: 1.0, warp: 3.9 },
    Cost { scene: "eyes-three-quarter", lavapipe: 1.0, warp: 3.6 },
    Cost { scene: "eyes-gaze", lavapipe: 1.1, warp: 4.5 },
    Cost { scene: "eyes-highlight", lavapipe: 0.9, warp: 4.4 },
    Cost { scene: "eyes-lids", lavapipe: 1.0, warp: 3.8 },
    Cost { scene: "eyes-mobile", lavapipe: 0.9, warp: 3.9 },
    Cost { scene: "debug-wireframe", lavapipe: 38.1, warp: 100.2 },
    Cost { scene: "debug-overdraw", lavapipe: 21.4, warp: 31.7 },
    Cost { scene: "debug-lights", lavapipe: 6.6, warp: 16.3 },
    Cost { scene: "debug-lights-mobile", lavapipe: 1.1, warp: 2.4 },
    Cost { scene: "debug-material-ids", lavapipe: 0.8, warp: 1.8 },
    Cost { scene: "debug-ramp-rows", lavapipe: 0.9, warp: 1.7 },
    Cost { scene: "debug-outline-widths", lavapipe: 2.1, warp: 9.7 },
    Cost { scene: "materials-day", lavapipe: 1.3, warp: 2.8 },
    Cost { scene: "materials-night", lavapipe: 1.4, warp: 3.6 },
    Cost { scene: "fade-taa", lavapipe: 6.3, warp: 19.5 },
    Cost { scene: "fade-mobile", lavapipe: 1.7, warp: 5.2 },
    Cost { scene: "lookdev-face", lavapipe: 34.3, warp: 73.0 },
    Cost { scene: "two-sided", lavapipe: 1.0, warp: 2.3 },
    Cost { scene: "shade-golden-hour", lavapipe: 1.3, warp: 4.6 },
    Cost { scene: "shade-hue-golden-hour", lavapipe: 1.2, warp: 5.0 },
    Cost { scene: "rim-folded", lavapipe: 0.5, warp: 1.9 },
    Cost { scene: "rim-folded-camera", lavapipe: 0.5, warp: 2.1 },
    Cost { scene: "hair-shadow-soft", lavapipe: 0.9, warp: 4.1 },
    Cost { scene: "hair-backlit", lavapipe: 1.1, warp: 4.2 },
    Cost { scene: "hair-transmission", lavapipe: 1.0, warp: 4.5 },
    Cost { scene: "arena-day", lavapipe: 39.5, warp: 78.3 },
    Cost { scene: "arena-fight-cut", lavapipe: 39.4, warp: 82.0 },
    Cost { scene: "arena-fight-hit", lavapipe: 45.8, warp: 65.1 },
    Cost { scene: "arena-fight-telegraph", lavapipe: 44.8, warp: 80.9 },
    Cost { scene: "arena-golden", lavapipe: 55.8, warp: 92.2 },
    Cost { scene: "arena-party-second", lavapipe: 38.6, warp: 76.7 },
    Cost { scene: "arena-party-third", lavapipe: 47.0, warp: 78.9 },
    Cost { scene: "arena-replay", lavapipe: 34.2, warp: 64.2 },
    Cost { scene: "debugdraw-shapes", lavapipe: 0.5, warp: 1.4 },
    Cost { scene: "debugdraw-volumes", lavapipe: 0.6, warp: 1.5 },
    Cost { scene: "hit-break-noon", lavapipe: 18.5, warp: 38.8 },
    Cost { scene: "hit-finish-dusk", lavapipe: 24.1, warp: 50.3 },
    Cost { scene: "hit-medium-noon", lavapipe: 16.2, warp: 34.1 },
    Cost { scene: "hud-notch", lavapipe: 4.8, warp: 13.8 },
    Cost { scene: "hud-tablet", lavapipe: 3.5, warp: 12.7 },
    Cost { scene: "hud-ultrawide", lavapipe: 4.7, warp: 13.5 },
    Cost { scene: "hud-wide", lavapipe: 6.7, warp: 13.3 },
    Cost { scene: "lookdev-clip-hit", lavapipe: 15.0, warp: 35.8 },
    Cost { scene: "lookdev-clip-idle", lavapipe: 17.6, warp: 36.3 },
    Cost { scene: "lookdev-clip-run", lavapipe: 15.1, warp: 36.2 },
    Cost { scene: "lookdev-clip-run-springs", lavapipe: 17.7, warp: 36.1 },
    Cost { scene: "lookdev-death-back", lavapipe: 9.1, warp: 28.7 },
    Cost { scene: "lookdev-enemy", lavapipe: 44.7, warp: 88.0 },
    Cost { scene: "lookdev-enemy-night", lavapipe: 69.4, warp: 118.2 },
    Cost { scene: "lookdev-graph-cancel", lavapipe: 15.9, warp: 38.2 },
    Cost { scene: "lookdev-graph-run", lavapipe: 28.1, warp: 60.1 },
    Cost { scene: "lookdev-ik-slope", lavapipe: 16.0, warp: 34.2 },
    Cost { scene: "lookdev-ik-step", lavapipe: 13.8, warp: 34.9 },
    Cost { scene: "lookdev-move-coil", lavapipe: 13.5, warp: 29.2 },
    Cost { scene: "lookdev-move-cut", lavapipe: 10.7, warp: 26.8 },
    Cost { scene: "lookdev-move-dash", lavapipe: 7.7, warp: 23.9 },
    Cost { scene: "lookdev-move-dodge", lavapipe: 8.0, warp: 22.0 },
    Cost { scene: "lookdev-move-landing", lavapipe: 19.0, warp: 45.5 },
    Cost { scene: "lookdev-move-windup", lavapipe: 16.8, warp: 40.6 },
    Cost { scene: "lookdev-strike-anticipation", lavapipe: 18.4, warp: 40.8 },
    Cost { scene: "lookdev-strike-face", lavapipe: 18.1, warp: 43.3 },
    Cost { scene: "lookdev-strike-fast", lavapipe: 16.6, warp: 42.1 },
    Cost { scene: "lookdev-strike-mark", lavapipe: 19.4, warp: 42.1 },
    Cost { scene: "lookdev-strike-settle", lavapipe: 19.4, warp: 41.9 },
    Cost { scene: "lookdev-strike-springs-after", lavapipe: 16.6, warp: 41.7 },
    Cost { scene: "lookdev-strike-springs-mark", lavapipe: 19.4, warp: 37.6 },
    Cost { scene: "vfx-arc-night", lavapipe: 21.8, warp: 48.5 },
    Cost { scene: "vfx-arc-noon", lavapipe: 18.4, warp: 48.5 },
    Cost { scene: "vfx-duel-mobile-low", lavapipe: 19.3, warp: 32.1 },
    Cost { scene: "vfx-duel-noon", lavapipe: 18.5, warp: 38.7 },
    Cost { scene: "vfx-hit-sunset", lavapipe: 16.6, warp: 42.1 },
    Cost { scene: "vfx-plain", lavapipe: 0.5, warp: 1.8 },
];
