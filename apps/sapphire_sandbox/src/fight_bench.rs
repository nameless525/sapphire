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
//! `--bench-draw-every` is the same load and the same fight with nearly no frames drawn, which checks a load's
//! pipeline inventory (exit test 7) where drawing costs seconds. It logs the pipelines' row, and no others.
//!
//! Rows the spec budgets on Desktop-Low and Mobile-Low only, as §3.2 does, have no budget on the other
//! two tiers. A row is per frame, so a Mobile-Low step's budget is doubled, as §3.2's "× 2 steps" says.

use std::fmt::Write as _;
use std::io::Write as _;
use std::num::NonZeroU32;
use std::path::Path;
use std::time::Duration;

use sapphire_core::alloc::{self, AllocScope};
use sapphire_ecs::prelude::WorldClock;
use sapphire_platform::AppInitError;
use sapphire_render::graph::GpuTimings;
use sapphire_render::quality::TierId;
use sapphire_render::{
    CHARACTER_PREPASS, MAIN_PASS, PREPASSES, UI_PASS, ambient, post, shadows, temporal, vfx,
};
use sapphire_rhi::wgpu;
use sapphire_time::{Stopwatch, TICK_HZ};

use crate::LOG_TARGET;
use crate::offscreen::Stage;
use crate::options::Options;
use game_combat::combat::Combat;
use game_combat::combat::CombatOption;
use sapphire_stage::FrameLaps;
use sapphire_stage::audio::OfflineAudio;

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
}

impl GpuTimes {
    fn of(timings: &GpuTimings, bloom_levels: u32, bloom_chain: bool) -> Self {
        let sum = |names: &[&str]| -> Option<Duration> {
            names.iter().try_fold(Duration::ZERO, |total, name| Some(total + timings.get(name)?.exclusive))
        };
        Self {
            timed: true,
            span: timings.every_pass.then(|| timings.span()),
            main: sum(&[MAIN_PASS]),
            prepasses: sum(&PREPASSES),
            character_prepass: sum(&[CHARACTER_PREPASS]),
            shadows: sum(&shadows::PASSES),
            skinning: sum(&["skinning"]),
            sky_cube: sum(&["sky cube"]),
            clustering_ao: sum(&ambient::PASSES)
                .zip(sum(&["light clusters"]))
                .map(|(ao, clusters)| ao + clusters),
            taa: sum(&temporal::PASSES),
            post: post::frame_passes(bloom_levels, bloom_chain)
                .try_fold(Duration::ZERO, |total, name| Some(total + timings.get(name)?.exclusive)),
            vfx: sum(&[vfx::PASS]),
            ui: sum(&[UI_PASS]),
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
}

/// A number the bench tracks per frame, and the budget the spec gives it on each tier.
struct Row {
    name: &'static str,
    /// Read from the `gpu` phase, the rest from `first` and `cpu`.
    gpu: bool,
    read: fn(&Traced) -> Option<Duration>,
    /// In µs a frame: Desktop-High, Desktop-Low, Mobile-High, Mobile-Low.
    budget: [Option<u32>; 4],
}

const fn cpu_row(name: &'static str, read: fn(&Traced) -> Option<Duration>, budget: [Option<u32>; 4]) -> Row {
    Row { name, gpu: false, read, budget }
}

const fn gpu_row(name: &'static str, read: fn(&Traced) -> Option<Duration>, budget: [Option<u32>; 4]) -> Row {
    Row { name, gpu: true, read, budget }
}

/// §3.2's two columns, for the rows that have only those.
const fn low(desktop: u32, mobile: u32) -> [Option<u32>; 4] {
    [None, Some(desktop), None, Some(mobile)]
}

const UNBUDGETED: [Option<u32>; 4] = [None; 4];

/// What's logged, in the order of spec §3: the whole, §3.2's rows, what §3.2 has no row for, §3.3's.
const ROWS: [Row; 27] = [
    cpu_row("cpu frame", |t| Some(t.cpu), [Some(8_000), Some(12_000), Some(12_000), Some(22_000)]),
    cpu_row("game side", |t| Some(t.cpu.saturating_sub(t.laps.prepare + t.laps.record)), low(6_000, 13_600)),
    cpu_row("simulation", |t| Some(t.laps.simulation), low(1_000, 3_000)),
    cpu_row("physics", |t| Some(t.laps.physics), low(700, 2_400)),
    cpu_row("animation and ik", |t| Some(t.animation.saturating_sub(t.springs)), low(1_500, 3_000)),
    cpu_row("secondary motion", |t| Some(t.springs), low(500, 1_000)),
    cpu_row("vfx simulation", |t| Some(t.vfx), low(300, 600)),
    cpu_row("audio update", |t| Some(t.audio), low(200, 300)),
    cpu_row("streaming activation", |t| Some(t.streaming), low(500, 1_000)),
    cpu_row("extract", |t| Some(t.laps.packet.saturating_sub(t.camera + t.hud)), low(700, 1_200)),
    cpu_row("render", |t| Some(t.laps.prepare + t.laps.record), low(4_000, 8_000)),
    cpu_row("cues", |t| Some(t.laps.cues), UNBUDGETED),
    cpu_row("scene step", |t| Some(t.laps.scene_step), UNBUDGETED),
    cpu_row("camera", |t| Some(t.camera), UNBUDGETED),
    cpu_row("hud", |t| Some(t.hud), UNBUDGETED),
    cpu_row("content", |t| Some(t.laps.content), UNBUDGETED),
    cpu_row("gpu wait", |t| Some(t.wait), UNBUDGETED),
    gpu_row("gpu frame", |t| t.gpu.span, [Some(12_000), Some(14_000), Some(13_000), Some(26_000)]),
    gpu_row("gpu prepasses", |t| t.gpu.prepasses, low(1_500, 1_500)),
    gpu_row("gpu shadows", |t| t.gpu.shadows, low(2_000, 3_000)),
    // §3.3 gives Mobile-Low none, and it runs neither.
    gpu_row("gpu clustering and ao", |t| t.gpu.clustering_ao, [None, Some(1_500), None, None]),
    // §3.3's forward opaque, outlines, and hair, eyes and transparents all draw in the main pass.
    gpu_row("gpu main", |t| t.gpu.main, low(4_900, 11_500)),
    gpu_row("gpu effects", |t| t.gpu.vfx, low(1_200, 3_000)),
    gpu_row("gpu taa", |t| t.gpu.taa, low(800, 1_000)),
    gpu_row("gpu post", |t| t.gpu.post, low(700, 2_500)),
    gpu_row("gpu ui", |t| t.gpu.ui, low(300, 1_000)),
    gpu_row("gpu skinning", |t| t.gpu.skinning, UNBUDGETED),
];

/// A row's place in [`Row::budget`].
const fn tier_column(tier: TierId) -> usize {
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
    Some(Spread { p50: at(50), p99: at(99), worst, worst_tick })
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
    option: &'a CombatOption,
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
}

/// The render and compute pipelines the stage's device has: zero where its backend can't say.
fn pipelines(stage: &Stage) -> usize {
    stage.gpu.instance().raw().generate_report().map_or(0, |report| {
        report.hub.render_pipelines.num_kept_from_user + report.hub.compute_pipelines.num_kept_from_user
    })
}

/// Plays and measures the fight the stage holds. See the [module docs](self). `frames` is the least any
/// phase measures, and `startup` what loading took.
pub(crate) fn run(
    stage: &mut Stage,
    option: &CombatOption,
    options: &Options,
    (frames, width, height): (u32, u32, u32),
    startup: Duration,
) -> Result<(), AppInitError> {
    let target = stage.target(width, height, wgpu::TextureUsages::empty());
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let rate = stage.quality.frame_rate().map_or(TICK_HZ, std::num::NonZero::get);
    let steps = steps_a_frame(rate);
    let settings = options.render_settings(stage.quality.render_settings());
    let audio = match stage.show.fight.show.cue_show().map(|show| show.table().sound_cues()) {
        Some(cues) => {
            Some(OfflineAudio::for_cues(&cues, &mut stage.content, &mut stage.renderer, &stage.gpu)?)
        }
        None => None,
    };
    // A fight's length, for what to reserve. A fight with no file measures just `frames`.
    let length = stage.show.fight.combat.as_ref().and_then(Combat::ticks_left).unwrap_or(0);
    let per_phase = u64::from(frames).max(length / u64::from(steps) + 1) as usize;
    let mut run = Run {
        stage,
        view: &view,
        option,
        audio,
        steps,
        bloom_levels: settings.bloom_levels,
        draw_every: options.bench_draw_every,
        trace: Vec::with_capacity(per_phase * 2 * Phase::ALL.len()),
        first_graph_frame: 1,
        first_traced: 0,
        last_timed: 0,
    };
    if run.draw_every.is_some() {
        run.phase(Phase::First, frames)?;
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
        let (mut played, mut fight, mut start_over) = (0_u32, 0_u32, phase != Phase::First);
        let at_start = pipelines(self.stage);
        let mut after_first_frame = at_start;
        loop {
            self.stage.lose_frames = self.draw_every.is_some_and(|every| played % every != 0);
            let frame = self.frame(phase, played, fight, start_over)?;
            self.trace.push(frame);
            played += 1;
            self.read_gpu_times();
            if played == 1 {
                after_first_frame = pipelines(self.stage);
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
                    "fight pipelines",
                );
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
        if start_over && matches!(self.option, CombatOption::Replay { .. }) {
            stage.show.fight.combat.take().map_or(Ok(()), Combat::finish).map_err(AppInitError::msg)?;
        }
        let cues_before =
            stage.show.fight.show.cue_show().map(game_combat::cues::CueShow::stats).unwrap_or_default();
        let sim_before =
            stage.show.fight.combat.as_ref().map_or(0, |combat| combat.simulation().stats().allocations);
        let before = alloc::stats();
        let mut stopwatch = Stopwatch::start();
        let cpu = Stopwatch::start();
        let mut restart = Duration::ZERO;
        if start_over {
            match self.option {
                CombatOption::Replay { .. } => {
                    let combat = stage.show.fight.prepare_stock_combat(self.option, &mut stage.content)?;
                    stage.show.fight.attach_prepared_combat(combat, &mut stage.scene, &stage.content)?;
                }
                _ => {
                    if let Some(combat) = &mut stage.show.fight.combat {
                        combat.restart();
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
        stopwatch.lap();
        stage.gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
        let wait = stopwatch.lap();
        let mix = self.audio.as_mut().map_or(Duration::ZERO, |audio| audio.mix(frame_dt));
        let after = alloc::stats();

        let anim = stage.scene.anim_stats();
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
            let values = frames()
                .filter(|frame| counts(row, frame, phase))
                .filter_map(|frame| Some(((row.read)(frame)?, frame.tick)));
            let Some(spread) = spread(values) else { continue };
            let budget = row.budget[column];
            tracing::info!(
                target: LOG_TARGET,
                phase = phase.name(),
                row = row.name,
                samples = frames().filter(|frame| counts(row, frame, phase)).count(),
                p50_us = format_args!("{:.1}", micros(spread.p50)),
                p99_us = format_args!("{:.1}", micros(spread.p99)),
                worst_us = format_args!("{:.1}", micros(spread.worst)),
                worst_tick = spread.worst_tick,
                budget_us = ?budget,
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
    }
}

/// The header of `--bench-trace`'s file: times in µs.
const TRACE_HEADER: &str = "phase,frame,fight,tick,steps,cpu,wait,restart,scene_step,simulation,physics,cues,\
audio,mix,content,animation,springs,poses,joints,packet,camera,hud,vfx,streaming,prepare,record,allocations,bytes,\
live_bytes,step_allocations,sim_allocations,animation_allocations,packet_allocations,packet_bytes,prepare_allocations,\
prepare_bytes,record_allocations,record_bytes,sprites,ribbon_points,cues_read,effects,sounds,screens,layers,\
hitstop_player,hitstop_enemy,world_scale,post_first_use,draws,gpu_timed,gpu_span,gpu_main,gpu_prepasses,\
gpu_character_prepass,gpu_shadows,gpu_skinning,gpu_sky_cube,gpu_clustering_ao,gpu_taa,gpu_post,gpu_vfx,gpu_ui";

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
    ] {
        match time {
            Some(time) => {
                let _ = write!(line, ",{:.1}", time.as_secs_f64() * 1e6);
            }
            None => line.push(','),
        }
    }
}

#[cfg(test)]
fn trace_line(frame: &Traced) -> String {
    let mut line = String::new();
    trace_line_into(frame, &mut line);
    line
}

fn write_trace(path: &Path, trace: &[Traced]) -> std::io::Result<()> {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(file, "{TRACE_HEADER}")?;
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

    fn traced(tick: u64, cpu_us: u64) -> Traced {
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
    }

    #[test]
    fn post_totals_require_all_passes_of_the_measured_frame() {
        use sapphire_render::graph::PassTime;
        let names: Vec<_> = post::frame_passes(3, true).collect();
        let mut timings = GpuTimings {
            frame: 7,
            every_pass: true,
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

    /// A trace's lines have as many fields as its header, whatever's in them.
    #[test]
    fn a_trace_line_matches_its_header() {
        let line = trace_line(&traced(12, 345));
        assert_eq!(line.split(',').count(), TRACE_HEADER.split(',').count(), "{line}");
        assert!(line.starts_with("cpu,0,0,12,1,345.0,"), "{line}");
    }
}
