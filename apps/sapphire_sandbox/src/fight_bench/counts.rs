//! What a bench of a fight or a walk counts beside its times (M5 B4): what each frame submitted, pass by pass
//! (`RenderStats::passes`), the ambient pass's compute dispatches and the reflection probes' among them
//! (M5 L3), what it skinned and lit, what the asset server held and let in, what animation posed, held and
//! thinned and in how many jobs (M5 C1), how many of a zone's ambient characters stood and how many of them
//! were in view (M5 C10), and the kind of core the frame's main thread was on.
//!
//! - **Counts need no rounds:** the same walk or fight draws the same frames, so a count row's spread is the
//!   scene's, not the machine's. Each is logged per phase with its 50th and 95th percentile, its worst
//!   frame, the tick and the metres walked it fell on, its total over the phase's frames, and spec §3.4's
//!   view budget where it has one (2,500 of the main view's draw calls and 64 lights a desktop frame, 700 and 8
//!   a phone's: m5-plan §3 puts the draws on `main draws`, and `draw calls`, every pass's, has none), and exit
//!   test 2's where it has one ([`Budget`], M5 Z2): nothing over a pool's budget in use and nothing loaded on
//!   the frame's thread, what's decoded and waiting held to the run's admission cap, and the streamed textures
//!   to the pool the run holds (`--texture-pool` where given), both read from its asset server. A row
//!   every frame read zero of says `not exercised`: nothing on this walk or fight fed it (a fourth cascade
//!   on a tier of three, say).
//! - **A plain bench of a walk** (`--zone-walk` with `--draw-every <n>`, which `bench.rs` runs) logs the same
//!   rows over the frames it drew ([`DrawnCounts`], M5 X4): only every `n`th frame runs its graph, and one
//!   that doesn't submits nothing. They're what CI's scale lane holds on a software GPU (`cargo xtask
//!   counts`), and they time nothing. So does a fight's bench under `--draw-every`, over the frames of its one
//!   phase that drew (M5 B5).
//! - **Core kinds:** each frame samples which kind of core the main thread was on as its CPU work ended
//!   (`sapphire_platform::current_core`), and each phase logs its frames' CPU and GPU times split by kind,
//!   which is what explains a ×2.6–3 spread between runs of one build: a frame on an efficiency core.

use std::time::Duration;

use sapphire_assets::{AssetBudgets, AssetKind, AssetStats};
use sapphire_platform::CoreKind;
use sapphire_render::{FrameCounts, RenderStats, RunStats, StateChanges};
use sapphire_stage::TextureStreamingStats;

use super::{Phase, Traced, spread};
use crate::LOG_TARGET;
use crate::offscreen::Stage;

/// What a frame counted, beside its times.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counted {
    pub passes: FrameCounts,
    /// The pipelines, materials and mesh streams the mesh passes set (M5 D1).
    pub state_changes: StateChanges,
    /// The ambient pass's compute dispatches, and of them the reflection probes' composition and prefilter
    /// (`AmbientStats::dispatches`, `AmbientStats::probe_dispatches`): compute work, which draws nothing.
    pub ambient_dispatches: u32,
    pub probe_dispatches: u32,
    /// The instanced runs the view lists formed (M5 D2).
    pub runs: RunStats,
    /// Scene instances, and of the skinned ones those skinned this frame, their joints and vertices.
    pub instances: u32,
    pub skinned: u32,
    pub joints: u32,
    pub skinned_vertices: u32,
    /// Local lights the frame drew with, culled, and dropped past the tier's cap.
    pub lights: u32,
    pub lights_culled: u32,
    pub lights_dropped: u32,
    /// The asset server's: bytes admitted and reads held back this frame, and bytes decoded and waiting.
    pub admitted: u64,
    pub reads_held: u64,
    pub decoded_waiting: u64,
    /// Loads the frame's thread did in play since the last frame counted (exit test 2,
    /// `sapphire_core::frame_loads`): reads, decodes and checks, which the asset server's workers do. The bench
    /// fills it in; [`Self::of`] leaves it 0.
    pub frame_thread_loads: u64,
    /// The texture and mesh pools' bytes, and what every pool held over its budget, in use.
    pub texture_pool: u64,
    pub mesh_pool: u64,
    pub over_budget: u64,
    /// Bytes the frame uploaded.
    pub uploaded: u64,
    /// The renderer's mesh pool on the GPU: what its buffers hold, and its floor, which a compaction
    /// gives the rest back down to (M5 D9).
    pub mesh_pool_capacity: u64,
    pub mesh_pool_floor: u64,
    /// Bytes of texture levels the renderer wrote raising textures this frame (M5 M4), and the bytes
    /// the app gave back to the asset server, lowered levels (`AssetStats::given_back`).
    pub raised: u64,
    pub given_back: u64,
    /// Animation's (M5 C1, `AnimStats`): characters with something to play, those posed, those a level
    /// held, those posed below full or held, those held off screen, and the jobs they ran in (0 on the
    /// frame's thread).
    pub animated: u32,
    pub posed: u32,
    pub held: u32,
    pub thinned: u32,
    pub off_screen: u32,
    pub animation_jobs: u32,
    /// Characters whose spring bones stepped and wrote their pose (M5 C12, `AnimStats::springs`): a
    /// party's member on the field and every enemy with chains, against the ambient's none.
    pub springs_stepped: u32,
    /// A zone's ambient characters (M5 C10, `AmbientStats`): those standing, and of them those a view or
    /// its shadow shows.
    pub ambient_spawned: u32,
    pub ambient_in_view: u32,
    /// Of those in view, the ones in the animation levels' middle and far bands, and the people holding
    /// spring bones' state (M5 C11), which none do.
    pub ambient_middle_band: u32,
    pub ambient_far_band: u32,
    pub ambient_springs: u32,
    /// Skinned instances every view drew past their first level (M5 C11, `SkinningStats::coarser`).
    pub skinned_coarser: u32,
    /// Instances the main view drew last frame at another level of detail (M5 D4, `LevelStats`).
    pub level_switches: u32,
    /// The texture streamer's (M5 M5): textures in debt and the worst debt in frames, levels raised and
    /// lowered and lowers waiting their turn, bytes held against its pool, the tails' share and what the
    /// views would have it hold, and what its work took in microseconds: the streamer's, the raises' share of
    /// it, and the coverage estimate's. The times are the trace's and the CPU table's (`texture streaming`,
    /// `texture raising`, `texture coverage`), never count rows, which CI's scale lane holds exactly.
    pub textures_in_debt: u64,
    pub worst_texture_debt: u64,
    pub levels_raised: u64,
    pub levels_lowered: u64,
    pub lowers_waiting: u64,
    /// Textures whose levels started loading (the trace's alone, no row).
    pub texture_starts: u64,
    pub streamed_bytes: u64,
    pub streamed_pool: u64,
    pub texture_tails: u64,
    pub texture_wanted: u64,
    pub streaming_us: u64,
    pub raising_us: u64,
    pub coverage_us: u64,
    /// The characters' capsules by cluster (M5 L18, `CapsuleBinStats`): the capsules binned in the
    /// shadows' and AO's lists, the main view's clusters with any bit set, the bits set over them all,
    /// and the most one cluster holds, which is the most lookups a pixel walks.
    pub shadow_capsules: u32,
    pub occlusion_capsules: u32,
    pub capsule_clusters: u32,
    pub capsule_bits: u32,
    pub capsule_fullest: u32,
    /// Spot lights' shadow tiles (M5 L16, `ShadowStats`): drawn again this frame, sampled as
    /// drawn before, and the spot layer's bytes the spot pass loaded.
    pub spot_tiles_redrawn: u32,
    pub spot_tiles_cached: u32,
    pub spot_load_bytes: u64,
    /// The cascades drawn this frame, the cached ones among them drawn under the fit they had
    /// since something in them changed, and the casters that change their depth without the plan
    /// seeing it (M5 D8, `ShadowStats`).
    pub cascades_drawn: u32,
    pub cached_cascades_redrawn: u32,
    pub moving_shadow_casters: u32,
}

impl Counted {
    /// These counts with the stage's own in its last frame: what animation did (`Scene::anim_stats`) and
    /// which of a zone's ambient characters stood (`Scene::ambient_stats`).
    pub(super) fn with_stage(self, stage: &Stage) -> Self {
        self.with_scene(&stage.scene)
    }

    /// These counts with `scene`'s own in its last frame (as [`Self::with_stage`]), for the overlay's
    /// counts panel (M5 X1).
    pub(crate) fn with_scene(self, scene: &sapphire_stage::Scene) -> Self {
        let anim = scene.anim_stats();
        let ambient = scene.ambient_stats();
        Self {
            animated: anim.characters,
            posed: anim.evaluated,
            held: anim.held,
            thinned: anim.thinned,
            off_screen: anim.off_screen,
            animation_jobs: anim.jobs,
            springs_stepped: anim.springs,
            ambient_spawned: ambient.spawned,
            ambient_in_view: ambient.in_view,
            ambient_middle_band: ambient.middle_band,
            ambient_far_band: ambient.far_band,
            ambient_springs: ambient.springs,
            ..self
        }
    }

    /// What texture streaming added to the frame's CPU: the streamer's work and the coverage estimate's.
    pub(super) fn texture_streaming(&self) -> Duration {
        Duration::from_micros(self.streaming_us + self.coverage_us)
    }

    /// Of [`Self::texture_streaming`], what the raises took on the renderer.
    pub(super) fn texture_raising(&self) -> Duration {
        Duration::from_micros(self.raising_us)
    }

    /// Of [`Self::texture_streaming`], what the coverage estimate took.
    pub(super) fn texture_coverage(&self) -> Duration {
        Duration::from_micros(self.coverage_us)
    }

    /// A frame's counts; `before` is the asset server's at the frame before, for what this one admitted, and
    /// `streaming` the texture streamer's.
    pub(crate) fn of(
        render: &RenderStats,
        assets: &AssetStats,
        before: &AssetStats,
        streaming: &TextureStreamingStats,
    ) -> Self {
        let pool = |kind: AssetKind| -> u64 {
            AssetKind::ALL
                .into_iter()
                .filter(|&counted| AssetBudgets::pool(counted) == kind)
                .map(|counted| assets.resident[counted.index()])
                .sum()
        };
        Self {
            passes: render.passes,
            state_changes: render.state_changes,
            ambient_dispatches: render.ambient.dispatches,
            probe_dispatches: render.ambient.probe_dispatches,
            runs: render.runs,
            instances: render.instances,
            skinned: render.skinning.skinned,
            joints: render.skinning.joints,
            skinned_vertices: render.skinning.vertices,
            skinned_coarser: render.skinning.coarser,
            lights: render.lights.drawn,
            lights_culled: render.lights.culled,
            lights_dropped: render.lights.dropped,
            admitted: assets.admitted_total.saturating_sub(before.admitted_total),
            reads_held: assets.reads_held.saturating_sub(before.reads_held),
            decoded_waiting: assets.decoded_waiting,
            texture_pool: pool(AssetKind::Texture),
            mesh_pool: pool(AssetKind::Mesh),
            over_budget: assets.over_budget.iter().sum(),
            uploaded: assets.last_upload.bytes,
            mesh_pool_capacity: render.mesh_pool_capacity,
            mesh_pool_floor: render.mesh_pool.floor,
            raised: render.mip_streaming.bytes_written,
            given_back: assets.given_back.saturating_sub(before.given_back),
            level_switches: render.levels.switches(),
            textures_in_debt: u64::from(streaming.in_debt),
            worst_texture_debt: u64::from(streaming.worst_debt),
            levels_raised: u64::from(streaming.levels_raised),
            levels_lowered: u64::from(streaming.levels_lowered),
            lowers_waiting: u64::from(streaming.lowers_waiting),
            texture_starts: u64::from(streaming.started),
            streamed_bytes: streaming.resident,
            streamed_pool: streaming.pool,
            texture_tails: streaming.tails,
            texture_wanted: streaming.wanted,
            streaming_us: streaming.time.as_micros() as u64,
            raising_us: streaming.raising.as_micros() as u64,
            coverage_us: streaming.coverage_time.as_micros() as u64,
            shadow_capsules: render.capsule_bins.shadow_capsules,
            occlusion_capsules: render.capsule_bins.occlusion_capsules,
            capsule_clusters: render.capsule_bins.marked_clusters,
            capsule_bits: render.capsule_bins.bits,
            capsule_fullest: render.capsule_bins.fullest,
            spot_tiles_redrawn: render.shadows.spot_tiles_drawn,
            spot_tiles_cached: render.shadows.spot_tiles_cached,
            spot_load_bytes: render.shadows.spot_load_bytes,
            cascades_drawn: render.shadows.cascades_drawn,
            cached_cascades_redrawn: render.shadows.cached_redrawn,
            moving_shadow_casters: render.shadows.moving_casters,
            ..Self::default()
        }
    }
}

/// A count the bench tracks per frame, its group, and spec §3.4's budget for it on each tier, if any: the one
/// source of the bench's count rows and the overlay's counts panel (M5 X1).
pub(crate) struct CountRow {
    /// The rows it is shown among in the overlay's counts panel, one of the group names below.
    #[cfg_attr(not(feature = "dev"), allow(dead_code, reason = "the overlay, in dev builds, reads it"))]
    pub group: &'static str,
    pub name: &'static str,
    pub read: fn(&Counted) -> u64,
    pub budget: Budget,
}

/// What a count row is budgeted against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Budget {
    /// A figure on each tier, or none: Desktop-High, Desktop-Low, Mobile-High, Mobile-Low.
    Tiers([Option<u64>; 4]),
    /// The asset server's admission cap as the run holds it ([`sapphire_assets::AssetBudgets::admission`]).
    Admission,
    /// The textures' pool as the run holds it: `--texture-pool` where given, else the tier's zone texture pool
    /// (the asset server's texture budget follows either, `Content::set_texture_pool`).
    TexturePool,
}

impl Budget {
    /// The figure on `run`, if any.
    pub(crate) fn on(self, run: &RunBudgets) -> Option<u64> {
        match self {
            Self::Tiers(tiers) => tiers[run.column],
            Self::Admission => Some(run.assets.admission),
            Self::TexturePool => Some(run.assets.resident(AssetKind::Texture)),
        }
    }
}

/// What a run's count rows are measured against: its tier's column of the fixed figures, and its asset server's
/// budgets as they stand, so a run's own pool is the pool's budget and nothing is a copy.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RunBudgets {
    pub column: usize,
    pub assets: AssetBudgets,
}

impl RunBudgets {
    pub(crate) fn of(stage: &Stage) -> Self {
        Self {
            column: super::tier_column(stage.quality.stats().tier.id),
            assets: stage.content.stats().budgets,
        }
    }
}

const fn row(group: &'static str, name: &'static str, read: fn(&Counted) -> u64) -> CountRow {
    CountRow { group, name, read, budget: Budget::Tiers([None; 4]) }
}

/// The count rows' groups, in their order.
const FRAME: &str = "the frame";
const PASSES: &str = "passes";
const CULLING: &str = "culling";
const LEVELS: &str = "levels of detail";
const STATE: &str = "state changes";
const RUNS: &str = "instanced runs";
const SKINNING: &str = "instances and skinning";
const LIGHTS: &str = "lights";
const ASSETS: &str = "the asset server";
const ANIMATION: &str = "animation";
const CROWD: &str = "a zone's crowd";
const STREAMING: &str = "texture streaming";
const CAPSULES: &str = "capsules by cluster";
/// Every group, in the rows' order (a test holds `COUNT_ROWS` to it).
#[cfg(test)]
const GROUPS: [&str; 13] = [
    FRAME, PASSES, CULLING, LEVELS, STATE, RUNS, SKINNING, LIGHTS, ASSETS, ANIMATION, CROWD, STREAMING,
    CAPSULES,
];

/// What's logged, the frame's whole first, then pass by pass (spot lights' shadow tiles among them), the
/// ambient pass's dispatches, then what culled, the levels drawn (the main view's and the shadows' apart,
/// and the main view's switches, M5 D4), the state the mesh passes changed, the instanced runs,
/// skinning, lights, the asset server's, animation's, a zone's crowd's, the texture streamer's and the
/// characters' capsules by cluster.
pub(crate) const COUNT_ROWS: [CountRow; 90] = [
    // Every call the frame submitted, across all passes: m5-plan §3's second row, with no budget.
    row(FRAME, "draw calls", |c| u64::from(c.passes.total().draws)),
    row(FRAME, "triangles", |c| c.passes.total().triangles),
    row(FRAME, "draw instances", |c| u64::from(c.passes.total().instances)),
    row(FRAME, "outline draws", |c| u64::from(c.passes.total().outline_draws)),
    // §3.4: a desktop frame's 2,500 draws, a phone's 700, on the main view's calls after culling (m5-plan §3; a
    // run of equal draws is one call).
    CountRow {
        name: "main draws",
        group: PASSES,
        read: |c| u64::from(c.passes.main.draws),
        budget: Budget::Tiers([Some(2_500), Some(2_500), Some(700), Some(700)]),
    },
    // The instances those calls draw (M5 D2).
    row(PASSES, "main instances", |c| u64::from(c.passes.main.instances)),
    row(PASSES, "main triangles", |c| c.passes.main.triangles),
    row(PASSES, "character prepass draws", |c| u64::from(c.passes.character_prepass.draws)),
    row(PASSES, "scene prepass draws", |c| u64::from(c.passes.scene_prepass.draws)),
    row(PASSES, "cascade 0 draws", |c| u64::from(c.passes.cascades[0].draws)),
    row(PASSES, "cascade 1 draws", |c| u64::from(c.passes.cascades[1].draws)),
    row(PASSES, "cascade 2 draws", |c| u64::from(c.passes.cascades[2].draws)),
    row(PASSES, "cascade 3 draws", |c| u64::from(c.passes.cascades[3].draws)),
    row(PASSES, "shadow triangles", |c| c.passes.shadows().triangles),
    row(PASSES, "character map draws", |c| u64::from(c.passes.character_maps.draws)),
    // Kept cascades (M5 D8): a still view redraws none, and a cached one is drawn again under its
    // fit only for what changed in it.
    row(PASSES, "cascades drawn", |c| u64::from(c.cascades_drawn)),
    row(PASSES, "cached cascades redrawn", |c| u64::from(c.cached_cascades_redrawn)),
    row(PASSES, "moving shadow casters", |c| u64::from(c.moving_shadow_casters)),
    // Spot lights' shadows (M5 L16): a still street redraws no tile.
    row(PASSES, "spot tiles redrawn", |c| u64::from(c.spot_tiles_redrawn)),
    row(PASSES, "spot tiles cached", |c| u64::from(c.spot_tiles_cached)),
    row(PASSES, "spot draws", |c| u64::from(c.passes.spots.draws)),
    row(PASSES, "spot load bytes", |c| c.spot_load_bytes),
    row(PASSES, "inset draws", |c| c.passes.insets.iter().map(|inset| u64::from(inset.draws)).sum()),
    row(PASSES, "temporal draws", |c| u64::from(c.passes.temporal.draws)),
    row(PASSES, "post draws", |c| u64::from(c.passes.post.draws)),
    row(PASSES, "upscale draws", |c| u64::from(c.passes.upscale.draws)),
    row(PASSES, "ui draws", |c| u64::from(c.passes.ui.draws)),
    row(PASSES, "ambient dispatches", |c| u64::from(c.ambient_dispatches)),
    row(PASSES, "probe dispatches", |c| u64::from(c.probe_dispatches)),
    row(CULLING, "mesh draws considered", |c| u64::from(c.passes.total().considered)),
    row(CULLING, "culled by view", |c| u64::from(c.passes.total().culled_by_frustum)),
    row(CULLING, "culled by a pass", |c| u64::from(c.passes.total().culled_by_pass)),
    row(CULLING, "culled past outline cutoff", |c| u64::from(c.passes.total().culled_by_cutoff)),
    row(CULLING, "culled by distance", |c| u64::from(c.passes.total().culled_by_distance)),
    row(CULLING, "culled by size", |c| u64::from(c.passes.total().culled_by_size)),
    // Mesh instances by level, a run's each (M5 D2).
    row(LEVELS, "lod0 draws", |c| u64::from(c.passes.total().levels[0])),
    row(LEVELS, "lod1+ draws", |c| c.passes.total().levels[1..].iter().map(|&level| u64::from(level)).sum()),
    row(LEVELS, "main draws above level 0", |c| {
        c.passes.main.levels[1..].iter().map(|&level| u64::from(level)).sum()
    }),
    row(LEVELS, "shadow draws above level 0", |c| {
        c.passes.shadows().levels[1..].iter().map(|&level| u64::from(level)).sum()
    }),
    row(LEVELS, "level switches", |c| u64::from(c.level_switches)),
    row(STATE, "pipelines set", |c| u64::from(c.state_changes.pipelines)),
    row(STATE, "materials bound", |c| u64::from(c.state_changes.materials)),
    row(STATE, "vertex buffers bound", |c| u64::from(c.state_changes.vertex_buffers)),
    // Runs of more than one draw, every view list's (an undrawn scene prepass's too).
    row(RUNS, "runs", |c| u64::from(c.runs.runs)),
    row(RUNS, "run instances", |c| u64::from(c.runs.instances)),
    row(RUNS, "longest run", |c| u64::from(c.runs.longest)),
    row(RUNS, "median run", |c| u64::from(c.runs.median)),
    row(SKINNING, "scene instances", |c| u64::from(c.instances)),
    row(SKINNING, "skinned instances", |c| u64::from(c.skinned)),
    row(SKINNING, "joints", |c| u64::from(c.joints)),
    row(SKINNING, "skinned vertices", |c| u64::from(c.skinned_vertices)),
    // Skinned instances every view draws past level 0, a character's levels (M5 D5, C11).
    row(SKINNING, "skinned instances above level 0", |c| u64::from(c.skinned_coarser)),
    // §3.4: 64 lights a desktop frame, 8 a phone's.
    CountRow {
        name: "lights",
        group: LIGHTS,
        read: |c| u64::from(c.lights),
        budget: Budget::Tiers([Some(64), Some(64), Some(8), Some(8)]),
    },
    row(LIGHTS, "lights dropped", |c| u64::from(c.lights_dropped)),
    // §3.4: a zone texture pool of 2.5 GB on the desktop, 600 MB on a phone and 400 MB on Mobile-Low, as
    // the tier table writes them (`texture_pool_mib`: 2,560, 2,560, 600 and 400 MiB).
    CountRow {
        group: ASSETS,
        name: "texture pool bytes",
        read: |c| c.texture_pool,
        budget: Budget::Tiers([Some(2_560 << 20), Some(2_560 << 20), Some(600 << 20), Some(400 << 20)]),
    },
    // Exit test 2: no pool holds more than its budget in use, on any tier.
    CountRow {
        group: ASSETS,
        name: "bytes over budget in use",
        read: |c| c.over_budget,
        budget: Budget::Tiers([Some(0); 4]),
    },
    row(ASSETS, "bytes admitted", |c| c.admitted),
    // Exit test 2's cap on what's decoded and waiting for the GPU: the asset server's admission, as the run holds
    // it (256 MiB on a desktop, 64 on a phone).
    CountRow {
        group: ASSETS,
        name: "bytes decoded waiting",
        read: |c| c.decoded_waiting,
        budget: Budget::Admission,
    },
    row(ASSETS, "reads held", |c| c.reads_held),
    // Exit test 2: nothing loads on the frame's thread, on any tier.
    CountRow {
        group: ASSETS,
        name: "frame thread loads",
        read: |c| c.frame_thread_loads,
        budget: Budget::Tiers([Some(0); 4]),
    },
    row(ASSETS, "texture bytes raised", |c| c.raised),
    row(ASSETS, "bytes given back", |c| c.given_back),
    row(ASSETS, "mesh pool capacity", |c| c.mesh_pool_capacity),
    row(ASSETS, "mesh pool floor", |c| c.mesh_pool_floor),
    row(ANIMATION, "characters animated", |c| u64::from(c.animated)),
    row(ANIMATION, "poses evaluated", |c| u64::from(c.posed)),
    row(ANIMATION, "poses held", |c| u64::from(c.held)),
    row(ANIMATION, "characters thinned", |c| u64::from(c.thinned)),
    row(ANIMATION, "held off screen", |c| u64::from(c.off_screen)),
    row(ANIMATION, "animation jobs", |c| u64::from(c.animation_jobs)),
    // The characters whose hair and cloth ran this frame (M5 C12).
    row(ANIMATION, "spring rigs stepped", |c| u64::from(c.springs_stepped)),
    row(CROWD, "ambient characters spawned", |c| u64::from(c.ambient_spawned)),
    row(CROWD, "ambient characters in view", |c| u64::from(c.ambient_in_view)),
    // Of those in view, by animation band, and those holding springs, which none may (M5 C11).
    row(CROWD, "ambient in the middle band", |c| u64::from(c.ambient_middle_band)),
    row(CROWD, "ambient in the far band", |c| u64::from(c.ambient_far_band)),
    row(CROWD, "ambient spring rigs", |c| u64::from(c.ambient_springs)),
    row(STREAMING, "textures in debt", |c| c.textures_in_debt),
    row(STREAMING, "worst texture debt frames", |c| c.worst_texture_debt),
    row(STREAMING, "texture levels raised", |c| c.levels_raised),
    row(STREAMING, "texture levels lowered", |c| c.levels_lowered),
    row(STREAMING, "texture lowers waiting", |c| c.lowers_waiting),
    // Exit test 2's pool: the one the run holds, `--texture-pool` where given (a binding pool), else the tier's.
    CountRow {
        group: STREAMING,
        name: "streamed texture bytes",
        read: |c| c.streamed_bytes,
        budget: Budget::TexturePool,
    },
    row(STREAMING, "streamed texture pool", |c| c.streamed_pool),
    row(STREAMING, "texture tail bytes", |c| c.texture_tails),
    row(STREAMING, "texture bytes wanted", |c| c.texture_wanted),
    row(CAPSULES, "shadow capsules", |c| u64::from(c.shadow_capsules)),
    row(CAPSULES, "ao capsules", |c| u64::from(c.occlusion_capsules)),
    row(CAPSULES, "capsule clusters marked", |c| u64::from(c.capsule_clusters)),
    row(CAPSULES, "capsule bits set", |c| u64::from(c.capsule_bits)),
    row(CAPSULES, "capsule bits in fullest cluster", |c| u64::from(c.capsule_fullest)),
];

/// A frame's counts, the tick it was drawn at and the metres walked by then.
type Counting<'a> = (&'a Counted, u64, Option<f32>);

/// Logs each count row over the frames of `phase` that ran their graph under `--draw-every <every>`, as a phase
/// of its own, `drawn`, as [`DrawnCounts`] does a walk's (M5 B5): a fight's bench under it times nothing, and a
/// frame that runs no graph submits nothing.
pub(super) fn report_drawn(trace: &[Traced], phase: Phase, every: u32, budgets: &RunBudgets) {
    let frames = || trace.iter().filter(move |frame| frame.phase == phase && frame.frame % every == 0);
    if frames().next().is_none() {
        return;
    }
    rows(|| frames().map(|frame| (&frame.counted, frame.tick, frame.walked)), "drawn", budgets);
}

/// Logs each count row over `phase`'s frames, and its frames' times by the kind of core they ran on.
pub(super) fn report(trace: &[Traced], phase: Phase, budgets: &RunBudgets) {
    let frames = || trace.iter().filter(move |frame| frame.phase == phase && frame.restart.is_zero());
    if frames().next().is_none() {
        return;
    }
    rows(|| frames().map(|frame| (&frame.counted, frame.tick, frame.walked)), phase.name(), budgets);
    for core in [Some(CoreKind::Performance), Some(CoreKind::Efficiency), None] {
        let on = || frames().filter(move |frame| frame.core == core);
        let samples = on().count();
        if samples == 0 {
            continue;
        }
        let cpu = spread(on().map(|frame| (frame.cpu, frame.tick))).expect("frames");
        let gpu = spread(on().filter_map(|frame| Some((frame.gpu.span?, frame.tick))));
        let micros = |duration: Duration| format!("{:.1}", duration.as_secs_f64() * 1e6);
        tracing::info!(
            target: LOG_TARGET,
            phase = phase.name(),
            core = core.map_or("unknown", CoreKind::name),
            samples,
            share = format_args!("{:.3}", samples as f64 / frames().count() as f64),
            cpu_us_p50 = micros(cpu.p50),
            cpu_us_p95 = micros(cpu.p95),
            cpu_us_worst = micros(cpu.worst),
            gpu_us_p50 = gpu.map(|gpu| micros(gpu.p50)),
            gpu_us_p95 = gpu.map(|gpu| micros(gpu.p95)),
            "frames by core",
        );
    }
}

/// Logs each count row over `frames`, with its total over them.
fn rows<'a, I: Iterator<Item = Counting<'a>>>(frames: impl Fn() -> I, phase: &str, budgets: &RunBudgets) {
    for row in &COUNT_ROWS {
        // Counts ride in a spread's Duration as nanoseconds: one per unit.
        let values = frames()
            .enumerate()
            .map(|(at, (counted, _, _))| (Duration::from_nanos((row.read)(counted)), at as u64));
        let Some(spread) = spread(values) else { continue };
        if spread.worst.is_zero() {
            tracing::info!(target: LOG_TARGET, phase, row = row.name, "count row not exercised");
            continue;
        }
        let (_, worst_tick, worst_walked) =
            frames().nth(spread.worst_tick as usize).expect("the spread's frame");
        let budget = row.budget.on(budgets);
        let count = |value: Duration| value.as_nanos() as u64;
        tracing::info!(
            target: LOG_TARGET,
            phase,
            row = row.name,
            frames = frames().count(),
            p50 = count(spread.p50),
            p95 = count(spread.p95),
            worst = count(spread.worst),
            total = frames().map(|(counted, _, _)| (row.read)(counted)).sum::<u64>(),
            worst_tick,
            worst_walked_m = worst_walked.map(|walked| format!("{walked:.1}")),
            budget = ?budget,
            over_p50 = budget.is_some_and(|budget| count(spread.p50) > budget),
            over_worst = budget.is_some_and(|budget| count(spread.worst) > budget),
            "count row",
        );
    }
}

/// What a plain bench of a walk drew, frame by frame (see the module docs): every frame's asset server is
/// read, for what the next admitted, and a drawn frame's counts kept. A drawn frame's loads on the frame's
/// thread are those since the last drawn frame, so none done on a frame that wasn't drawn goes uncounted.
pub(crate) struct DrawnCounts {
    frames: Vec<(Counted, u64, Option<f32>)>,
    assets: AssetStats,
    loads: u64,
}

impl DrawnCounts {
    /// Before the bench's first frame, with room for `frames` drawn.
    pub(crate) fn new(stage: &Stage, frames: usize) -> Self {
        let loads = sapphire_core::frame_loads::loads().total();
        Self { frames: Vec::with_capacity(frames), assets: stage.content.stats(), loads }
    }

    /// After frame `frame`, which ran its graph if `drawn`.
    pub(crate) fn frame(&mut self, stage: &Stage, frame: u64, drawn: bool) {
        let assets = stage.content.stats();
        if drawn {
            let streaming = stage.content.texture_streaming_stats();
            let loads = sapphire_core::frame_loads::loads().total();
            let counted = Counted {
                frame_thread_loads: loads - std::mem::replace(&mut self.loads, loads),
                ..Counted::of(&stage.renderer.stats(), &assets, &self.assets, &streaming).with_stage(stage)
            };
            self.frames.push((counted, frame, stage.scene.zone_walked()));
        }
        self.assets = assets;
    }

    /// Logs each count row over the frames drawn, as a phase of its own, `drawn`.
    pub(crate) fn log(&self, stage: &Stage) {
        let budgets = RunBudgets::of(stage);
        rows(
            || self.frames.iter().map(|(counted, tick, walked)| (counted, *tick, *walked)),
            "drawn",
            &budgets,
        );
    }
}

/// `--bench-trace`'s columns from `walked`.
pub(super) const TRACE_COLUMNS: &str = "walked,core,draw_calls,triangles,draw_instances,considered,culled_view,\
culled_pass,culled_cutoff,culled_distance,culled_size,lod0,lod1_up,main_draws,prepass_draws,shadow_draws,post_draws,ui_draws,skinned,joints,\
skinned_vertices,lights,admitted,decoded_waiting,reads_held,texture_pool,mesh_pool,over_budget,uploaded,raised,\
given_back,mesh_pool_capacity,mesh_pool_floor,ambient_dispatches,probe_dispatches,animated,posed,held,thinned,\
off_screen,animation_jobs,ambient_spawned,ambient_in_view,textures_in_debt,worst_texture_debt,levels_raised,\
levels_lowered,streamed_bytes,streaming_us,lowers_waiting,texture_wanted,raising_us,coverage_us,texture_starts";

/// A frame's `--bench-trace` columns, from `walked`.
pub(super) fn trace_into(frame: &Traced, line: &mut String) {
    use std::fmt::Write as _;
    match frame.walked {
        Some(walked) => {
            let _ = write!(line, ",{walked:.2}");
        }
        None => line.push(','),
    }
    let core = match frame.core {
        Some(CoreKind::Performance) => "p",
        Some(CoreKind::Efficiency) => "e",
        None => "",
    };
    let c = &frame.counted;
    let total = c.passes.total();
    let passes = &c.passes;
    let _ = write!(
        line,
        ",{core},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        total.draws,
        total.triangles,
        total.instances,
        total.considered,
        total.culled_by_frustum,
        total.culled_by_pass,
        total.culled_by_cutoff,
        total.culled_by_distance,
        total.culled_by_size,
        total.levels[0],
        total.levels[1..].iter().sum::<u32>(),
        passes.main.draws,
        passes.character_prepass.draws + passes.scene_prepass.draws,
        passes.shadows().draws,
        passes.post.draws + passes.temporal.draws + passes.upscale.draws,
        passes.ui.draws,
        c.skinned,
        c.joints,
        c.skinned_vertices,
        c.lights,
        c.admitted,
        c.decoded_waiting,
        c.reads_held,
        c.texture_pool,
        c.mesh_pool,
        c.over_budget,
        c.uploaded,
        c.raised,
        c.given_back,
        c.mesh_pool_capacity,
        c.mesh_pool_floor,
        c.ambient_dispatches,
        c.probe_dispatches,
        c.animated,
        c.posed,
        c.held,
        c.thinned,
        c.off_screen,
        c.animation_jobs,
        c.ambient_spawned,
        c.ambient_in_view,
    );
    let _ = write!(
        line,
        ",{},{},{},{},{},{},{},{},{},{},{}",
        c.textures_in_debt,
        c.worst_texture_debt,
        c.levels_raised,
        c.levels_lowered,
        c.streamed_bytes,
        c.streaming_us,
        c.lowers_waiting,
        c.texture_wanted,
        c.raising_us,
        c.coverage_us,
        c.texture_starts,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_rows_run_in_their_groups_order() {
        let mut seen: Vec<&str> = Vec::new();
        for row in &COUNT_ROWS {
            if seen.last() != Some(&row.group) {
                assert!(!seen.contains(&row.group), "{} comes back to {}", row.name, row.group);
                seen.push(row.group);
            }
        }
        assert_eq!(seen, GROUPS, "every group, in order, and none empty");
    }

    #[test]
    fn budgets_are_spec_section_3_4s_and_exit_test_2s_alone() {
        // Desktop-High, Desktop-Low, Mobile-High, Mobile-Low; nothing measured.
        let mib = |mib: u64| Some(mib << 20);
        let budgeted: Vec<(&str, Budget)> = COUNT_ROWS
            .iter()
            .filter(|row| row.budget != Budget::Tiers([None; 4]))
            .map(|row| (row.name, row.budget))
            .collect();
        assert_eq!(
            budgeted,
            [
                ("main draws", Budget::Tiers([Some(2_500), Some(2_500), Some(700), Some(700)])),
                ("lights", Budget::Tiers([Some(64), Some(64), Some(8), Some(8)])),
                ("texture pool bytes", Budget::Tiers([mib(2_560), mib(2_560), mib(600), mib(400)])),
                ("bytes over budget in use", Budget::Tiers([Some(0); 4])),
                ("bytes decoded waiting", Budget::Admission),
                ("frame thread loads", Budget::Tiers([Some(0); 4])),
                ("streamed texture bytes", Budget::TexturePool),
            ]
        );
        // The pool's figures are the tier table's, which writes §3.4's row.
        let pool = COUNT_ROWS.iter().find(|row| row.name == "texture pool bytes").expect("a row");
        let table = sapphire_render::quality::TierTable::shipped();
        for tier in table.tiers() {
            let run =
                RunBudgets { column: super::super::tier_column(tier.id), assets: AssetBudgets::DESKTOP };
            assert_eq!(pool.budget.on(&run), mib(u64::from(tier.texture_pool_mib)), "{:?}", tier.id);
        }
    }

    /// Exit test 2 under a binding pool (M5 Z2): what's decoded and waiting is held to the admission cap, and the
    /// streamed textures to the pool the run holds, both read from the run's asset server, never a copy.
    #[test]
    fn the_admission_cap_and_the_pool_are_the_runs_own() {
        use sapphire_core::target::FormFactor;
        let row = |name: &str| COUNT_ROWS.iter().find(|row| row.name == name).expect("a row").budget;
        let desktop = RunBudgets { column: 1, assets: AssetBudgets::for_form_factor(FormFactor::Desktop) };
        let phone = RunBudgets { column: 3, assets: AssetBudgets::for_form_factor(FormFactor::Mobile) };
        assert_eq!(row("bytes decoded waiting").on(&desktop), Some(256 << 20));
        assert_eq!(row("bytes decoded waiting").on(&phone), Some(64 << 20));
        let binding =
            RunBudgets { assets: phone.assets.with_resident(AssetKind::Texture, 331 << 20), ..phone };
        assert_eq!(row("streamed texture bytes").on(&binding), Some(331 << 20), "the run's --texture-pool");
        assert_eq!(
            row("streamed texture bytes").on(&desktop),
            Some(desktop.assets.resident(AssetKind::Texture))
        );
    }
}
