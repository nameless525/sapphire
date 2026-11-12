//! The game leaving the foreground and losing its device, from outside its crates (M4 S6), on the app's road:
//! the frame's menus and controls, its steps from the driver, and what the window calls as the OS moves it.
//!
//! - **A trip to the background,** in a run someone's playing at the Lantern Ward's court: it pauses, the save
//!   and the run's file are on disk as the game has them, nothing steps while it's away or back at the pause,
//!   whatever's pressed, and the pause's first row goes on. The run's checksums are a run's with no trip, tick
//!   for tick, under the same presses: the pause stops steps and scales no clock.
//! - **A lost device:** the show ends for a reload, a new stage stands on a new device, and the game is back at
//!   the save, at the pause, drawing, with nothing built that the load didn't build.
//! - **A device lost while the game boots (M4 H4):** at each of the boot's steps that holds something on it,
//!   the device destroyed and one more boot frame drawn on it, as a window may before wgpu reports the loss;
//!   the boot is let go of with its warm-up's builds still running, and a boot on a new device reaches the
//!   title with nothing built or waited for on a frame. The window boots again so (`GameApp`'s `recover`), and
//!   `--lose-device` counts the boot screen's frames, so a device or a simulator can lose one mid-boot too. Lost
//!   as the renderer builds (its own pipelines or the packs'), the next boot's ceiling is the tier below (M4 H9).
//! - **A tier the device can't build (M4 H9):** a boot at Mobile-High whose renderer is refused, or whose build
//!   the device reports an error in, falls back to Mobile-Low and stands the game there, with nothing built on a
//!   frame; Mobile-High isn't asked for again. Refused at Mobile-Low too, the boot fails, naming the lowest tier.
//!
//! The platform's loop must own the process's main thread, so this test supplies its own `main`
//! (`harness = false`). It needs a GPU and cooked content: `cargo xtask content`, then
//! `cargo test -p game_app --features dev --test comes_back -- --ignored`. CI's `App tests` job runs it on
//! lavapipe.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use game_app::{GameApp, GameShow, stand_game};
use game_flow::boot::{Boot, MakeRenderer};
use game_flow::flow::{FlowCommand, FlowState};
use game_flow::game::GameSetup;
use game_flow::progress::{self, Progress, progress_store};
use game_ui::menus::{MenuScreen, PlayerSettings};
use sapphire_gameplay::replay::Replay;
use sapphire_gameplay::{FlagValues, ZoneStart};
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, Event, KeyEvent, Keycode, Modifiers, Platform, RunOptions,
    Scancode, Timestamp, WindowId, run_with,
};
use sapphire_render::quality::{QualitySettings, TierId};
use sapphire_render::{RenderError, Renderer};
use sapphire_rhi::wgpu;
use sapphire_serialize::AssetId;
use sapphire_stage::{Readback, Stage, headless_gpu};
use sapphire_time::{FixedStepConfig, FixedStepDriver};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const FRAME: Duration = Duration::from_nanos(16_666_667);
/// The ward's zone, and the encounters a save at the court has fought: the market's and the garden's.
const WARD: &str = "f15b7d04-631d-59b5-8c42-338147fc4e86";
const MARKET: &str = "9c124ff2-ab3d-4b6c-a281-79dc6eade2ec";
const GARDEN: &str = "f2a2a203-7ac5-4a8b-91ec-4366daa62246";
/// The court's checkpoint, where the save stands the player.
const CHECKPOINT: &str = "court";
const CHECKPOINT_AT: [f32; 3] = [0.0, 0.0, -63.0];
/// How long each run plays, and the ticks the player swings at.
const TICKS: u64 = 240;
const SWINGS: [u64; 4] = [20, 60, 140, 200];
/// Where the run goes to the background.
const AWAY_AT: u64 = 100;
/// The boot's steps a device is lost in: as the renderer builds, as the packs start to warm, as the fight
/// stands on the stage, and as the show's declared pipelines warm.
const LOST_IN: [&str; 4] = ["renderer", "packs", "stand", "declared"];
/// More frames than a boot takes on lavapipe to reach any step.
const MOST_BOOT_FRAMES: u32 = 100_000;

fn id(raw: &str) -> AssetId {
    raw.parse().expect("an ID")
}

/// A run of the game on a stage, played a frame at a time as the window plays one.
struct Run {
    stage: Stage<GameShow>,
    driver: FixedStepDriver,
    clock: u64,
    /// Each tick's checksum, as it was stepped.
    checksums: Vec<u64>,
    dir: PathBuf,
}

impl Run {
    /// The game's flow over a save at the court, kept with the run's file in `dir`, stood on a new device.
    fn stand(platform: &Platform, dir: &Path, settings: PlayerSettings) -> Result<Self, AppInitError> {
        let setup =
            GameSetup { store: Some(store(dir)), record: Some(dir.to_owned()), ..GameSetup::default() };
        let (show, first) = GameShow::from_content(platform.content(), settings, setup)?;
        let stage = stand_game(platform.host(), headless_gpu()?, FORMAT, show, &first)?;
        Ok(Self {
            stage,
            driver: FixedStepDriver::new(FixedStepConfig::default()),
            clock: 1_000_000_000,
            checksums: Vec::new(),
            dir: dir.to_owned(),
        })
    }

    fn now(&self) -> Timestamp {
        Timestamp::from_nanos(self.clock)
    }

    fn tick(&self) -> u64 {
        self.stage.show.shown.fight.combat.as_ref().expect("a run stands").simulation().tick()
    }

    fn state(&self) -> FlowState {
        self.stage.show.game().expect("the content names a flow").flow().state()
    }

    fn pause_is_up(&self) -> bool {
        self.stage.show.menus().is_some_and(|menus| menus.is_open(MenuScreen::PAUSE))
    }

    /// A key, down or up, as the window sends it, just after the last frame.
    fn key(&mut self, scancode: Scancode, down: bool) {
        self.stage.show.handle_event(&Event::Key(KeyEvent {
            timestamp: Timestamp::from_nanos(self.clock + 1),
            window: WindowId::from_raw(1),
            scancode,
            keycode: Keycode(0),
            modifiers: Modifiers::default(),
            down,
            repeat: false,
        }));
    }

    /// The frame's input as the window reads it, with no window.
    fn input(&mut self) {
        self.clock += FRAME.as_nanos() as u64;
        let now = self.now();
        self.stage.show.update_menus(now);
        self.stage.show.controls.update(now);
    }

    /// A frame of the window's: its input, the steps the driver gives it and the view, then a picture read back
    /// if `draw`, else the frame prepared as a window's with no image is.
    fn frame(&mut self, draw: bool) -> Result<Option<Vec<u8>>, AppInitError> {
        self.input();
        let stage = &mut self.stage;
        let steps = stage.show.advance(&stage.scene, &mut self.driver, FRAME);
        for _ in 0..steps.count {
            stage.step();
            let combat = stage.show.shown.fight.combat.as_ref().expect("a run stands");
            self.checksums.push(combat.simulation().checksum());
        }
        stage.show.drive_view(FRAME);
        if !draw {
            stage.skip_frame(steps.alpha);
            return Ok(None);
        }
        let target = stage.target(WIDTH, HEIGHT, wgpu::TextureUsages::COPY_SRC);
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let readback = Readback::new(&stage.gpu, WIDTH, HEIGHT);
        stage.render(&view, steps.alpha, |graph, target| readback.record(graph, target))?;
        stage.check_gpu()?;
        readback.read(&stage.gpu).map(Some).map_err(AppInitError::msg)
    }

    /// Plays to `tick`, swinging where [`SWINGS`] says: a press in the frame whose steps reach it.
    fn play_to(&mut self, tick: u64) -> Result<(), AppInitError> {
        let mut frames = 0;
        while self.tick() < tick {
            assert!(frames < 4 * TICKS, "stuck at tick {} in {:?}", self.tick(), self.state());
            let swing = SWINGS.contains(&(self.tick() + 1));
            if swing {
                self.key(Scancode::J, true);
            }
            self.frame(false)?;
            if swing {
                self.key(Scancode::J, false);
            }
            frames += 1;
        }
        Ok(())
    }

    /// The save on disk, as the game has it.
    fn saved(&self) -> Option<Progress> {
        progress::load(&store(&self.dir))
    }

    /// The pause's first row, the "tap to continue": on a phone it's a tap, which `game_ui`'s menu tests and
    /// the emulator's run press; here the keyboard's confirm presses the same row. Returns the frames until
    /// the steps go on.
    fn go_on(&mut self) -> Result<u32, AppInitError> {
        self.key(Scancode::RETURN, true);
        self.frame(false)?;
        self.key(Scancode::RETURN, false);
        let mut frames = 1;
        while self.stage.show.controls.is_paused() {
            assert!(frames < 30, "the pause's first row never went on");
            self.frame(false)?;
            frames += 1;
        }
        Ok(frames)
    }
}

fn store(dir: &Path) -> sapphire_save::ProgressStore<Progress> {
    progress_store(dir, |path, bytes| std::fs::write(path, bytes))
}

fn near(at: [f32; 3], to: [f32; 3]) -> bool {
    at.iter().zip(to).map(|(at, to)| (at - to).abs()).fold(0.0, f32::max) < 1.0
}

fn player(run: &Run) -> [f32; 3] {
    let combat = run.stage.show.shown.fight.combat.as_ref().expect("a run stands");
    combat.simulation().actor(combat.cast()[0]).expect("the player stands").position.to_array()
}

/// A directory of its own for a run's save and file, holding a save at the court's checkpoint.
fn saves(name: &str) -> Result<PathBuf, AppInitError> {
    let dir = std::env::temp_dir().join(format!("game_app-comes-back-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mut cleared = vec![id(MARKET), id(GARDEN)];
    cleared.sort_unstable();
    let saved = Progress {
        zone: id(WARD),
        start: ZoneStart { checkpoint: Some(CHECKPOINT.to_owned()), cleared },
        flags: FlagValues::default(),
    };
    let store = store(&dir);
    store.write(&store.snapshot(&saved).map_err(AppInitError::msg)?).map_err(AppInitError::msg)?;
    Ok(dir)
}

/// Someone at the title's continue: the run from the court, played.
fn played(platform: &Platform, name: &str) -> Result<Run, AppInitError> {
    let mut run = Run::stand(platform, &saves(name)?, PlayerSettings::default())?;
    run.frame(false)?;
    run.stage.show.choose(FlowCommand::Continue);
    // The title's frame stepped its own scene: the run's checksums start at its first tick.
    run.checksums.clear();
    assert_eq!(run.state(), FlowState::Playing);
    assert!(run.stage.show.is_played(), "nobody's script");
    Ok(run)
}

/// Whether a picture shows something: not one colour all over.
fn shows_something(pixels: &[u8]) -> bool {
    let (texels, _) = pixels.as_chunks::<4>();
    texels.iter().any(|texel| texel != &texels[0])
}

/// A boot of the game on `gpu`, as the window starts one: a new game's, at the player's tier.
fn boot_on(platform: &Platform, gpu: sapphire_rhi::GpuContext) -> Result<Boot, AppInitError> {
    let (show, first) =
        GameShow::from_content(platform.content(), PlayerSettings::default(), GameSetup::default())?;
    let quality = game_app::tiers::starting_quality(&show, platform.system_info().device_class);
    Boot::start(platform.host(), Arc::new(gpu), FORMAT, quality, show, &first)
}

/// A target on `boot`'s device for its screen to draw into.
fn boot_target(boot: &Boot) -> wgpu::TextureView {
    let texture = boot.gpu().device().create_texture(&wgpu::TextureDescriptor {
        label: boot.gpu().label("boot screen"),
        size: wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A device lost while the game boots, at each of [`LOST_IN`] (the module's docs), then a boot on a new one.
fn lost_while_booting(platform: &Platform) -> Result<(), AppInitError> {
    let dt = FRAME.as_secs_f32();
    for step in LOST_IN {
        let mut boot = boot_on(platform, headless_gpu()?)?;
        let target = boot_target(&boot);
        let mut frames = 0;
        while boot.step().name() != step {
            assert!(frames < MOST_BOOT_FRAMES, "the boot never reached its {step} step");
            assert!(boot.frame(Some(&target), dt)?.is_none(), "the boot ended before its {step} step");
            frames += 1;
        }
        let gpu = boot.gpu().clone();
        gpu.device().destroy();
        let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
        assert!(gpu.is_lost(), "the device lost in the boot's {step} step says so");
        // The next frame touches nothing on the dead device, and says why, for the app to boot again.
        let dead = boot.frame(Some(&target), dt).map(|stage| stage.is_some());
        let error = dead.as_ref().expect_err("a boot on a lost device goes no further").to_string();
        assert!(error.contains("lost"), "{error}");
        // Lost as its renderer built, the next boot builds the tier below; lost later, the same tier (M4 H9).
        let building = matches!(step, "renderer" | "packs");
        let ceiling = building.then(|| boot.tier().lower()).flatten();
        assert_eq!(boot.ceiling_after_loss(), ceiling, "lost in the {step} step at {}", boot.tier());
        let let_go = platform.now();
        drop((boot, target));
        let waited = platform.now().saturating_duration_since(let_go);
        tracing::info!(
            step,
            frames,
            ?dead,
            ?waited,
            "a device lost while the game booted, and the boot let go"
        );
    }
    // And on a new device, the game boots as if nothing had been lost.
    let boot = boot_on(platform, headless_gpu()?)?;
    let (stage, report) = boot.run_offscreen((WIDTH, HEIGHT), false)?;
    assert!(stage.show.on_title(), "the title");
    let booted = report.pipelines;
    let hitches = (booted.first_use_builds, booted.frame_thread_builds, booted.frame_waits);
    assert_eq!(hitches, (0, 0, 0), "{report:?}");
    Ok(())
}

/// How a test's renderer is refused at a tier (M4 H9).
#[derive(Clone, Copy, Debug)]
enum Refusal {
    /// `Renderer::new` returns an error, as for a shadow map past the device's limit.
    Error,
    /// The device reports an error as it builds, as for a pipeline its driver won't link, and the build goes on.
    DeviceError,
}

/// `Renderer::new`, refused `how` at each of `refused`.
fn refusing(refused: &'static [TierId], how: Refusal) -> MakeRenderer {
    Arc::new(move |gpu, format, tier, settings| {
        if !refused.contains(&tier) {
            return Renderer::new(gpu, format, settings);
        }
        tracing::info!(%tier, ?how, "the test refuses the tier's renderer");
        match how {
            Refusal::Error => {
                Err(RenderError::InvalidSettings { field: "tier", problem: "is refused by the test" })
            }
            Refusal::DeviceError => {
                // Mapped for reading and writing at once: an error the device reports, uncaptured.
                let _ = gpu.device().create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 4,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::MAP_WRITE,
                    mapped_at_creation: false,
                });
                Renderer::new(gpu, format, settings)
            }
        }
    })
}

/// A boot at Mobile-High, as a player holding it starts one, with its renderer made by `make`.
fn boot_mobile_high(platform: &Platform, make: MakeRenderer) -> Result<Boot, AppInitError> {
    let (show, first) =
        GameShow::from_content(platform.content(), PlayerSettings::default(), GameSetup::default())?;
    let held = QualitySettings { tier: Some(TierId::MobileHigh), ..QualitySettings::default() };
    Boot::start_making(platform.host(), Arc::new(headless_gpu()?), FORMAT, held, show, &first, make)
}

/// A tier the device can't build steps down, and only the lowest's failure ends the boot (M4 H9).
fn falls_back_a_tier(platform: &Platform) -> Result<(), AppInitError> {
    for how in [Refusal::Error, Refusal::DeviceError] {
        let boot = boot_mobile_high(platform, refusing(&[TierId::MobileHigh], how))?;
        let (stage, report) = boot.run_offscreen((WIDTH, HEIGHT), false)?;
        assert_eq!(report.fell_back_from, [TierId::MobileHigh], "{how:?}: {report:?}");
        assert_eq!(stage.quality.active().id, TierId::MobileLow, "{how:?}");
        assert_eq!(stage.quality.settings().ceiling, Some(TierId::MobileLow));
        assert_eq!(stage.quality.wanted_tier(), None, "{how:?}: Mobile-High isn't asked for again");
        assert!(stage.show.on_title(), "{how:?}: the title, at Mobile-Low");
        let booted = report.pipelines;
        let hitches = (booted.first_use_builds, booted.frame_thread_builds, booted.frame_waits);
        assert_eq!(hitches, (0, 0, 0), "{how:?}: {report:?}");
        tracing::info!(?how, frames = report.frames, "a boot refused Mobile-High stood at Mobile-Low");
    }
    let boot =
        boot_mobile_high(platform, refusing(&[TierId::MobileHigh, TierId::MobileLow], Refusal::Error))?;
    let error =
        boot.run_offscreen((WIDTH, HEIGHT), false).err().expect("no tier left to boot at").to_string();
    assert!(error.contains("Mobile-Low, the lowest tier of its class"), "{error}");
    Ok(())
}

/// Does it all in `init`, and the first frame exits.
struct OutsideCaller;

impl App for OutsideCaller {
    const METADATA: AppMetadata = AppMetadata {
        name: "Game Comes Back Test",
        organization: "Sapphire",
        identifier: "dev.sapphire.game_comes_back_test",
        version: GameApp::METADATA.version,
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        // What the run does with nobody leaving: its checksums are what any trip must match.
        let mut stayed = played(platform, "stayed")?;
        stayed.play_to(TICKS)?;
        let expected = std::mem::take(&mut stayed.checksums);
        assert_eq!(expected.len() as u64, TICKS);
        std::fs::remove_dir_all(&stayed.dir)?;
        drop(stayed);

        let mut run = played(platform, "away")?;
        run.play_to(AWAY_AT)?;
        let paused_at = expected[AWAY_AT as usize - 1];

        // Going down, as the window's duties go: the pause, then what's on its way to disk.
        run.stage.show.leave_foreground();
        assert!(run.stage.show.controls.is_paused(), "the steps stop at once");
        assert!(matches!(run.state(), FlowState::Paused { .. }), "{:?}", run.state());
        assert!(run.pause_is_up(), "the pause's screen is what the return draws");
        let game = run.stage.show.game().expect("a flow");
        assert_eq!(run.saved().as_ref(), game.progress(), "the save on disk is the game's");
        let kept = std::fs::read(run.dir.join(format!("{}-1.sreplay", game.flow().document().scenario)))?;
        let kept = Replay::decode(&kept).map_err(|error| AppInitError::msg(error.to_string()))?;
        assert_eq!(u64::from(kept.ticks()), AWAY_AT, "the run's file, as far as it got");

        // Away: a phone may still run a frame's input, and steps nothing.
        for _ in 0..30 {
            run.input();
        }
        // Back, at the pause: whatever's pressed, nothing moves.
        let back = run.frame(true)?.expect("drawn");
        assert!(shows_something(&back));
        for pressed in [Scancode::J, Scancode::K, Scancode::E, Scancode::LEFT_SHIFT] {
            run.key(pressed, true);
            run.frame(false)?;
            run.key(pressed, false);
            run.frame(false)?;
        }
        assert_eq!(run.tick(), AWAY_AT, "no step at the pause");
        assert_eq!(*run.checksums.last().expect("stepped"), paused_at);
        assert!(run.pause_is_up());

        let frames = run.go_on()?;
        assert!(!run.pause_is_up() && run.state() == FlowState::Playing, "{:?}", run.state());
        tracing::info!(frames, "the pause's first row went on");
        run.play_to(TICKS)?;
        assert_eq!(run.checksums, expected, "a trip to the background moves no checksum");

        // A lost device takes the stage, and the show ends for the next, which stands from the save.
        run.stage.gpu.device().destroy();
        // wgpu reports it once the queue drains, during a poll.
        let _ = run.stage.gpu.device().poll(wgpu::PollType::wait_indefinitely());
        assert!(run.stage.gpu.is_lost());
        let reload = run.stage.show.end_for_reload();
        let dir = run.dir.clone();
        drop(run);
        let mut run = Run::stand(platform, &dir, reload.settings.clone())?;
        run.stage.show.reload_from(&reload);
        assert!(run.stage.show.is_played());
        assert!(matches!(run.state(), FlowState::Paused { .. }), "{:?}", run.state());
        assert!(run.pause_is_up(), "back at the pause");
        assert_eq!(run.tick(), 0, "from the save");
        assert!(near(player(&run), CHECKPOINT_AT), "where the save says: {:?}", player(&run));
        let reloaded = run.frame(true)?.expect("drawn");
        assert!(shows_something(&reloaded), "the new device draws");
        run.go_on()?;
        run.play_to(60)?;
        let playing = run.frame(true)?.expect("drawn");
        assert!(shows_something(&playing));
        assert_ne!(playing, reloaded, "the pause has gone and the fight moves");
        let first_uses = run.stage.renderer.stats().pipelines.first_use_builds;
        assert_eq!(first_uses, 0, "the reload's load missed: {}", run.stage.renderer.first_use_report());
        run.stage.show.finish().map_err(AppInitError::msg)?;
        drop(run);
        std::fs::remove_dir_all(&dir)?;

        lost_while_booting(platform)?;
        falls_back_a_tier(platform)?;
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
    if !std::env::args().any(|argument| argument == "--ignored") {
        eprintln!("comes_back: needs a GPU and cooked content; run it with --ignored");
        return ExitCode::SUCCESS;
    }
    sapphire_core::log::init(Default::default(), None).expect("initialize app test logging");
    if std::env::var_os("SAPPHIRE_CONTENT_DIR").is_none() {
        let cook = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/content/desktop");
        // SAFETY: nothing else runs yet: the platform's loop and its threads start below.
        unsafe { std::env::set_var("SAPPHIRE_CONTENT_DIR", cook) };
    }
    run_with::<OutsideCaller>(RunOptions { background: true, headless: true })
}
