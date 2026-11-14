//! What a bench of a fight or a walk counts beside its times (M5 B4): what each frame submitted, pass by pass
//! (`RenderStats::passes`), what it skinned and lit, what the asset server held and let in, and the kind of
//! core the frame's main thread was on.
//!
//! - **Counts need no rounds:** the same walk or fight draws the same frames, so a count row's spread is the
//!   scene's, not the machine's. Each is logged per phase with its 50th and 95th percentile and its worst
//!   frame, the tick and the metres walked it fell on, and spec §3.4's view budget where it has one (2,500
//!   draws and 64 lights a desktop frame, 700 and 8 a phone's). A row every frame read zero of says `not
//!   exercised`: nothing on this walk or fight fed it (a fourth cascade on a tier of three, say).
//! - **Core kinds:** each frame samples which kind of core the main thread was on as its CPU work ended
//!   (`sapphire_platform::current_core`), and each phase logs its frames' CPU and GPU times split by kind,
//!   which is what explains a ×2.6–3 spread between runs of one build: a frame on an efficiency core.

use std::time::Duration;

use sapphire_assets::{AssetKind, AssetStats};
use sapphire_platform::CoreKind;
use sapphire_render::{FrameCounts, RenderStats};

use super::{Phase, Traced, spread};
use crate::LOG_TARGET;

/// What a frame counted, beside its times.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Counted {
    pub passes: FrameCounts,
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
}

impl Counted {
    /// A frame's counts; `before` is the asset server's at the frame before, for what this one admitted.
    pub(super) fn of(render: &RenderStats, assets: &AssetStats, before: &AssetStats) -> Self {
        let pool = |kind: AssetKind| -> u64 {
            AssetKind::ALL
                .into_iter()
                .filter(|&counted| sapphire_assets::AssetBudgets::pool(counted) == kind)
                .map(|counted| assets.resident[counted.index()])
                .sum()
        };
        Self {
            passes: render.passes,
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

/// What's logged, the frame's whole first, then pass by pass, then what culled, the levels drawn, skinning,
/// lights, and the asset server's.
pub(super) const COUNT_ROWS: [CountRow; 39] = [
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
    row("ui draws", |c| u64::from(c.passes.ui.draws)),
    row("mesh draws considered", |c| u64::from(c.passes.total().considered)),
    row("culled by view", |c| u64::from(c.passes.total().culled_by_frustum)),
    row("culled by a pass", |c| u64::from(c.passes.total().culled_by_pass)),
    row("culled past outline cutoff", |c| u64::from(c.passes.total().culled_by_cutoff)),
    row("lod0 draws", |c| u64::from(c.passes.total().levels[0])),
    row("lod1+ draws", |c| c.passes.total().levels[1..].iter().map(|&level| u64::from(level)).sum()),
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
];

/// Logs each count row over `phase`'s frames, and its frames' times by the kind of core they ran on.
pub(super) fn report(trace: &[Traced], phase: Phase, column: usize) {
    let frames = || trace.iter().filter(move |frame| frame.phase == phase && frame.restart.is_zero());
    if frames().next().is_none() {
        return;
    }
    for row in &COUNT_ROWS {
        // Counts ride in a spread's Duration as nanoseconds: one per unit.
        let values = frames()
            .enumerate()
            .map(|(at, frame)| (Duration::from_nanos((row.read)(&frame.counted)), at as u64));
        let Some(spread) = spread(values) else { continue };
        if spread.worst.is_zero() {
            tracing::info!(target: LOG_TARGET, phase = phase.name(), row = row.name, "count row not exercised");
            continue;
        }
        let worst = frames().nth(spread.worst_tick as usize).expect("the spread's frame");
        let budget = row.budget[column];
        let count = |value: Duration| value.as_nanos() as u64;
        tracing::info!(
            target: LOG_TARGET,
            phase = phase.name(),
            row = row.name,
            p50 = count(spread.p50),
            p95 = count(spread.p95),
            worst = count(spread.worst),
            worst_tick = worst.tick,
            worst_walked_m = worst.walked.map(|walked| format!("{walked:.1}")),
            budget = ?budget,
            over_p50 = budget.is_some_and(|budget| count(spread.p50) > budget),
            over_worst = budget.is_some_and(|budget| count(spread.worst) > budget),
            "count row",
        );
    }
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

/// `--bench-trace`'s columns from `walked`.
pub(super) const TRACE_COLUMNS: &str = "walked,core,draw_calls,triangles,draw_instances,considered,culled_view,\
culled_pass,culled_cutoff,lod0,lod1_up,main_draws,prepass_draws,shadow_draws,post_draws,ui_draws,skinned,joints,\
skinned_vertices,lights,admitted,decoded_waiting,reads_held,texture_pool,mesh_pool,over_budget,uploaded,raised,\
given_back,mesh_pool_capacity,mesh_pool_floor";

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
        ",{core},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        total.draws,
        total.triangles,
        total.instances,
        total.considered,
        total.culled_by_frustum,
        total.culled_by_pass,
        total.culled_by_cutoff,
        total.levels[0],
        total.levels[1..].iter().sum::<u32>(),
        passes.main.draws,
        passes.character_prepass.draws + passes.scene_prepass.draws,
        passes.shadows().draws,
        passes.post.draws + passes.temporal.draws,
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
    );
}
