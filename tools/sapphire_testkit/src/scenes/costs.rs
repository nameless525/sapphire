//! What each golden scene costs on the CPU rasterizers CI shards, so `--shard` can split a
//! family's scenes into slices that take about as long (review BC-107).
//!
//! A cost is a scene's `ci_seconds` in the document (`tests/golden/scenes.ron`, M4 R1): its median
//! wall time in seconds on CI's `main` runs, the sandbox's start, its capture and the comparison, as
//! the runner's `ok` lines time them. Each scene is a fresh process that compiles its own pipelines,
//! so on lavapipe and WARP the costs range from under a second to several minutes (WARP's
//! `game-ward-gate-ultrawide-ja`, 7.6 minutes), and dealing the scenes out in turn left one slice
//! minutes longer than the others. A scene without them counts as the family's costliest, so a new
//! scene never lands on an already full slice. The document's header says where its times come from
//! and how to refresh them. M4 I1's table, from the medians of three runs, left lavapipe's slowest of
//! three shards 1.03 to 1.04 times its share and WARP's slowest of six 1.07 to 1.16 times, against
//! each of those runs from the other two; the table before, which lacked 75 scenes, left lavapipe's
//! slower of two 1.12 to 1.26 times and WARP's slowest 1.17 to 1.28.

use super::{CiSeconds, Scene};

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

    fn seconds(self, seconds: CiSeconds) -> f32 {
        match self {
            Self::Lavapipe => seconds.lavapipe,
            Self::Warp => seconds.warp,
        }
    }
}

/// Each scene's cost on `family`, in tenths of a second: its CI time, or the costliest scene's when it has
/// none yet. All equal without a family.
pub(crate) fn costs(scenes: &[Scene], family: Option<Family>) -> Vec<u32> {
    let Some(family) = family else { return vec![1; scenes.len()] };
    let timed = |scene: &Scene| scene.ci_seconds.map(|seconds| family.seconds(seconds));
    let costliest = scenes.iter().filter_map(timed).fold(0.0, f32::max);
    // Tenths: whole numbers keep the split exact and the same on every host.
    scenes.iter().map(|scene| (timed(scene).unwrap_or(costliest) * 10.0).round() as u32).collect()
}
