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
//! Runners vary too much for one run: these three's lavapipe totals were 705, 759 and 1003 s. The
//! values below are the medians of runs 41397708420, 41399037465 and 41398694806 (c17cd35 on its PR and on `main`,
//! and 6e39c8c on its PR, 95 scenes). Against those runs, a table from the other two left
//! lavapipe's slower shard 1.01 to 1.08 times its share and WARP's slowest 1.10 to 1.13 times, where
//! the table before took 1.00 to 1.29 and 1.18 to 1.23.

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
    Cost { scene: "turntable-start", lavapipe: 3.2, warp: 1.7 },
    Cost { scene: "turntable", lavapipe: 6.0, warp: 1.7 },
    Cost { scene: "turntable-msaa-off", lavapipe: 0.4, warp: 1.7 },
    Cost { scene: "turntable-lights", lavapipe: 0.6, warp: 1.5 },
    Cost { scene: "turntable-render-scale", lavapipe: 0.4, warp: 1.4 },
    Cost { scene: "turntable-skinned", lavapipe: 0.5, warp: 2.1 },
    Cost { scene: "turntable-sunset", lavapipe: 0.4, warp: 1.5 },
    Cost { scene: "turntable-swatches", lavapipe: 0.6, warp: 1.5 },
    Cost { scene: "turntable-night", lavapipe: 0.3, warp: 1.5 },
    Cost { scene: "faces", lavapipe: 7.2, warp: 4.5 },
    Cost { scene: "sky-day", lavapipe: 1.2, warp: 1.5 },
    Cost { scene: "sky-sunset", lavapipe: 0.8, warp: 1.9 },
    Cost { scene: "sky-night", lavapipe: 1.3, warp: 1.9 },
    Cost { scene: "turntable-taa", lavapipe: 0.9, warp: 6.8 },
    Cost { scene: "turntable-taa-motion", lavapipe: 2.9, warp: 5.5 },
    Cost { scene: "turntable-taa-upsampled", lavapipe: 0.7, warp: 4.5 },
    Cost { scene: "environment-day", lavapipe: 6.0, warp: 8.9 },
    Cost { scene: "environment-sunset", lavapipe: 5.5, warp: 11.6 },
    Cost { scene: "environment-night", lavapipe: 4.1, warp: 11.3 },
    Cost { scene: "post-day", lavapipe: 2.4, warp: 7.4 },
    Cost { scene: "post-sunset", lavapipe: 1.1, warp: 2.1 },
    Cost { scene: "post-flare", lavapipe: 0.6, warp: 2.4 },
    Cost { scene: "outlines-distance", lavapipe: 0.5, warp: 1.4 },
    Cost { scene: "outlines-telephoto", lavapipe: 0.3, warp: 1.2 },
    Cost { scene: "outlines-hard-edges", lavapipe: 0.4, warp: 1.4 },
    Cost { scene: "outlines-turning-taa", lavapipe: 0.7, warp: 7.8 },
    Cost { scene: "outlines-turning-taa-upsampled", lavapipe: 1.2, warp: 5.2 },
    Cost { scene: "shadows-contact", lavapipe: 4.0, warp: 11.5 },
    Cost { scene: "shadows-pillar", lavapipe: 1.8, warp: 8.3 },
    Cost { scene: "shadows-faces", lavapipe: 7.7, warp: 31.8 },
    Cost { scene: "shadows-courtyard", lavapipe: 4.2, warp: 17.0 },
    Cost { scene: "shadows-sunset", lavapipe: 6.9, warp: 17.3 },
    Cost { scene: "shadows-capsule", lavapipe: 0.8, warp: 3.2 },
    Cost { scene: "effects-blur-lines", lavapipe: 4.3, warp: 11.6 },
    Cost { scene: "effects-impact-frame", lavapipe: 3.7, warp: 11.9 },
    Cost { scene: "effects-chromatic-inversion", lavapipe: 0.8, warp: 4.1 },
    Cost { scene: "fog-day", lavapipe: 3.3, warp: 12.5 },
    Cost { scene: "fog-sunset", lavapipe: 5.8, warp: 12.8 },
    Cost { scene: "fog-night", lavapipe: 6.7, warp: 10.3 },
    Cost { scene: "fog-valley", lavapipe: 3.1, warp: 12.0 },
    Cost { scene: "fog-cards", lavapipe: 5.6, warp: 8.9 },
    Cost { scene: "fog-cards-mobile", lavapipe: 0.7, warp: 2.0 },
    Cost { scene: "fog-characters", lavapipe: 2.7, warp: 8.7 },
    Cost { scene: "ambient-day", lavapipe: 5.1, warp: 23.4 },
    Cost { scene: "ambient-sunset", lavapipe: 10.3, warp: 14.8 },
    Cost { scene: "ambient-night", lavapipe: 7.6, warp: 17.2 },
    Cost { scene: "ambient-gtao", lavapipe: 4.4, warp: 24.7 },
    Cost { scene: "ambient-capsule", lavapipe: 2.8, warp: 13.5 },
    Cost { scene: "ambient-mobile", lavapipe: 3.1, warp: 5.3 },
    Cost { scene: "rim-angles", lavapipe: 0.8, warp: 1.9 },
    Cost { scene: "rim-backlit", lavapipe: 0.4, warp: 1.9 },
    Cost { scene: "hair-highlights", lavapipe: 1.4, warp: 4.1 },
    Cost { scene: "hair-shadow", lavapipe: 0.7, warp: 4.0 },
    Cost { scene: "hair-shadow-mobile", lavapipe: 0.7, warp: 4.1 },
    Cost { scene: "eyes-through-hair", lavapipe: 1.4, warp: 3.4 },
    Cost { scene: "foliage-day", lavapipe: 33.0, warp: 58.4 },
    Cost { scene: "foliage-backlit", lavapipe: 15.2, warp: 23.5 },
    Cost { scene: "foliage-ground", lavapipe: 54.7, warp: 116.1 },
    Cost { scene: "foliage-mobile", lavapipe: 2.6, warp: 4.5 },
    Cost { scene: "foliage-wind", lavapipe: 33.4, warp: 74.0 },
    Cost { scene: "night-character", lavapipe: 4.5, warp: 18.6 },
    Cost { scene: "lookdev-town-day", lavapipe: 75.7, warp: 95.8 },
    Cost { scene: "lookdev-town-hero", lavapipe: 24.4, warp: 75.6 },
    Cost { scene: "lookdev-town-golden", lavapipe: 72.9, warp: 115.3 },
    Cost { scene: "lookdev-town-blue", lavapipe: 36.0, warp: 64.9 },
    Cost { scene: "lookdev-town-night", lavapipe: 58.9, warp: 83.3 },
    Cost { scene: "lookdev-street-night", lavapipe: 63.2, warp: 78.4 },
    Cost { scene: "gi-golden-hour", lavapipe: 1.4, warp: 3.4 },
    Cost { scene: "gi-clamped", lavapipe: 1.6, warp: 4.4 },
    Cost { scene: "eyes-front", lavapipe: 0.9, warp: 4.0 },
    Cost { scene: "eyes-three-quarter", lavapipe: 0.6, warp: 2.8 },
    Cost { scene: "eyes-gaze", lavapipe: 0.8, warp: 4.2 },
    Cost { scene: "eyes-highlight", lavapipe: 0.7, warp: 4.2 },
    Cost { scene: "eyes-lids", lavapipe: 0.7, warp: 3.8 },
    Cost { scene: "eyes-mobile", lavapipe: 0.9, warp: 3.5 },
    Cost { scene: "debug-wireframe", lavapipe: 42.2, warp: 61.4 },
    Cost { scene: "debug-overdraw", lavapipe: 13.3, warp: 27.0 },
    Cost { scene: "debug-lights", lavapipe: 4.8, warp: 14.2 },
    Cost { scene: "debug-lights-mobile", lavapipe: 0.8, warp: 2.0 },
    Cost { scene: "debug-material-ids", lavapipe: 0.6, warp: 1.8 },
    Cost { scene: "debug-ramp-rows", lavapipe: 0.9, warp: 1.3 },
    Cost { scene: "debug-outline-widths", lavapipe: 1.6, warp: 10.4 },
    Cost { scene: "materials-day", lavapipe: 0.8, warp: 2.0 },
    Cost { scene: "materials-night", lavapipe: 2.7, warp: 3.4 },
    Cost { scene: "fade-taa", lavapipe: 4.8, warp: 17.4 },
    Cost { scene: "fade-mobile", lavapipe: 2.6, warp: 5.2 },
    Cost { scene: "lookdev-face", lavapipe: 44.4, warp: 74.2 },
    Cost { scene: "two-sided", lavapipe: 1.4, warp: 2.1 },
    Cost { scene: "shade-golden-hour", lavapipe: 1.4, warp: 4.1 },
    Cost { scene: "shade-hue-golden-hour", lavapipe: 1.4, warp: 4.4 },
    Cost { scene: "rim-folded", lavapipe: 0.5, warp: 1.6 },
    Cost { scene: "rim-folded-camera", lavapipe: 0.4, warp: 1.9 },
    Cost { scene: "hair-shadow-soft", lavapipe: 0.7, warp: 4.0 },
    Cost { scene: "hair-backlit", lavapipe: 1.1, warp: 3.7 },
    Cost { scene: "hair-transmission", lavapipe: 1.1, warp: 4.1 },
];
