//! The game's flow on its stage, from outside the crates (M3 G1): the content's flow, the Lantern Ward, over a
//! save at the court's checkpoint. The title's continue stands the player there; stepping back through the
//! court's gate starts its encounter, which brings the gate's cutscene, framed from the checkpoint; a skip ends
//! it, and the guide's conversation opens on the fight's panel, where the keyboard lights the ward. The story's
//! flags are saved; the pause goes back to the title under the wipe, and a continue from there plays neither
//! the cutscene nor the conversation again. The music is the flow's at the title and the fight's in play. No
//! window, and no frame builds what the load didn't.
//!
//! The platform's loop must own the process's main thread, so this test supplies its own `main`
//! (`harness = false`). It needs a GPU and cooked content: `cargo xtask content`, then
//! `cargo test -p game_app --features dev --test plays_the_ward -- --ignored`. CI's `App tests` job runs it on
//! lavapipe.

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use game_app::{GameApp, GameShow};
use game_combat::combat::SCENARIOS;
use game_flow::document::FlowDocument;
use game_flow::flow::{FlowCommand, FlowState, Run};
use game_flow::game::GameSetup;
use game_flow::progress::{self, Progress, progress_store};
use game_ui::menus::PlayerSettings;
use sapphire_core::Name;
use sapphire_gameplay::replay::find_scenario;
use sapphire_gameplay::{EncounterPhase, FlagValues, ZoneStart};
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, Event, KeyEvent, Keycode, Modifiers, Platform, RunOptions,
    Scancode, Timestamp, WindowId, run_with,
};
use sapphire_rhi::wgpu;
use sapphire_serialize::AssetId;
use sapphire_stage::{Readback, Stage};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
/// The ward's zone, and the encounters a save at the court has fought: the market's and the garden's.
const WARD: &str = "f15b7d04-631d-59b5-8c42-338147fc4e86";
const MARKET: &str = "9c124ff2-ab3d-4b6c-a281-79dc6eade2ec";
const GARDEN: &str = "f2a2a203-7ac5-4a8b-91ec-4366daa62246";
/// The court's encounter, which its gate starts.
const COURT: &str = "3e7d6965-beed-4aee-86c8-ea292239a816";
/// The court's checkpoint, inside its gate, where the gate's cutscene is framed from.
const CHECKPOINT: &str = "court";
const CHECKPOINT_AT: [f32; 3] = [0.0, 0.0, -63.0];
/// Longer than the wipe, a walk back through the gate or a skip takes.
const MOST_FRAMES: u32 = 600;

fn id(raw: &str) -> AssetId {
    raw.parse().expect("an ID")
}

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
    stage.show.follow_input(Timestamp::from_nanos(*clock));
    stage.step();
    stage.show.drive_view(Duration::from_millis(16));
    frame(stage)?;
    Ok(())
}

/// A key, down or up, as the window would send it.
fn key(stage: &mut Stage<GameShow>, clock: u64, scancode: Scancode, down: bool) {
    stage.show.handle_event(&Event::Key(KeyEvent {
        timestamp: Timestamp::from_nanos(clock + 1),
        window: WindowId::from_raw(1),
        scancode,
        keycode: Keycode(0),
        modifiers: Modifiers::default(),
        down,
        repeat: false,
    }));
}

fn state(stage: &Stage<GameShow>) -> FlowState {
    stage.show.game().expect("the content names a flow").flow().state()
}

fn player(stage: &Stage<GameShow>) -> [f32; 3] {
    let combat = stage.show.shown.fight.combat.as_ref().expect("a run stands");
    let id = combat.cast()[0];
    combat.simulation().actor(id).expect("the player stands").position.to_array()
}

fn near(at: [f32; 3], to: [f32; 3]) -> bool {
    at.iter().zip(to).map(|(at, to)| (at - to).abs()).fold(0.0, f32::max) < 1.0
}

fn flag(stage: &Stage<GameShow>, name: &str) -> i32 {
    let combat = stage.show.shown.fight.combat.as_ref().expect("a run stands");
    combat.simulation().flags().get(Name::new(name))
}

/// A saved flag's value, by its name.
fn saved_flag(flags: &FlagValues, name: &str) -> Option<i32> {
    let name = Name::new(name).as_u64();
    flags.flags.iter().find(|&&(held, _)| held == name).map(|&(_, value)| value)
}

fn court(stage: &Stage<GameShow>) -> EncounterPhase {
    let combat = stage.show.shown.fight.combat.as_ref().expect("a run stands");
    let zone = combat.simulation().zone().expect("the ward");
    zone.encounters().iter().find(|held| held.id() == id(COURT)).expect("the court's").phase()
}

/// Steps back out through the court's gate until its encounter starts: a continue stands the player facing
/// the gate, so the stick's forward. Returns the frames it took.
fn back_through_the_gate(stage: &mut Stage<GameShow>, clock: &mut u64) -> Result<u32, AppInitError> {
    key(stage, *clock, Scancode::W, true);
    let mut frames = 0;
    while court(stage) == EncounterPhase::Waiting {
        assert!(frames < MOST_FRAMES, "never through the gate: at {:?}", player(stage));
        play(stage, clock)?;
        frames += 1;
    }
    key(stage, *clock, Scancode::W, false);
    Ok(frames)
}

/// Does it all in `init`, and the first frame exits.
struct OutsideCaller;

impl App for OutsideCaller {
    const METADATA: AppMetadata = AppMetadata {
        name: "Game Ward Test",
        organization: "Sapphire",
        identifier: "dev.sapphire.game_ward_test",
        version: GameApp::METADATA.version,
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let saves = std::env::temp_dir().join(format!("game_app-plays-the-ward-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&saves);
        std::fs::create_dir_all(&saves)?;
        let store = progress_store(&saves, |path, bytes| std::fs::write(path, bytes));
        // A save at the court's checkpoint, the market and the garden fought, the court not yet.
        let mut cleared = vec![id(MARKET), id(GARDEN)];
        cleared.sort_unstable();
        let saved = Progress {
            zone: id(WARD),
            start: ZoneStart { checkpoint: Some(CHECKPOINT.to_owned()), cleared },
            flags: FlagValues::default(),
        };
        store.write(&store.snapshot(&saved).map_err(AppInitError::msg)?).map_err(AppInitError::msg)?;

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
        assert_eq!(state(&stage), FlowState::Title { stood: Run::Saved });
        assert_eq!(stage.show.music_phase(), Some("waiting"), "the title's music is the flow's");
        // Every conversation the flow's scenario names, from the pack.
        let flow = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/game.flow.ron");
        let flow = FlowDocument::from_ron(&std::fs::read_to_string(flow)?).map_err(AppInitError::msg)?;
        let scenario = find_scenario(&SCENARIOS, &flow.scenario).expect("this build holds the flow's run");
        let named = scenario.dialogues().map_err(AppInitError::msg)?.len();
        assert!(named > 0, "the run's scenario names the guide's conversation");
        assert_eq!(stage.show.shown.fight.dialogue.dialogues().len(), named, "each, from the pack");

        // The title's continue: the run from the court's checkpoint, on the fight's music.
        let mut clock = 1_000_000_000;
        play(&mut stage, &mut clock)?;
        stage.show.choose(FlowCommand::Continue);
        assert_eq!(state(&stage), FlowState::Playing);
        assert!(near(player(&stage), CHECKPOINT_AT), "where the save says: {:?}", player(&stage));
        for _ in 0..10 {
            play(&mut stage, &mut clock)?;
        }
        assert_eq!(stage.show.music_phase(), None, "in play, the fight's encounters choose");
        assert_eq!(court(&stage), EncounterPhase::Waiting);

        // Back through the gate: the court's encounter starts, and the gate's cutscene with it, the player
        // brought back to the checkpoint its shots are framed from.
        back_through_the_gate(&mut stage, &mut clock)?;
        let mut frames = 0;
        while !matches!(state(&stage), FlowState::Cutscene { .. }) {
            assert!(frames < 4, "the gate's beat fires on the step after: {:?}", state(&stage));
            play(&mut stage, &mut clock)?;
            frames += 1;
        }
        // The beat's commands land at the run's next step, and the stage follows the run's timeline after it.
        let mut frames = 0;
        while stage.show.shown.fight.show.timeline_player().is_none() {
            assert!(frames < 8, "the stage never played the gate: {:?}", state(&stage));
            play(&mut stage, &mut clock)?;
            frames += 1;
        }
        assert!(near(player(&stage), CHECKPOINT_AT), "aligned: {:?}", player(&stage));

        // A skip ends it, and the guide opens on the fight's runner, the fight held while it talks.
        stage.show.choose(FlowCommand::Skip);
        let mut frames = 0;
        while !stage.show.shown.fight.dialogue.is_open() {
            assert!(frames < MOST_FRAMES, "the guide never opened: {:?}", state(&stage));
            play(&mut stage, &mut clock)?;
            frames += 1;
        }
        assert!(matches!(state(&stage), FlowState::Dialogue { .. }), "{:?}", state(&stage));
        let tick = stage.show.shown.fight.combat.as_ref().expect("a run").simulation().tick();
        // The keyboard: next to the question, its first answer lights the ward, then the thanks and farewell.
        let mut presses = 0;
        while stage.show.shown.fight.dialogue.is_open() {
            assert!(presses < 8, "the conversation never ended");
            key(&mut stage, clock, Scancode::RETURN, true);
            play(&mut stage, &mut clock)?;
            key(&mut stage, clock, Scancode::RETURN, false);
            for _ in 0..30 {
                play(&mut stage, &mut clock)?;
            }
            presses += 1;
        }
        assert_eq!((flag(&stage, "ward_lit"), flag(&stage, "met_guide")), (1, 1), "the choice lit the ward");
        let after = stage.show.shown.fight.combat.as_ref().expect("a run").simulation().tick();
        assert!(after > tick, "the fight goes on once it ends");
        assert_eq!(state(&stage), FlowState::Playing, "and the flow with it");
        for _ in 0..5 {
            play(&mut stage, &mut clock)?;
        }
        let kept = stage.show.game().and_then(|game| game.progress()).expect("a save");
        assert!(saved_flag(&kept.flags, "met_guide") == Some(1), "the story's saved: {kept:?}");

        // The pause's way back to the title, under the wipe, and a continue: the court's gate again, which plays
        // neither the cutscene nor the conversation once the guide's been met.
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
        assert_eq!(stage.show.music_phase(), Some("waiting"));
        stage.show.choose(FlowCommand::Continue);
        assert_eq!(state(&stage), FlowState::Playing);
        assert_eq!(flag(&stage, "met_guide"), 1, "the save's story stands");
        back_through_the_gate(&mut stage, &mut clock)?;
        for _ in 0..60 {
            play(&mut stage, &mut clock)?;
            assert_eq!(state(&stage), FlowState::Playing, "no cutscene, no conversation");
        }

        let first_uses = stage.renderer.stats().pipelines.first_use_builds;
        assert_eq!(first_uses, 0, "the load missed: {}", stage.renderer.first_use_report());
        stage.show.finish().map_err(AppInitError::msg)?;
        let saved = progress::load(&store).expect("a save");
        assert_eq!(saved_flag(&saved.flags, "ward_lit"), Some(1), "and so says the file");
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
        eprintln!("plays_the_ward: needs a GPU and cooked content; run it with --ignored");
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
