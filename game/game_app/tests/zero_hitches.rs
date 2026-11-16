//! Exit test 1, the scripted playthrough (M4 H4): the game booted as its window boots it, then the title and a
//! new game; the Lantern Ward's whole walk (`tests/replays/ward-whole-walk.sreplay`, as `wins_the_ward` plays it)
//! with its cutscene, conversation, encounters and boss; at the gate cutscene's first line a pause, where every
//! setting a player can step is pressed round its values (the language among them, as H3's walk does), the
//! quality tier is changed by its row and changed back, and heat forced to critical walks the thermal ladder
//! down and, cooled, back up; then on to the boss's defeat, and back to the title under the restart's wipe.
//!
//! After every frame, from the boot screen's first, the renderer's ledger is read: a pipeline built on a first
//! use or on the frame's thread, or a frame that waited for one, fails the lane, naming where the walk was (the
//! boot's step, or the flow's state, the tick and what was pressed) and the pipeline. The lane shows its own
//! check failing at the end, on a frame drawing a debug line the game never declares.
//!
//! Load screens (the boot screen, a wipe, a zone trip) are timed, each frame from its input to its GPU work done,
//! and the worst frame of each is reported against twice the tier's interval, exit test 1's limit. With
//! `SAPPHIRE_HITCH_TIMED=1` a frame over it fails the lane: a run on Metal, from a cold cache, alone on the
//! machine (`docs/m4-plan.md` H4). Anywhere else the times are only read. Asked to run timed without
//! `--ignored`, the lane fails rather than skip: a timed run is judged by its exit code (M4 H6).
//!
//! A timed run traces the boot screen's frames too (M5 X3): each frame says where its time went on its
//! thread (`Boot::trace_frames`: the step's own work, the screen's layout and uploads, its submit, each in wall
//! and CPU time), how long it waited for its own submit on the GPU and then for what other threads submitted
//! after it, whether the queue held another thread's work as it started, how many pipelines the warm-up's
//! threads were building as it started and ended, what the process read from the disk meanwhile, how busy the
//! machine's cores were and which kind the frame's thread ended on. Every frame past the tier's interval prints
//! a `boot frame` line, the worst among them always, and a `boot trace` line sums the boot up.
//!
//! The process's footprint (`sapphire_platform::process_memory`) is read after every frame too, and the most
//! each part of the walk held is reported with the process's own most: the boot, the title, the walk, a fight,
//! the pause, a load screen and the title again (spec §3.7, M4 Z1).
//!
//! - `SAPPHIRE_HITCH_TIER=<key>`: the tier the player holds (default `desktop-high`), the class's other tier the
//!   one the pause changes to and back. A phone's tier is walked on a phone's class of device over this one's GPU
//!   (M4 H9), so the graphics row's automatic is the tier a phone's rules choose, never this machine's own.
//! - `SAPPHIRE_HITCH_SIZE=<w>x<h>`: what's drawn into (default 480x270). The pause's pages draw into at least
//!   480x270, so that every page's rows lay out as a player's do.
//! - `SAPPHIRE_HITCH_DRAW_EVERY=<n>`: one frame in `n` drawn (default 1). The rest are prepared and recorded but
//!   not run (`Stage::lose_frames`), so their first uses still count: CI's lavapipe can't draw them all in its
//!   job's time. Load screens and the frames the tier or a rung changes on are always drawn.
//! - `SAPPHIRE_HITCH_BOOT_ONLY=1`: the boot alone, reported (and traced, if timed), then the lane ends: what a
//!   run of many cold boots times, a few seconds each where the walk takes minutes (M5 X3).
//!
//! The platform's loop must own the process's main thread, so this test supplies its own `main`
//! (`harness = false`). It needs a GPU and cooked content: `cargo xtask content`, then
//! `cargo test -p game_app --features dev --test zero_hitches -- --ignored`.

mod drive;

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use drive::{Draw, Driver};
use game_app::tiers::{Graphics, TierFollower};
use game_app::{GameApp, GameShow};
use game_flow::boot::{Boot, BootFrameTrace, BootLap};
use game_flow::flow::FlowState;
use game_flow::game::GameSetup;
use game_ui::menus::{MenuDocument, PlayerSettings, QualityChoice, Setting};
use sapphire_core::debug_draw;
use sapphire_core::target::FormFactor;
use sapphire_core::thermal::{ThermalReading, ThermalState};
use sapphire_gameplay::{EncounterPhase, Simulation};
use sapphire_math::Vec3;
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, CoreKind, CoreTicks, Event, Platform, ProcessReads, Scancode,
    SystemInfo, core_ticks, current_core, process_memory, process_reads, thread_cpu_time,
};
use sapphire_render::quality::{QualitySettings, TierId, TierTable};
use sapphire_render::{PipelineStats, Renderer, WarmUpBuilds, warm_up_builds};
use sapphire_rhi::{GpuContext, wgpu};
use sapphire_stage::audio::SoundsLoad;
use sapphire_stage::{FrameLaps, Stage, headless_gpu};
use sapphire_time::Stopwatch;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// The replay, from the repository's root.
const WALK: &str = "tests/replays/ward-whole-walk.sreplay";
/// Longer than the walk: fifteen minutes of play, the cutscene's held frames among them.
const MOST_STEPS: u32 = 15 * 60 * 60;
/// The title's frames before its row's pressed: long enough for what its first draw opened to come in.
const TITLE_FRAMES: u32 = 60;
/// How long heat stays critical, in frames.
const HOT_FRAMES: u32 = 60;
/// Longer than any climb back up a ladder takes: every rung's hold, at 60 frames a second, and some.
const MOST_CLIMB_FRAMES: u32 = 4 * 60 * 60;
/// Longer than a tier's renderer takes to build off the frame, even on lavapipe.
const MOST_CHANGE: Duration = Duration::from_secs(600);
/// Longer than the show's sounds take to decode off the frame, even on CI's runners.
const MOST_SOUND_FRAMES: u32 = 30 * 60;
/// Longer than a win takes to go back to the title under its wipe.
const MOST_RETURN_FRAMES: u32 = 30 * 60;
/// Frames a second where a tier has no rate of its own.
const DEFAULT_RATE: u32 = 60;
/// The least the pause's pages are drawn into: every page's rows lay out as a player's do.
const PAGES: (u32, u32) = (480, 270);

/// The tier the player holds: `SAPPHIRE_HITCH_TIER`, or Desktop-High.
fn held_tier() -> Result<TierId, String> {
    let Ok(name) = std::env::var("SAPPHIRE_HITCH_TIER") else {
        return Ok(TierId::DesktopHigh);
    };
    let shipped: Vec<TierId> = TierTable::shipped().tiers().iter().map(|tier| tier.id).collect();
    TierId::from_key(name.trim()).filter(|tier| shipped.contains(tier)).ok_or_else(|| {
        let keys: Vec<&str> = shipped.iter().map(|tier| tier.key()).collect();
        format!("SAPPHIRE_HITCH_TIER: {name:?} isn't a shipped tier: {}", keys.join(", "))
    })
}

/// The device class `tier` is of, and the graphics page's choice that holds it.
fn choice_of(tier: TierId) -> (FormFactor, QualityChoice) {
    match tier {
        TierId::DesktopHigh => (FormFactor::Desktop, QualityChoice::High),
        TierId::DesktopLow => (FormFactor::Desktop, QualityChoice::Low),
        TierId::MobileHigh => (FormFactor::Mobile, QualityChoice::High),
        TierId::MobileLow => (FormFactor::Mobile, QualityChoice::Low),
    }
}

/// The class's other choice: the one the pause changes to.
fn other(choice: QualityChoice) -> QualityChoice {
    if choice == QualityChoice::High { QualityChoice::Low } else { QualityChoice::High }
}

/// `SAPPHIRE_HITCH_SIZE`, or 480 × 270.
fn size() -> Result<(u32, u32), String> {
    let Ok(raw) = std::env::var("SAPPHIRE_HITCH_SIZE") else {
        return Ok((480, 270));
    };
    let parsed = raw.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
    parsed
        .filter(|&(w, h): &(u32, u32)| w > 0 && h > 0)
        .ok_or_else(|| format!("SAPPHIRE_HITCH_SIZE: {raw:?}"))
}

/// `SAPPHIRE_HITCH_DRAW_EVERY`, or 1.
fn draw_every() -> u32 {
    std::env::var("SAPPHIRE_HITCH_DRAW_EVERY")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(1)
}

fn timed() -> bool {
    std::env::var("SAPPHIRE_HITCH_TIMED").is_ok_and(|raw| raw == "1")
}

/// `SAPPHIRE_HITCH_BOOT_ONLY=1`: the boot alone.
fn boot_only() -> bool {
    std::env::var("SAPPHIRE_HITCH_BOOT_ONLY").is_ok_and(|raw| raw == "1")
}

/// Whether a frame built or waited for a pipeline: exit test 1's counts, all three.
fn hitched(pipelines: PipelineStats) -> bool {
    pipelines.first_use_builds > 0 || pipelines.frame_thread_builds > 0 || pipelines.frame_waits > 0
}

/// What a frame that hitched says, naming `place` and the pipeline.
fn hitch(place: &str, renderer: &Renderer) -> AppInitError {
    let built = renderer.stats().pipelines;
    AppInitError::msg(format!(
        "a frame hitched at {place}: {} first use, {} built on the frame's thread, {} waited for: {}",
        built.first_use_builds,
        built.frame_thread_builds,
        built.frame_waits,
        renderer.first_use_report()
    ))
}

/// Waits for the GPU's work, so a timed frame holds it.
fn finish_gpu(gpu: &GpuContext) {
    // A lost device's poll fails, and the frame's own checks say so.
    let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
}

/// Twice the interval at `rate` frames a second: exit test 1's limit for a load screen's frame.
fn limit_at(rate: Option<NonZeroU32>) -> Duration {
    Duration::from_secs(2) / rate.map_or(DEFAULT_RATE, NonZeroU32::get)
}

/// One load screen's frames: how many, the worst and what it was doing, and the limit.
struct LoadScreen {
    name: String,
    frames: u32,
    worst: Duration,
    /// How much of the worst frame waited for the GPU's work, after the frame's own (M4 H8).
    worst_waited: Duration,
    /// The boot's step the worst frame was in, for the boot screen's (M4 H8).
    worst_in: Option<&'static str>,
    /// Where the worst frame of a stood stage went before its GPU wait: `Stage`'s own laps (M4 H8).
    worst_laps: Option<FrameLaps>,
    limit: Duration,
    /// Whether a timed run fails on a frame over `limit`, or only says so.
    held: bool,
}

impl LoadScreen {
    fn new(name: String, limit: Duration) -> Self {
        Self {
            name,
            frames: 0,
            worst: Duration::ZERO,
            worst_waited: Duration::ZERO,
            worst_in: None,
            worst_laps: None,
            limit,
            held: true,
        }
    }

    /// Frames timed against `limit` and reported, which a timed run doesn't fail on.
    fn reported(name: String, limit: Duration) -> Self {
        Self { held: false, ..Self::new(name, limit) }
    }

    /// A frame that `took` this long, `waited` of it for the GPU.
    fn frame(&mut self, took: Duration, waited: Duration) {
        self.frames += 1;
        if took > self.worst {
            (self.worst, self.worst_waited) = (took, waited);
        }
    }

    /// A frame of the stood stage, its laps `laps`.
    fn stage_frame(&mut self, took: Duration, waited: Duration, laps: FrameLaps) {
        if took > self.worst {
            self.worst_laps = Some(laps);
        }
        self.frame(took, waited);
    }

    /// A frame of the boot, in its step `step`.
    fn boot_frame(&mut self, took: Duration, waited: Duration, step: &'static str) {
        if took > self.worst {
            self.worst_in = Some(step);
        }
        self.frame(took, waited);
    }

    /// Prints its line, saying whether a timed run fails on it.
    fn report(&self) -> bool {
        let ms = |duration: Duration| duration.as_secs_f64() * 1e3;
        let step = match (self.worst_in, self.worst_laps) {
            (Some(step), _) => format!(" in the boot's {step} step"),
            (None, Some(laps)) => format!(
                " (content {:.1}, packet {:.1}, prepare {:.1}, record and submit {:.1})",
                ms(laps.content),
                ms(laps.packet),
                ms(laps.prepare),
                ms(laps.record)
            ),
            (None, None) => String::new(),
        };
        eprintln!(
            "zero_hitches: {}: {} frames, the worst {:.1} ms{step} ({:.1} of it waiting for the GPU) against \
             {:.1} ms{}",
            self.name,
            self.frames,
            ms(self.worst),
            ms(self.worst_waited),
            ms(self.limit),
            if self.held { "" } else { ", reported only" }
        );
        self.held && self.worst > self.limit
    }
}

/// What a traced boot frame reads before it starts and after its GPU work is done (M5 X3).
#[derive(Clone, Copy)]
struct Around {
    builds: WarmUpBuilds,
    reads: Option<ProcessReads>,
    ticks: Option<CoreTicks>,
    /// The frame's thread's CPU time.
    cpu: Option<Duration>,
}

impl Around {
    fn read() -> Self {
        Self { builds: warm_up_builds(), reads: process_reads(), ticks: core_ticks(), cpu: thread_cpu_time() }
    }
}

/// One boot screen frame, traced.
struct TracedFrame {
    frame: u32,
    took: Duration,
    laps: BootFrameTrace,
    /// The queue held work another thread submitted, as the frame started: the frame before waited for all of
    /// its own.
    queue_busy: bool,
    /// Waiting for the frame's own submit, the step's uploads before it among it; then for whatever other
    /// threads submitted after it.
    own_wait: Duration,
    rest_wait: Duration,
    before: Around,
    after: Around,
    /// The kind of core the frame's thread was on as it ended.
    core: Option<CoreKind>,
}

impl TracedFrame {
    /// Its `boot frame` line: `key=value` fields, times in ms, a lap as its wall time and its thread's CPU time.
    fn line(&self, worst: bool) -> String {
        let ms = |duration: Duration| format!("{:.2}", duration.as_secs_f64() * 1e3);
        let lap = |lap: BootLap| format!("{}/{}", ms(lap.wall), lap.cpu.map_or("-".to_owned(), ms));
        let (before, after) = (self.before, self.after);
        let reads = after.reads.zip(before.reads).map(|(after, before)| after.since(before));
        let ticks = after.ticks.zip(before.ticks).map(|(after, before)| after.since(before));
        let share = |share: Option<f32>| share.map_or("-".to_owned(), |share| format!("{share:.2}"));
        let cpu = after.cpu.zip(before.cpu).map(|(after, before)| after.saturating_sub(before));
        format!(
            "zero_hitches: boot frame {}: step={} left_in={} took={} advance={} prepare={} submit={} own_wait={} \
             rest_wait={} queue_busy={} building={}/{} started={} finished={} disk_kib={} page_ins={} \
             thread_cpu={} p_busy={} e_busy={} core={}{}",
            self.frame,
            self.laps.step,
            self.laps.left_in,
            ms(self.took),
            lap(self.laps.advance),
            lap(self.laps.prepare),
            lap(self.laps.submit),
            ms(self.own_wait),
            ms(self.rest_wait),
            u8::from(self.queue_busy),
            before.builds.building,
            after.builds.building,
            after.builds.started.saturating_sub(before.builds.started),
            after.builds.finished().saturating_sub(before.builds.finished()),
            reads.map_or("-".to_owned(), |reads| (reads.disk_bytes / 1024).to_string()),
            reads.map_or("-".to_owned(), |reads| reads.page_ins.to_string()),
            cpu.map_or("-".to_owned(), ms),
            share(ticks.and_then(|ticks| ticks.performance.busy_share())),
            share(ticks.and_then(|ticks| ticks.efficiency.busy_share())),
            self.core.map_or("-", CoreKind::name),
            if worst { " worst=1" } else { "" },
        )
    }
}

/// The boot screen's frames, traced (M5 X3): a timed run's.
struct BootTrace {
    frames: Vec<TracedFrame>,
    /// The tier's interval: a frame past it prints its line.
    interval: Duration,
    limit: Duration,
}

impl BootTrace {
    /// Prints every frame past the tier's interval, the worst always, and the boot's sum.
    fn report(&self) {
        let worst =
            self.frames.iter().enumerate().max_by_key(|(_, frame)| frame.took).map(|(index, _)| index);
        for (index, frame) in self.frames.iter().enumerate() {
            let is_worst = Some(index) == worst;
            if frame.took > self.interval || is_worst {
                eprintln!("{}", frame.line(is_worst));
            }
        }
        let count = |over: Duration| self.frames.iter().filter(|frame| frame.took > over).count();
        let busy = self.frames.iter().filter(|frame| frame.queue_busy).count();
        let most_building = self
            .frames
            .iter()
            .map(|frame| frame.before.builds.building.max(frame.after.builds.building))
            .max();
        let read = |frame: &TracedFrame| frame.after.reads.zip(frame.before.reads).map(|(a, b)| a.since(b));
        let disk: u64 = self.frames.iter().filter_map(read).map(|reads| reads.disk_bytes).sum();
        let page_ins: u64 = self.frames.iter().filter_map(read).map(|reads| reads.page_ins).sum();
        let ms = |duration: Duration| duration.as_secs_f64() * 1e3;
        eprintln!(
            "zero_hitches: boot trace: frames={} over_interval={} over_limit={} interval_ms={:.2} limit_ms={:.2} \
             queue_busy_frames={busy} most_building={} disk_kib={} page_ins={page_ins}",
            self.frames.len(),
            count(self.interval),
            count(self.limit),
            ms(self.interval),
            ms(self.limit),
            most_building.unwrap_or(0),
            disk / 1024,
        );
    }
}

/// What a frame added to `Stage`'s laps, which add up until reset: the four the report reads.
fn laps_since(before: FrameLaps, after: FrameLaps) -> FrameLaps {
    let mut laps = after;
    laps.content = after.content.saturating_sub(before.content);
    laps.packet = after.packet.saturating_sub(before.packet);
    laps.prepare = after.prepare.saturating_sub(before.prepare);
    laps.record = after.record.saturating_sub(before.record);
    laps
}

/// The most footprint each part of the walk held, in the order the parts first came, and the process's own most.
#[derive(Default)]
struct Memory {
    parts: Vec<(&'static str, u64)>,
    most: u64,
}

impl Memory {
    /// Reads the process's footprint into `part`'s most. An OS that can't say leaves it unread.
    fn read(&mut self, part: &'static str) {
        let Some(read) = process_memory() else { return };
        self.most = self.most.max(read.most);
        match self.parts.iter_mut().find(|(name, _)| *name == part) {
            Some((_, most)) => *most = (*most).max(read.now),
            None => self.parts.push((part, read.now)),
        }
    }

    /// Which part of the walk a frame of the stood game is in.
    fn part(stage: &Stage<GameShow>, what: &str) -> &'static str {
        let fighting = || {
            let combat = stage.show.shown.fight.combat.as_ref();
            let zone = combat.and_then(|combat| combat.simulation().zone());
            zone.is_some_and(|zone| {
                zone.encounters().iter().any(|held| held.phase() == EncounterPhase::Running)
            })
        };
        match stage.show.game().map(|game| game.flow().state()) {
            _ if Watch::load_screen(stage) => "a load screen",
            Some(FlowState::Title { .. }) if what == "the title, again" => "the title again",
            Some(FlowState::Title { .. }) => "the title",
            Some(FlowState::Paused { .. }) => "the pause",
            _ if fighting() => "a fight",
            _ => "the walk",
        }
    }
}

/// How the lane draws a frame of the stood game, and what it reads after: the window's tier follower first, as
/// `GameApp` runs it before a frame, then the frame drawn or lost, timed, and the ledger checked.
struct Watch {
    target: wgpu::TextureView,
    /// What the pause's pages draw into, while `on_pages`.
    pages: wgpu::TextureView,
    on_pages: bool,
    tiers: TierFollower,
    /// The heat the platform reports.
    heat: ThermalReading,
    every: u32,
    frames: u32,
    drawn: u32,
    /// Every load screen so far, the last still up if `in_load`.
    screens: Vec<LoadScreen>,
    in_load: bool,
    /// The frames a tier changed on, and how long each took.
    changes: Vec<(TierId, Duration)>,
    /// The longest frame drawn outside a load screen, and where.
    worst: (Duration, String),
    /// The title's first frames, while the show's sounds read and decode (M4 H8) and on until the fighters' fades
    /// the boot left to it are built (M4 C7), which the new game waits for: timed against a load screen's
    /// limit, as the first frames the player sees after the boot screen, and reported. The first is the stage's
    /// first on the GPU, which waits for it about 27 ms on Desktop-High on an M5 (spec §3.7), so a run doesn't
    /// fail on them. Some while they're counted.
    title: Option<LoadScreen>,
    memory: Memory,
}

impl Watch {
    /// Whether a load screen's up: a wipe on its way, covering or going, or a zone trip.
    fn load_screen(stage: &Stage<GameShow>) -> bool {
        let shown = &stage.show.shown;
        shown.wipe.as_ref().is_some_and(|wipe| !wipe.clear()) || shown.fight.show.trip_phase().is_some()
    }

    /// Where the walk is, for a failure to name.
    fn place(stage: &Stage<GameShow>, what: &str) -> String {
        let state = stage.show.game().map(|game| game.flow().state());
        let tick = stage.show.shown.fight.combat.as_ref().map(|combat| combat.simulation().tick());
        format!("{what} ({:?} at tick {tick:?}, {})", state, stage.quality.active().id)
    }
}

impl Draw for Watch {
    fn draw(
        &mut self,
        stage: &mut Stage<GameShow>,
        began: &Stopwatch,
        what: &str,
    ) -> Result<(), AppInitError> {
        if let Some(chosen) = stage.show.take_changed_settings() {
            self.tiers.follow_settings(&chosen, stage);
        }
        let (changes, depth) = (self.tiers.changes().len(), stage.quality.ladder().state().depth);
        // The pacing it returns is the platform's: a lane times frames as fast as they come.
        let _ = self.tiers.frame(stage, self.heat);
        let changed = self.tiers.changes().len() > changes;
        let load = Self::load_screen(stage);
        let draw = load || changed || self.frames.is_multiple_of(self.every);
        stage.lose_frames = !draw;
        let target = if self.on_pages { &self.pages } else { &self.target };
        let before = stage.laps;
        stage.render(target, 1.0, |_, _| {})?;
        let laps = laps_since(before, stage.laps);
        let recorded = began.elapsed();
        if draw {
            finish_gpu(&stage.gpu);
        }
        stage.check_gpu()?;
        let took = began.elapsed();
        let waited = took.saturating_sub(recorded);
        self.frames += 1;
        self.drawn += u32::from(draw);
        if hitched(stage.renderer.stats().pipelines) {
            return Err(hitch(&Self::place(stage, what), &stage.renderer));
        }
        let moved = stage.quality.ladder().state().depth != depth;
        if changed {
            self.changes.push((stage.quality.active().id, took));
        } else if load {
            if !self.in_load {
                let name = format!("load screen a wipe, at {}", Self::place(stage, what));
                self.screens.push(LoadScreen::new(name, limit_at(stage.quality.frame_rate())));
            }
            self.screens.last_mut().expect("pushed").stage_frame(took, waited, laps);
        } else {
            if let Some(title) = self.title.as_mut().filter(|_| draw) {
                title.stage_frame(took, waited, laps);
            }
            if draw && !moved && took > self.worst.0 {
                self.worst = (took, Self::place(stage, what));
            }
        }
        self.in_load = load;
        self.memory.read(Memory::part(stage, what));
        Ok(())
    }
}

type Walk = Driver<Watch>;

fn state(walk: &Walk) -> FlowState {
    walk.stage.show.game().expect("the content names a flow").flow().state()
}

fn simulation(walk: &Walk) -> &Simulation {
    walk.stage.show.shown.fight.combat.as_ref().expect("a run stands").simulation()
}

/// Boots the game as its window does, a frame at a time into `target`, each timed and its ledger read, to the
/// stage it hands back; each frame traced into `trace`, if given.
fn boot(
    boot: &mut Boot,
    target: &wgpu::TextureView,
    screen: &mut LoadScreen,
    memory: &mut Memory,
    mut trace: Option<&mut BootTrace>,
) -> Result<Stage<GameShow>, AppInitError> {
    let dt = (Duration::from_secs(1) / DEFAULT_RATE).as_secs_f32();
    boot.trace_frames(trace.is_some());
    loop {
        // Read before the frame's clock starts, so a traced frame times what an untraced one does.
        let before = trace.is_some().then(|| {
            // Without waiting: the frame before waited for everything, so work here is another thread's.
            let polled = boot.gpu().device().poll(wgpu::PollType::Poll);
            (Around::read(), !polled.is_ok_and(|status| status.is_queue_empty()))
        });
        let began = Stopwatch::start();
        let step = boot.step().name();
        let stood = boot.frame(Some(target), dt)?;
        let renderer = stood.as_ref().map(|stage| &stage.renderer).or(boot.renderer());
        let recorded = began.elapsed();
        let (mut own_wait, mut rest_wait) = (Duration::ZERO, Duration::ZERO);
        match boot.last_frame().filter(|_| trace.is_some()).and_then(|laps| laps.submission.clone()) {
            Some(own) => {
                let waiting = Stopwatch::start();
                let own = wgpu::PollType::Wait { submission_index: Some(own), timeout: None };
                let polled = boot.gpu().device().poll(own);
                own_wait = waiting.elapsed();
                if !polled.is_ok_and(|status| status.is_queue_empty()) {
                    let waiting = Stopwatch::start();
                    finish_gpu(boot.gpu());
                    rest_wait = waiting.elapsed();
                }
            }
            None => finish_gpu(boot.gpu()),
        }
        let took = began.elapsed();
        if let (Some(trace), Some((before, queue_busy))) = (trace.as_deref_mut(), before) {
            let frame = trace.frames.len() as u32;
            let laps = boot.last_frame().cloned().unwrap_or_default();
            let (after, core) = (Around::read(), current_core());
            trace.frames.push(TracedFrame {
                frame,
                took,
                laps,
                queue_busy,
                own_wait,
                rest_wait,
                before,
                after,
                core,
            });
        }
        screen.boot_frame(took, took.saturating_sub(recorded), step);
        memory.read("the boot");
        if let Some(renderer) = renderer
            && hitched(renderer.stats().pipelines)
        {
            return Err(hitch(&format!("the boot's {step} step"), renderer));
        }
        if let Some(stage) = stood {
            return Ok(*stage);
        }
    }
}

/// At the pause over the gate cutscene's line: the thermal ladder walked down by critical heat and back up as it
/// cools, every setting pressed round, and the tier changed by its row and back. The ladder goes first, from
/// rest: the battery saver's row moves it too, and a ladder sent down again soon after it climbed doubles its
/// holds. Returns the frames the climb took.
fn at_the_pause(
    walk: &mut Walk,
    screens: &MenuDocument,
    held: (FormFactor, QualityChoice),
) -> Result<u32, AppInitError> {
    walk.stage.show.controls.set_paused(true);
    walk.what = "the pause".to_owned();
    walk.settle("the pause", |walk| walk.is_open("pause"))?;

    // Critical heat steps the ladder to its last rung at once; cooled, it climbs a rung at a time as its holds
    // allow. A held tier holds whatever the heat, so its last rung, a lower tier, is only asked for.
    let rungs = walk.stage.quality.ladder().rungs().len();
    assert_eq!(walk.stage.quality.ladder().state().depth, 0, "the ladder at rest");
    walk.what = "critical heat".to_owned();
    walk.draw.heat = ThermalReading { state: ThermalState::Critical, ..ThermalReading::default() };
    for _ in 0..HOT_FRAMES {
        walk.held()?;
    }
    let hot = walk.stage.quality.ladder().state();
    assert_eq!(hot.depth, rungs, "critical heat takes the ladder to its last rung");
    assert_eq!(walk.stage.renderer.shed(), hot.shed, "shed in place on the renderer");
    walk.what = "the ladder's climb".to_owned();
    walk.draw.heat = ThermalReading { state: ThermalState::Nominal, ..ThermalReading::default() };
    let mut climb = 0;
    let mut depths = vec![rungs];
    while walk.stage.quality.ladder().state().depth > 0 {
        assert!(climb < MOST_CLIMB_FRAMES, "the ladder never climbed back: {depths:?}");
        walk.held()?;
        let depth = walk.stage.quality.ladder().state().depth;
        if depths.last() != Some(&depth) {
            depths.push(depth);
        }
        climb += 1;
    }
    assert_eq!(depths, (0..=rungs).rev().collect::<Vec<_>>(), "a rung at a time");
    assert!(walk.draw.tiers.changes().is_empty(), "a held tier holds whatever the heat");

    // Every setting, as H3's walk presses them: the language through every shipped one among them.
    walk.draw.on_pages = true;
    walk.stepped.clear();
    walk.opened.clear();
    walk.walk_screen("pause")?;
    drive::walked_every_row(screens, &walk.stepped, &walk.opened, "at the pause");

    // The tier, by the graphics page's row, to the class's other and back; each built off the frame and taken
    // while the pause holds still.
    let (class, choice) = held;
    let (screen, row, opened) = walk.open_to("pause", Setting::Quality)?;
    for wanted in [other(choice), choice] {
        let tier = wanted.tier(class).expect("a held choice holds a tier");
        walk.step_until(&screen, &row, Setting::Quality, |settings| settings.quality == wanted)?;
        walk.what = format!("the change to {tier}");
        let waited = Stopwatch::start();
        while walk.stage.quality.active().id != tier {
            let changes = walk.draw.tiers.changes();
            assert!(waited.elapsed() < MOST_CHANGE, "{tier}'s renderer never came: {changes:?}");
            walk.held()?;
        }
    }
    walk.close(&opened, "pause")?;
    walk.what = "the pause, closing".to_owned();
    walk.stage.show.controls.set_paused(false);
    walk.settle("the pause, closing", |walk| !matches!(state(walk), FlowState::Paused { .. }))?;
    walk.draw.on_pages = false;
    Ok(climb)
}

/// What the walk came to.
struct Walked {
    steps: u32,
    tick: u64,
    cutscenes: u32,
    /// The frames the ladder's climb took at the pause, if it paused.
    climb: Option<u32>,
}

/// From the title to the win, and back to the title.
fn walk_the_ward(
    walk: &mut Walk,
    screens: &MenuDocument,
    held: (FormFactor, QualityChoice),
) -> Result<Walked, AppInitError> {
    walk.what = "the title".to_owned();
    // The window's game reads its sounds on its first title frame and decodes them off the frame, as here.
    let stopwatch = Stopwatch::start();
    let Stage { show, content, renderer, gpu, .. } = &mut walk.stage;
    let mut sounds = show
        .shown
        .fight
        .show
        .cue_show()
        .map(|cues| SoundsLoad::begin(&cues.sounds_to_load(), content, renderer, gpu))
        .transpose()?
        .expect("the game's fight has sounds");
    let read = stopwatch.elapsed();
    let limit = limit_at(walk.stage.quality.frame_rate());
    // Its first frames, and on until the fades the boot left to it are built: the frames they build beside.
    let title = format!("the title's first {TITLE_FRAMES} frames and its fades' warm-up");
    walk.draw.title = Some(LoadScreen::reported(title, limit));
    let mut frame = 0;
    let mut decoded = None;
    // The title holds a new game until the fades the boot left to warm there are built (M4 C7): a player at it
    // presses once they are, as here, or has the press held until then.
    let mut fades_in = None;
    while frame < TITLE_FRAMES || decoded.is_none() || walk.stage.show.play_waits() {
        assert!(frame < MOST_SOUND_FRAMES, "the sounds never decoded");
        walk.play()?;
        frame += 1;
        if frame >= TITLE_FRAMES && !walk.stage.show.play_waits() {
            walk.draw.screens.extend(walk.draw.title.take());
        }
        if decoded.is_none()
            && let Some(ready) = sounds.take()
        {
            decoded = Some((frame, ready?.sounds()));
        }
        if fades_in.is_none() && !walk.stage.show.play_waits() {
            fades_in = Some(frame);
        }
    }
    eprintln!(
        "zero_hitches: the title's fades built off the frame by its frame {}",
        fades_in.expect("built")
    );
    let (frame, count) = decoded.expect("decoded");
    eprintln!(
        "zero_hitches: the title's sounds read in {:.1} ms on its first frame, {count} decoded off the frame by its \
         frame {frame}",
        read.as_secs_f64() * 1e3
    );
    assert!(matches!(state(walk), FlowState::Title { .. }), "{:?}", state(walk));
    // The title's first row is the new game, and has the focus.
    walk.what = "the title's new game".to_owned();
    walk.press(Scancode::RETURN)?;
    walk.settle("the new game", |walk| !walk.stage.show.on_title())?;

    let (mut steps, mut cutscenes, mut was_cutscene, mut climb) = (0, 0, false, None);
    walk.what = "the walk".to_owned();
    while !matches!(state(walk), FlowState::Victory { .. }) {
        assert!(steps < MOST_STEPS, "never won: {:?} at tick {}", state(walk), simulation(walk).tick());
        assert!(!matches!(state(walk), FlowState::Defeat { .. }), "the player fell");
        walk.play()?;
        steps += 1;
        let cutscene = matches!(state(walk), FlowState::Cutscene { .. });
        cutscenes += u32::from(cutscene && !was_cutscene);
        was_cutscene = cutscene;
        let says =
            walk.stage.show.shown.fight.show.timeline_player().and_then(|player| player.subtitle()).is_some();
        if cutscene && says && climb.is_none() {
            climb = Some(at_the_pause(walk, screens, held)?);
            walk.what = "the walk".to_owned();
        }
    }
    // The win's own frame.
    walk.play()?;
    let sim = simulation(walk);
    let tick = sim.tick();
    let zone = sim.zone().expect("the ward");
    assert!(
        zone.encounters().iter().all(|held| held.phase() == EncounterPhase::Cleared),
        "every encounter's cleared"
    );
    assert!(sapphire_core::alloc::is_installed(), "the stage counts");
    assert_eq!(sim.stats().allocations, 0, "rule R6, over the whole walk");
    // The walk on the stage is the walk the file recorded, pause and all.
    walk.stage.show.shown.fight.check().map_err(AppInitError::msg)?;

    // Back to the title, as a win goes, under the wipe while the run stands again behind it.
    walk.what = "the way back to the title".to_owned();
    let mut frames = 0;
    while !matches!(state(walk), FlowState::Title { .. }) || Watch::load_screen(&walk.stage) {
        assert!(frames < MOST_RETURN_FRAMES, "never back at the title: {:?}", state(walk));
        walk.play()?;
        frames += 1;
    }
    walk.what = "the title, again".to_owned();
    for _ in 0..TITLE_FRAMES {
        walk.play()?;
    }
    Ok(Walked { steps, tick, cutscenes, climb })
}

/// The lane's own check, shown failing: a debug line, which the game never declares, drawn on one frame.
fn a_hitch_fails(walk: &mut Walk) {
    walk.what = "the injected debug line".to_owned();
    debug_draw::set_enabled(true);
    debug_draw::line(Vec3::ZERO, Vec3::Y, debug_draw::Color::RED);
    let failed = walk.held();
    debug_draw::set_enabled(false);
    let error = failed.expect_err("a pipeline nothing declared, drawn, fails the lane").to_string();
    assert!(error.contains("the injected debug line") && error.contains("debug"), "{error}");
    eprintln!("zero_hitches: the check fails as it should: {error}");
}

/// Does it all in `init`, and the first frame exits.
struct OutsideCaller;

impl App for OutsideCaller {
    const METADATA: AppMetadata = AppMetadata {
        name: "Game Zero Hitches Test",
        organization: "Sapphire",
        identifier: "dev.sapphire.game_zero_hitches_test",
        version: GameApp::METADATA.version,
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let tier = held_tier().map_err(AppInitError::msg)?;
        let size = size().map_err(AppInitError::msg)?;
        let held = choice_of(tier);
        let started = platform.now();
        let screens = MenuDocument::read(platform.content()).map_err(AppInitError::msg)?;
        let replay = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(WALK);
        let settings = PlayerSettings { quality: held.1, ..PlayerSettings::default() };
        let setup = GameSetup { replay: Some(replay), ..GameSetup::default() };
        let (show, first) = GameShow::from_content(platform.content(), settings.clone(), setup)?;
        let gpu = Arc::new(headless_gpu()?);
        // The player's tier from the start, as the window's boot makes it.
        let quality = Graphics::of(&settings, held.0).quality_settings(QualitySettings::default());
        // A phone's tier on a phone's class of device, whose automatic is a phone's tier (M4 H9).
        let host = platform.host().as_device(SystemInfo { device_class: held.0, ..*platform.system_info() });
        let mut booting = Boot::start(&host, gpu.clone(), FORMAT, quality, show, &first)?;
        let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: gpu.label("zero_hitches boot target"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let limit = booting.report().interval_limit;
        let mut booted = LoadScreen::new("load screen the boot screen".to_owned(), limit);
        let mut memory = Memory::default();
        let mut trace = timed().then(|| BootTrace { frames: Vec::new(), interval: limit / 2, limit });
        let mut stage = boot(&mut booting, &view, &mut booted, &mut memory, trace.as_mut())?;
        let report = booting.report().clone();
        assert_eq!(stage.quality.active().id, tier, "the boot made the held tier's renderer");
        // Said now, so a failure later in the walk keeps it.
        if let Some(trace) = &trace {
            trace.report();
        }
        let ms = |duration: Duration| duration.as_secs_f64() * 1e3;
        if boot_only() {
            eprintln!(
                "zero_hitches: {tier} at {}x{}: booted in {:.0} ms ({} frames, longest {:.1} ms in its {} step, \
                 {:.1} of it drawing), the boot alone; the renderer made by {:.0} ms, the packs warm by {:.0}, the \
                 stage stood by {:.0}; {} pipelines prewarmed, {} of them off the frame",
                size.0,
                size.1,
                ms(report.warm),
                report.frames,
                ms(report.longest_frame),
                report.longest_in,
                ms(report.longest_drawing),
                ms(report.renderer),
                ms(report.packs),
                ms(report.stood),
                report.pipelines.prewarmed,
                report.pipelines.warmed_off_frame,
            );
            let over = booted.report();
            if timed() && over {
                return Err(AppInitError::msg("the boot screen had a frame over twice the interval"));
            }
            stage.show.finish().map_err(AppInitError::msg)?;
            return Ok(Self);
        }
        let target = stage.target(size.0, size.1, wgpu::TextureUsages::empty());
        let pages = stage.target(size.0.max(PAGES.0), size.1.max(PAGES.1), wgpu::TextureUsages::empty());
        let watch = Watch {
            target: target.create_view(&wgpu::TextureViewDescriptor::default()),
            pages: pages.create_view(&wgpu::TextureViewDescriptor::default()),
            on_pages: false,
            tiers: TierFollower::new(&settings, held.0),
            heat: ThermalReading { state: ThermalState::Nominal, ..ThermalReading::default() },
            every: draw_every(),
            frames: 0,
            drawn: 0,
            screens: vec![booted],
            in_load: false,
            changes: Vec::new(),
            worst: (Duration::ZERO, String::new()),
            title: None,
            memory,
        };
        let mut walk = Driver::new(stage, watch);
        let walked = walk_the_ward(&mut walk, &screens, held)?;
        let climb = walked.climb.expect("the walk paused at the gate's line");
        assert!(walked.cutscenes >= 1, "the court's gate among them, watched");
        let pipelines = walk.stage.renderer.stats().pipelines;
        a_hitch_fails(&mut walk);

        let seconds = platform.now().saturating_duration_since(started).as_secs_f32();
        eprintln!(
            "zero_hitches: {tier} at {}x{}: booted in {:.0} ms ({} frames, longest {:.1} ms in its {} step, {:.1} of it drawing); won at \
             tick {} after {} steps; the ladder climbed back in {climb} frames; {} frames, {} drawn, in \
             {seconds:.1} s; warmed off the frame {}, first use 0, on the frame's thread 0, waited for 0",
            size.0,
            size.1,
            ms(report.warm),
            report.frames,
            ms(report.longest_frame),
            report.longest_in,
            ms(report.longest_drawing),
            walked.tick,
            walked.steps,
            walk.draw.frames,
            walk.draw.drawn,
            pipelines.warmed_off_frame,
        );
        eprintln!(
            "zero_hitches: the boot's warm-up: the renderer made by {:.0} ms, the packs warm by {:.0}, the stage \
             stood by {:.0}, all warm by {:.0}; {} pipelines prewarmed, {} of them off the frame",
            ms(report.renderer),
            ms(report.packs),
            ms(report.stood),
            ms(report.warm),
            report.pipelines.prewarmed,
            report.pipelines.warmed_off_frame,
        );
        for (tier, took) in &walk.draw.changes {
            eprintln!("zero_hitches: the change to {tier} at the pause took {:.1} ms", ms(*took));
        }
        let (worst, at) = &walk.draw.worst;
        eprintln!("zero_hitches: the longest frame of play drawn, {:.1} ms, at {at}", ms(*worst));
        let mut over = Vec::new();
        for screen in &walk.draw.screens {
            if screen.report() {
                over.push(screen.name.as_str());
            }
        }
        let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        let memory = &walk.draw.memory;
        let parts: Vec<String> =
            memory.parts.iter().map(|(part, most)| format!("{part} {:.0}", mb(*most))).collect();
        eprintln!(
            "zero_hitches: memory, the most footprint each part held (MiB): {}; the process's most {:.0}",
            parts.join(", "),
            mb(memory.most)
        );
        if timed() && !over.is_empty() {
            return Err(AppInitError::msg(format!("screens with a frame over twice the interval: {over:?}")));
        }
        walk.stage.show.finish().map_err(AppInitError::msg)?;
        Ok(Self)
    }

    fn event(&mut self, _: &mut Platform, _: &Event) -> AppControl {
        AppControl::Continue
    }

    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        AppControl::Exit
    }
}

fn main() -> ExitCode {
    if let Err(error) = held_tier().and(size()) {
        eprintln!("zero_hitches: {error}");
        return ExitCode::FAILURE;
    }
    // A timed run is judged by its exit code (docs/m4-plan.md H4), so one that would skip, as the harness does
    // without `--ignored`, fails rather than pass having played nothing (M4 H6).
    if timed() && !std::env::args().any(|argument| argument == "--ignored") {
        eprintln!(
            "zero_hitches: SAPPHIRE_HITCH_TIMED=1 asks for a timed run, which needs --ignored; nothing was played"
        );
        return ExitCode::FAILURE;
    }
    drive::main::<OutsideCaller>("zero_hitches")
}
