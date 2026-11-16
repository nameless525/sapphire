//! `--bench <frames>` over a fight (`--replay`, or `--fight --script`; M2 Z1): what a whole fight
//! costs, frame by frame, against spec §3.1, §3.2 and §3.3.
//!
//! A plain `--bench` warms up, measures, then times the GPU over whatever comes next, which for a fight
//! would be another stretch of it each time, and could be past its end. A fight has a shape, with hits,
//! freezes, slow motion and a fall, so this measures all of it:
//!
//! - **Phases:** `first`, from the frame after loading with no warm-up, so what a fight builds or grows
//!   the first time shows at its tick. `cpu`, the fight again with nothing to learn, for CPU times and
//!   allocations. `gpu`, once more with the frame graph's GPU timing on. Each plays whole fights until it
//!   has at least `<frames>` frames, so `--bench 1` plays one.
//! - **Frames as the tier has them:** [`steps_a_frame`] fixed steps a frame at the tier's own rate, two
//!   on Mobile-Low, with `Stage::fixed_step` and the frame's end around them, as the windowed app does.
//! - **A fight starting over** happens inside a frame, where a real one would: a replay's new
//!   [`Combat`] (which reads the file a fight with no file doesn't), or `Combat::restart`. The fight's
//!   checksums are held to the headless run's at the end of each.
//! - **Sound** runs through `OfflineAudio`: the real mixer and the fight's real cues, with no device, so
//!   §3.2's audio row is measured and no run that draws opens a device. The mix runs after the frame's
//!   CPU time, since a phone mixes on the audio callback.
//! - **What it can't see:** a window's acquire and present, vsync, events and input, and thermal limits.
//!   A Mac's numbers say nothing about a phone's.
//!
//! Each row of [`ROWS`] is logged for each phase with its 50th and 99th percentile, its worst frame, the
//! tick that frame fell on and the budget it has on this tier. `--bench-trace <csv>` writes every frame:
//! a spike's tick says which moment of the fight it was. Nothing it records allocates in a frame.
//!
//! `--draw-every` is the same load and the same fight with nearly no frames drawn, which checks a load's
//! pipeline inventory (exit test 7) where drawing costs seconds. It logs the pipelines' row, a zone's memory and
//! its trips' steps, which count frames and bytes rather than time them, and no other rows.
//!
//! **A walk through a zone** (M3 Z1) is a fight like any other here: `--arena --replay` of a replay whose
//! scenario walks one, such as `walk-the-ward`, or that goes between zones, such as `zone-trip`. §3.2's script
//! and streaming rows read as they do in a fight, and say `not exercised` where nothing gave them anything to
//! time; [`zone`] adds memory across the walk and each trip's steps. So is a zone stage's walk with no fight in
//! it (`--zone <stem> --zone-walk <rail>`, M5 B4): each phase walks the rail from its start, so `first` is the
//! tour cold and `cpu` and `gpu` the same tour again, and a row's worst frame says how far along it fell.
//!
//! [`mod@counts`] logs what each frame submitted, skinned, lit and loaded beside the times, and its frames split by
//! the kind of core they ran on (M5 B4).
//!
//! Rows the spec budgets on Desktop-Low and Mobile-Low only, as §3.2 does, have no budget on the other
//! two tiers; §3.3's carry M5 B4's proposed figures there, logged as `proposed_us`, which the exit run's report
//! shows and fails nothing on (M5 Z2). §3.3's skinning row is proposed on all four tiers. A grouped row (`gpu main`, `gpu prepasses`) is held to the sum of its
//! members' figures; Metal overlaps passes, so exclusive pass times are a Vulkan or DX12 GPU's. A row is per frame, so a Mobile-Low step's budget is doubled, as §3.2's "× 2 steps" says.

use std::fmt::Write as _;
use std::io::Write as _;
use std::num::NonZeroU32;
use std::path::Path;
use std::time::Duration;

use sapphire_core::alloc::{self, AllocScope};
use sapphire_ecs::prelude::WorldClock;
use sapphire_platform::{AppInitError, CoreKind};
use sapphire_render::graph::GpuTimings;
use sapphire_render::quality::TierId;
use sapphire_render::{
    CHARACTER_PREPASS, DISTORTION_PASS, MAIN_PASS, PREPASSES, UI_PASS, ambient, post, shadows, temporal, vfx,
};
use sapphire_rhi::wgpu;
use sapphire_time::{Stopwatch, TICK_HZ};

use crate::LOG_TARGET;
use crate::offscreen::Stage;
use crate::options::Options;
use game_combat::combat::Combat;
use game_combat::combat::CombatOption;
use game_combat::fight::trip::TripPhase;
use sapphire_stage::FrameLaps;
use sapphire_stage::audio::OfflineAudio;
use sapphire_stage::{LONG_WAIT, RERAISE_WITHIN, TextureStreamingStats, WAIT_BINS, WaitReason};

pub(crate) mod counts;
mod zone;

pub(crate) use counts::DrawnCounts;

/// What a phase measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    First,
    Cpu,
    Gpu,
}

impl Phase {
    const ALL: [Self; 3] = [Self::First, Self::Cpu, Self::Gpu];

    const fn name(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
        }
    }
}

/// A frame's GPU times, by the groups §3.3 budgets: each group's exclusive times, what it added to the
/// frame (`sapphire_render::graph::PassTime`). Apple's GPUs overlap passes, so only the span and the sum
/// of these say much on Metal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct GpuTimes {
    /// Whether this frame's timings came back.
    timed: bool,
    /// A complete timed span exists only when timestamps cover every graph pass.
    span: Option<Duration>,
    main: Option<Duration>,
    prepasses: Option<Duration>,
    character_prepass: Option<Duration>,
    shadows: Option<Duration>,
    skinning: Option<Duration>,
    sky_cube: Option<Duration>,
    /// Light clusters and the ambient pass: §3.3's "Light clustering, AO".
    clustering_ao: Option<Duration>,
    taa: Option<Duration>,
    /// The bloom chain and the composite: §3.3's "Bloom + tonemap + LUT".
    post: Option<Duration>,
    vfx: Option<Duration>,
    ui: Option<Duration>,
    /// When the frame's GPU work ended, on the GPU's own clock (`GpuTimings::end`), when its span reads: what
    /// lines a round's slow frames up against a display's refresh (M5 Z2).
    end: Option<Duration>,
}

impl GpuTimes {
    fn of(timings: &GpuTimings, bloom_levels: u32, bloom_chain: bool) -> Self {
        let every = |names: &[&str]| -> Option<Duration> {
            names.iter().try_fold(Duration::ZERO, |total, name| Some(total + timings.get(name)?.exclusive))
        };
        // On a frame whose every timed pass decoded, a pass that's absent and took no timestamps
        // didn't run: a shadow cascade not redrawn this frame costs nothing. One that ran untimed
        // leaves the total unknown, and so does a frame that dropped a pair.
        let sum = |names: &[&str]| -> Option<Duration> {
            if !timings.every_timed_pass {
                return every(names);
            }
            if names.iter().any(|name| timings.untimed.contains(name)) {
                return None;
            }
            let ran = names.iter().filter_map(|name| timings.get(name));
            ran.map(|pass| pass.exclusive).reduce(|total, exclusive| total + exclusive)
        };
        Self {
            timed: true,
            // On Apple GPUs a copy takes no timestamps, so the span is the timed passes' (M3 Z1).
            span: timings.every_timed_pass.then(|| timings.span()),
            main: sum(&[MAIN_PASS]),
            prepasses: sum(&PREPASSES),
            character_prepass: sum(&[CHARACTER_PREPASS]),
            shadows: sum(&shadows::PASSES),
            skinning: sum(&["skinning"]),
            sky_cube: sum(&["sky cube"]),
            // Either can sit a frame out: the clusters when the lights weren't binned again.
            clustering_ao: sum(&[ambient::PASSES[0], "light clusters"]),
            taa: sum(&temporal::PASSES),
            post: post::frame_passes(bloom_levels, bloom_chain)
                .try_fold(Duration::ZERO, |total, name| Some(total + timings.get(name)?.exclusive)),
            // §3.3's "Particles, distortion": the effects' pass and, in a frame with bends, their offsets (M5 L6).
            vfx: sum(&[vfx::PASS]).map(|effects| {
                effects + timings.get(DISTORTION_PASS).map_or(Duration::ZERO, |pass| pass.exclusive)
            }),
            ui: sum(&[UI_PASS]),
            end: timings.every_timed_pass.then(|| timings.end()).flatten(),
        }
    }
}

/// One frame of a fight, and what was going on in it.
#[derive(Clone, Copy, Debug)]
struct Traced {
    phase: Phase,
    /// Its place in the phase.
    frame: u32,
    /// Which of the phase's fights.
    fight: u32,
    /// The simulation's tick when it was drawn.
    tick: u64,
    steps: u32,
    /// Everything on the frame's CPU path, from its first step to its submit.
    cpu: Duration,
    /// Waiting for the GPU after that.
    wait: Duration,
    /// Starting a fight over, within [`Self::cpu`].
    restart: Duration,
    laps: FrameLaps,
    /// The audio system's update, within [`Self::cpu`].
    audio: Duration,
    /// The mix, outside it.
    mix: Duration,
    /// Both fighters' animation with its IK and springs, and within it their springs.
    animation: Duration,
    springs: Duration,
    /// Solves and joints it did.
    poses: u32,
    joints: u32,
    /// The camera's frame, within [`FrameLaps::packet`].
    camera: Duration,
    /// The HUD's, within it too.
    hud: Duration,
    /// The effects' simulation and frame write, in the cue steps and the packet.
    vfx: Duration,
    /// The zone's streaming, within [`FrameLaps::content`].
    streaming: Duration,
    /// A zone streams on the stage: what makes the streaming row's frames count.
    streams: bool,
    /// Deciding who of a zone's crowd stands, and standing them (M5 C10): `None` with no crowd loaded.
    ambient: Option<Duration>,
    /// The scripts' hooks and collection, within the steps, or `None` with no script host (M3 Z1).
    script: Option<Duration>,
    /// Zones on their way: the stage's part, then building their pipelines, within [`FrameLaps::content`].
    prefetch: Duration,
    /// Where the trip under way had got by the frame's end, if one was.
    trip: Option<TripPhase>,
    /// What the engine held after it, by its own accounting.
    memory: zone::Held,
    /// Allocations and bytes in the whole frame, the mix and the wait included.
    allocations: u64,
    bytes: u64,
    /// What's held on the heap after it.
    live_bytes: usize,
    /// Allocations on this thread inside its fixed steps, which rule R6 holds to zero with the cues'.
    step_allocations: u64,
    /// Of those, the simulation's own count of what its step allocated.
    sim_allocations: u64,
    sprites: u32,
    ribbon_points: u32,
    /// Cues read, effects spawned, sounds asked for and screen effects pushed in this frame's steps.
    cues: u64,
    effects: u64,
    sounds: u64,
    screens: u64,
    /// Screen effect layers playing.
    layers: u32,
    /// Ticks of hitstop left on each fighter.
    hitstop: [u16; 2],
    /// The simulation's world clock's speed: under 1 in a perfect dodge's slow motion.
    world_scale: f32,
    /// Post variants built on the frame they first drew, so far.
    post_first_use: u32,
    draws: u32,
    gpu: GpuTimes,
    /// Post work of this submitted frame, used when its delayed timestamps arrive.
    bloom_chain: bool,
    bloom_levels: u32,
    /// How far a zone stage's walk had gone along its rail, in metres, if there is one.
    walked: Option<f32>,
    /// The kind of core the main thread was on as the frame's CPU work ended, where the OS says.
    core: Option<CoreKind>,
    /// What the frame submitted, skinned, lit and loaded.
    counted: counts::Counted,
}

/// A number the bench tracks per frame, and the budget the spec gives it on each tier.
struct Row {
    name: &'static str,
    /// Read from the `gpu` phase, the rest from `first` and `cpu`.
    gpu: bool,
    read: fn(&Traced) -> Option<Duration>,
    /// In µs a frame: Desktop-High, Desktop-Low, Mobile-High, Mobile-Low.
    budget: [Option<u32>; 4],
    /// A figure proposed where the spec has adopted none (M5 B4's Desktop-High and Mobile-High columns of §3.3),
    /// logged beside the row as `proposed_us` for the report to show and fail nothing on. Adopting one is moving
    /// it into [`Self::budget`].
    proposed: [Option<u32>; 4],
}

const fn cpu_row(name: &'static str, read: fn(&Traced) -> Option<Duration>, budget: [Option<u32>; 4]) -> Row {
    Row { name, gpu: false, read, budget, proposed: [None; 4] }
}

const fn gpu_row(name: &'static str, read: fn(&Traced) -> Option<Duration>, budget: [Option<u32>; 4]) -> Row {
    Row { name, gpu: true, read, budget, proposed: [None; 4] }
}

impl Row {
    /// With B4's proposed Desktop-High and Mobile-High figures (spec §3.3, "Proposed columns and rows").
    const fn proposing(self, desktop_high: u32, mobile_high: u32) -> Self {
        Self { proposed: [Some(desktop_high), None, Some(mobile_high), None], ..self }
    }

    /// With a figure proposed on every tier, for a row the spec proposes whole (§3.3's skinning).
    const fn proposing_all(self, figures: [u32; 4]) -> Self {
        let [desktop_high, desktop_low, mobile_high, mobile_low] = figures;
        Self {
            proposed: [Some(desktop_high), Some(desktop_low), Some(mobile_high), Some(mobile_low)],
            ..self
        }
    }
}

/// §3.2's two columns, for the rows that have only those.
const fn low(desktop: u32, mobile: u32) -> [Option<u32>; 4] {
    [None, Some(desktop), None, Some(mobile)]
}

const UNBUDGETED: [Option<u32>; 4] = [None; 4];

/// What's logged, in the order of spec §3: the whole, §3.2's rows, what §3.2 has no row for, §3.3's.
const ROWS: [Row; 33] = [
    cpu_row("cpu frame", |t| Some(t.cpu), [Some(8_000), Some(12_000), Some(12_000), Some(22_000)]),
    cpu_row("game side", |t| Some(t.cpu.saturating_sub(t.laps.prepare + t.laps.record)), low(6_000, 13_600)),
    cpu_row("simulation", |t| Some(t.laps.simulation), low(1_000, 3_000)),
    cpu_row("physics", |t| Some(t.laps.physics), low(700, 2_400)),
    cpu_row("animation and ik", |t| Some(t.animation.saturating_sub(t.springs)), low(1_500, 3_000)),
    cpu_row("secondary motion", |t| Some(t.springs), low(500, 1_000)),
    cpu_row("vfx simulation", |t| Some(t.vfx), low(300, 600)),
    // A fight with no script host, or a frame with no zone streaming, doesn't exercise these, and says so.
    cpu_row("script events", |t| t.script, low(300, 500)),
    cpu_row("streaming activation", |t| t.streams.then_some(t.streaming), low(500, 1_000)),
    // A zone's crowd comes and goes with its cells, so it takes their budget too (M5 C10).
    cpu_row("ambient activation", |t| t.ambient, low(500, 1_000)),
    // The same row's bar for what texture streaming adds (M5 M5): the streamer and the coverage estimate.
    cpu_row("texture streaming", |t| Some(t.counted.texture_streaming()), low(500, 1_000)),
    cpu_row("audio update", |t| Some(t.audio), low(200, 300)),
    cpu_row("extract", |t| Some(t.laps.packet.saturating_sub(t.camera + t.hud)), low(700, 1_200)),
    cpu_row("render", |t| Some(t.laps.prepare + t.laps.record), low(4_000, 8_000)),
    cpu_row("cues", |t| Some(t.laps.cues), UNBUDGETED),
    cpu_row("scene step", |t| Some(t.laps.scene_step), UNBUDGETED),
    cpu_row("camera", |t| Some(t.camera), UNBUDGETED),
    cpu_row("hud", |t| Some(t.hud), UNBUDGETED),
    cpu_row("content", |t| Some(t.laps.content), UNBUDGETED),
    // `texture streaming`'s parts (M5 M5): the raises' share of the streamer, and the coverage estimate.
    cpu_row("texture raising", |t| Some(t.counted.texture_raising()), UNBUDGETED),
    cpu_row("texture coverage", |t| Some(t.counted.texture_coverage()), UNBUDGETED),
    // Only the frames a zone was on its way in, as a hub's exits come behind a trip.
    cpu_row("zone prefetch", |t| (!t.prefetch.is_zero()).then_some(t.prefetch), UNBUDGETED),
    cpu_row("gpu wait", |t| Some(t.wait), UNBUDGETED),
    gpu_row("gpu frame", |t| t.gpu.span, [Some(12_000), Some(14_000), Some(13_000), Some(26_000)]),
    // A grouped row is held to the sum of its members' figures (M5 Z2): culling and the prepasses.
    gpu_row("gpu prepasses", |t| t.gpu.prepasses, low(1_500, 1_500)).proposing(1_200, 1_000),
    gpu_row("gpu shadows", |t| t.gpu.shadows, low(2_000, 3_000)).proposing(2_000, 1_600),
    // §3.3 gives Mobile-Low none, and it runs neither.
    gpu_row("gpu clustering and ao", |t| t.gpu.clustering_ao, [None, Some(1_500), None, None])
        .proposing(1_200, 800),
    // §3.3's forward opaque, outlines, and hair, eyes and transparents all draw in the main pass: their sum,
    // 3.5 + 0.6 + 0.8 and 8.0 + 1.5 + 2.0 ms, and B4's 2.8 + 0.5 + 0.7 and 3.4 + 1.0 + 0.9.
    gpu_row("gpu main", |t| t.gpu.main, low(4_900, 11_500)).proposing(4_000, 5_300),
    gpu_row("gpu effects", |t| t.gpu.vfx, low(1_200, 3_000)).proposing(800, 1_000),
    gpu_row("gpu taa", |t| t.gpu.taa, low(800, 1_000)).proposing(900, 600),
    gpu_row("gpu post", |t| t.gpu.post, low(700, 2_500)).proposing(800, 1_200),
    gpu_row("gpu ui", |t| t.gpu.ui, low(300, 1_000)).proposing(200, 500),
    // Skinning and morphs, the whole cast's compute (§3.3's new row, M5 B4): a proposal on every tier, as the spec
    // says, so shown and judged on nothing (the orchestrator's ruling, M5 Z2): 0.4, 0.5, 0.6 and 1.0 ms.
    gpu_row("gpu skinning", |t| t.gpu.skinning, UNBUDGETED).proposing_all([400, 500, 600, 1_000]),
];

/// A row's place in [`Row::budget`].
pub(crate) const fn tier_column(tier: TierId) -> usize {
    match tier {
        TierId::DesktopHigh => 0,
        TierId::DesktopLow => 1,
        TierId::MobileHigh => 2,
        TierId::MobileLow => 3,
    }
}

/// How many fixed steps a frame takes at `rate` frames a second: at least one, as a tier that draws
/// faster than the simulation still does here.
pub(crate) fn steps_a_frame(rate: u32) -> u32 {
    TICK_HZ.checked_div(rate).unwrap_or(1).max(1)
}

/// The shortest a frame takes at `rate` frames a second, as whole fixed steps.
#[cfg(any(feature = "dev", test))]
pub(crate) fn frame_length(rate: u32) -> Duration {
    Duration::from_nanos(u64::from(steps_a_frame(rate)) * (1_000_000_000 / u64::from(TICK_HZ)))
}

/// A row's spread over some frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Spread {
    p50: Duration,
    p95: Duration,
    p99: Duration,
    worst: Duration,
    worst_tick: u64,
}

/// `values` by frame, with each one's tick. `None` for no frames.
fn spread(values: impl Iterator<Item = (Duration, u64)>) -> Option<Spread> {
    let mut sorted: Vec<(Duration, u64)> = values.collect();
    sorted.sort_unstable();
    let (worst, worst_tick) = *sorted.last()?;
    let at = |percent: usize| sorted[(sorted.len() - 1) * percent / 100].0;
    Some(Spread { p50: at(50), p95: at(95), p99: at(99), worst, worst_tick })
}

/// Whether `row` counts this frame in `phase`: a frame that started a fight over is reported on its own,
/// and GPU rows read only frames whose timings came back.
fn counts(row: &Row, frame: &Traced, phase: Phase) -> bool {
    frame.phase == phase
        && frame.restart.is_zero()
        && (!row.gpu || frame.gpu.timed)
        && (row.read)(frame).is_some()
}

/// What's mutable across the phases.
struct Run<'a> {
    stage: &'a mut Stage,
    view: &'a wgpu::TextureView,
    /// The fight, or `None` for a zone stage's walk.
    option: Option<&'a CombatOption>,
    audio: Option<OfflineAudio>,
    steps: u32,
    bloom_levels: u32,
    draw_every: Option<NonZeroU32>,
    trace: Vec<Traced>,
    /// The graph frame the phase's first frame is, as [`GpuTimings::frame`] counts.
    first_graph_frame: u64,
    /// Where the phase's frames start in [`Self::trace`].
    first_traced: usize,
    /// The newest graph frame whose timings came back.
    last_timed: u64,
    /// The asset server's counters after the last frame, for what the next one admitted.
    assets: sapphire_assets::AssetStats,
    /// The loads the frame's thread had done in play after the last frame (exit test 2), for what the next did.
    loads: u64,
    /// A walk under a running day (`--day-length`): the hour its tour starts at, which each phase's restart of
    /// the tour sets again, so every phase walks the same stretch of the day (M5 Z1). `None` where time stands
    /// still or for a fight, which nothing here changes.
    walk_hour: Option<f32>,
}

/// The render and compute pipelines the stage's device has: zero where its backend can't say.
fn pipelines(stage: &Stage) -> usize {
    stage.gpu.instance().raw().generate_report().map_or(0, |report| {
        report.hub.render_pipelines.num_kept_from_user + report.hub.compute_pipelines.num_kept_from_user
    })
}

/// Plays and measures the fight the stage holds, or walks its zone stage's rail (`option` `None`). See the
/// [module docs](self). `frames` is the least any phase measures, and `startup` what loading took.
pub(crate) fn run(
    stage: &mut Stage,
    option: Option<&CombatOption>,
    options: &Options,
    (frames, width, height): (u32, u32, u32),
    startup: Duration,
) -> Result<(), AppInitError> {
    let target = stage.target(width, height, wgpu::TextureUsages::empty());
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let rate = stage.quality.frame_rate().map_or(TICK_HZ, std::num::NonZero::get);
    let steps = steps_a_frame(rate);
    let settings = options.render_settings(stage.quality.render_settings());
    let audio = match stage.show.fight.show.cue_show().map(game_combat::cues::CueShow::sounds_to_load) {
        Some(sounds) => {
            Some(OfflineAudio::for_show(&sounds, &mut stage.content, &mut stage.renderer, &stage.gpu)?)
        }
        None => None,
    };
    // A fight's length, for what to reserve. A fight with no file measures just `frames`.
    let length = stage.show.fight.combat.as_ref().and_then(Combat::ticks_left).unwrap_or(0);
    let per_phase = u64::from(frames).max(length / u64::from(steps) + 1) as usize;
    let assets = stage.content.stats();
    // In play from here, the audio and the stage ready: what this thread loads from now on counts (exit test 2).
    let _in_play = sapphire_core::frame_loads::InPlay::begin();
    let loads = sapphire_core::frame_loads::loads().total();
    let mut run = Run {
        stage,
        view: &view,
        option,
        audio,
        steps,
        bloom_levels: settings.bloom_levels,
        draw_every: options.draw_every,
        trace: Vec::with_capacity(per_phase * 2 * Phase::ALL.len()),
        first_graph_frame: 1,
        first_traced: 0,
        last_timed: 0,
        assets,
        loads,
        walk_hour: None,
    };
    if option.is_none() && run.stage.scene.time_of_day().clock().day_length().is_some() {
        run.walk_hour = Some(run.stage.scene.time_of_day().clock().hours(1.0));
    }
    if let Some(every) = run.draw_every {
        run.phase(Phase::First, frames)?;
        zone::report(run.stage, &run.trace, Phase::First);
        counts::report_drawn(&run.trace, Phase::First, every.get(), &counts::RunBudgets::of(run.stage));
        if let Some(path) = &options.bench_trace {
            write_trace(path, &run.trace)
                .map_err(|error| AppInitError::msg(format!("--bench-trace {}: {error}", path.display())))?;
        }
        stage.lose_frames = false;
        stage.check_gpu()?;
        return stage.show.fight.finish().map_err(AppInitError::msg);
    }
    let mut measured = [0; 3];
    for (index, phase) in Phase::ALL.into_iter().enumerate() {
        if phase == Phase::Gpu && !run.stage.gpu.caps().timestamp_query {
            tracing::info!(target: LOG_TARGET, "no timestamp queries on this device: GPU times not measured");
            continue;
        }
        measured[index] = run.phase(phase, frames)?;
    }
    report(&run, options, (width, height), rate, startup);
    let trace = std::mem::take(&mut run.trace);
    if let Some(audio) = &run.audio {
        tracing::info!(target: LOG_TARGET, stats = ?audio.stats(), "the fight's sound, with no device");
    }
    if let Some(path) = &options.bench_trace {
        write_trace(path, &trace)
            .map_err(|error| AppInitError::msg(format!("--bench-trace {}: {error}", path.display())))?;
    }
    tracing::info!(target: LOG_TARGET, ?measured, "frames measured by phase: first, cpu, gpu");
    stage.check_gpu()?;
    stage.show.fight.finish().map_err(AppInitError::msg)
}

impl Run<'_> {
    /// Exit test 7, as a plain bench of a zone holds it too.
    fn check_first_uses(&self) -> Result<(), AppInitError> {
        crate::bench::check_first_uses(self.stage)
    }

    /// Plays whole fights until it's measured `frames`. Returns how many it did.
    fn phase(&mut self, phase: Phase, frames: u32) -> Result<u32, AppInitError> {
        self.stage.measure_gpu = phase == Phase::Gpu;
        self.first_graph_frame = self.stage.graph.frames() + 1;
        self.first_traced = self.trace.len();
        self.stage.content.start_texture_waits_over();
        let (mut played, mut fight, mut start_over) = (0_u32, 0_u32, phase != Phase::First);
        let at_start = pipelines(self.stage);
        let mut after_first_frame = at_start;
        let mut first_hour = 0.0;
        loop {
            self.stage.lose_frames = self.draw_every.is_some_and(|every| played % every != 0);
            let frame = self.frame(phase, played, fight, start_over)?;
            self.trace.push(frame);
            played += 1;
            self.read_gpu_times();
            if played == 1 {
                after_first_frame = pipelines(self.stage);
                first_hour = self.stage.scene.time_of_day().clock().hours(1.0);
            }
            self.check_first_uses()?;
            start_over = false;
            let over = self.stage.show.fight.combat.as_ref().and_then(Combat::ticks_left) == Some(0);
            let endless = self.stage.show.fight.combat.as_ref().and_then(Combat::ticks_left).is_none();
            if played >= frames && (over || endless) {
                // Anything built after a phase's first frame stalled the frame it was first drawn on.
                let builds = self.stage.renderer.stats().pipelines;
                tracing::info!(
                    target: LOG_TARGET,
                    phase = phase.name(),
                    at_start,
                    after_first_frame,
                    at_end = pipelines(self.stage),
                    prewarmed = builds.prewarmed,
                    first_use = builds.first_use_builds,
                    rebuilt = builds.rebuilt,
                    warmed_off_frame = builds.warmed_off_frame,
                    frame_thread = builds.frame_thread_builds,
                    frame_waits = builds.frame_waits,
                    first_hour = format_args!("{first_hour:.4}"),
                    "fight pipelines",
                );
                report_texture_waits(phase.name(), &self.stage.content.texture_streaming_stats());
                // Loads after the phase's last drawn frame go to it, so none goes unlogged.
                if let Some(every) = self.draw_every {
                    let loads = sapphire_core::frame_loads::loads().total();
                    let mut phase_frames =
                        self.trace.iter_mut().rev().take_while(|traced| traced.phase == phase);
                    if let Some(last) = phase_frames.find(|traced| traced.frame.is_multiple_of(every.get())) {
                        last.counted.frame_thread_loads += loads - std::mem::replace(&mut self.loads, loads);
                    }
                }
                return Ok(played);
            }
            if over {
                start_over = true;
                fight += 1;
            }
        }
    }

    /// One frame, its steps through its submit, and the wait for the GPU.
    fn frame(
        &mut self,
        phase: Phase,
        frame: u32,
        fight: u32,
        start_over: bool,
    ) -> Result<Traced, AppInitError> {
        let stage = &mut *self.stage;
        stage.laps = FrameLaps::default();
        // Checking the last one's checksums and logging is the bench's, not a frame's.
        if start_over && matches!(self.option, Some(CombatOption::Replay { .. })) {
            stage.show.fight.combat.take().map_or(Ok(()), Combat::finish).map_err(AppInitError::msg)?;
        }
        let cues_before =
            stage.show.fight.show.cue_show().map(game_combat::cues::CueShow::stats).unwrap_or_default();
        let sim_before =
            stage.show.fight.combat.as_ref().map_or(0, |combat| combat.simulation().stats().allocations);
        let script_before = stage.show.fight.combat.as_ref().and_then(Combat::script_time);
        let before = alloc::stats();
        let mut stopwatch = Stopwatch::start();
        let cpu = Stopwatch::start();
        let mut restart = Duration::ZERO;
        if start_over {
            match self.option {
                Some(option @ CombatOption::Replay { .. }) => {
                    let combat = stage.show.fight.prepare_stock_combat(option, &mut stage.content)?;
                    stage.show.fight.attach_prepared_combat(combat, &mut stage.scene, &stage.content)?;
                }
                Some(_) => {
                    if let Some(combat) = &mut stage.show.fight.combat {
                        combat.restart();
                    }
                }
                // A walk starts its tour again, at the hour it first started at if the day runs.
                None => {
                    stage.scene.restart_zone_walk();
                    if let Some(hour) = self.walk_hour {
                        stage.scene.time_of_day_mut().clock_mut().set_hours(hour);
                    }
                }
            }
            restart = stopwatch.lap();
        }
        let scope = AllocScope::begin();
        let mut steps = 0;
        for _ in 0..self.steps {
            if stage.show.fight.combat.as_ref().and_then(Combat::ticks_left) == Some(0) {
                break;
            }
            stage.fixed_step();
            steps += 1;
        }
        let step_allocations = scope.counts().allocations;
        let frame_dt = steps as f32 / TICK_HZ as f32;
        stopwatch.lap();
        if let Some(audio) = &mut self.audio {
            audio.update(frame_dt, stage.scene.sound(), stage.show.fight.show.cue_sounds());
        }
        let audio_time = stopwatch.lap();
        stage.show.fight.show.end_cue_frame(&mut stage.scene, frame_dt, frame_dt, 1.0, &mut ());
        stage.render(self.view, 1.0, |_, _| {})?;
        stage.scene.end_frame();
        let cpu = cpu.elapsed();
        let core = sapphire_platform::current_core();
        stopwatch.lap();
        stage.gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
        let wait = stopwatch.lap();
        let mix = self.audio.as_mut().map_or(Duration::ZERO, |audio| audio.mix(frame_dt));
        let after = alloc::stats();

        let anim = stage.scene.anim_stats();
        let ambient = stage.scene.ambient_stats();
        let stats = stage.renderer.stats();
        let cues =
            stage.show.fight.show.cue_show().map(game_combat::cues::CueShow::stats).unwrap_or_default();
        let (tick, sim_allocations, hitstop, world_scale) = match &stage.show.fight.combat {
            Some(combat) => {
                let sim = combat.simulation();
                let mut hitstop = [0; 2];
                for (held, id) in hitstop.iter_mut().zip(combat.cast()) {
                    *held = sim.actor(*id).map_or(0, |actor| actor.hitstop_ticks);
                }
                let scale = sim.world().resource::<WorldClock>().0.effective_scale();
                (sim.tick(), sim.stats().allocations.wrapping_sub(sim_before), hitstop, scale)
            }
            None => (0, 0, [0; 2], 1.0),
        };
        let layers = stage.scene.with_screen_effects(|stack, _| stack.len()) as u32;
        let script = stage.show.fight.combat.as_ref().and_then(Combat::script_time);
        let script = script.map(|after| after.saturating_sub(script_before.unwrap_or(Duration::ZERO)));
        let memory = zone::Held::of(stage, after.live_bytes);
        let assets = stage.content.stats();
        let streaming = stage.content.texture_streaming_stats();
        // Under `--draw-every` only drawn frames' counts are logged ([`counts::report_drawn`]): one takes the loads
        // since the last, so none on a frame that wasn't drawn goes uncounted.
        let drawn = self.draw_every.is_none_or(|every| frame.is_multiple_of(every.get()));
        let loads = sapphire_core::frame_loads::loads().total();
        let counted = counts::Counted {
            frame_thread_loads: if drawn { loads - std::mem::replace(&mut self.loads, loads) } else { 0 },
            ..counts::Counted::of(&stats, &assets, &self.assets, &streaming).with_stage(stage)
        };
        self.assets = assets;
        Ok(Traced {
            phase,
            frame,
            fight,
            tick,
            steps,
            cpu,
            wait,
            restart,
            laps: stage.laps,
            audio: audio_time,
            mix,
            animation: anim.time,
            springs: anim.spring_time,
            poses: anim.evaluated,
            joints: anim.joints,
            camera: stage.show.fight.show.camera_time(),
            hud: stage.show.hud.as_ref().map_or(Duration::ZERO, |hud| hud.last_frame().1),
            vfx: stats.vfx.simulation.simulation,
            streaming: stage.scene.stream_stats().map_or(Duration::ZERO, |stream| stream.time),
            streams: stage.scene.stream_stats().is_some(),
            ambient: (ambient.people > 0).then_some(ambient.activation),
            script,
            prefetch: stage.scene.prefetch_time() + stage.content.zone_prewarm_time(),
            trip: stage.show.fight.show.trip_phase(),
            memory,
            allocations: after.total_allocations - before.total_allocations,
            bytes: after.total_bytes - before.total_bytes,
            live_bytes: after.live_bytes,
            step_allocations,
            sim_allocations,
            sprites: stats.vfx.sprites,
            ribbon_points: stats.vfx.ribbon_points,
            cues: cues.cues - cues_before.cues,
            effects: cues.effects - cues_before.effects,
            sounds: cues.sounds - cues_before.sounds,
            screens: cues.screens - cues_before.screens,
            layers,
            hitstop,
            world_scale,
            post_first_use: stats.post.first_use_builds,
            draws: stats.draws,
            gpu: GpuTimes::default(),
            bloom_chain: stats.post.bloom_chain,
            bloom_levels: self.bloom_levels,
            walked: stage.scene.zone_walked(),
            core,
            counted,
        })
    }

    /// Puts the newest GPU times that have come back on the frame they belong to.
    fn read_gpu_times(&mut self) {
        let Some(timings) = self.stage.graph.gpu_timings() else { return };
        if timings.frame <= self.last_timed || timings.frame < self.first_graph_frame {
            return;
        }
        self.last_timed = timings.frame;
        let at = self.first_traced + (timings.frame - self.first_graph_frame) as usize;
        if let Some(frame) = self.trace.get_mut(at) {
            frame.gpu = GpuTimes::of(timings, frame.bloom_levels, frame.bloom_chain);
        }
    }
}

/// Logs each phase's rows, what the fight's frames allocated, and what a start-over took.
fn report(run: &Run<'_>, options: &Options, (width, height): (u32, u32), rate: u32, startup: Duration) {
    let micros = |duration: Duration| duration.as_secs_f64() * 1e6;
    let quality = run.stage.quality.stats();
    let (scene_width, scene_height) = run.stage.renderer.stats().scene_size;
    let column = tier_column(quality.tier.id);
    tracing::info!(
        target: LOG_TARGET,
        tier = quality.tier.id.name(),
        width,
        height,
        scene = format_args!("{scene_width}x{scene_height}"),
        rate,
        steps_a_frame = run.steps,
        rung = quality.ladder.state.depth,
        arena = ?options.arena,
        startup_ms = format_args!("{:.1}", startup.as_secs_f64() * 1e3),
        graph = ?run.stage.graph.stats(),
        render = ?run.stage.renderer.stats(),
        "bench of a fight",
    );
    for phase in Phase::ALL {
        let frames = || run.trace.iter().filter(move |frame| frame.phase == phase);
        let measured = || frames().filter(|frame| frame.restart.is_zero());
        for row in ROWS.iter().filter(|row| row.gpu == (phase == Phase::Gpu)) {
            // Each value with its frame's place in the phase, which names the worst frame's tick and where a
            // walk was.
            let values = frames()
                .enumerate()
                .filter(|(_, frame)| counts(row, frame, phase))
                .filter_map(|(at, frame)| Some(((row.read)(frame)?, at as u64)));
            let Some(spread) = spread(values) else {
                // Not 0: nothing on this walk or fight gave it anything to time.
                tracing::info!(target: LOG_TARGET, phase = phase.name(), row = row.name, "fight row not exercised");
                continue;
            };
            let (budget, proposed) = (row.budget[column], row.proposed[column]);
            let worst = frames().nth(spread.worst_tick as usize).expect("the spread's frame");
            tracing::info!(
                target: LOG_TARGET,
                phase = phase.name(),
                row = row.name,
                samples = frames().filter(|frame| counts(row, frame, phase)).count(),
                p50_us = format_args!("{:.1}", micros(spread.p50)),
                p95_us = format_args!("{:.1}", micros(spread.p95)),
                p99_us = format_args!("{:.1}", micros(spread.p99)),
                worst_us = format_args!("{:.1}", micros(spread.worst)),
                worst_tick = worst.tick,
                worst_walked_m = worst.walked.map(|walked| format!("{walked:.1}")),
                budget_us = ?budget,
                proposed_us = ?proposed,
                over_p50 = budget.is_some_and(|budget| micros(spread.p50) > f64::from(budget)),
                over_worst = budget.is_some_and(|budget| micros(spread.worst) > f64::from(budget)),
                "fight row",
            );
        }
        let count = measured().count().max(1) as u64;
        let allocations =
            spread(measured().map(|frame| (Duration::from_nanos(frame.allocations), frame.tick)));
        tracing::info!(
            target: LOG_TARGET,
            phase = phase.name(),
            frames = frames().count(),
            allocations_per_frame = measured().map(|frame| frame.allocations).sum::<u64>() / count,
            allocations_worst = allocations.map_or(0, |spread| spread.worst.as_nanos() as u64),
            allocations_worst_tick = allocations.map_or(0, |spread| spread.worst_tick),
            bytes_per_frame = measured().map(|frame| frame.bytes).sum::<u64>() / count,
            step_allocations = frames().map(|frame| frame.step_allocations).sum::<u64>(),
            sim_allocations = frames().map(|frame| frame.sim_allocations).sum::<u64>(),
            restart_step_allocations = frames().filter(|frame| !frame.restart.is_zero()).map(|frame| frame.step_allocations).sum::<u64>(),
            restart_us = format_args!(
                "{:.1}",
                frames().map(|frame| micros(frame.restart)).fold(0.0, f64::max)
            ),
            restart_frame_us = format_args!(
                "{:.1}",
                frames().filter(|frame| !frame.restart.is_zero()).map(|frame| micros(frame.cpu)).fold(0.0, f64::max)
            ),
            timed_frames = frames().filter(|frame| frame.gpu.timed).count(),
            post_first_use = frames().map(|frame| frame.post_first_use).max().unwrap_or(0),
            mix_us_p50 = format_args!(
                "{:.1}",
                spread(frames().map(|frame| (frame.mix, frame.tick))).map_or(0.0, |spread| micros(spread.p50))
            ),
            sprites_worst = frames().map(|frame| frame.sprites).max().unwrap_or(0),
            ribbon_points_worst = frames().map(|frame| frame.ribbon_points).max().unwrap_or(0),
            "fight phase",
        );
        zone::report(run.stage, &run.trace, phase);
        counts::report(&run.trace, phase, &counts::RunBudgets::of(run.stage));
    }
}

/// A phase's waits for a texture's level, while streaming (m5-plan exit test 2), from the phase's start: every
/// wait completed in it, of both kinds, with their 50th and 99th percentiles and the worst, in frames; the
/// pool's share of them; and those open at its end, with the longest. A wait open at a phase's start lands
/// in the phase it ends in, with its whole length.
pub(crate) fn report_texture_waits(phase: &'static str, stats: &TextureStreamingStats) {
    if !stats.streaming {
        return;
    }
    let line = WaitsLine::of(phase, stats);
    tracing::info!(
        target: LOG_TARGET,
        phase = line.phase,
        completed = line.completed,
        p50_frames = %line.p50,
        p99_frames = %line.p99,
        worst_frames = %line.worst,
        pool_held = line.pool_held,
        pool_held_p99_frames = %line.pool_p99,
        pool_held_worst_frames = %line.pool_worst,
        open = line.open,
        longest_open_frames = line.longest_open,
        pool_open = line.pool_open,
        pool_longest_open_frames = line.pool_longest_open,
        "texture waits",
    );
    // Beside it, what held the waits longest, the churn and the arrival's waits (M5 Z2): read, never judged.
    let mut reasons = String::new();
    for reason in WaitReason::ALL {
        let index = reason as usize;
        let (waited, long) = (stats.reasons.waited[index], stats.reasons.long[index]);
        let _ = write!(reasons, " {}={waited} {}_long={long}", reason.name(), reason.name());
    }
    tracing::info!(
        target: LOG_TARGET,
        phase,
        long_over_frames = LONG_WAIT,
        raised_bytes = stats.raised_bytes,
        reraised_bytes = stats.reraised_bytes,
        reraised_within_updates = RERAISE_WITHIN,
        tails_over_pool = stats.tails_over_pool,
        // Opened before the zone arrived: the arrival's load, under the game's wipe, apart from the line above.
        arrival_waits = stats.arrival.completed,
        arrival_long = stats.arrival.long,
        arrival_open = stats.arrival.open,
        arrived_at_update = stats.arrival.arrived_at,
        arrival_held_to_bound = stats.arrival.held_to_bound,
        "texture wait reasons{reasons}",
    );
}

/// What a phase's `texture waits` line says.
#[derive(Debug, PartialEq, Eq)]
struct WaitsLine {
    phase: &'static str,
    completed: u32,
    p50: String,
    p99: String,
    worst: String,
    pool_held: u32,
    pool_p99: String,
    pool_worst: String,
    open: u32,
    longest_open: u32,
    pool_open: u32,
    pool_longest_open: u32,
}

impl WaitsLine {
    fn of(phase: &'static str, stats: &TextureStreamingStats) -> Self {
        let (all, pool) = (stats.all_waits(), stats.pool_waits);
        Self {
            phase,
            completed: all.completed(),
            p50: wait_frames(all.percentile(0.5)),
            p99: wait_frames(all.percentile(0.99)),
            worst: wait_frames(all.worst()),
            pool_held: pool.completed(),
            pool_p99: wait_frames(pool.percentile(0.99)),
            pool_worst: wait_frames(pool.worst()),
            open: all.open,
            longest_open: all.longest_open,
            pool_open: pool.open,
            pool_longest_open: pool.longest_open,
        }
    }
}

/// A bin of [`sapphire_stage::TextureWaits`] in frames: the last reads `1024+`, none `-`.
fn wait_frames(bin: Option<u32>) -> String {
    match bin {
        None => "-".to_owned(),
        Some(bin) if bin as usize == WAIT_BINS - 1 => format!("{bin}+"),
        Some(bin) => bin.to_string(),
    }
}

/// The header of `--bench-trace`'s file: times in µs.
const TRACE_HEADER: &str = "phase,frame,fight,tick,steps,cpu,wait,restart,scene_step,simulation,physics,cues,\
audio,mix,content,animation,springs,poses,joints,packet,camera,hud,vfx,streaming,prepare,record,allocations,bytes,\
live_bytes,step_allocations,sim_allocations,animation_allocations,packet_allocations,packet_bytes,prepare_allocations,\
prepare_bytes,record_allocations,record_bytes,sprites,ribbon_points,cues_read,effects,sounds,screens,layers,\
hitstop_player,hitstop_enemy,world_scale,post_first_use,draws,gpu_timed,gpu_span,gpu_main,gpu_prepasses,\
gpu_character_prepass,gpu_shadows,gpu_skinning,gpu_sky_cube,gpu_clustering_ao,gpu_taa,gpu_post,gpu_vfx,gpu_ui,gpu_end,\
script,prefetch,trip,resident,mesh_pool,textures,entities,process,";

/// One frame's line of `--bench-trace`'s file.
fn trace_line_into(frame: &Traced, line: &mut String) {
    line.clear();
    let _ =
        write!(line, "{},{},{},{},{}", frame.phase.name(), frame.frame, frame.fight, frame.tick, frame.steps);
    let mut times = |times: &[Duration]| {
        for time in times {
            let _ = write!(line, ",{:.1}", time.as_secs_f64() * 1e6);
        }
    };
    let laps = &frame.laps;
    times(&[
        frame.cpu,
        frame.wait,
        frame.restart,
        laps.scene_step,
        laps.simulation,
        laps.physics,
        laps.cues,
        frame.audio,
        frame.mix,
        laps.content,
        frame.animation,
        frame.springs,
    ]);
    let _ = write!(line, ",{},{}", frame.poses, frame.joints);
    let mut times = |times: &[Duration]| {
        for time in times {
            let _ = write!(line, ",{:.1}", time.as_secs_f64() * 1e6);
        }
    };
    times(&[laps.packet, frame.camera, frame.hud, frame.vfx, frame.streaming, laps.prepare, laps.record]);
    let _ = write!(
        line,
        ",{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{:.4},{},{},{}",
        frame.allocations,
        frame.bytes,
        frame.live_bytes,
        frame.step_allocations,
        frame.sim_allocations,
        laps.animation_allocations.allocations,
        laps.packet_allocations.allocations,
        laps.packet_allocations.bytes,
        laps.prepare_allocations.allocations,
        laps.prepare_allocations.bytes,
        laps.record_allocations.allocations,
        laps.record_allocations.bytes,
        frame.sprites,
        frame.ribbon_points,
        frame.cues,
        frame.effects,
        frame.sounds,
        frame.screens,
        frame.layers,
        frame.hitstop[0],
        frame.hitstop[1],
        frame.world_scale,
        frame.post_first_use,
        frame.draws,
        u8::from(frame.gpu.timed),
    );
    let gpu = &frame.gpu;
    for time in [
        gpu.span,
        gpu.main,
        gpu.prepasses,
        gpu.character_prepass,
        gpu.shadows,
        gpu.skinning,
        gpu.sky_cube,
        gpu.clustering_ao,
        gpu.taa,
        gpu.post,
        gpu.vfx,
        gpu.ui,
        gpu.end,
    ] {
        match time {
            Some(time) => {
                let _ = write!(line, ",{:.1}", time.as_secs_f64() * 1e6);
            }
            None => line.push(','),
        }
    }
    match frame.script {
        Some(time) => {
            let _ = write!(line, ",{:.1}", time.as_secs_f64() * 1e6);
        }
        None => line.push(','),
    }
    let trip = frame.trip.map_or("", |trip| match trip {
        TripPhase::Covering => "covering",
        TripPhase::Swapping => "swapping",
        TripPhase::Arriving => "arriving",
        TripPhase::Uncovering => "uncovering",
    });
    let _ = write!(line, ",{:.1},{trip},", frame.prefetch.as_secs_f64() * 1e6);
    frame.memory.trace_into(line);
    counts::trace_into(frame, line);
}

#[cfg(test)]
fn trace_line(frame: &Traced) -> String {
    let mut line = String::new();
    trace_line_into(frame, &mut line);
    line
}

fn write_trace(path: &Path, trace: &[Traced]) -> std::io::Result<()> {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(file, "{TRACE_HEADER}{}", counts::TRACE_COLUMNS)?;
    let mut line = String::with_capacity(2048);
    for frame in trace {
        trace_line_into(frame, &mut line);
        writeln!(file, "{line}")?;
    }
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn traced(tick: u64, cpu_us: u64) -> Traced {
        Traced {
            phase: Phase::Cpu,
            frame: 0,
            fight: 0,
            tick,
            steps: 1,
            cpu: Duration::from_micros(cpu_us),
            wait: Duration::ZERO,
            restart: Duration::ZERO,
            laps: FrameLaps::default(),
            audio: Duration::ZERO,
            mix: Duration::ZERO,
            animation: Duration::ZERO,
            springs: Duration::ZERO,
            poses: 0,
            joints: 0,
            camera: Duration::ZERO,
            hud: Duration::ZERO,
            vfx: Duration::ZERO,
            streaming: Duration::ZERO,
            streams: false,
            script: None,
            ambient: None,
            prefetch: Duration::ZERO,
            trip: None,
            memory: zone::Held::default(),
            allocations: 0,
            bytes: 0,
            live_bytes: 0,
            step_allocations: 0,
            sim_allocations: 0,
            sprites: 0,
            ribbon_points: 0,
            cues: 0,
            effects: 0,
            sounds: 0,
            screens: 0,
            layers: 0,
            hitstop: [0; 2],
            world_scale: 1.0,
            post_first_use: 0,
            draws: 0,
            gpu: GpuTimes::default(),
            bloom_chain: false,
            bloom_levels: 0,
            walked: None,
            core: None,
            counted: counts::Counted::default(),
        }
    }

    #[test]
    fn a_tier_takes_as_many_steps_as_its_rate_leaves_room_for() {
        assert_eq!(steps_a_frame(60), 1);
        assert_eq!(steps_a_frame(30), 2);
        assert_eq!(steps_a_frame(120), 1, "a tier that draws faster still steps");
        assert_eq!(steps_a_frame(0), 1);
        assert_eq!(frame_length(60), Duration::from_nanos(16_666_666));
        assert_eq!(frame_length(30), Duration::from_nanos(33_333_332));
    }

    /// A row's worst frame keeps its tick, which is what names a spike.
    #[test]
    fn a_spread_names_the_tick_of_its_worst_frame() {
        let frames = [(3, 10), (9, 90), (5, 50), (1, 30), (7, 70)].map(|(tick, us)| traced(tick, us));
        let spread = spread(frames.iter().map(|frame| (frame.cpu, frame.tick))).expect("frames");
        assert_eq!(spread.worst, Duration::from_micros(90));
        assert_eq!(spread.worst_tick, 9);
        assert_eq!(spread.p50, Duration::from_micros(50));
        assert_eq!(spread.p99, Duration::from_micros(70), "the rank below the last, of five");
        assert_eq!(super::spread(std::iter::empty()), None);
    }

    /// The frame that starts a fight over is reported on its own, so a restart reads as what it is
    /// rather than as the fight's worst frame, and GPU rows skip frames whose timings never came back.
    #[test]
    fn a_rows_frames_leave_out_restarts_and_untimed_frames() {
        let cpu = &ROWS[0];
        let gpu = ROWS.iter().find(|row| row.gpu).expect("a GPU row");
        let frame = traced(1, 10);
        assert!(counts(cpu, &frame, Phase::Cpu));
        assert!(!counts(cpu, &frame, Phase::Gpu), "another phase's");
        assert!(!counts(cpu, &Traced { restart: Duration::from_micros(1), ..frame }, Phase::Cpu));
        assert!(!counts(gpu, &frame, Phase::Cpu), "never timed");
        let timed = Traced {
            gpu: GpuTimes { timed: true, span: Some(Duration::ZERO), ..GpuTimes::default() },
            ..frame
        };
        assert!(counts(gpu, &timed, Phase::Cpu));
    }

    /// Spec §3.2's rows as they're held: a Mobile-Low frame's budget for a step is two steps'.
    #[test]
    fn the_budgets_are_the_specs() {
        let budget = |name: &str, tier| {
            let row = ROWS.iter().find(|row| row.name == name).expect("a row");
            row.budget[tier_column(tier)]
        };
        assert_eq!(budget("simulation", TierId::DesktopLow), Some(1_000));
        assert_eq!(budget("simulation", TierId::MobileLow), Some(3_000));
        assert_eq!(budget("physics", TierId::MobileLow), Some(2_400));
        assert_eq!(budget("secondary motion", TierId::MobileLow), Some(1_000));
        assert_eq!(budget("cpu frame", TierId::MobileLow), Some(22_000));
        assert_eq!(budget("gpu frame", TierId::MobileLow), Some(26_000));
        assert_eq!(budget("gpu frame", TierId::DesktopHigh), Some(12_000));
        assert_eq!(budget("simulation", TierId::DesktopHigh), None, "§3.2 has no column for it");
        // B4's proposed columns are logged beside a row and never as its budget.
        let proposed = |name: &str, tier| {
            let row = ROWS.iter().find(|row| row.name == name).expect("a row");
            (row.budget[tier_column(tier)], row.proposed[tier_column(tier)])
        };
        assert_eq!(proposed("gpu main", TierId::DesktopHigh), (None, Some(4_000)));
        assert_eq!(proposed("gpu main", TierId::MobileHigh), (None, Some(5_300)));
        // §3.3's skinning row is a proposal on all four tiers: shown, and no tier's budget.
        let skinning = TierId::ALL.map(|tier| proposed("gpu skinning", tier));
        assert_eq!(skinning, [(None, Some(400)), (None, Some(500)), (None, Some(600)), (None, Some(1_000))]);
        assert_eq!(
            proposed("gpu frame", TierId::DesktopHigh),
            (Some(12_000), None),
            "the frame's is adopted"
        );
        assert!(
            ROWS.iter().all(|row| (0..4).all(|at| row.budget[at].is_none() || row.proposed[at].is_none()))
        );
    }

    #[test]
    fn post_totals_require_all_passes_of_the_measured_frame() {
        use sapphire_render::graph::PassTime;
        let names: Vec<_> = post::frame_passes(3, true).collect();
        let mut timings = GpuTimings {
            frame: 7,
            every_pass: true,
            every_timed_pass: false,
            untimed: Vec::new(),
            start: None,
            passes: names
                .iter()
                .enumerate()
                .map(|(index, &name)| PassTime {
                    name,
                    start: Duration::from_micros(index as u64),
                    duration: Duration::from_micros(1),
                    exclusive: Duration::from_micros(1),
                })
                .collect(),
        };
        let frame = Traced { bloom_chain: true, bloom_levels: 3, ..traced(7, 1) };
        let complete = GpuTimes::of(&timings, frame.bloom_levels, frame.bloom_chain);
        assert_eq!(complete.post, Some(Duration::from_micros(names.len() as u64)));
        assert_eq!(complete.main, None, "an absent pass is not zero time");
        timings.passes.remove(0);
        assert_eq!(GpuTimes::of(&timings, frame.bloom_levels, frame.bloom_chain).post, None);
        assert!(
            GpuTimes::of(&timings, 3, false).post.is_some(),
            "using a later frame's disabled bloom would incorrectly accept a partial old total"
        );
        let missing = Traced { gpu: GpuTimes { timed: true, ..GpuTimes::default() }, ..frame };
        let post_row = ROWS.iter().find(|row| row.name == "gpu post").expect("post row");
        assert!(!counts(post_row, &missing, Phase::Cpu));
    }

    /// On a frame whose every timed pass decoded, as Metal's are, the span reads, and a group reads the
    /// passes of it that ran: a cascade not redrawn costs nothing, but one that ran untimed is unknown
    /// (M3 Z1).
    #[test]
    fn a_frame_whose_timed_passes_all_decoded_reads_its_span_and_the_passes_that_ran() {
        use sapphire_render::graph::PassTime;
        let pass = |name, start| PassTime {
            name,
            start: Duration::from_micros(start),
            duration: Duration::from_micros(10),
            exclusive: Duration::from_micros(10),
        };
        let mut timings = GpuTimings {
            frame: 9,
            every_pass: false,
            every_timed_pass: true,
            untimed: vec!["uploads"],
            passes: vec![pass(shadows::PASSES[0], 0), pass(MAIN_PASS, 10)],
            start: Some(Duration::from_secs(5)),
        };
        let read = GpuTimes::of(&timings, 0, false);
        assert_eq!(read.span, Some(Duration::from_micros(20)), "Metal's frames have a span");
        assert_eq!(
            read.end,
            Some(Duration::from_secs(5) + Duration::from_micros(20)),
            "and an end on the GPU's clock"
        );
        assert_eq!(read.shadows, Some(Duration::from_micros(10)), "the cascades that ran");
        assert_eq!(read.main, Some(Duration::from_micros(10)));
        assert_eq!(read.taa, None, "nothing of it ran");
        timings.untimed.push(shadows::PASSES[1]);
        assert_eq!(GpuTimes::of(&timings, 0, false).shadows, None, "a cascade ran untimed");
        timings.every_timed_pass = false;
        let dropped = GpuTimes::of(&timings, 0, false);
        assert_eq!((dropped.span, dropped.shadows, dropped.end), (None, None, None), "a pair was dropped");
        assert_eq!(dropped.main, Some(Duration::from_micros(10)));
    }

    /// A trace's lines have as many fields as its header, whatever's in them.
    #[test]
    fn a_trace_line_matches_its_header() {
        let line = trace_line(&traced(12, 345));
        let header = format!("{TRACE_HEADER}{}", counts::TRACE_COLUMNS);
        assert_eq!(line.split(',').count(), header.split(',').count(), "{line}");
        assert!(line.starts_with("cpu,0,0,12,1,345.0,"), "{line}");
    }

    /// The trace's `gpu_end` column holds the frame's end on the GPU's clock in µs, and is empty untimed (M5 Z2).
    #[test]
    fn a_trace_line_holds_the_frames_gpu_end() {
        let header = format!("{TRACE_HEADER}{}", counts::TRACE_COLUMNS);
        let column = header.split(',').position(|name| name == "gpu_end").expect("a gpu_end column");
        let end = Duration::from_secs(5) + Duration::from_nanos(123_456);
        let timed = Traced {
            gpu: GpuTimes { timed: true, end: Some(end), ..GpuTimes::default() },
            ..traced(12, 345)
        };
        assert_eq!(trace_line(&timed).split(',').nth(column), Some("5000123.5"));
        assert_eq!(trace_line(&traced(12, 345)).split(',').nth(column), Some(""));
    }

    /// The waits line reads the last bin as its frames or more, and no wait as none.
    #[test]
    fn a_wait_bin_reads_in_frames() {
        let last = WAIT_BINS as u32 - 1;
        let names = [None, Some(0), Some(32), Some(last - 1), Some(last)].map(wait_frames);
        assert_eq!(names, ["-", "0", "32", "1023", "1024+"]);
    }

    /// A phase's waits line: its label, both kinds together, the pool's share, and `-` for no waits.
    #[test]
    fn a_phases_waits_line_reads_both_kinds() {
        let mut stats = TextureStreamingStats { streaming: true, ..TextureStreamingStats::default() };
        let line = WaitsLine::of(Phase::First.name(), &stats);
        assert_eq!(
            (line.phase, line.completed, line.p99.as_str(), line.pool_worst.as_str()),
            ("first", 0, "-", "-")
        );
        stats.waits.frames[3] = 99;
        stats.pool_waits.frames[WAIT_BINS - 1] = 1;
        (stats.pool_waits.open, stats.pool_waits.longest_open) = (1, 500);
        let line = WaitsLine::of(Phase::Cpu.name(), &stats);
        assert_eq!(line.phase, "cpu");
        assert_eq!(
            (line.completed, line.p50.as_str(), line.p99.as_str(), line.worst.as_str()),
            (100, "3", "3", "1024+")
        );
        assert_eq!((line.pool_held, line.pool_p99.as_str(), line.pool_worst.as_str()), (1, "1024+", "1024+"));
        assert_eq!((line.open, line.longest_open, line.pool_open, line.pool_longest_open), (1, 500, 1, 500));
    }
}
