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
//! scene. To refresh it, take the medians of at least three recent `main` runs: each golden job
//! uploads `timings.json` with its captures (`gh run download <run> -p 'golden-*'`). The values
//! below are the medians of runs 40583188109, 40567331959, 40501247084 and 40498123764
//! (`main` at a359d78, 63f8380, 9f4fce5 and dd8ffa7, 95 scenes), read from the jobs' logs.

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
    Cost { scene: "turntable-start", lavapipe: 5.4, warp: 2.6 },
    Cost { scene: "turntable", lavapipe: 4.1, warp: 2.6 },
    Cost { scene: "turntable-msaa-off", lavapipe: 1.1, warp: 2.6 },
    Cost { scene: "turntable-lights", lavapipe: 0.7, warp: 2.5 },
    Cost { scene: "turntable-render-scale", lavapipe: 0.7, warp: 2.2 },
    Cost { scene: "turntable-skinned", lavapipe: 1.3, warp: 2.6 },
    Cost { scene: "turntable-sunset", lavapipe: 0.6, warp: 2.3 },
    Cost { scene: "turntable-swatches", lavapipe: 0.8, warp: 2.3 },
    Cost { scene: "turntable-night", lavapipe: 0.6, warp: 2.3 },
    Cost { scene: "faces", lavapipe: 7.3, warp: 6.7 },
    Cost { scene: "sky-day", lavapipe: 1.2, warp: 2.6 },
    Cost { scene: "sky-sunset", lavapipe: 1.4, warp: 2.4 },
    Cost { scene: "sky-night", lavapipe: 1.3, warp: 2.7 },
    Cost { scene: "turntable-taa", lavapipe: 3.4, warp: 10.6 },
    Cost { scene: "turntable-taa-motion", lavapipe: 2.0, warp: 9.9 },
    Cost { scene: "turntable-taa-upsampled", lavapipe: 1.6, warp: 7.7 },
    Cost { scene: "environment-day", lavapipe: 5.7, warp: 14.3 },
    Cost { scene: "environment-sunset", lavapipe: 6.7, warp: 16.5 },
    Cost { scene: "environment-night", lavapipe: 6.3, warp: 15.7 },
    Cost { scene: "post-day", lavapipe: 4.7, warp: 11.2 },
    Cost { scene: "post-sunset", lavapipe: 1.2, warp: 3.2 },
    Cost { scene: "post-flare", lavapipe: 1.1, warp: 4.0 },
    Cost { scene: "outlines-distance", lavapipe: 0.7, warp: 2.4 },
    Cost { scene: "outlines-telephoto", lavapipe: 0.7, warp: 2.4 },
    Cost { scene: "outlines-hard-edges", lavapipe: 0.5, warp: 2.3 },
    Cost { scene: "outlines-turning-taa", lavapipe: 1.7, warp: 11.5 },
    Cost { scene: "outlines-turning-taa-upsampled", lavapipe: 1.4, warp: 8.9 },
    Cost { scene: "shadows-contact", lavapipe: 4.8, warp: 14.8 },
    Cost { scene: "shadows-pillar", lavapipe: 3.9, warp: 16.2 },
    Cost { scene: "shadows-faces", lavapipe: 18.3, warp: 37.8 },
    Cost { scene: "shadows-courtyard", lavapipe: 6.6, warp: 21.7 },
    Cost { scene: "shadows-sunset", lavapipe: 7.7, warp: 19.0 },
    Cost { scene: "shadows-capsule", lavapipe: 1.9, warp: 5.6 },
    Cost { scene: "effects-blur-lines", lavapipe: 5.4, warp: 19.1 },
    Cost { scene: "effects-impact-frame", lavapipe: 4.6, warp: 20.8 },
    Cost { scene: "effects-chromatic-inversion", lavapipe: 3.1, warp: 7.3 },
    Cost { scene: "fog-day", lavapipe: 5.7, warp: 15.2 },
    Cost { scene: "fog-sunset", lavapipe: 6.5, warp: 15.2 },
    Cost { scene: "fog-night", lavapipe: 6.1, warp: 20.0 },
    Cost { scene: "fog-valley", lavapipe: 6.1, warp: 13.2 },
    Cost { scene: "fog-cards", lavapipe: 5.5, warp: 15.2 },
    Cost { scene: "fog-cards-mobile", lavapipe: 1.3, warp: 2.8 },
    Cost { scene: "fog-characters", lavapipe: 4.6, warp: 15.8 },
    Cost { scene: "ambient-day", lavapipe: 11.0, warp: 23.9 },
    Cost { scene: "ambient-sunset", lavapipe: 9.1, warp: 28.8 },
    Cost { scene: "ambient-night", lavapipe: 8.7, warp: 28.7 },
    Cost { scene: "ambient-gtao", lavapipe: 7.2, warp: 31.1 },
    Cost { scene: "ambient-capsule", lavapipe: 3.5, warp: 16.7 },
    Cost { scene: "ambient-mobile", lavapipe: 1.7, warp: 7.8 },
    Cost { scene: "rim-angles", lavapipe: 1.1, warp: 2.8 },
    Cost { scene: "rim-backlit", lavapipe: 0.8, warp: 2.8 },
    Cost { scene: "hair-highlights", lavapipe: 1.5, warp: 7.0 },
    Cost { scene: "hair-shadow", lavapipe: 5.1, warp: 6.2 },
    Cost { scene: "hair-shadow-mobile", lavapipe: 1.5, warp: 6.0 },
    Cost { scene: "eyes-through-hair", lavapipe: 1.5, warp: 6.5 },
    Cost { scene: "foliage-day", lavapipe: 60.7, warp: 80.9 },
    Cost { scene: "foliage-backlit", lavapipe: 13.8, warp: 29.2 },
    Cost { scene: "foliage-ground", lavapipe: 99.7, warp: 130.7 },
    Cost { scene: "foliage-mobile", lavapipe: 2.1, warp: 7.4 },
    Cost { scene: "foliage-wind", lavapipe: 61.2, warp: 71.3 },
    Cost { scene: "night-character", lavapipe: 7.6, warp: 25.9 },
    Cost { scene: "lookdev-town-day", lavapipe: 75.8, warp: 105.5 },
    Cost { scene: "lookdev-town-hero", lavapipe: 42.6, warp: 83.8 },
    Cost { scene: "lookdev-town-golden", lavapipe: 75.3, warp: 108.2 },
    Cost { scene: "lookdev-town-blue", lavapipe: 57.5, warp: 107.6 },
    Cost { scene: "lookdev-town-night", lavapipe: 60.9, warp: 91.4 },
    Cost { scene: "lookdev-street-night", lavapipe: 57.7, warp: 107.5 },
    Cost { scene: "gi-golden-hour", lavapipe: 1.9, warp: 6.2 },
    Cost { scene: "gi-clamped", lavapipe: 1.6, warp: 6.4 },
    Cost { scene: "eyes-front", lavapipe: 1.3, warp: 5.9 },
    Cost { scene: "eyes-three-quarter", lavapipe: 1.2, warp: 5.9 },
    Cost { scene: "eyes-gaze", lavapipe: 1.7, warp: 6.0 },
    Cost { scene: "eyes-highlight", lavapipe: 1.6, warp: 6.7 },
    Cost { scene: "eyes-lids", lavapipe: 1.4, warp: 5.9 },
    Cost { scene: "eyes-mobile", lavapipe: 1.2, warp: 5.8 },
    Cost { scene: "debug-wireframe", lavapipe: 43.5, warp: 79.4 },
    Cost { scene: "debug-overdraw", lavapipe: 22.0, warp: 32.5 },
    Cost { scene: "debug-lights", lavapipe: 8.8, warp: 20.4 },
    Cost { scene: "debug-lights-mobile", lavapipe: 1.5, warp: 3.1 },
    Cost { scene: "debug-material-ids", lavapipe: 1.1, warp: 2.5 },
    Cost { scene: "debug-ramp-rows", lavapipe: 0.9, warp: 2.6 },
    Cost { scene: "debug-outline-widths", lavapipe: 2.7, warp: 13.3 },
    Cost { scene: "materials-day", lavapipe: 1.8, warp: 3.7 },
    Cost { scene: "materials-night", lavapipe: 2.8, warp: 4.2 },
    Cost { scene: "fade-taa", lavapipe: 8.4, warp: 24.1 },
    Cost { scene: "fade-mobile", lavapipe: 3.3, warp: 7.9 },
    Cost { scene: "lookdev-face", lavapipe: 41.2, warp: 84.5 },
    Cost { scene: "two-sided", lavapipe: 1.7, warp: 2.9 },
    Cost { scene: "shade-golden-hour", lavapipe: 1.6, warp: 6.4 },
    Cost { scene: "shade-hue-golden-hour", lavapipe: 1.8, warp: 6.4 },
    Cost { scene: "rim-folded", lavapipe: 0.7, warp: 2.8 },
    Cost { scene: "rim-folded-camera", lavapipe: 0.7, warp: 2.6 },
    Cost { scene: "hair-shadow-soft", lavapipe: 1.3, warp: 6.2 },
    Cost { scene: "hair-backlit", lavapipe: 1.5, warp: 6.1 },
    Cost { scene: "hair-transmission", lavapipe: 1.4, warp: 6.3 },
];
