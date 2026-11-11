//! A playtest's mark, drawn (M4 F2): the game's capture plays a run's file through the flow, as
//! `game_app --replay <file> --at-tick <n> --capture <png>` does, and its last frame is the run at that tick:
//! not one before, not one after, however many frames the title took. A tick past the file's end is an error,
//! not a picture of something else.
//!
//! The platform's loop must own the process's main thread, so this test supplies its own `main`
//! (`harness = false`). It needs a GPU and cooked content: `cargo xtask content`, then
//! `cargo test -p game_app --test draws_a_marked_tick -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use game_app::{GameApp, GameShow};
use game_flow::capture::{GameCapture, capture_with};
use game_flow::flow::FlowCommand;
use game_flow::game::GameSetup;
use game_ui::hud::OffscreenDisplay;
use game_ui::menus::PlayerSettings;
use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, Platform, RunOptions, run_with};
use sapphire_render::quality::QualitySettings;
use sapphire_stage::Stage;

/// The game's own run, from a new game to the win, from the repository's root.
const WALK: &str = "tests/replays/ward-whole-walk.sreplay";
/// The tick drawn.
const MARKED: u64 = 600;

fn run_tick(stage: &Stage<GameShow>) -> Option<u64> {
    Some(stage.show.shown.fight.combat.as_ref()?.simulation().tick())
}

/// Draws `at_tick` of the walk within `frames`, and gives the ticks the run stood at after each frame drawn.
fn draw(platform: &Platform, at_tick: u64, frames: u32) -> Result<Vec<u64>, AppInitError> {
    let replay = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(WALK);
    let setup = GameSetup { replay: Some(replay), ..GameSetup::default() };
    let (show, first) = GameShow::from_content(platform.content(), PlayerSettings::default(), setup)?;
    let capture = GameCapture {
        choose: Some(FlowCommand::NewGame),
        size: (64, 36),
        ticks: frames,
        at_tick: Some(at_tick),
        quality: QualitySettings::default(),
        forward: false,
        draw_from: 0,
        boot: false,
        display: OffscreenDisplay::default(),
    };
    let mut ticks = Vec::new();
    capture_with(platform.host(), show, &first, &capture, None, |stage| ticks.extend(run_tick(stage)))?;
    Ok(ticks)
}

/// Does it all in `init`, and the first frame exits.
struct OutsideCaller;

impl App for OutsideCaller {
    const METADATA: AppMetadata = AppMetadata {
        name: "Game Marked Tick Test",
        organization: "Sapphire",
        identifier: "dev.sapphire.game_marked_tick_test",
        version: GameApp::METADATA.version,
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let ticks = draw(platform, MARKED, MARKED as u32 + 600)?;
        // After each frame drawn, then after the last one's readback.
        assert_eq!(
            ticks.iter().max(),
            Some(&MARKED),
            "the last frame is the marked tick's, and none went past it"
        );
        assert!(
            ticks.len() > MARKED as usize,
            "the title's frames came first, so frames aren't ticks: {}",
            ticks.len()
        );

        let short = draw(platform, 20_000, 400)
            .expect_err("the walk ends at 14,320 ticks, and 400 frames get nowhere near");
        assert!(short.to_string().contains("short of tick 20000"), "{short}");
        eprintln!("draws_a_marked_tick: tick {MARKED} drawn after {} frames", ticks.len() - 1);
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
        eprintln!("draws_a_marked_tick: needs a GPU and cooked content; run it with --ignored");
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
