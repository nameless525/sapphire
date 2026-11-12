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
//! machine (`docs/m4-plan.md` H4). Anywhere else the times are only read.
//!
//! - `SAPPHIRE_HITCH_TIER=<key>`: the tier the player holds (default `desktop-high`), the class's other tier the
//!   one the pause changes to and back.
//! - `SAPPHIRE_HITCH_SIZE=<w>x<h>`: what's drawn into (default 480x270). The pause's pages draw into at least
//!   480x270, so that every page's rows lay out as a player's do.
//! - `SAPPHIRE_HITCH_DRAW_EVERY=<n>`: one frame in `n` drawn (default 1). The rest are prepared and recorded but
//!   not run (`Stage::lose_frames`), so their first uses still count: CI's lavapipe can't draw them all in its
//!   job's time. Load screens and the frames the tier or a rung changes on are always drawn.
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
use game_flow::boot::Boot;
use game_flow::flow::FlowState;
use game_flow::game::GameSetup;
use game_ui::menus::{MenuDocument, PlayerSettings, QualityChoice, Setting};
use sapphire_core::debug_draw;
use sapphire_core::target::FormFactor;
use sapphire_core::thermal::{ThermalReading, ThermalState};
use sapphire_gameplay::{EncounterPhase, Simulation};
use sapphire_math::Vec3;
use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, Platform, Scancode};
use sapphire_render::quality::{QualitySettings, TierId, TierTable};
use sapphire_render::{PipelineStats, Renderer};
use sapphire_rhi::{GpuContext, wgpu};
use sapphire_stage::audio::SoundsLoad;
use sapphire_stage::{Stage, headless_gpu};
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

/// One load screen's frames: how many, the worst, and the limit.
struct LoadScreen {
    name: String,
    frames: u32,
    worst: Duration,
    limit: Duration,
}

impl LoadScreen {
    fn new(name: String, limit: Duration) -> Self {
        Self { name, frames: 0, worst: Duration::ZERO, limit }
    }

    fn frame(&mut self, took: Duration) {
        self.frames += 1;
        self.worst = self.worst.max(took);
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
        stage.render(target, 1.0, |_, _| {})?;
        if draw {
            finish_gpu(&stage.gpu);
        }
        stage.check_gpu()?;
        let took = began.elapsed();
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
                let name = format!("a wipe, at {}", Self::place(stage, what));
                self.screens.push(LoadScreen::new(name, limit_at(stage.quality.frame_rate())));
            }
            self.screens.last_mut().expect("pushed").frame(took);
        } else if draw && !moved && took > self.worst.0 {
            self.worst = (took, Self::place(stage, what));
        }
        self.in_load = load;
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
/// stage it hands back.
fn boot(
    boot: &mut Boot,
    target: &wgpu::TextureView,
    screen: &mut LoadScreen,
) -> Result<Stage<GameShow>, AppInitError> {
    let dt = (Duration::from_secs(1) / DEFAULT_RATE).as_secs_f32();
    loop {
        let began = Stopwatch::start();
        let step = boot.step().name();
        let stood = boot.frame(Some(target), dt)?;
        let renderer = stood.as_ref().map(|stage| &stage.renderer).or(boot.renderer());
        finish_gpu(boot.gpu());
        screen.frame(began.elapsed());
        if let Some(renderer) = renderer
            && hitched(renderer.stats().pipelines)
        {
            return Err(hitch(&format!("the boot's {step} step"), renderer));
        }
        if let Some(stage) = stood {
            return Ok(stage);
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
    let mut frame = 0;
    let mut decoded = None;
    while frame < TITLE_FRAMES || decoded.is_none() {
        assert!(frame < MOST_SOUND_FRAMES, "the sounds never decoded");
        walk.play()?;
        frame += 1;
        if decoded.is_none()
            && let Some(ready) = sounds.take()
        {
            decoded = Some((frame, ready?.sounds()));
        }
    }
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
        let mut booting = Boot::start(platform.host(), gpu.clone(), FORMAT, quality, show, &first)?;
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
        let mut booted = LoadScreen::new("the boot screen".to_owned(), booting.report().interval_limit);
        let stage = boot(&mut booting, &view, &mut booted)?;
        let report = booting.report().clone();
        assert_eq!(stage.quality.active().id, tier, "the boot made the held tier's renderer");
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
        };
        let mut walk = Driver::new(stage, watch);
        let walked = walk_the_ward(&mut walk, &screens, held)?;
        let climb = walked.climb.expect("the walk paused at the gate's line");
        assert!(walked.cutscenes >= 1, "the court's gate among them, watched");
        let pipelines = walk.stage.renderer.stats().pipelines;
        a_hitch_fails(&mut walk);

        let seconds = platform.now().saturating_duration_since(started).as_secs_f32();
        let ms = |duration: Duration| duration.as_secs_f64() * 1e3;
        eprintln!(
            "zero_hitches: {tier} at {}x{}: booted in {:.0} ms ({} frames, longest {:.1} ms in its {} step); won at \
             tick {} after {} steps; the ladder climbed back in {climb} frames; {} frames, {} drawn, in \
             {seconds:.1} s; warmed off the frame {}, first use 0, on the frame's thread 0, waited for 0",
            size.0,
            size.1,
            ms(report.warm),
            report.frames,
            ms(report.longest_frame),
            report.longest_in,
            walked.tick,
            walked.steps,
            walk.draw.frames,
            walk.draw.drawn,
            pipelines.warmed_off_frame,
        );
        for (tier, took) in &walk.draw.changes {
            eprintln!("zero_hitches: the change to {tier} at the pause took {:.1} ms", ms(*took));
        }
        let (worst, at) = &walk.draw.worst;
        eprintln!("zero_hitches: the longest frame of play drawn, {:.1} ms, at {at}", ms(*worst));
        let mut over = Vec::new();
        for screen in &walk.draw.screens {
            eprintln!(
                "zero_hitches: load screen {}: {} frames, the worst {:.1} ms against {:.1} ms",
                screen.name,
                screen.frames,
                ms(screen.worst),
                ms(screen.limit)
            );
            if screen.worst > screen.limit {
                over.push(screen.name.as_str());
            }
        }
        if timed() && !over.is_empty() {
            return Err(AppInitError::msg(format!(
                "load screens with a frame over twice the interval: {over:?}"
            )));
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
    drive::main::<OutsideCaller>("zero_hitches")
}
