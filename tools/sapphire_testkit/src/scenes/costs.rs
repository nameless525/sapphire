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
//! Runners vary too much for one run: these three's lavapipe totals were 1441, 2201 and 2226 s. The
//! values below are the medians of runs 41509049520, 41512320437 and 41518819419 (410cf24 and two rounds
//! of the clocks' re-baseline, 142 scenes). Against those runs, a table from the other two left
//! lavapipe's slower shard 1.00 to 1.03 times its share and WARP's slowest 1.04 to 1.07 times, where the
//! table before took 1.02 to 1.05 and 1.13 to 1.20.

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
    Cost { scene: "turntable-start", lavapipe: 4.0, warp: 1.8 },
    Cost { scene: "turntable", lavapipe: 0.4, warp: 1.9 },
    Cost { scene: "turntable-msaa-off", lavapipe: 2.7, warp: 2.1 },
    Cost { scene: "turntable-lights", lavapipe: 0.5, warp: 2.1 },
    Cost { scene: "turntable-render-scale", lavapipe: 0.5, warp: 1.4 },
    Cost { scene: "turntable-skinned", lavapipe: 1.1, warp: 2.2 },
    Cost { scene: "turntable-sunset", lavapipe: 0.4, warp: 1.5 },
    Cost { scene: "turntable-swatches", lavapipe: 0.6, warp: 1.5 },
    Cost { scene: "turntable-night", lavapipe: 0.4, warp: 1.5 },
    Cost { scene: "faces", lavapipe: 2.6, warp: 4.5 },
    Cost { scene: "sky-day", lavapipe: 1.2, warp: 2.2 },
    Cost { scene: "sky-sunset", lavapipe: 1.2, warp: 1.9 },
    Cost { scene: "sky-night", lavapipe: 1.4, warp: 2.0 },
    Cost { scene: "turntable-taa", lavapipe: 1.6, warp: 7.8 },
    Cost { scene: "turntable-taa-motion", lavapipe: 1.2, warp: 7.3 },
    Cost { scene: "turntable-taa-upsampled", lavapipe: 1.8, warp: 5.6 },
    Cost { scene: "environment-day", lavapipe: 6.1, warp: 11.8 },
    Cost { scene: "environment-sunset", lavapipe: 5.5, warp: 11.5 },
    Cost { scene: "environment-night", lavapipe: 6.2, warp: 12.9 },
    Cost { scene: "post-day", lavapipe: 4.1, warp: 9.3 },
    Cost { scene: "post-sunset", lavapipe: 1.1, warp: 2.7 },
    Cost { scene: "post-flare", lavapipe: 1.3, warp: 2.6 },
    Cost { scene: "outlines-distance", lavapipe: 0.6, warp: 1.5 },
    Cost { scene: "outlines-telephoto", lavapipe: 0.5, warp: 1.5 },
    Cost { scene: "outlines-hard-edges", lavapipe: 0.4, warp: 1.6 },
    Cost { scene: "outlines-turning-taa", lavapipe: 1.3, warp: 7.3 },
    Cost { scene: "outlines-turning-taa-upsampled", lavapipe: 1.2, warp: 5.9 },
    Cost { scene: "shadows-contact", lavapipe: 3.0, warp: 11.5 },
    Cost { scene: "shadows-pillar", lavapipe: 2.8, warp: 11.4 },
    Cost { scene: "shadows-faces", lavapipe: 14.1, warp: 31.3 },
    Cost { scene: "shadows-courtyard", lavapipe: 7.0, warp: 17.2 },
    Cost { scene: "shadows-sunset", lavapipe: 6.5, warp: 17.4 },
    Cost { scene: "shadows-capsule", lavapipe: 1.3, warp: 3.3 },
    Cost { scene: "effects-blur-lines", lavapipe: 4.0, warp: 12.5 },
    Cost { scene: "effects-impact-frame", lavapipe: 3.5, warp: 12.0 },
    Cost { scene: "effects-chromatic-inversion", lavapipe: 1.7, warp: 4.4 },
    Cost { scene: "fog-day", lavapipe: 5.9, warp: 12.4 },
    Cost { scene: "fog-sunset", lavapipe: 5.5, warp: 12.5 },
    Cost { scene: "fog-night", lavapipe: 6.9, warp: 13.6 },
    Cost { scene: "fog-valley", lavapipe: 5.5, warp: 12.2 },
    Cost { scene: "fog-cards", lavapipe: 5.8, warp: 12.4 },
    Cost { scene: "fog-cards-mobile", lavapipe: 1.0, warp: 2.0 },
    Cost { scene: "fog-characters", lavapipe: 4.8, warp: 11.5 },
    Cost { scene: "ambient-day", lavapipe: 9.4, warp: 23.4 },
    Cost { scene: "ambient-sunset", lavapipe: 8.2, warp: 23.9 },
    Cost { scene: "ambient-night", lavapipe: 7.3, warp: 23.6 },
    Cost { scene: "ambient-gtao", lavapipe: 7.5, warp: 24.8 },
    Cost { scene: "ambient-capsule", lavapipe: 2.4, warp: 13.5 },
    Cost { scene: "ambient-mobile", lavapipe: 1.6, warp: 5.3 },
    Cost { scene: "rim-angles", lavapipe: 0.7, warp: 1.9 },
    Cost { scene: "rim-backlit", lavapipe: 0.8, warp: 1.9 },
    Cost { scene: "hair-highlights", lavapipe: 1.2, warp: 3.9 },
    Cost { scene: "hair-shadow", lavapipe: 1.2, warp: 4.1 },
    Cost { scene: "hair-shadow-mobile", lavapipe: 2.7, warp: 4.1 },
    Cost { scene: "eyes-through-hair", lavapipe: 1.3, warp: 4.2 },
    Cost { scene: "foliage-day", lavapipe: 59.0, warp: 71.6 },
    Cost { scene: "foliage-backlit", lavapipe: 13.8, warp: 23.7 },
    Cost { scene: "foliage-ground", lavapipe: 93.0, warp: 124.2 },
    Cost { scene: "foliage-mobile", lavapipe: 2.1, warp: 4.4 },
    Cost { scene: "foliage-wind", lavapipe: 57.1, warp: 74.1 },
    Cost { scene: "night-character", lavapipe: 7.2, warp: 18.8 },
    Cost { scene: "lookdev-town-day", lavapipe: 65.1, warp: 95.0 },
    Cost { scene: "lookdev-town-hero", lavapipe: 37.7, warp: 73.6 },
    Cost { scene: "lookdev-town-golden", lavapipe: 68.8, warp: 109.9 },
    Cost { scene: "lookdev-town-blue", lavapipe: 69.3, warp: 94.6 },
    Cost { scene: "lookdev-town-night", lavapipe: 59.0, warp: 81.7 },
    Cost { scene: "lookdev-street-night", lavapipe: 63.4, warp: 98.6 },
    Cost { scene: "gi-golden-hour", lavapipe: 1.6, warp: 4.5 },
    Cost { scene: "gi-clamped", lavapipe: 1.6, warp: 4.3 },
    Cost { scene: "eyes-front", lavapipe: 1.0, warp: 3.7 },
    Cost { scene: "eyes-three-quarter", lavapipe: 1.1, warp: 3.9 },
    Cost { scene: "eyes-gaze", lavapipe: 1.4, warp: 4.2 },
    Cost { scene: "eyes-highlight", lavapipe: 1.1, warp: 4.2 },
    Cost { scene: "eyes-lids", lavapipe: 1.1, warp: 3.8 },
    Cost { scene: "eyes-mobile", lavapipe: 1.0, warp: 3.8 },
    Cost { scene: "debug-wireframe", lavapipe: 39.3, warp: 101.1 },
    Cost { scene: "debug-overdraw", lavapipe: 24.3, warp: 31.2 },
    Cost { scene: "debug-lights", lavapipe: 7.9, warp: 21.3 },
    Cost { scene: "debug-lights-mobile", lavapipe: 1.1, warp: 2.2 },
    Cost { scene: "debug-material-ids", lavapipe: 0.7, warp: 2.0 },
    Cost { scene: "debug-ramp-rows", lavapipe: 0.7, warp: 1.7 },
    Cost { scene: "debug-outline-widths", lavapipe: 2.2, warp: 12.2 },
    Cost { scene: "materials-day", lavapipe: 1.4, warp: 2.8 },
    Cost { scene: "materials-night", lavapipe: 1.7, warp: 3.3 },
    Cost { scene: "fade-taa", lavapipe: 8.9, warp: 19.1 },
    Cost { scene: "fade-mobile", lavapipe: 1.5, warp: 5.3 },
    Cost { scene: "lookdev-face", lavapipe: 44.6, warp: 74.9 },
    Cost { scene: "two-sided", lavapipe: 1.1, warp: 2.2 },
    Cost { scene: "shade-golden-hour", lavapipe: 1.5, warp: 4.6 },
    Cost { scene: "shade-hue-golden-hour", lavapipe: 1.3, warp: 4.5 },
    Cost { scene: "rim-folded", lavapipe: 0.5, warp: 1.9 },
    Cost { scene: "rim-folded-camera", lavapipe: 0.5, warp: 1.9 },
    Cost { scene: "hair-shadow-soft", lavapipe: 1.2, warp: 4.1 },
    Cost { scene: "hair-backlit", lavapipe: 1.2, warp: 4.3 },
    Cost { scene: "hair-transmission", lavapipe: 1.1, warp: 4.1 },
    Cost { scene: "arena-day", lavapipe: 52.1, warp: 77.5 },
    Cost { scene: "arena-fight-cut", lavapipe: 48.6, warp: 80.2 },
    Cost { scene: "arena-fight-hit", lavapipe: 50.5, warp: 78.6 },
    Cost { scene: "arena-fight-telegraph", lavapipe: 46.5, warp: 77.4 },
    Cost { scene: "arena-golden", lavapipe: 58.0, warp: 93.6 },
    Cost { scene: "arena-party-second", lavapipe: 50.1, warp: 77.0 },
    Cost { scene: "arena-party-third", lavapipe: 48.8, warp: 80.5 },
    Cost { scene: "arena-replay", lavapipe: 37.6, warp: 62.3 },
    Cost { scene: "debugdraw-shapes", lavapipe: 0.5, warp: 1.5 },
    Cost { scene: "debugdraw-volumes", lavapipe: 0.6, warp: 1.5 },
    Cost { scene: "hit-break-noon", lavapipe: 19.2, warp: 38.2 },
    Cost { scene: "hit-finish-dusk", lavapipe: 26.6, warp: 55.7 },
    Cost { scene: "hit-medium-noon", lavapipe: 21.0, warp: 38.8 },
    Cost { scene: "hud-notch", lavapipe: 5.7, warp: 14.4 },
    Cost { scene: "hud-tablet", lavapipe: 6.9, warp: 12.3 },
    Cost { scene: "hud-ultrawide", lavapipe: 5.0, warp: 13.6 },
    Cost { scene: "hud-wide", lavapipe: 7.0, warp: 14.9 },
    Cost { scene: "lookdev-clip-hit", lavapipe: 19.4, warp: 36.3 },
    Cost { scene: "lookdev-clip-idle", lavapipe: 19.4, warp: 37.6 },
    Cost { scene: "lookdev-clip-run", lavapipe: 18.3, warp: 35.9 },
    Cost { scene: "lookdev-clip-run-springs", lavapipe: 18.3, warp: 38.6 },
    Cost { scene: "lookdev-death-back", lavapipe: 9.4, warp: 29.1 },
    Cost { scene: "lookdev-enemy", lavapipe: 57.9, warp: 107.8 },
    Cost { scene: "lookdev-enemy-night", lavapipe: 71.5, warp: 119.0 },
    Cost { scene: "lookdev-graph-cancel", lavapipe: 19.0, warp: 37.3 },
    Cost { scene: "lookdev-graph-run", lavapipe: 36.3, warp: 61.4 },
    Cost { scene: "lookdev-ik-slope", lavapipe: 16.5, warp: 34.0 },
    Cost { scene: "lookdev-ik-step", lavapipe: 17.9, warp: 34.7 },
    Cost { scene: "lookdev-move-coil", lavapipe: 16.3, warp: 33.0 },
    Cost { scene: "lookdev-move-cut", lavapipe: 12.1, warp: 26.9 },
    Cost { scene: "lookdev-move-dash", lavapipe: 9.2, warp: 23.8 },
    Cost { scene: "lookdev-move-dodge", lavapipe: 9.1, warp: 21.8 },
    Cost { scene: "lookdev-move-landing", lavapipe: 24.6, warp: 46.6 },
    Cost { scene: "lookdev-move-windup", lavapipe: 20.5, warp: 51.6 },
    Cost { scene: "lookdev-strike-anticipation", lavapipe: 20.5, warp: 40.1 },
    Cost { scene: "lookdev-strike-face", lavapipe: 23.7, warp: 42.6 },
    Cost { scene: "lookdev-strike-fast", lavapipe: 20.1, warp: 42.0 },
    Cost { scene: "lookdev-strike-mark", lavapipe: 21.5, warp: 41.9 },
    Cost { scene: "lookdev-strike-settle", lavapipe: 20.1, warp: 41.6 },
    Cost { scene: "lookdev-strike-springs-after", lavapipe: 20.1, warp: 41.5 },
    Cost { scene: "lookdev-strike-springs-mark", lavapipe: 21.4, warp: 42.0 },
    Cost { scene: "vfx-arc-night", lavapipe: 26.1, warp: 48.1 },
    Cost { scene: "vfx-arc-noon", lavapipe: 20.4, warp: 37.5 },
    Cost { scene: "vfx-duel-mobile-low", lavapipe: 25.0, warp: 36.6 },
    Cost { scene: "vfx-duel-noon", lavapipe: 18.8, warp: 39.5 },
    Cost { scene: "vfx-hit-sunset", lavapipe: 21.5, warp: 46.1 },
    Cost { scene: "vfx-plain", lavapipe: 0.5, warp: 1.8 },
];
