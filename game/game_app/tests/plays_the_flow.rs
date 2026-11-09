//! The game's flow on its stage, from outside the crates (M3 G1): the content's flow stands its run behind the
//! title, the title's new game plays it, a checkpoint's reached and saved, the pause goes back to the title
//! under the wipe, which stands the saved run, and the title's continue lands where the save says. Then a trip
//! through the zone on stage's exit, which leaves the save alone. No window, and no frame builds what the load
//! didn't.
//!
//! The platform's loop must own the process's main thread, so this test supplies its own `main`
//! (`harness = false`). It needs a GPU and cooked content: `cargo xtask content`, then
//! `cargo test -p game_app --features dev --test plays_the_flow -- --ignored`. CI's `App tests` job runs it on
//! lavapipe.

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use game_app::{GameApp, GameShow};
use game_flow::flow::{FlowCommand, FlowState, Run};
use game_flow::game::GameSetup;
use game_flow::progress::{self, progress_store};
use game_ui::menus::PlayerSettings;
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, Event, KeyEvent, Keycode, Modifiers, Platform, RunOptions,
    Scancode, Timestamp, WindowId, run_with,
};
use sapphire_rhi::wgpu;
use sapphire_stage::{Readback, Stage};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
/// The square's checkpoint at its south paving, and where it stands.
const CHECKPOINT: &str = "south_paving";
const PAVING: [f32; 3] = [0.0, 0.0, 12.0];
/// The town's exit to the proving hub, on the shelf of the run the flow stands.
const EXIT: &str = "to_proving_hub";
/// Longer than the wipe or a trip takes.
const MOST_FRAMES: u32 = 2_000;

/// Draws the stage's next frame into a new texture and reads it back.
fn frame(stage: &mut Stage<GameShow>) -> Result<Vec<u8>, AppInitError> {
    let target = stage.target(WIDTH, HEIGHT, wgpu::TextureUsages::COPY_SRC);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = Readback::new(&stage.gpu, WIDTH, HEIGHT);
    stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
    stage.check_gpu()?;
    readback.read(&stage.gpu).map_err(AppInitError::msg)
}

/// A frame of the app's, with no window: the menus, a step, the view and a picture.
fn play(stage: &mut Stage<GameShow>, clock: &mut u64) -> Result<(), AppInitError> {
    *clock += 16_666_667;
    stage.show.update_menus(Timestamp::from_nanos(*clock));
    // What the window's `read_input` does after the menus: a pause on its way out ends here.
    stage.show.controls.update(Timestamp::from_nanos(*clock));
    stage.step();
    stage.show.drive_view(Duration::from_millis(16));
    frame(stage)?;
    Ok(())
}

fn state(stage: &Stage<GameShow>) -> FlowState {
    stage.show.game().expect("the content names a flow").flow().state()
}

fn checkpoint(stage: &Stage<GameShow>) -> Option<String> {
    let game = stage.show.game().expect("a game");
    game.progress().and_then(|saved| saved.start.checkpoint.clone())
}

fn player(stage: &Stage<GameShow>) -> [f32; 3] {
    let combat = stage.show.shown.fight.combat.as_ref().expect("a run stands");
    let id = combat.cast()[0];
    combat.simulation().actor(id).expect("the player stands").position.to_array()
}

fn tick(stage: &Stage<GameShow>) -> u64 {
    stage.show.shown.fight.combat.as_ref().expect("a run stands").simulation().tick()
}

/// Does it all in `init`, and the first frame exits.
struct OutsideCaller;

impl App for OutsideCaller {
    const METADATA: AppMetadata = AppMetadata {
        name: "Game Flow Test",
        organization: "Sapphire",
        identifier: "dev.sapphire.game_flow_test",
        version: GameApp::METADATA.version,
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let saves = std::env::temp_dir().join(format!("game_app-plays-the-flow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&saves);
        std::fs::create_dir_all(&saves)?;
        let store = progress_store(&saves, |path, bytes| std::fs::write(path, bytes));
        let setup = GameSetup { store: Some(store.clone()), ..GameSetup::default() };
        let (show, first) = GameShow::from_content(platform.content(), PlayerSettings::default(), setup)?;
        let gpu = sapphire_stage::headless_gpu()?;
        let quality = sapphire_render::quality::QualitySettings::default();
        let desc = game_combat::fight::fight_stage(sapphire_stage::StageDesc::new(
            platform.host(),
            &gpu,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            quality,
        ));
        let mut content = sapphire_stage::Content::new(platform.host(), desc.character, desc.sky)?;
        let mut show = show;
        let combat = show.shown.fight.prepare_stock_combat(&first, &mut content)?;
        let mut stage = Stage::new_with_content(gpu, desc, content)?.with_show(show);
        let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
        show.stand(combat, scene, content, renderer, gpu, quality.active().id)?;
        assert!(stage.show.shown.wipe.is_some(), "a game's restarts have a wipe");
        assert_eq!(state(&stage), FlowState::Title { stood: Run::New });

        // The title holds the run behind it; its new game, the first row with nothing to continue, plays it.
        let mut clock = 0;
        for _ in 0..10 {
            play(&mut stage, &mut clock)?;
        }
        assert_eq!(tick(&stage), 0, "the title holds the run");
        let press = Event::Key(KeyEvent {
            timestamp: Timestamp::from_nanos(clock + 1),
            window: WindowId::from_raw(1),
            scancode: Scancode::RETURN,
            keycode: Keycode(0),
            modifiers: Modifiers::default(),
            down: true,
            repeat: false,
        });
        stage.show.update_menus(Timestamp::from_nanos(3_000_000_000));
        clock = 3_000_000_000;
        stage.show.handle_event(&press);
        play(&mut stage, &mut clock)?;
        assert_eq!(state(&stage), FlowState::Playing, "the title's new game plays");
        assert!(!stage.show.on_title());
        for _ in 0..30 {
            play(&mut stage, &mut clock)?;
        }
        assert!(tick(&stage) > 0, "it steps");
        assert_eq!(checkpoint(&stage), None, "a new game's save has no checkpoint");

        // The south paving, as a beat's align would bring the player there: reached, and saved.
        let combat = stage.show.shown.fight.combat.as_mut().expect("a run");
        combat.return_to_checkpoint(CHECKPOINT).map_err(AppInitError::msg)?;
        for _ in 0..5 {
            play(&mut stage, &mut clock)?;
        }
        assert_eq!(checkpoint(&stage).as_deref(), Some(CHECKPOINT), "the checkpoint's saved");

        // The pause's way back to the title: the wipe covers, the saved run stands under it, and it clears.
        stage.show.controls.set_paused(true);
        play(&mut stage, &mut clock)?;
        assert!(matches!(state(&stage), FlowState::Paused { .. }), "{:?}", state(&stage));
        stage.show.choose(FlowCommand::ToTitle);
        let mut covered = 0;
        for _ in 0..MOST_FRAMES {
            play(&mut stage, &mut clock)?;
            covered +=
                u32::from(stage.show.shown.wipe.as_ref().is_some_and(game_ui::wipe::ZoneWipe::covered));
            if matches!(state(&stage), FlowState::Title { .. }) {
                break;
            }
        }
        assert_eq!(state(&stage), FlowState::Title { stood: Run::Saved }, "back at the title");
        assert!(covered > 0, "under the wipe");
        assert!(stage.show.on_title());
        assert_eq!(tick(&stage), 0, "a new run stands, held by the title");

        // The title's continue: the run from the south paving.
        stage.show.choose(FlowCommand::Continue);
        assert_eq!(state(&stage), FlowState::Playing);
        let at = player(&stage);
        let off = at.iter().zip(PAVING).map(|(at, to)| (at - to).abs()).fold(0.0, f32::max);
        assert!(off < 2.0, "the run starts where the save says: {at:?}");
        for _ in 0..10 {
            play(&mut stage, &mut clock)?;
        }

        // Out through the town's exit: the trip covers, and away from the flow's zone nothing's saved.
        let Stage { show, scene, content, .. } = &mut stage;
        show.shown.fight.take_exit(scene, content, EXIT).map_err(AppInitError::msg)?;
        let mut went = false;
        for _ in 0..MOST_FRAMES {
            play(&mut stage, &mut clock)?;
            let under_way = stage.show.shown.fight.show.trip_phase().is_some();
            went |= under_way;
            if went && !under_way {
                break;
            }
        }
        let last = stage.show.shown.fight.show.last_trip().expect("it went");
        assert!(last.error.is_none(), "{last:?}");
        assert_eq!(checkpoint(&stage).as_deref(), Some(CHECKPOINT), "the trip leaves the save alone");

        let first_uses = stage.renderer.stats().pipelines.first_use_builds;
        assert_eq!(first_uses, 0, "the load missed: {}", stage.renderer.first_use_report());
        stage.show.finish().map_err(AppInitError::msg)?;
        let saved = progress::load(&store).expect("a save");
        assert_eq!(saved.start.checkpoint.as_deref(), Some(CHECKPOINT), "and so says the file");
        std::fs::remove_dir_all(&saves)?;
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
        eprintln!("plays_the_flow: needs a GPU and cooked content; run it with --ignored");
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
