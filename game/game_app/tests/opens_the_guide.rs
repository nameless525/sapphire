//! A conversation in the game's show, from outside the crates (M3 Q2b): a recorded fight on `dialogue-flag`
//! loads the Lantern Ward guide's dialogue from the cooked pack, the run opens it as a game's flow would, and
//! the keyboard walks the panel through it to the choice that lights the ward. The fight holds while it's
//! open; the choice's flag lands in the simulation; the end comes back once; no frame builds a pipeline the
//! load didn't, and no step of the simulation allocates.
//!
//! Like `reaches_the_fight`, it supplies its own `main` (`harness = false`) and runs only when asked:
//! `cargo xtask content`, then `cargo test -p game_app --test opens_the_guide -- --ignored`.

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use game_app::{GameApp, GameShow};
use game_combat::combat::CombatOption;
use game_combat::documents::FightDocuments;
use game_combat::fight::fight_stage;
use game_ui::menus::{MenuDocument, PlayerSettings};
use sapphire_core::Name;
use sapphire_gameplay::DialogueId;
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, Event, KeyEvent, Keycode, Modifiers, Platform, RunOptions,
    Scancode, Timestamp, WindowId, run_with,
};
use sapphire_render::quality::QualitySettings;
use sapphire_rhi::wgpu;
use sapphire_stage::{Content, Readback, Stage, StageDesc, headless_gpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;
/// The guide's dialogue: the only one `dialogue-flag` loads.
const GUIDE: DialogueId = DialogueId::from_raw(0);
/// Ticks fought before the conversation, and after it.
const BEFORE: u32 = 60;
const AFTER: u32 = 120;

/// Draws the stage's next frame into a new texture and reads it back.
fn frame(stage: &mut Stage<GameShow>) -> Result<Vec<u8>, AppInitError> {
    let target = stage.target(WIDTH, HEIGHT, wgpu::TextureUsages::COPY_SRC);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = Readback::new(&stage.gpu, WIDTH, HEIGHT);
    stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
    stage.check_gpu()?;
    readback.read(&stage.gpu).map_err(AppInitError::msg)
}

fn simulation(stage: &Stage<GameShow>) -> &sapphire_gameplay::Simulation {
    stage.show.shown.fight.combat.as_ref().expect("a fight stands").simulation()
}

/// Presses and lets go of Return at `second`, a frame apart, as a window's events would come: the frame after.
fn press_return(stage: &mut Stage<GameShow>, second: u64) -> Result<Vec<u8>, AppInitError> {
    let at = |quarter: u64| Timestamp::from_nanos(second * 1_000_000_000 + quarter * 250_000_000);
    for (quarter, down) in [(0, true), (2, false)] {
        stage.show.handle_event(&Event::Key(KeyEvent {
            timestamp: at(quarter),
            window: WindowId::from_raw(1),
            scancode: Scancode::RETURN,
            keycode: Keycode(0),
            modifiers: Modifiers::default(),
            down,
            repeat: false,
        }));
        stage.show.update_menus(at(quarter + 1));
        frame(stage)?;
    }
    frame(stage)
}

/// Does it all in `init`, and the first frame exits.
struct Visitor;

impl App for Visitor {
    const METADATA: AppMetadata = AppMetadata {
        name: "Game Conversation Test",
        organization: "Sapphire",
        identifier: "dev.sapphire.game_conversation_test",
        version: GameApp::METADATA.version,
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let gpu = headless_gpu()?;
        let documents = FightDocuments::read(platform.content()).map_err(AppInitError::msg)?;
        let screens = MenuDocument::read(platform.content()).map_err(AppInitError::msg)?;
        let desc = fight_stage(StageDesc::new(platform.host(), &gpu, FORMAT, QualitySettings::default()));
        let mut content = Content::new(platform.host(), desc.character, desc.sky)?;
        let mut show = GameShow::new(documents, screens, PlayerSettings::default())?;
        let path = std::env::temp_dir().join(format!("opens_the_guide-{}.sreplay", std::process::id()));
        let option = CombatOption::Record {
            path,
            scenario: "dialogue-flag".to_owned(),
            script: Some("fight-back".to_owned()),
        };
        let combat = show.shown.fight.prepare_stock_combat(&option, &mut content)?;
        let mut stage = Stage::new_with_content(gpu, desc, content)?.with_show(show);
        let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
        show.stand(combat, scene, content, renderer, gpu, quality.active().id)?;
        assert_eq!(
            stage.show.shown.fight.dialogue.dialogues().len(),
            1,
            "the guide, from the pack by its digest"
        );

        // The title, then its start.
        stage.show.update_menus(Timestamp::from_nanos(1_000_000_000));
        frame(&mut stage)?;
        press_return(&mut stage, 2)?;
        assert!(!stage.show.on_title(), "the title's start opens the fight");
        for _ in 0..BEFORE {
            stage.step();
            stage.show.drive_view(Duration::from_millis(16));
        }
        let opened = simulation(&stage).tick();
        assert_eq!(opened, u64::from(BEFORE));
        let ward_lit = Name::new("ward_lit");
        assert_eq!(simulation(&stage).flags().get(ward_lit), 0);

        // The game's flow opens the guide; the next frame puts the panel up, with the greeting.
        stage.show.open_dialogue(GUIDE).map_err(AppInitError::msg)?;
        let fight = frame(&mut stage)?;
        let lines = |stage: &Stage<GameShow>| stage.show.shown.fight.dialogue.lines_shown();
        assert_eq!(lines(&stage), 1);
        for _ in 0..10 {
            stage.step();
        }
        assert_eq!(simulation(&stage).tick(), opened, "the fight holds while the guide talks");

        // Next to the question; the first answer lights the ward; the thanks, the farewell, and it's done.
        let mut second = 4;
        let mut shown = Vec::new();
        while stage.show.shown.fight.dialogue.is_open() {
            assert!(second < 12, "the conversation never ended");
            shown.push(press_return(&mut stage, second)?);
            second += 2;
        }
        assert_eq!(lines(&stage), 4, "greeting, question, thanks, farewell");
        assert_ne!(shown[0], fight, "the panel's on the picture");
        assert_eq!(simulation(&stage).flags().get(ward_lit), 1, "the choice lit the ward");
        let ended = stage.show.take_dialogue_ended().expect("the end comes back");
        assert_eq!((ended.dialogue, ended.tick), (GUIDE, opened));
        assert!(stage.show.take_dialogue_ended().is_none(), "once");

        // The fight goes on. No step of the simulation allocated, before, while or after it talked.
        for _ in 0..AFTER {
            stage.step();
            stage.show.drive_view(Duration::from_millis(16));
        }
        assert_eq!(simulation(&stage).tick(), opened + u64::from(AFTER));
        assert!(sapphire_core::alloc::is_installed(), "the stage counts");
        assert_eq!(simulation(&stage).stats().allocations, 0, "rule R6, with a conversation in the run");
        frame(&mut stage)?;

        let first_uses = stage.renderer.stats().pipelines.first_use_builds;
        assert_eq!(first_uses, 0, "the load missed: {}", stage.renderer.first_use_report());
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
        eprintln!("opens_the_guide: needs a GPU and cooked content; run it with --ignored");
        return ExitCode::SUCCESS;
    }
    sapphire_core::log::init(Default::default(), None).expect("initialize app test logging");
    if std::env::var_os("SAPPHIRE_CONTENT_DIR").is_none() {
        let cook = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/content/desktop");
        // SAFETY: nothing else runs yet: the platform's loop and its threads start below.
        unsafe { std::env::set_var("SAPPHIRE_CONTENT_DIR", cook) };
    }
    run_with::<Visitor>(RunOptions { background: true, headless: true })
}
