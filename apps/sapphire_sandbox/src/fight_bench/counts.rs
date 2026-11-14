//! What a bench of a fight or a walk counts beside its times (M5 B4): what each frame submitted, pass by pass
//! (`RenderStats::passes`), the ambient pass's compute dispatches and the reflection probes' among them
//! (M5 L3), what it skinned and lit, what the asset server held and let in, what animation posed, held and
//! thinned and in how many jobs (M5 C1), how many of a zone's ambient characters stood and how many of them
//! were in view (M5 C10), and the kind of core the frame's main thread was on.
//!
//! - **Counts need no rounds:** the same walk or fight draws the same frames, so a count row's spread is the
//!   scene's, not the machine's. Each is logged per phase with its 50th and 95th percentile, its worst
//!   frame, the tick and the metres walked it fell on, its total over the phase's frames, and spec §3.4's
//!   view budget where it has one (2,500 draws and 64 lights a desktop frame, 700 and 8 a phone's). A row
//!   every frame read zero of says `not exercised`: nothing on this walk or fight fed it (a fourth cascade
//!   on a tier of three, say).
//! - **A plain bench of a walk** (`--zone-walk` with `--draw-every <n>`, which `bench.rs` runs) logs the same
//!   rows over the frames it drew ([`DrawnCounts`], M5 X4): only every `n`th frame runs its graph, and one
//!   that doesn't submits nothing. They're what CI's scale lane holds on a software GPU (`cargo xtask
//!   counts`), and they time nothing.
//! - **Core kinds:** each frame samples which kind of core the main thread was on as its CPU work ended
//!   (`sapphire_platform::current_core`), and each phase logs its frames' CPU and GPU times split by kind,
//!   which is what explains a ×2.6–3 spread between runs of one build: a frame on an efficiency core.

use std::time::Duration;

use sapphire_assets::{AssetKind, AssetStats};
use sapphire_platform::CoreKind;
use sapphire_render::{FrameCounts, RenderStats, RunStats, StateChanges};
use sapphire_stage::TextureStreamingStats;

use super::{Phase, Traced, spread};
use crate::LOG_TARGET;
use crate::offscreen::Stage;

/// What a frame counted, beside its times.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Counted {
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
    /// A zone's ambient characters (M5 C10, `AmbientStats`): those standing, and of them those a view or
    /// its shadow shows.
    pub ambient_spawned: u32,
    pub ambient_in_view: u32,
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
}

impl Counted {
    /// These counts with the stage's own in its last frame: what animation did (`Scene::anim_stats`) and
    /// which of a zone's ambient characters stood (`Scene::ambient_stats`).
    pub(super) fn with_stage(self, stage: &Stage) -> Self {
        let anim = stage.scene.anim_stats();
        let ambient = stage.scene.ambient_stats();
        Self {
            animated: anim.characters,
            posed: anim.evaluated,
            held: anim.held,
            thinned: anim.thinned,
            off_screen: anim.off_screen,
            animation_jobs: anim.jobs,
            ambient_spawned: ambient.spawned,
            ambient_in_view: ambient.in_view,
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
    pub(super) fn of(
        render: &RenderStats,
        assets: &AssetStats,
        before: &AssetStats,
        streaming: &TextureStreamingStats,
    ) -> Self {
        let pool = |kind: AssetKind| -> u64 {
            AssetKind::ALL
                .into_iter()
                .filter(|&counted| sapphire_assets::AssetBudgets::pool(counted) == kind)
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
            ..Self::default()
        }
    }
}

/// A count the bench tracks per frame, and spec §3.4's budget for it on each tier, if any.
pub(super) struct CountRow {
    pub name: &'static str,
    pub read: fn(&Counted) -> u64,
    /// Desktop-High, Desktop-Low, Mobile-High, Mobile-Low.
    pub budget: [Option<u64>; 4],
}

const fn row(name: &'static str, read: fn(&Counted) -> u64) -> CountRow {
    CountRow { name, read, budget: [None; 4] }
}

/// What's logged, the frame's whole first, then pass by pass, the ambient pass's dispatches, then what
/// culled, the levels drawn (the main view's and the shadows' apart, and the main view's switches, M5
/// D4), the state the mesh passes changed, the instanced runs, skinning, lights, the asset server's,
/// animation's, a zone's crowd's, and the texture streamer's.
pub(super) const COUNT_ROWS: [CountRow; 71] = [
    // §3.4: a desktop frame's 2,500 draws, a phone's 700.
    CountRow {
        name: "draw calls",
        read: |c| u64::from(c.passes.total().draws),
        budget: [Some(2_500), Some(2_500), Some(700), Some(700)],
    },
    row("triangles", |c| c.passes.total().triangles),
    row("draw instances", |c| u64::from(c.passes.total().instances)),
    row("outline draws", |c| u64::from(c.passes.total().outline_draws)),
    row("main draws", |c| u64::from(c.passes.main.draws)),
    row("main triangles", |c| c.passes.main.triangles),
    row("character prepass draws", |c| u64::from(c.passes.character_prepass.draws)),
    row("scene prepass draws", |c| u64::from(c.passes.scene_prepass.draws)),
    row("cascade 0 draws", |c| u64::from(c.passes.cascades[0].draws)),
    row("cascade 1 draws", |c| u64::from(c.passes.cascades[1].draws)),
    row("cascade 2 draws", |c| u64::from(c.passes.cascades[2].draws)),
    row("cascade 3 draws", |c| u64::from(c.passes.cascades[3].draws)),
    row("shadow triangles", |c| c.passes.shadows().triangles),
    row("character map draws", |c| u64::from(c.passes.character_maps.draws)),
    row("inset draws", |c| c.passes.insets.iter().map(|inset| u64::from(inset.draws)).sum()),
    row("temporal draws", |c| u64::from(c.passes.temporal.draws)),
    row("post draws", |c| u64::from(c.passes.post.draws)),
    row("upscale draws", |c| u64::from(c.passes.upscale.draws)),
    row("ui draws", |c| u64::from(c.passes.ui.draws)),
    row("ambient dispatches", |c| u64::from(c.ambient_dispatches)),
    row("probe dispatches", |c| u64::from(c.probe_dispatches)),
    row("mesh draws considered", |c| u64::from(c.passes.total().considered)),
    row("culled by view", |c| u64::from(c.passes.total().culled_by_frustum)),
    row("culled by a pass", |c| u64::from(c.passes.total().culled_by_pass)),
    row("culled past outline cutoff", |c| u64::from(c.passes.total().culled_by_cutoff)),
    row("culled by distance", |c| u64::from(c.passes.total().culled_by_distance)),
    row("culled by size", |c| u64::from(c.passes.total().culled_by_size)),
    // Mesh instances by level, a run's each (M5 D2).
    row("lod0 draws", |c| u64::from(c.passes.total().levels[0])),
    row("lod1+ draws", |c| c.passes.total().levels[1..].iter().map(|&level| u64::from(level)).sum()),
    row("main draws above level 0", |c| {
        c.passes.main.levels[1..].iter().map(|&level| u64::from(level)).sum()
    }),
    row("shadow draws above level 0", |c| {
        c.passes.shadows().levels[1..].iter().map(|&level| u64::from(level)).sum()
    }),
    row("level switches", |c| u64::from(c.level_switches)),
    row("pipelines set", |c| u64::from(c.state_changes.pipelines)),
    row("materials bound", |c| u64::from(c.state_changes.materials)),
    row("vertex buffers bound", |c| u64::from(c.state_changes.vertex_buffers)),
    // Runs of more than one draw, every view list's (an undrawn scene prepass's too).
    row("runs", |c| u64::from(c.runs.runs)),
    row("run instances", |c| u64::from(c.runs.instances)),
    row("longest run", |c| u64::from(c.runs.longest)),
    row("median run", |c| u64::from(c.runs.median)),
    row("scene instances", |c| u64::from(c.instances)),
    row("skinned instances", |c| u64::from(c.skinned)),
    row("joints", |c| u64::from(c.joints)),
    row("skinned vertices", |c| u64::from(c.skinned_vertices)),
    // §3.4: 64 lights a desktop frame, 8 a phone's.
    CountRow {
        name: "lights",
        read: |c| u64::from(c.lights),
        budget: [Some(64), Some(64), Some(8), Some(8)],
    },
    row("lights dropped", |c| u64::from(c.lights_dropped)),
    row("texture pool bytes", |c| c.texture_pool),
    row("bytes over budget in use", |c| c.over_budget),
    row("bytes admitted", |c| c.admitted),
    row("bytes decoded waiting", |c| c.decoded_waiting),
    row("reads held", |c| c.reads_held),
    row("texture bytes raised", |c| c.raised),
    row("bytes given back", |c| c.given_back),
    row("mesh pool capacity", |c| c.mesh_pool_capacity),
    row("mesh pool floor", |c| c.mesh_pool_floor),
    row("characters animated", |c| u64::from(c.animated)),
    row("poses evaluated", |c| u64::from(c.posed)),
    row("poses held", |c| u64::from(c.held)),
    row("characters thinned", |c| u64::from(c.thinned)),
    row("held off screen", |c| u64::from(c.off_screen)),
    row("animation jobs", |c| u64::from(c.animation_jobs)),
    row("ambient characters spawned", |c| u64::from(c.ambient_spawned)),
    row("ambient characters in view", |c| u64::from(c.ambient_in_view)),
    row("textures in debt", |c| c.textures_in_debt),
    row("worst texture debt frames", |c| c.worst_texture_debt),
    row("texture levels raised", |c| c.levels_raised),
    row("texture levels lowered", |c| c.levels_lowered),
    row("texture lowers waiting", |c| c.lowers_waiting),
    row("streamed texture bytes", |c| c.streamed_bytes),
    row("streamed texture pool", |c| c.streamed_pool),
    row("texture tail bytes", |c| c.texture_tails),
    row("texture bytes wanted", |c| c.texture_wanted),
];

/// A frame's counts, the tick it was drawn at and the metres walked by then.
type Counting<'a> = (&'a Counted, u64, Option<f32>);

/// Logs each count row over `phase`'s frames, and its frames' times by the kind of core they ran on.
pub(super) fn report(trace: &[Traced], phase: Phase, column: usize) {
    let frames = || trace.iter().filter(move |frame| frame.phase == phase && frame.restart.is_zero());
    if frames().next().is_none() {
        return;
    }
    rows(|| frames().map(|frame| (&frame.counted, frame.tick, frame.walked)), phase.name(), column);
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
fn rows<'a, I: Iterator<Item = Counting<'a>>>(frames: impl Fn() -> I, phase: &str, column: usize) {
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
        let budget = row.budget[column];
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
/// read, for what the next admitted, and a drawn frame's counts kept.
pub(crate) struct DrawnCounts {
    frames: Vec<(Counted, u64, Option<f32>)>,
    assets: AssetStats,
}

impl DrawnCounts {
    /// Before the bench's first frame, with room for `frames` drawn.
    pub(crate) fn new(stage: &Stage, frames: usize) -> Self {
        Self { frames: Vec::with_capacity(frames), assets: stage.content.stats() }
    }

    /// After frame `frame`, which ran its graph if `drawn`.
    pub(crate) fn frame(&mut self, stage: &Stage, frame: u64, drawn: bool) {
        let assets = stage.content.stats();
        if drawn {
            let streaming = stage.content.texture_streaming_stats();
            let counted =
                Counted::of(&stage.renderer.stats(), &assets, &self.assets, &streaming).with_stage(stage);
            self.frames.push((counted, frame, stage.scene.zone_walked()));
        }
        self.assets = assets;
    }

    /// Logs each count row over the frames drawn, as a phase of its own, `drawn`.
    pub(crate) fn log(&self, stage: &Stage) {
        let column = super::tier_column(stage.quality.stats().tier.id);
        rows(|| self.frames.iter().map(|(counted, tick, walked)| (counted, *tick, *walked)), "drawn", column);
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
