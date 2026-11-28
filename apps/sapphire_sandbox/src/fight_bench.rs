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
//! - **A fight starting over** happens as the game's does (M6 J2): a replay's next [`Combat`] (which reads the file
//!   a fight with no file doesn't) is prepared on the warm-up pool (`Fight::prepare_run_off_frame`) from the
//!   frame after the last one ends, and stands on the first frame it's ready; a fight with no file starts over
//!   in place (`Combat::restart`). Each frame of it is a restart frame, with its own share of the restart. The
//!   fight's checksums are held to the headless run's at the end of each, after the frame that ended it.
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
//! **Frames left out are rows of their own** (M6 B3): a frame that starts a fight over, one drawn at a tick outside
//! `--bench-ticks` (`outside`, M6 B5, logged only by a run with them) and in the GPU phase one whose timings never
//! came back are in no row's spread, and one whose timings came back short, a pair dropped, in no `gpu frame`'s
//! (`incomplete`, fixed after B3's review); each phase logs them by reason (`fight excluded`) with
//! their count, their CPU frame's median and worst, a restart's own time and what it allocated on the frame's
//! thread (M6 J2, where counts are kept by thread), and what the frames allocated, the warm-up pool's work beside
//! them included. **The thread's CPU time** (`sapphire_platform::thread_cpu_time`) is read beside the wall time
//! over the frame's CPU span, its record (`FrameLaps::record_on_core`) and its wait for the GPU, as the
//! `... on core` rows: what the wall has over it, the thread spent off its core, waiting on the encoders' workers,
//! the driver or the GPU, or set aside by the machine's other work. Drawn offscreen, the GPU wait stands where a window's acquire would block.
//!
//! **Overlapping** (`--bench-overlap`, M6 B7, `crate::bench::overlap`), each phase starts from an empty queue and each
//! frame waits for the frame graph's submission two frames back, not for everything: the `gpu wait` rows are that
//! wait, and the GPU phase logs `bench overlap`, the queue's age on the GPU's clock and whether its frames were CPU-
//! or GPU-limited. Off, every frame waits for the GPU, as it always has.
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
use std::ops::RangeInclusive;
use std::path::Path;
use std::time::Duration;

use sapphire_core::alloc::{self, AllocScope};
use sapphire_ecs::prelude::WorldClock;
use sapphire_platform::{AppInitError, CoreKind};
use sapphire_render::graph::GpuTimings;
use sapphire_render::quality::TierId;
use sapphire_render::{
    CHARACTER_PREPASS, DISTORTION_PASS, MAIN_PASS, MainDepth, PREPASSES, UI_PASS, ambient, post, shadows,
    temporal, vfx,
};
use sapphire_rhi::wgpu;
use sapphire_time::{Stopwatch, TICK_HZ};

use crate::LOG_TARGET;
use crate::bench::overlap::{self, GpuWait, Sample};
use crate::offscreen::Stage;
use crate::options::Options;
use game_combat::combat::Combat;
use game_combat::combat::CombatOption;
use game_combat::fight::NextRun;
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
    /// The prepasses and the shadow cascades that ran, together (M5 Z2): one depth-only row where the backend
    /// overlaps them.
    depth_only: Option<Duration>,
    skinning: Option<Duration>,
    sky_cube: Option<Duration>,
    /// Light clusters and the ambient pass: §3.3's "Light clustering, AO".
    clustering_ao: Option<Duration>,
    taa: Option<Duration>,
    /// The bloom chain and the composite: §3.3's "Bloom + tonemap + LUT".
    post: Option<Duration>,
    vfx: Option<Duration>,
    ui: Option<Duration>,
    /// When the frame's GPU work started (`GpuTimings::start`), when its span reads: with [`Self::end`], what an
    /// overlapped phase's reading takes (M6 B7).
    start: Option<Duration>,
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
            depth_only: sum(&PREPASSES.iter().chain(&shadows::PASSES).copied().collect::<Vec<_>>()),
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
            // None on a phone's frame below scale 1, whose interface draws in the upscaler's sharpen pass (M6 F1);
            // a dev build's `--measure-unfolded-overlays` keeps its pass, for this row.
            ui: sum(&[UI_PASS]),
            start: timings.every_timed_pass.then_some(timings.start).flatten(),
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
    /// Waiting for the GPU after that: for every submission, or overlapping for the one two frames back.
    wait: Duration,
    /// Overlapping (M6 B7), when [`Self::cpu`] ended, at the frame's submit, on the bench's clock.
    submitted: Option<Duration>,
    /// The thread's CPU time over [`Self::cpu`] and over [`Self::wait`] (M6 B3), `None` where the OS doesn't say.
    cpu_on_core: Option<Duration>,
    wait_on_core: Option<Duration>,
    /// Its share of starting a fight over, within [`Self::cpu`]: `None` on a frame that started nothing over. A
    /// fight's restart spreads over frames (M6 J2), each of them a restart frame with its own share.
    restart: Option<Duration>,
    /// Drawn at a tick outside `--bench-ticks` (M6 B5): in no row, as a frame that starts a fight over isn't.
    outside: bool,
    /// Allocations on the frame's thread within [`Self::restart`]: the frame's own share, where
    /// [`Self::allocations`] counts the whole process, the warm-up pool preparing the next fight included. Where
    /// counts aren't kept by thread (`alloc::PER_THREAD_COUNTS`, Android's target) it's the process's too, and
    /// [`report_excluded`] doesn't log it.
    restart_allocations: u64,
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
    /// Whether its main pass tested `Equal` against the scene prepass's depth (M5 D10, `--main-depth prepass`),
    /// which the renderer falls back from, saying nothing, where it can't (M6 B9: F5's probes read it).
    main_depth_from_prepass: bool,
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
const ROWS: [Row; 39] = [
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
    // Each span's thread CPU time beside its wall time (M6 B3): what of it the frame's thread spent on its core, so a
    // frame that waits (on the encoders' workers, the driver, a lock, or a busy machine's other work) reads apart
    // from one that works. Offscreen, the GPU wait is where a window's acquire would block on a GPU behind it.
    cpu_row("cpu frame on core", |t| t.cpu_on_core, UNBUDGETED),
    cpu_row("record", |t| Some(t.laps.record), UNBUDGETED),
    cpu_row("record on core", |t| t.laps.record_on_core, UNBUDGETED),
    cpu_row("gpu wait on core", |t| t.wait_on_core, UNBUDGETED),
    gpu_row("gpu frame", |t| t.gpu.span, [Some(12_000), Some(14_000), Some(13_000), Some(26_000)]),
    // A grouped row is held to the sum of its members' figures (M5 Z2): culling and the prepasses.
    gpu_row("gpu prepasses", |t| t.gpu.prepasses, low(1_500, 1_500)).proposing(1_200, 1_000),
    gpu_row("gpu shadows", |t| t.gpu.shadows, low(2_000, 3_000)).proposing(2_000, 1_600),
    // The two depth-only rows as one, each frame's sum against the sum of their figures (M5 Z2): what the report
    // judges in their place where the backend overlaps its passes, as Metal runs the scene prepass beside a
    // shadow cascade, so how the frame's depth work splits between them isn't read.
    gpu_row("gpu depth only", |t| t.gpu.depth_only, low(3_500, 4_500)).proposing(3_200, 2_600),
    // §3.3 gives Mobile-Low none, and it runs neither.
    gpu_row("gpu clustering and ao", |t| t.gpu.clustering_ao, [None, Some(1_500), None, None])
        .proposing(1_200, 800),
    // §3.3's forward opaque, outlines, and hair, eyes and transparents all draw in the main pass: their sum,
    // 3.5 + 0.6 + 0.8 and 8.0 + 1.5 + 2.0 ms, and B4's 2.8 + 0.5 + 0.7 and 3.4 + 1.0 + 0.9.
    gpu_row("gpu main", |t| t.gpu.main, low(4_900, 11_500)).proposing(4_000, 5_300),
    gpu_row("gpu effects", |t| t.gpu.vfx, low(1_200, 3_000)).proposing(800, 1_000),
    gpu_row("gpu taa", |t| t.gpu.taa, low(800, 1_000)).proposing(900, 600),
    gpu_row("gpu post", |t| t.gpu.post, low(700, 2_500)).proposing(800, 1_200),
    // Post on the frames a screen effect plays on (M6 B5): what the fight's flashes, impact frames and the effects
    // pack add to the fused pass, which has no share of its own in §3.3, read beside the row and held to its line.
    gpu_row("gpu post under screen effects", |t| t.gpu.post.filter(|_| t.layers > 0), low(700, 2_500))
        .proposing(800, 1_200),
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    frame.phase == phase && frame.read() && (!row.gpu || frame.gpu.timed) && (row.read)(frame).is_some()
}

impl Traced {
    /// Whether the rows read it: it started no fight over, and its tick is one `--bench-ticks` names, if given.
    fn read(&self) -> bool {
        self.restart.is_none() && !self.outside
    }

    /// Bytes the frame uploaded, new assets and raised texture levels both.
    fn uploaded(&self) -> u64 {
        self.counted.uploaded + self.counted.raised
    }
}

/// The frame thread's CPU time across a frame's spans (M6 B3), as `sapphire_platform` reads it (R2): a clock read
/// a span, which allocates nothing. `None` where the OS doesn't say.
struct OnCore(Option<Duration>);

impl OnCore {
    fn start() -> Self {
        Self(sapphire_platform::thread_cpu_time())
    }

    /// The thread's CPU time since the last read, and a span from now.
    fn lap(&mut self) -> Option<Duration> {
        let now = sapphire_platform::thread_cpu_time();
        let since = self.0.zip(now).map(|(from, to)| to.saturating_sub(from));
        self.0 = now;
        since
    }
}

/// Why a frame is left out of the rows' statistics, each reported on its own as a row of its own (M6 B3): of
/// every row's, or for [`Self::Incomplete`] of the frame's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Excluded {
    /// It started a fight over (or a walk's tour), which a row would read as the fight's worst frame.
    Restart,
    /// Drawn at a tick outside `--bench-ticks` (M6 B5), which reads only the fight's ticks it names.
    Outside,
    /// A GPU phase's frame whose timings never came back, which the GPU rows can't read.
    Untimed,
    /// A GPU phase's frame whose timings came back short (fixed after B3's review, M6 backfill): a timestamp pair
    /// dropped as invalid (`GpuTimings::every_timed_pass`), so it has no span and `gpu frame` can't read it. A pass
    /// row still reads it where its own passes decoded, as [`GpuTimes::of`] sums them.
    Incomplete,
}

impl Excluded {
    const ALL: [Self; 4] = [Self::Restart, Self::Outside, Self::Untimed, Self::Incomplete];

    const fn name(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Outside => "outside",
            Self::Untimed => "untimed",
            Self::Incomplete => "incomplete",
        }
    }

    /// Whether a phase can leave frames out for this reason: only the GPU phase times the GPU, and only a run with
    /// `--bench-ticks` (`ticks`) draws frames outside them.
    fn applies(self, phase: Phase, ticks: bool) -> bool {
        match self {
            Self::Restart => true,
            Self::Outside => ticks,
            Self::Untimed | Self::Incomplete => phase == Phase::Gpu,
        }
    }

    /// Whether `frame`, of a phase this [`applies`](Self::applies) to, is left out for this reason. A restart frame
    /// is a restart frame first, and one outside the ticks read is that before it's untimed.
    fn holds(self, frame: &Traced, phase: Phase) -> bool {
        match self {
            Self::Restart => frame.restart.is_some(),
            Self::Outside => frame.restart.is_none() && frame.outside,
            Self::Untimed => phase == Phase::Gpu && frame.read() && !frame.gpu.timed,
            Self::Incomplete => {
                phase == Phase::Gpu && frame.read() && frame.gpu.timed && frame.gpu.span.is_none()
            }
        }
    }
}

/// Each frame's: whether an upload was in flight, bytes uploaded in the frames from two before it to one after
/// (M5 Z2). On a backend that overlaps its passes an upload's copies land in whichever pass is drawing, so a pass
/// row is read on the frames with none.
fn uploads_in_flight(uploaded: &[u64]) -> Vec<bool> {
    (0..uploaded.len())
        .map(|at| uploaded[at.saturating_sub(2)..(at + 2).min(uploaded.len())].iter().any(|&bytes| bytes > 0))
        .collect()
}

/// A frame over this many times its round's median, in a pass of fixed cost, is slow (M5 Z2).
const SLOW: f64 = 1.15;

/// The fewest slow frames the display's lock is read from.
const LOCK_FEWEST: usize = 20;

/// The display's 60 Hz lock (M5 Z2): how closely the slow frames' GPU ends line up at one phase of a 60 Hz
/// period, the mean resultant length of their phases (0 spread evenly, 1 all at one phase) at the period within
/// 0.03% of 1/60 s that lines them up most, with that period in µs. A display or compositor drawing at 60 Hz
/// beside the run (a video playing) slows the passes it lands on at its own beat, which a quiet sitting doesn't
/// have. `None` with fewer than [`LOCK_FEWEST`] frames.
fn display_lock(ends_us: &[f64]) -> Option<(f64, f64)> {
    let &first = ends_us.first()?;
    if ends_us.len() < LOCK_FEWEST {
        return None;
    }
    (-300..=300)
        .step_by(2)
        .map(|ppm: i32| {
            let period = 1e6 / 60.0 * (1.0 + f64::from(ppm) * 1e-6);
            let (cos, sin) = ends_us.iter().fold((0.0, 0.0), |(cos, sin), &end| {
                let angle = std::f64::consts::TAU * (end - first).rem_euclid(period) / period;
                (cos + angle.cos(), sin + angle.sin())
            });
            (f64::hypot(cos, sin) / ends_us.len() as f64, period)
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
}

/// Of `frames`' values, the shares (in %) over [`SLOW`] times their median among the frames `in_flight` marks
/// and among the others.
fn slow_shares(values: &[(Duration, bool)]) -> (f64, f64) {
    let mut sorted: Vec<Duration> = values.iter().map(|&(value, _)| value).collect();
    sorted.sort_unstable();
    let Some(&median) = sorted.get(sorted.len().saturating_sub(1) / 2) else { return (0.0, 0.0) };
    let share = |flight: bool| {
        let of: Vec<Duration> =
            values.iter().filter(|&&(_, at)| at == flight).map(|&(value, _)| value).collect();
        let slow = of.iter().filter(|value| value.as_secs_f64() > median.as_secs_f64() * SLOW).count();
        100.0 * slow as f64 / of.len().max(1) as f64
    };
    (share(true), share(false))
}

/// Whether a frame's passes can overlap on the GPU (M5 Z2): Metal runs them side by side, but not under
/// `--serial-passes` (M6 B2), which submits each alone and waits for it, so none overlaps another (fixed after B2's
/// review, M6 backfill). The report reads it to judge the pass rows on the frames with no upload in flight and the
/// prepasses and shadows as one, or every row on every frame.
fn passes_overlap(backend: wgpu::Backend, serial: bool) -> bool {
    backend == wgpu::Backend::Metal && !serial
}

/// The GPU phase's reading (M5 Z2), for the exit run's rules on a backend that overlaps its passes: the
/// backend and whether its passes overlapped (not where `serial` ran them alone); the timed frames and those with
/// no upload in flight, which its pass rows are read on; the uploads' cost, the share of frames slow in TAA and in
/// post with an upload in flight and without; and the display's 60 Hz lock over the frames slow in either, which
/// voids a round. The counts are of the frames the rows read; the shares and the lock are the sitting's, read over
/// every timed frame that isn't a restart, `--bench-ticks` or not (M6 B5): a display's beat is the machine's, and
/// the 50 frames of a cut-in hold too few slow ones to show it.
fn gpu_reading(backend: wgpu::Backend, serial: bool, frames: &[&Traced], in_flight: &[bool]) {
    let (timed, sitting) = reading_frames(frames);
    let quiet = timed.iter().filter(|&&at| !in_flight[at]).count();
    let pass = |read: fn(&GpuTimes) -> Option<Duration>| -> Vec<(usize, Duration)> {
        sitting.iter().filter_map(|&at| Some((at, read(&frames[at].gpu)?))).collect()
    };
    let (taa, post) = (pass(|gpu| gpu.taa), pass(|gpu| gpu.post));
    let shares = |values: &[(usize, Duration)]| {
        slow_shares(&values.iter().map(|&(at, value)| (value, in_flight[at])).collect::<Vec<_>>())
    };
    let ((taa_flight, taa_quiet), (post_flight, post_quiet)) = (shares(&taa), shares(&post));
    // The frames over the 95th percentile of TAA or of post, pooled, as one beat slows them both.
    let over = |values: &[(usize, Duration)]| -> Vec<usize> {
        let p95 =
            spread(values.iter().map(|&(at, value)| (value, at as u64))).map_or(Duration::MAX, |s| s.p95);
        values.iter().filter(|&&(_, value)| value > p95).map(|&(at, _)| at).collect()
    };
    let mut slow = over(&taa);
    slow.extend(over(&post));
    slow.sort_unstable();
    slow.dedup();
    let ends: Vec<f64> =
        slow.iter().filter_map(|&at| frames[at].gpu.end).map(|end| end.as_secs_f64() * 1e6).collect();
    let lock = display_lock(&ends);
    tracing::info!(
        target: LOG_TARGET,
        phase = Phase::Gpu.name(),
        backend = backend.to_str(),
        overlaps = passes_overlap(backend, serial),
        timed_frames = timed.len(),
        no_upload_frames = quiet,
        in_flight_pct = format_args!("{:.1}", 100.0 * (timed.len() - quiet) as f64 / timed.len().max(1) as f64),
        taa_slow_in_flight_pct = format_args!("{taa_flight:.1}"),
        taa_slow_no_upload_pct = format_args!("{taa_quiet:.1}"),
        post_slow_in_flight_pct = format_args!("{post_flight:.1}"),
        post_slow_no_upload_pct = format_args!("{post_quiet:.1}"),
        slow_frames = ends.len(),
        lock = %lock.map_or_else(|| "-".to_owned(), |(lock, _)| format!("{lock:.3}")),
        lock_period_us = %lock.map_or_else(|| "-".to_owned(), |(_, period)| format!("{period:.3}")),
        "fight gpu reading",
    );
}

/// Of a GPU phase's `frames`, the timed ones the rows read and the timed ones of the sitting, every one that isn't a
/// restart, which [`gpu_reading`]'s shares and lock are read over.
fn reading_frames(frames: &[&Traced]) -> (Vec<usize>, Vec<usize>) {
    let timed = |keep: fn(&Traced) -> bool| -> Vec<usize> {
        (0..frames.len()).filter(|&at| frames[at].gpu.timed && keep(frames[at])).collect()
    };
    (timed(Traced::read), timed(|frame| frame.restart.is_none()))
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
    /// `--bench-ticks`: the fight's ticks the rows read (M6 B5), every tick where `None`.
    ticks: Option<RangeInclusive<u64>>,
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
    /// How each frame waits for the GPU: for everything, or overlapping (M6 B7).
    waits: GpuWait,
    /// A walk under a running day (`--day-length`): the hour its tour starts at, which each phase's restart of
    /// the tour sets again, so every phase walks the same stretch of the day (M5 Z1). `None` where time stands
    /// still or for a fight, which nothing here changes.
    walk_hour: Option<f32>,
    /// A replay's next fight, on the warm-up pool from the frame its restart began until the frame it stands.
    next_run: Option<NextRun>,
    /// Whether the fight stood on the arena (`--arena`), so its next run is checked for it as its first was.
    on_the_arena: bool,
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
        ticks: options.bench_ticks.clone(),
        trace: Vec::with_capacity(per_phase * 2 * Phase::ALL.len()),
        first_graph_frame: 1,
        first_traced: 0,
        last_timed: 0,
        assets,
        loads,
        walk_hour: None,
        waits: GpuWait::new(options.bench_overlap),
        next_run: None,
        on_the_arena: options.arena.is_some(),
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
        self.waits.drain(self.stage.gpu.device()).map_err(AppInitError::msg)?;
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
            // A fight's restart goes on until the next one stands (M6 J2), and no phase ends inside one.
            start_over = self.next_run.is_some();
            if start_over {
                continue;
            }
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
        let cues_before =
            stage.show.fight.show.cue_show().map(game_combat::cues::CueShow::stats).unwrap_or_default();
        let before = alloc::stats();
        let mut stopwatch = Stopwatch::start();
        let cpu = Stopwatch::start();
        let mut on_core = OnCore::start();
        let mut restart = None;
        let mut restart_allocations = 0;
        // The fight a restart ended: checking its checksums and letting it go are the bench's, not a frame's.
        let mut ended = None;
        if start_over {
            let scope = AllocScope::begin();
            match self.option {
                // As a game's run starts over under its wipe (M4 H4): the next fight is prepared on the warm-up pool
                // while the frames go on, each holding only its share of asking for it, and stands on the first
                // frame it's ready (M6 J2).
                Some(option @ CombatOption::Replay { .. }) => {
                    let on_the_arena = self.on_the_arena;
                    let next = self
                        .next_run
                        .get_or_insert_with(|| stage.show.fight.prepare_run_off_frame(option, on_the_arena));
                    let ready = next
                        .poll(&mut stage.show.fight, || Ok(&mut stage.content))
                        .map_err(AppInitError::msg)?;
                    if let Some(combat) = ready {
                        self.next_run = None;
                        ended = stage.show.fight.combat.take();
                        stage.show.fight.attach_prepared_combat(*combat, &mut stage.scene, &stage.content)?;
                    }
                }
                // A fight with no file starts over in place, allocating nothing (`FightWorld::restart`).
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
            restart = Some(stopwatch.lap());
            restart_allocations = scope.counts().allocations;
        }
        // Of the fight the frame steps, which a restart may just have stood.
        let sim_before =
            stage.show.fight.combat.as_ref().map_or(0, |combat| combat.simulation().stats().allocations);
        let script_before = stage.show.fight.combat.as_ref().and_then(Combat::script_time);
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
        let cpu_on_core = on_core.lap();
        let core = sapphire_platform::current_core();
        stopwatch.lap();
        let submitted =
            self.waits.wait(stage.gpu.device(), stage.graph.submitted()).map_err(AppInitError::msg)?;
        let wait = stopwatch.lap();
        let wait_on_core = on_core.lap();
        let mix = self.audio.as_mut().map_or(Duration::ZERO, |audio| audio.mix(frame_dt));
        let after = alloc::stats();
        if let Some(ended) = ended {
            ended.finish().map_err(AppInitError::msg)?;
        }

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
        let audio = self.audio.as_ref().map(OfflineAudio::stats);
        let memory = zone::Held::of(stage, audio.as_ref(), after.live_bytes);
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
            submitted,
            cpu_on_core,
            wait_on_core,
            restart,
            outside: self.ticks.as_ref().is_some_and(|ticks| !ticks.contains(&tick)),
            restart_allocations,
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
            main_depth_from_prepass: stats.main_depth == MainDepth::FromPrepass,
            gpu: GpuTimes::default(),
            bloom_chain: stats.post.bloom_chain,
            bloom_levels: self.bloom_levels,
            walked: stage.scene.zone_walked(),
            core,
            counted,
        })
    }

    /// Puts the newest GPU times that have come back on the frame they belong to; overlapping, every frame's that
    /// came back, since one frame's begin can read two (M6 B7).
    fn read_gpu_times(&mut self) {
        let read = match self.stage.graph.gpu_timings() {
            _ if self.waits.overlaps() => self.stage.graph.gpu_timings_read(),
            Some(timings) => std::slice::from_ref(timings),
            None => &[],
        };
        for timings in read {
            if timings.frame <= self.last_timed || timings.frame < self.first_graph_frame {
                continue;
            }
            self.last_timed = timings.frame;
            let at = self.first_traced + (timings.frame - self.first_graph_frame) as usize;
            if let Some(frame) = self.trace.get_mut(at) {
                frame.gpu = GpuTimes::of(timings, frame.bloom_levels, frame.bloom_chain);
            }
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
        let measured = || frames().filter(|frame| frame.read());
        let in_flight = uploads_in_flight(&frames().map(Traced::uploaded).collect::<Vec<_>>());
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
            if row.gpu && row.name != "gpu frame" {
                // The pass row on the frames with no texture upload in flight (M5 Z2): what the report judges
                // where the backend overlaps its passes, as an upload's copies land in whichever pass runs.
                let quiet: Vec<(Duration, u64)> = frames()
                    .enumerate()
                    .filter(|&(at, frame)| !in_flight[at] && counts(row, frame, phase))
                    .filter_map(|(at, frame)| Some(((row.read)(frame)?, at as u64)))
                    .collect();
                let samples = quiet.len();
                let spread = self::spread(quiet.into_iter()).unwrap_or_default();
                tracing::info!(
                    target: LOG_TARGET,
                    phase = phase.name(),
                    row = row.name,
                    samples,
                    p50_us = format_args!("{:.1}", micros(spread.p50)),
                    p95_us = format_args!("{:.1}", micros(spread.p95)),
                    p99_us = format_args!("{:.1}", micros(spread.p99)),
                    "fight row no upload",
                );
            }
        }
        if phase == Phase::Gpu {
            let backend = run.stage.gpu.caps().backend();
            gpu_reading(backend, options.serial_passes, &frames().collect::<Vec<_>>(), &in_flight);
            if run.waits.overlaps() {
                overlap::log(phase.name(), quality.tier.id.name(), &overlap_samples(frames()));
            }
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
            timed_frames = frames().filter(|frame| frame.gpu.timed).count(),
            post_first_use = frames().map(|frame| frame.post_first_use).max().unwrap_or(0),
            main_depth_prepass = frames().filter(|frame| frame.main_depth_from_prepass).count(),
            mix_us_p50 = format_args!(
                "{:.1}",
                spread(frames().map(|frame| (frame.mix, frame.tick))).map_or(0.0, |spread| micros(spread.p50))
            ),
            sprites_worst = frames().map(|frame| frame.sprites).max().unwrap_or(0),
            ribbon_points_worst = frames().map(|frame| frame.ribbon_points).max().unwrap_or(0),
            "fight phase",
        );
        for reason in Excluded::ALL.into_iter().filter(|reason| reason.applies(phase, run.ticks.is_some())) {
            report_excluded(
                &frames().filter(|frame| reason.holds(frame, phase)).collect::<Vec<_>>(),
                phase,
                reason,
            );
        }
        zone::report(run.stage, &run.trace, phase);
        counts::report(&run.trace, phase, &counts::RunBudgets::of(run.stage));
    }
}

/// An overlapped phase's frames as its reading takes them (M6 B7): a restart's frame, or one whose GPU times never
/// came back whole, has no GPU times, so neither it nor the frame after it is read.
fn overlap_samples<'a>(frames: impl Iterator<Item = &'a Traced>) -> Vec<Sample> {
    frames
        .filter_map(|frame| {
            let gpu = frame.gpu.start.zip(frame.gpu.end).filter(|_| frame.restart.is_none());
            Some(Sample { submitted: frame.submitted?, cpu: frame.cpu, wait: frame.wait, gpu })
        })
        .collect()
}

/// What a phase's frames left out of the rows for `reason` cost (M6 B3), as a row of their own: how many, their
/// CPU frame's median and worst with the thread's CPU time over that worst frame, a restart's own time and the most
/// it allocated on the frame's thread (M6 J2, only where counts are kept by thread), and what they allocated, all of
/// it and the most in one frame. With none, a line saying so: none left out, not unlogged.
fn report_excluded(frames: &[&Traced], phase: Phase, reason: Excluded) {
    let micros = |duration: Duration| format!("{:.1}", duration.as_secs_f64() * 1e6);
    let Some(cpu) = spread(frames.iter().enumerate().map(|(at, frame)| (frame.cpu, at as u64))) else {
        tracing::info!(target: LOG_TARGET, phase = phase.name(), reason = reason.name(), frames = 0, "fight excluded");
        return;
    };
    let worst = frames[cpu.worst_tick as usize];
    let restart =
        spread(frames.iter().map(|frame| (frame.restart.unwrap_or_default(), frame.tick))).expect("frames");
    tracing::info!(
        target: LOG_TARGET,
        phase = phase.name(),
        reason = reason.name(),
        frames = frames.len(),
        cpu_us_p50 = micros(cpu.p50),
        cpu_us_worst = micros(cpu.worst),
        worst_tick = worst.tick,
        on_core_us_worst = worst.cpu_on_core.map(micros),
        restart_us_p50 = micros(restart.p50),
        restart_us_worst = micros(restart.worst),
        restart_allocations_worst = alloc::PER_THREAD_COUNTS
            .then(|| frames.iter().map(|frame| frame.restart_allocations).max().unwrap_or(0)),
        allocations = frames.iter().map(|frame| frame.allocations).sum::<u64>(),
        allocations_worst = frames.iter().map(|frame| frame.allocations).max().unwrap_or(0),
        bytes = frames.iter().map(|frame| frame.bytes).sum::<u64>(),
        step_allocations = frames.iter().map(|frame| frame.step_allocations).sum::<u64>(),
        "fight excluded",
    );
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

/// The thread's CPU time over the frame's CPU span, its record and its GPU wait, in µs (M6 B3), after the counts'
/// columns, so a reader by position keeps its own; empty where the OS doesn't say.
const ON_CORE_COLUMNS: &str = ",cpu_on_core,record_on_core,wait_on_core";

/// The pixels the frame's effect sprites and ribbons shade (M6 B5, the `effect pixels` count row), last, so every
/// reader by position keeps its own.
const EFFECT_COLUMNS: &str = ",effect_pixels";

/// `--bench-trace`'s whole header.
fn trace_header() -> String {
    format!("{TRACE_HEADER}{}{ON_CORE_COLUMNS}{EFFECT_COLUMNS}", counts::TRACE_COLUMNS)
}

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
        frame.restart.unwrap_or_default(),
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
    for time in [frame.cpu_on_core, frame.laps.record_on_core, frame.wait_on_core] {
        match time {
            Some(time) => {
                let _ = write!(line, ",{:.1}", time.as_secs_f64() * 1e6);
            }
            None => line.push(','),
        }
    }
    let _ = write!(line, ",{}", frame.counted.effect_pixels);
}

#[cfg(test)]
fn trace_line(frame: &Traced) -> String {
    let mut line = String::new();
    trace_line_into(frame, &mut line);
    line
}

fn write_trace(path: &Path, trace: &[Traced]) -> std::io::Result<()> {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(file, "{}", trace_header())?;
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
            submitted: None,
            cpu_on_core: None,
            wait_on_core: None,
            restart: None,
            outside: false,
            restart_allocations: 0,
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
            main_depth_from_prepass: false,
            gpu: GpuTimes::default(),
            bloom_chain: false,
            bloom_levels: 0,
            walked: None,
            core: None,
            counted: counts::Counted::default(),
        }
    }

    /// Every run of the benches at scale and of M6's probes (`bench/scale.ron`, `bench/probes.ron`, M6 B9) is one
    /// this sandbox takes: each arm's arguments, after the runner's own, parse with nothing unknown on every tier
    /// it names, and each row a pair's table shows is a row a bench of a fight or a walk logs. A switch only `dev`
    /// builds take (B6's `--measure-*`) is held in a `dev` build alone.
    #[test]
    fn every_benched_arm_is_a_flag_and_every_row_one_the_bench_logs() {
        use sapphire_serialize::ron::{Value, from_str};

        let key = |map: &Value, name: &str| match map {
            Value::Map(map) => map.get(&Value::String(name.to_owned())).cloned(),
            _ => None,
        };
        let strings = |value: Option<Value>| -> Vec<String> {
            match value {
                Some(Value::Seq(items)) => items
                    .into_iter()
                    .map(|item| match item {
                        Value::String(text) => text,
                        other => panic!("not a string: {other:?}"),
                    })
                    .collect(),
                _ => Vec::new(),
            }
        };
        let text = |value: Option<Value>| match value {
            Some(Value::String(text)) => text,
            other => panic!("not a string: {other:?}"),
        };
        let counted: Vec<&str> = counts::COUNT_ROWS.iter().map(|row| row.name).collect();
        let timed: Vec<&str> = ROWS.iter().map(|row| row.name).collect();
        let mut arms = 0;
        for (file, source) in [
            ("bench/scale.ron", include_str!("../../../bench/scale.ron")),
            ("bench/probes.ron", include_str!("../../../bench/probes.ron")),
        ] {
            let manifest: Value = from_str(source).unwrap_or_else(|error| panic!("{file}: {error}"));
            let data = key(&manifest, "data").expect("its data");
            let tiers: Vec<String> = match key(&data, "tiers") {
                Some(Value::Seq(tiers)) => tiers.iter().map(|tier| text(key(tier, "name"))).collect(),
                _ => panic!("{file} lists its tiers"),
            };
            let Some(Value::Seq(runs)) = key(&data, "runs") else { panic!("{file} lists its runs") };
            for run in &runs {
                let name = text(key(run, "name"));
                let on = strings(key(run, "tiers"));
                let variants: Vec<Vec<String>> = match key(run, "variants") {
                    Some(Value::Seq(variants)) if !variants.is_empty() => {
                        variants.iter().map(|variant| strings(key(variant, "args"))).collect()
                    }
                    _ => vec![Vec::new()],
                };
                for tier in tiers.iter().filter(|tier| on.is_empty() || on.contains(tier)) {
                    let tier_args = key(run, "tier_args").and_then(|args| key(&args, tier));
                    for variant in &variants {
                        let mut args: Vec<String> =
                            ["sapphire_sandbox", "--bench", "1", "--tier", tier, "--size", "64x36"]
                                .map(str::to_owned)
                                .to_vec();
                        args.extend(strings(key(run, "args")));
                        args.extend(strings(tier_args.clone()));
                        args.extend(variant.iter().cloned());
                        let args: Vec<String> =
                            args.iter().map(|arg| arg.replace("{replays}", "replays")).collect();
                        if cfg!(not(feature = "dev")) && args.iter().any(|arg| arg.starts_with("--measure-"))
                        {
                            continue;
                        }
                        let (options, unknown) = Options::parse(&args)
                            .unwrap_or_else(|error| panic!("{file} {name} on {tier}: {error}"));
                        assert!(unknown.is_empty(), "{file} {name} on {tier}: {unknown:?} unknown");
                        options.refuse_unknown(&unknown).unwrap();
                        arms += 1;
                    }
                }
                // A walk's or a fight's pair reads its rows from `fight row` and `count row` lines, or the GPU
                // phase's main depth (M6 B9); a plain bench's (one phase) reads its own fields, which aren't held here.
                let plain =
                    matches!(key(run, "phases"), Some(Value::Number(phases)) if phases.into_f64() == 1.0);
                if !plain {
                    for row in strings(key(run, "rows")) {
                        assert!(
                            timed.contains(&row.as_str())
                                || counted.contains(&row.as_str())
                                || row == "main_depth_prepass",
                            "{file} {name}: the bench logs no row {row:?}"
                        );
                    }
                }
            }
        }
        assert!(arms > 100, "{arms} arms parsed");
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
        assert!(
            !counts(cpu, &Traced { restart: Some(Duration::ZERO), ..frame }, Phase::Cpu),
            "a restart's however short"
        );
        assert!(!counts(gpu, &frame, Phase::Cpu), "never timed");
        let timed = Traced {
            gpu: GpuTimes { timed: true, span: Some(Duration::ZERO), ..GpuTimes::default() },
            ..frame
        };
        assert!(counts(gpu, &timed, Phase::Cpu));
    }

    /// Every frame a row leaves out is a frame of a reason reported on its own (M6 B3): a restart in any phase, one
    /// drawn at a tick outside `--bench-ticks` (M6 B5), and in the GPU phase a frame whose timings never came back or
    /// came back with a pair dropped (B3's review); a restart frame is reported once, as a restart, and a frame
    /// outside the ticks as that, timed or not.
    #[test]
    fn every_frame_a_row_leaves_out_is_reported_with_its_reason() {
        use sapphire_render::graph::PassTime;
        let cpu = &ROWS[0];
        let gpu = ROWS.iter().find(|row| row.name == "gpu frame").expect("the GPU frame's row");
        let main = ROWS.iter().find(|row| row.name == "gpu main").expect("the main pass's row");
        let timed = GpuTimes { timed: true, span: Some(Duration::from_micros(1)), ..GpuTimes::default() };
        // The main pass decoded, a pair after it didn't: timings back, short.
        let pass = PassTime {
            name: MAIN_PASS,
            start: Duration::ZERO,
            duration: Duration::from_micros(10),
            exclusive: Duration::from_micros(10),
        };
        let short = GpuTimings {
            frame: 3,
            every_pass: false,
            every_timed_pass: false,
            untimed: Vec::new(),
            passes: vec![pass],
            start: Some(Duration::from_secs(1)),
        };
        let dropped = GpuTimes::of(&short, 0, false);
        assert!(dropped.timed && dropped.span.is_none(), "{dropped:?}");
        let restart = Some(Duration::from_micros(7));
        let frames = [
            traced(1, 10),
            Traced { restart, ..traced(2, 10) },
            Traced { gpu: timed, ..traced(3, 10) },
            Traced { gpu: timed, restart, ..traced(4, 10) },
            Traced { outside: true, ..traced(5, 10) },
            Traced { gpu: timed, outside: true, ..traced(6, 10) },
            Traced { restart, outside: true, ..traced(7, 10) },
            Traced { gpu: dropped, ..traced(8, 10) },
            Traced { gpu: dropped, restart, ..traced(9, 10) },
            Traced { gpu: dropped, outside: true, ..traced(10, 10) },
        ];
        for phase in Phase::ALL {
            let row = if phase == Phase::Gpu { gpu } else { cpu };
            for frame in frames.map(|frame| Traced { phase, ..frame }) {
                let reasons: Vec<Excluded> = Excluded::ALL
                    .into_iter()
                    .filter(|reason| reason.applies(phase, true) && reason.holds(&frame, phase))
                    .collect();
                // Read by the row, or left out for one reason: never both, never neither.
                assert_eq!(counts(row, &frame, phase), reasons.is_empty(), "{phase:?} {frame:?}");
                assert!(reasons.len() <= 1, "{reasons:?}");
                if frame.restart.is_some() {
                    assert_eq!(reasons, [Excluded::Restart]);
                } else if frame.outside {
                    assert_eq!(reasons, [Excluded::Outside]);
                } else if phase == Phase::Gpu && frame.gpu == dropped {
                    assert_eq!(reasons, [Excluded::Incomplete], "a pair dropped, its span with it");
                }
            }
        }
        // A pass row reads the passes of it that decoded, as `GpuTimes::of` sums them.
        let short = Traced { phase: Phase::Gpu, gpu: dropped, ..traced(8, 10) };
        assert!(counts(main, &short, Phase::Gpu) && !counts(gpu, &short, Phase::Gpu));
        assert!(!Excluded::Untimed.applies(Phase::Cpu, true), "a CPU phase times no GPU");
        assert!(
            !Excluded::Incomplete.applies(Phase::Cpu, true)
                && Excluded::Incomplete.applies(Phase::Gpu, false)
        );
        assert!(!Excluded::Outside.applies(Phase::Gpu, false), "a run with no ticks draws none outside them");
        assert!(Excluded::Restart.applies(Phase::Cpu, false) && Excluded::Untimed.applies(Phase::Gpu, false));
    }

    /// A frame's passes overlap only on Metal, and not when they run serially (M6 B2's review): a serial run's GPU
    /// reading says so, and the report then judges each pass row on every frame and the prepasses and shadows apart.
    #[test]
    fn passes_run_serially_overlap_nothing() {
        assert!(passes_overlap(wgpu::Backend::Metal, false));
        assert!(!passes_overlap(wgpu::Backend::Metal, true), "each pass alone on the GPU");
        assert!(!passes_overlap(wgpu::Backend::Vulkan, false) && !passes_overlap(wgpu::Backend::Dx12, true));
    }

    /// The GPU reading counts the frames the rows read, and reads the display's lock over the sitting's (M6 B5): a
    /// frame outside `--bench-ticks` is the sitting's, a restart's isn't, and an untimed one is neither.
    #[test]
    fn the_gpu_reading_reads_the_lock_over_frames_outside_the_ticks() {
        let timed = GpuTimes { timed: true, ..GpuTimes::default() };
        let restart = Some(Duration::from_micros(7));
        let frames = [
            Traced { gpu: timed, ..traced(1, 10) },
            Traced { gpu: timed, outside: true, ..traced(2, 10) },
            Traced { gpu: timed, restart, ..traced(3, 10) },
            Traced { gpu: timed, restart, outside: true, ..traced(4, 10) },
            Traced { outside: true, ..traced(5, 10) },
            traced(6, 10),
        ];
        let (read, sitting) = reading_frames(&frames.iter().collect::<Vec<_>>());
        assert_eq!(read, [0]);
        assert_eq!(sitting, [0, 1]);
    }

    /// What the bench adds a frame for the thread's CPU time allocates nothing (M6 B3): the clock's reads, the rows
    /// that read them and the reasons a frame is left out.
    #[test]
    fn the_threads_cpu_time_and_its_rows_allocate_nothing_a_frame() {
        assert!(alloc::is_installed(), "the sandbox counts its allocations");
        let frame = Traced {
            restart: Some(Duration::from_micros(5)),
            cpu_on_core: Some(Duration::ZERO),
            ..traced(3, 100)
        };
        let rows: Vec<&Row> =
            ROWS.iter().filter(|row| row.name.ends_with(" on core") || row.name == "record").collect();
        assert_eq!(rows.len(), 4);
        let scope = AllocScope::begin();
        for _ in 0..100 {
            let mut on_core = OnCore::start();
            std::hint::black_box(on_core.lap());
            std::hint::black_box(on_core.lap());
            for row in &rows {
                std::hint::black_box((row.read)(&frame));
            }
            for reason in Excluded::ALL {
                std::hint::black_box(reason.holds(&frame, Phase::Gpu));
            }
        }
        assert_eq!(scope.counts().allocations, 0);
    }

    /// The thread's CPU time over a span of work is some of its wall time, where the OS says.
    #[test]
    fn a_span_of_work_reads_on_its_threads_core() {
        let mut on_core = OnCore::start();
        let wall = Stopwatch::start();
        let mut sum = 0_u64;
        for i in 0..5_000_000_u64 {
            sum = sum.wrapping_add(std::hint::black_box(i));
        }
        std::hint::black_box(sum);
        let wall = wall.elapsed();
        if let Some(cpu) = on_core.lap() {
            assert!(cpu > Duration::ZERO && cpu <= wall + Duration::from_millis(5), "{cpu:?} of {wall:?}");
        }
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
            (read.start, read.end),
            (Some(Duration::from_secs(5)), Some(Duration::from_secs(5) + Duration::from_micros(20))),
            "and a start and an end on the GPU's clock"
        );
        assert_eq!(read.shadows, Some(Duration::from_micros(10)), "the cascades that ran");
        assert_eq!(read.main, Some(Duration::from_micros(10)));
        assert_eq!(read.taa, None, "nothing of it ran");
        assert_eq!(read.depth_only, Some(Duration::from_micros(10)), "the depth passes that ran (M5 Z2)");
        timings.untimed.push(shadows::PASSES[1]);
        let untimed = GpuTimes::of(&timings, 0, false);
        assert_eq!((untimed.shadows, untimed.depth_only), (None, None), "a cascade ran untimed");
        timings.every_timed_pass = false;
        let dropped = GpuTimes::of(&timings, 0, false);
        assert_eq!(
            (dropped.span, dropped.shadows, dropped.start, dropped.end),
            (None, None, None, None),
            "a pair was dropped"
        );
        assert_eq!(dropped.main, Some(Duration::from_micros(10)));
    }

    /// An overlapped phase's reading (M6 B7) takes each frame's submit and its GPU's start and end, and leaves out the
    /// GPU times of a restart's frame and of one whose span didn't read; a bench that didn't overlap has none.
    #[test]
    fn an_overlapped_phase_reads_its_frames_gpu_times_but_not_a_restarts() {
        let ms = Duration::from_millis;
        let gpu = |at: u64| GpuTimes {
            timed: true,
            start: Some(ms(at)),
            end: Some(ms(at + 4)),
            ..GpuTimes::default()
        };
        let frame = |at: u64| Traced { submitted: Some(ms(at)), gpu: gpu(at + 1), ..traced(at, 2_000) };
        let frames = [
            frame(0),
            Traced { restart: Some(ms(1)), ..frame(10) },
            frame(20),
            Traced { gpu: GpuTimes { timed: true, ..GpuTimes::default() }, ..frame(30) },
            frame(40),
            frame(50),
        ];
        let samples = overlap_samples(frames.iter());
        let read: Vec<_> = samples.iter().map(|sample| (sample.submitted, sample.gpu.is_some())).collect();
        assert_eq!(
            read,
            [(ms(0), true), (ms(10), false), (ms(20), true), (ms(30), false), (ms(40), true), (ms(50), true)]
        );
        assert_eq!(samples[0].gpu, Some((ms(1), ms(5))));
        assert_eq!(samples[5].cpu, ms(2));
        assert_eq!(
            overlap::read(&samples).map(|reading| reading.frames),
            Some(1),
            "only 40 then 50 in a row"
        );
        assert!(overlap_samples(frames.map(|frame| Traced { submitted: None, ..frame }).iter()).is_empty());
    }

    /// A trace's lines have as many fields as its header, whatever's in them.
    #[test]
    fn a_trace_line_matches_its_header() {
        let line = trace_line(&traced(12, 345));
        let header = trace_header();
        assert_eq!(line.split(',').count(), header.split(',').count(), "{line}");
        assert!(line.starts_with("cpu,0,0,12,1,345.0,"), "{line}");
    }

    /// The thread's CPU time is the three columns after every older one (M6 B3), empty where the OS doesn't say, and
    /// the effects' pixels the last (M6 B5).
    #[test]
    fn a_trace_line_ends_with_the_threads_cpu_time_and_the_effects_pixels() {
        let header = trace_header();
        assert!(
            header.starts_with(TRACE_HEADER)
                && header.ends_with(&format!("{ON_CORE_COLUMNS}{EFFECT_COLUMNS}"))
        );
        let mut frame = traced(12, 345);
        frame.cpu_on_core = Some(Duration::from_micros(300));
        frame.laps.record_on_core = Some(Duration::from_nanos(41_300));
        frame.counted.effect_pixels = 51_840;
        let line = trace_line(&frame);
        assert!(line.ends_with(",300.0,41.3,,51840"), "{line}");
        assert!(trace_line(&traced(12, 345)).ends_with(",,,0"));
    }

    /// The trace's `gpu_end` column holds the frame's end on the GPU's clock in µs, and is empty untimed (M5 Z2).
    #[test]
    fn a_trace_line_holds_the_frames_gpu_end() {
        let header = trace_header();
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

    /// The depth-only row is each frame's prepasses and shadows together, against the sum of their figures (M5
    /// Z2); `GpuTimes::of` sums the depth passes that ran.
    #[test]
    fn the_depth_only_row_is_the_prepasses_and_shadows_together() {
        let row = ROWS.iter().find(|row| row.name == "gpu depth only").expect("the row");
        let figures = |name: &str| ROWS.iter().find(|row| row.name == name).expect("a row").budget;
        let (prepasses, shadows) = (figures("gpu prepasses"), figures("gpu shadows"));
        for tier in [TierId::DesktopLow, TierId::MobileLow] {
            let column = tier_column(tier);
            assert_eq!(
                row.budget[column],
                prepasses[column].zip(shadows[column]).map(|(a, b)| a + b),
                "{tier:?}"
            );
        }
        let mut frame = traced(1, 100);
        frame.gpu.depth_only = Some(Duration::from_micros(3_200));
        assert_eq!((row.read)(&frame), Some(Duration::from_micros(3_200)));
    }

    /// A frame has an upload in flight when bytes went up in it, the two frames before or the one after (M5 Z2).
    #[test]
    fn an_upload_is_in_flight_from_two_frames_before_to_one_after() {
        let flight = uploads_in_flight(&[0, 0, 0, 0, 7, 0, 0, 0, 0]);
        assert_eq!(flight, [false, false, false, true, true, true, true, false, false]);
        assert!(uploads_in_flight(&[]).is_empty());
    }

    /// The display's lock is near 1 for slow frames ending at one phase of a 60 Hz beat, and near 0 for ends
    /// spread over it; too few frames read none (M5 Z2).
    #[test]
    fn slow_frames_at_one_phase_of_60_hz_read_as_the_displays_lock() {
        let beat = 1e6 / 60.0;
        let locked: Vec<f64> = (0..200).map(|frame| 5e9 + f64::from(frame * 7) * beat + 40.0).collect();
        let (lock, period) = display_lock(&locked).expect("enough frames");
        assert!(lock > 0.99 && (period - beat).abs() < 0.2, "{lock} at {period}");
        // Ends a golden-ratio step apart in phase spread evenly over the beat.
        let spread: Vec<f64> = (0..200).map(|frame| 5e9 + f64::from(frame) * beat * 7.618_034).collect();
        let (lock, _) = display_lock(&spread).expect("enough frames");
        assert!(lock < 0.1, "{lock}");
        assert_eq!(display_lock(&locked[..LOCK_FEWEST - 1]), None);
    }

    /// The uploads' cost: the share of frames over 1.15 times the median, with an upload in flight and without.
    #[test]
    fn the_slow_shares_split_by_uploads_in_flight() {
        let us = Duration::from_micros;
        let values = [
            (us(100), false),
            (us(100), false),
            (us(100), false),
            (us(120), false),
            (us(200), true),
            (us(100), true),
        ];
        let (flight, quiet) = slow_shares(&values);
        assert_eq!((flight, quiet), (50.0, 25.0));
        assert_eq!(slow_shares(&[]), (0.0, 0.0));
    }
}
