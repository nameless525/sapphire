//! What each golden scene costs on the CPU rasterizers CI shards, so `--shard` can split a
//! family's scenes into slices that take about as long (review BC-107).
//!
//! A cost is a scene's median wall time in seconds on CI's `main` runs: the sandbox's start, its
//! capture and the comparison, as the runner's `ok` lines time them. Each scene is a fresh process
//! that compiles its own pipelines, so on lavapipe and WARP the costs range from under a second to
//! several minutes (WARP's `game-ward-gate-ultrawide-ja`, 7.6 minutes), and dealing the scenes out
//! in turn left one slice minutes longer than the others.
//!
//! The table is keyed by scene name. A scene missing from it counts as the family's costliest, so a
//! new scene never lands on an already full slice; a test fails on a name that is no longer a
//! scene. To refresh it, take the medians of at least three recent CI runs on the current code, on
//! `main` or on a PR that is about to land: each golden job uploads `timings.json` with its
//! captures (`gh run download <run> -p 'golden-dx12-warp-*' -p 'golden-vulkan-lavapipe-*'`).
//! Runners vary too much for one run: these three's lavapipe totals were 2835 to 4260 s and WARP's
//! 5603 to 7386 s. The values below are the medians of runs 42772431295, 42790087819 and
//! 42799339433 (d684625, 4fe253e and 0755aaf; 217 scenes, 211 in the first; M4 I1). Against each of
//! those runs, a table from the other two left lavapipe's slowest of three shards 1.03 to 1.04
//! times its share and WARP's slowest of six 1.07 to 1.16 times, where the table before, which
//! lacked 75 scenes, left lavapipe's slower of two 1.12 to 1.26 times and WARP's slowest 1.17 to
//! 1.28.

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
    Cost { scene: "turntable-start", lavapipe: 5.0, warp: 1.7 },
    Cost { scene: "turntable", lavapipe: 0.4, warp: 1.8 },
    Cost { scene: "turntable-msaa-off", lavapipe: 3.6, warp: 1.7 },
    Cost { scene: "turntable-lights", lavapipe: 1.6, warp: 1.5 },
    Cost { scene: "turntable-render-scale", lavapipe: 0.5, warp: 1.4 },
    Cost { scene: "turntable-skinned", lavapipe: 0.6, warp: 1.8 },
    Cost { scene: "turntable-sunset", lavapipe: 0.4, warp: 2.0 },
    Cost { scene: "turntable-swatches", lavapipe: 0.5, warp: 1.5 },
    Cost { scene: "turntable-night", lavapipe: 0.4, warp: 1.7 },
    Cost { scene: "faces", lavapipe: 7.8, warp: 4.5 },
    Cost { scene: "sky-day", lavapipe: 1.0, warp: 1.8 },
    Cost { scene: "sky-sunset", lavapipe: 0.7, warp: 1.8 },
    Cost { scene: "sky-night", lavapipe: 1.7, warp: 2.0 },
    Cost { scene: "turntable-taa", lavapipe: 2.0, warp: 5.2 },
    Cost { scene: "turntable-taa-motion", lavapipe: 1.4, warp: 6.3 },
    Cost { scene: "turntable-taa-upsampled", lavapipe: 1.1, warp: 5.6 },
    Cost { scene: "environment-day", lavapipe: 6.3, warp: 12.1 },
    Cost { scene: "environment-sunset", lavapipe: 4.6, warp: 10.5 },
    Cost { scene: "environment-night", lavapipe: 6.9, warp: 13.7 },
    Cost { scene: "post-day", lavapipe: 4.4, warp: 9.7 },
    Cost { scene: "post-sunset", lavapipe: 1.4, warp: 2.4 },
    Cost { scene: "post-flare", lavapipe: 1.2, warp: 2.5 },
    Cost { scene: "outlines-distance", lavapipe: 0.5, warp: 1.5 },
    Cost { scene: "outlines-telephoto", lavapipe: 0.4, warp: 1.5 },
    Cost { scene: "outlines-hard-edges", lavapipe: 0.4, warp: 1.5 },
    Cost { scene: "outlines-turning-taa", lavapipe: 1.3, warp: 7.7 },
    Cost { scene: "outlines-turning-taa-upsampled", lavapipe: 0.9, warp: 6.1 },
    Cost { scene: "shadows-contact", lavapipe: 2.2, warp: 11.4 },
    Cost { scene: "shadows-pillar", lavapipe: 4.2, warp: 11.7 },
    Cost { scene: "shadows-faces", lavapipe: 17.8, warp: 34.0 },
    Cost { scene: "shadows-courtyard", lavapipe: 7.1, warp: 13.5 },
    Cost { scene: "shadows-sunset", lavapipe: 5.6, warp: 16.7 },
    Cost { scene: "shadows-capsule", lavapipe: 1.0, warp: 3.2 },
    Cost { scene: "effects-blur-lines", lavapipe: 3.7, warp: 12.6 },
    Cost { scene: "effects-impact-frame", lavapipe: 4.7, warp: 12.4 },
    Cost { scene: "effects-chromatic-inversion", lavapipe: 3.0, warp: 4.9 },
    Cost { scene: "fog-day", lavapipe: 6.1, warp: 9.4 },
    Cost { scene: "fog-sunset", lavapipe: 6.0, warp: 12.5 },
    Cost { scene: "fog-night", lavapipe: 5.4, warp: 13.6 },
    Cost { scene: "fog-valley", lavapipe: 4.2, warp: 12.0 },
    Cost { scene: "fog-cards", lavapipe: 6.0, warp: 11.9 },
    Cost { scene: "fog-cards-mobile", lavapipe: 1.2, warp: 2.4 },
    Cost { scene: "fog-characters", lavapipe: 3.7, warp: 9.2 },
    Cost { scene: "ambient-day", lavapipe: 6.7, warp: 24.1 },
    Cost { scene: "ambient-sunset", lavapipe: 10.5, warp: 23.8 },
    Cost { scene: "ambient-night", lavapipe: 5.9, warp: 23.7 },
    Cost { scene: "ambient-gtao", lavapipe: 5.8, warp: 24.6 },
    Cost { scene: "ambient-capsule", lavapipe: 2.0, warp: 14.7 },
    Cost { scene: "ambient-mobile", lavapipe: 1.4, warp: 5.5 },
    Cost { scene: "zone-ward-hub", lavapipe: 55.6, warp: 87.4 },
    Cost { scene: "fight-ward-market", lavapipe: 31.9, warp: 73.6 },
    Cost { scene: "fight-ward-terrace", lavapipe: 40.9, warp: 62.4 },
    Cost { scene: "zone-ward-court", lavapipe: 36.6, warp: 79.9 },
    Cost { scene: "rim-angles", lavapipe: 0.8, warp: 1.9 },
    Cost { scene: "rim-backlit", lavapipe: 0.6, warp: 2.0 },
    Cost { scene: "hair-highlights", lavapipe: 2.0, warp: 3.9 },
    Cost { scene: "hair-shadow", lavapipe: 1.3, warp: 4.1 },
    Cost { scene: "hair-shadow-mobile", lavapipe: 0.9, warp: 4.3 },
    Cost { scene: "eyes-through-hair", lavapipe: 1.6, warp: 4.3 },
    Cost { scene: "foliage-day", lavapipe: 45.0, warp: 58.4 },
    Cost { scene: "foliage-backlit", lavapipe: 15.3, warp: 22.6 },
    Cost { scene: "foliage-ground", lavapipe: 99.3, warp: 122.8 },
    Cost { scene: "foliage-mobile", lavapipe: 1.5, warp: 5.0 },
    Cost { scene: "foliage-wind", lavapipe: 60.8, warp: 73.8 },
    Cost { scene: "night-character", lavapipe: 7.7, warp: 19.2 },
    Cost { scene: "lookdev-town-day", lavapipe: 53.4, warp: 99.8 },
    Cost { scene: "lookdev-town-hero", lavapipe: 33.1, warp: 75.3 },
    Cost { scene: "lookdev-town-golden", lavapipe: 76.4, warp: 93.6 },
    Cost { scene: "lookdev-town-blue", lavapipe: 50.4, warp: 100.7 },
    Cost { scene: "lookdev-town-night", lavapipe: 62.0, warp: 85.6 },
    Cost { scene: "lookdev-street-night", lavapipe: 67.2, warp: 103.1 },
    Cost { scene: "gi-golden-hour", lavapipe: 1.6, warp: 4.4 },
    Cost { scene: "gi-clamped", lavapipe: 1.6, warp: 5.2 },
    Cost { scene: "eyes-front", lavapipe: 1.1, warp: 3.9 },
    Cost { scene: "eyes-three-quarter", lavapipe: 0.8, warp: 3.9 },
    Cost { scene: "eyes-gaze", lavapipe: 1.1, warp: 4.3 },
    Cost { scene: "eyes-highlight", lavapipe: 1.1, warp: 4.7 },
    Cost { scene: "eyes-lids", lavapipe: 1.1, warp: 4.6 },
    Cost { scene: "eyes-mobile", lavapipe: 0.8, warp: 3.8 },
    Cost { scene: "debug-wireframe", lavapipe: 43.4, warp: 106.1 },
    Cost { scene: "debug-overdraw", lavapipe: 18.2, warp: 30.9 },
    Cost { scene: "debug-lights", lavapipe: 6.6, warp: 13.9 },
    Cost { scene: "debug-lights-mobile", lavapipe: 1.5, warp: 2.3 },
    Cost { scene: "debug-material-ids", lavapipe: 1.1, warp: 1.7 },
    Cost { scene: "debug-ramp-rows", lavapipe: 0.7, warp: 1.8 },
    Cost { scene: "debug-outline-widths", lavapipe: 2.6, warp: 12.4 },
    Cost { scene: "debugdraw-shapes", lavapipe: 0.8, warp: 1.5 },
    Cost { scene: "debugdraw-volumes", lavapipe: 0.8, warp: 2.0 },
    Cost { scene: "timeline-camera-post", lavapipe: 16.0, warp: 47.0 },
    Cost { scene: "trip-wipe-mid", lavapipe: 38.9, warp: 65.4 },
    Cost { scene: "trip-wipe-covered", lavapipe: 10.8, warp: 32.6 },
    Cost { scene: "arena-day", lavapipe: 53.7, warp: 85.4 },
    Cost { scene: "arena-golden", lavapipe: 48.2, warp: 105.6 },
    Cost { scene: "arena-replay", lavapipe: 40.6, warp: 65.4 },
    Cost { scene: "arena-fight-telegraph", lavapipe: 39.1, warp: 80.6 },
    Cost { scene: "arena-fight-hit", lavapipe: 39.8, warp: 85.6 },
    Cost { scene: "arena-fight-slash", lavapipe: 53.1, warp: 71.6 },
    Cost { scene: "arena-fight-cut", lavapipe: 53.4, warp: 84.3 },
    Cost { scene: "arena-party-second", lavapipe: 40.4, warp: 83.6 },
    Cost { scene: "arena-party-third", lavapipe: 54.1, warp: 71.0 },
    Cost { scene: "cutscene-gate-warden", lavapipe: 16.0, warp: 49.0 },
    Cost { scene: "cutscene-gate-guard", lavapipe: 27.0, warp: 58.2 },
    Cost { scene: "cutscene-gate-return", lavapipe: 30.6, warp: 69.2 },
    Cost { scene: "arena-ultimate-chop", lavapipe: 50.5, warp: 81.4 },
    Cost { scene: "arena-skirmish", lavapipe: 56.8, warp: 132.4 },
    Cost { scene: "arena-boss", lavapipe: 45.3, warp: 64.1 },
    Cost { scene: "arena-captain", lavapipe: 45.2, warp: 90.4 },
    Cost { scene: "materials-day", lavapipe: 1.1, warp: 3.1 },
    Cost { scene: "materials-night", lavapipe: 1.2, warp: 3.5 },
    Cost { scene: "fade-taa", lavapipe: 9.5, warp: 19.1 },
    Cost { scene: "fade-mobile", lavapipe: 1.3, warp: 4.6 },
    Cost { scene: "lookdev-face", lavapipe: 35.4, warp: 77.9 },
    Cost { scene: "lookdev-strike-anticipation", lavapipe: 16.5, warp: 42.5 },
    Cost { scene: "lookdev-strike-fast", lavapipe: 17.5, warp: 48.4 },
    Cost { scene: "lookdev-strike-mark", lavapipe: 23.0, warp: 43.1 },
    Cost { scene: "lookdev-strike-settle", lavapipe: 17.5, warp: 40.1 },
    Cost { scene: "lookdev-strike-face", lavapipe: 18.8, warp: 45.0 },
    Cost { scene: "lookdev-ik-slope", lavapipe: 14.2, warp: 45.2 },
    Cost { scene: "lookdev-ik-step", lavapipe: 14.6, warp: 34.2 },
    Cost { scene: "lookdev-death-back", lavapipe: 10.5, warp: 29.7 },
    Cost { scene: "lookdev-strike-springs-mark", lavapipe: 23.0, warp: 44.4 },
    Cost { scene: "lookdev-strike-springs-after", lavapipe: 23.1, warp: 43.4 },
    Cost { scene: "lookdev-clip-idle", lavapipe: 15.8, warp: 37.3 },
    Cost { scene: "lookdev-clip-run", lavapipe: 15.9, warp: 38.1 },
    Cost { scene: "lookdev-clip-hit", lavapipe: 20.8, warp: 38.6 },
    Cost { scene: "lookdev-clip-run-springs", lavapipe: 21.0, warp: 37.7 },
    Cost { scene: "lookdev-graph-run", lavapipe: 29.8, warp: 64.1 },
    Cost { scene: "lookdev-graph-cancel", lavapipe: 21.9, warp: 40.1 },
    Cost { scene: "lookdev-enemy", lavapipe: 46.5, warp: 92.0 },
    Cost { scene: "lookdev-enemy-night", lavapipe: 79.3, warp: 117.8 },
    Cost { scene: "lookdev-lancer", lavapipe: 61.3, warp: 90.9 },
    Cost { scene: "lookdev-move-cut", lavapipe: 9.2, warp: 26.7 },
    Cost { scene: "lookdev-move-windup", lavapipe: 22.6, warp: 41.2 },
    Cost { scene: "lookdev-move-landing", lavapipe: 25.3, warp: 45.0 },
    Cost { scene: "lookdev-move-dash", lavapipe: 7.6, warp: 23.6 },
    Cost { scene: "lookdev-move-dodge", lavapipe: 6.9, warp: 19.7 },
    Cost { scene: "lookdev-move-coil", lavapipe: 13.8, warp: 33.2 },
    Cost { scene: "two-sided", lavapipe: 0.9, warp: 2.2 },
    Cost { scene: "shade-golden-hour", lavapipe: 1.6, warp: 4.4 },
    Cost { scene: "shade-hue-golden-hour", lavapipe: 1.2, warp: 5.2 },
    Cost { scene: "rim-folded", lavapipe: 0.5, warp: 2.0 },
    Cost { scene: "rim-folded-camera", lavapipe: 0.5, warp: 1.7 },
    Cost { scene: "hair-shadow-soft", lavapipe: 1.0, warp: 4.1 },
    Cost { scene: "hair-backlit", lavapipe: 0.9, warp: 4.1 },
    Cost { scene: "hair-transmission", lavapipe: 1.3, warp: 4.0 },
    Cost { scene: "vfx-arc-noon", lavapipe: 21.9, warp: 38.6 },
    Cost { scene: "vfx-arc-night", lavapipe: 29.8, warp: 50.1 },
    Cost { scene: "vfx-hit-sunset", lavapipe: 17.4, warp: 46.0 },
    Cost { scene: "vfx-duel-noon", lavapipe: 16.7, warp: 34.7 },
    Cost { scene: "vfx-duel-mobile-low", lavapipe: 18.2, warp: 36.4 },
    Cost { scene: "vfx-plain", lavapipe: 0.4, warp: 1.7 },
    Cost { scene: "hit-medium-noon", lavapipe: 22.9, warp: 39.9 },
    Cost { scene: "hit-break-noon", lavapipe: 22.9, warp: 39.7 },
    Cost { scene: "hit-finish-dusk", lavapipe: 28.4, warp: 52.0 },
    Cost { scene: "game-title-tablet-en", lavapipe: 0.8, warp: 2.0 },
    Cost { scene: "game-title-tablet-ja", lavapipe: 1.4, warp: 2.1 },
    Cost { scene: "game-title-wide-en", lavapipe: 0.7, warp: 2.0 },
    Cost { scene: "game-title-wide-ja", lavapipe: 1.3, warp: 1.8 },
    Cost { scene: "game-title-ultrawide-en", lavapipe: 0.8, warp: 2.3 },
    Cost { scene: "game-title-ultrawide-ja", lavapipe: 1.5, warp: 2.5 },
    Cost { scene: "game-pause-tablet-en", lavapipe: 0.6, warp: 1.9 },
    Cost { scene: "game-pause-tablet-ja", lavapipe: 1.1, warp: 1.9 },
    Cost { scene: "game-pause-wide-en", lavapipe: 0.6, warp: 2.0 },
    Cost { scene: "game-pause-wide-ja", lavapipe: 1.2, warp: 1.7 },
    Cost { scene: "game-pause-ultrawide-en", lavapipe: 0.6, warp: 2.2 },
    Cost { scene: "game-pause-ultrawide-ja", lavapipe: 1.3, warp: 2.3 },
    Cost { scene: "game-settings-tablet-en", lavapipe: 0.6, warp: 2.0 },
    Cost { scene: "game-settings-tablet-ja", lavapipe: 0.6, warp: 2.0 },
    Cost { scene: "game-settings-wide-en", lavapipe: 0.6, warp: 2.0 },
    Cost { scene: "game-settings-wide-ja", lavapipe: 0.6, warp: 1.8 },
    Cost { scene: "game-settings-ultrawide-en", lavapipe: 0.6, warp: 2.2 },
    Cost { scene: "game-settings-ultrawide-ja", lavapipe: 0.6, warp: 2.3 },
    Cost { scene: "game-character-tablet-en", lavapipe: 0.8, warp: 3.0 },
    Cost { scene: "game-character-tablet-ja", lavapipe: 0.8, warp: 2.7 },
    Cost { scene: "game-character-wide-en", lavapipe: 0.8, warp: 2.4 },
    Cost { scene: "game-character-wide-ja", lavapipe: 0.8, warp: 2.5 },
    Cost { scene: "game-character-ultrawide-en", lavapipe: 0.8, warp: 2.7 },
    Cost { scene: "game-character-ultrawide-ja", lavapipe: 0.8, warp: 2.7 },
    Cost { scene: "game-ward-title", lavapipe: 52.6, warp: 116.9 },
    Cost { scene: "game-ward-checkpoint", lavapipe: 114.0, warp: 193.7 },
    Cost { scene: "game-ward-gate-tablet-en", lavapipe: 209.8, warp: 353.9 },
    Cost { scene: "game-ward-gate-tablet-ja", lavapipe: 187.1, warp: 257.9 },
    Cost { scene: "game-ward-gate-wide-en", lavapipe: 236.3, warp: 339.8 },
    Cost { scene: "game-ward-gate-wide-ja", lavapipe: 209.8, warp: 395.3 },
    Cost { scene: "game-ward-gate-ultrawide-en", lavapipe: 296.6, warp: 415.9 },
    Cost { scene: "game-ward-gate-ultrawide-ja", lavapipe: 263.2, warp: 453.4 },
    Cost { scene: "game-character-phone-en", lavapipe: 1.3, warp: 2.8 },
    Cost { scene: "game-dialogue-tablet-en", lavapipe: 0.5, warp: 1.6 },
    Cost { scene: "game-dialogue-tablet-ja", lavapipe: 0.6, warp: 1.9 },
    Cost { scene: "game-dialogue-wide-en", lavapipe: 0.6, warp: 1.9 },
    Cost { scene: "game-dialogue-wide-ja", lavapipe: 0.6, warp: 1.9 },
    Cost { scene: "game-dialogue-ultrawide-en", lavapipe: 0.6, warp: 2.1 },
    Cost { scene: "game-dialogue-ultrawide-ja", lavapipe: 0.6, warp: 2.0 },
    Cost { scene: "game-guide-tablet-en", lavapipe: 0.6, warp: 1.6 },
    Cost { scene: "game-guide-tablet-ja", lavapipe: 0.6, warp: 1.9 },
    Cost { scene: "game-guide-wide-en", lavapipe: 0.6, warp: 1.9 },
    Cost { scene: "game-guide-wide-ja", lavapipe: 0.6, warp: 1.9 },
    Cost { scene: "game-guide-ultrawide-en", lavapipe: 0.6, warp: 2.1 },
    Cost { scene: "game-guide-ultrawide-ja", lavapipe: 0.6, warp: 2.0 },
    Cost { scene: "game-hud-tablet-en", lavapipe: 1.1, warp: 1.6 },
    Cost { scene: "game-hud-tablet-ja", lavapipe: 0.6, warp: 1.9 },
    Cost { scene: "game-hud-wide-en", lavapipe: 1.2, warp: 2.0 },
    Cost { scene: "game-hud-wide-ja", lavapipe: 0.6, warp: 1.9 },
    Cost { scene: "game-hud-ultrawide-en", lavapipe: 1.3, warp: 2.1 },
    Cost { scene: "game-hud-ultrawide-ja", lavapipe: 0.6, warp: 2.0 },
    Cost { scene: "game-pause-entering-ja", lavapipe: 1.1, warp: 1.7 },
    Cost { scene: "game-settings-walked-ja", lavapipe: 0.6, warp: 2.1 },
    Cost { scene: "hud-wide", lavapipe: 14.2, warp: 14.1 },
    Cost { scene: "hud-tablet", lavapipe: 4.4, warp: 13.3 },
    Cost { scene: "hud-ultrawide", lavapipe: 5.8, warp: 14.2 },
    Cost { scene: "hud-notch", lavapipe: 6.2, warp: 14.8 },
    Cost { scene: "display-text-latin", lavapipe: 5.1, warp: 14.3 },
    Cost { scene: "display-text-japanese", lavapipe: 7.4, warp: 21.2 },
    Cost { scene: "hud-energy-empty-wide", lavapipe: 5.3, warp: 13.9 },
    Cost { scene: "hud-energy-full-wide", lavapipe: 4.0, warp: 14.2 },
    Cost { scene: "hud-energy-empty-notch", lavapipe: 5.7, warp: 12.6 },
    Cost { scene: "hud-energy-full-notch", lavapipe: 4.2, warp: 14.4 },
];
