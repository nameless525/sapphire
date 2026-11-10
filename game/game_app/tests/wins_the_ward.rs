//! The whole walk on the game's stage, from outside the crates (M3 G1c, exit tests 1 and 7): the content's flow
//! over `tests/replays/ward-whole-walk.sreplay`, a new game from the title along the Lantern Ward's tour, the
//! market's, the garden's and the court's fights, the court's gate cutscene watched, the guide's conversation
//! as the file recorded it, and the terrace's boss, to the flow's victory. Once on Desktop-Low's settings and
//! once on Mobile-Low's: each draws the walk's frames, builds no pipeline the load didn't, allocates in no
//! step, and reaches the checksums the replay's file holds. No window.
//!
//! `SAPPHIRE_WALK_TIERS=<key>[,<key>]` walks only the tiers it names (`desktop-low`, `mobile-low`), so CI can
//! walk each on a runner of its own; unset, both.
//!
//! A frame for every step by default. `SAPPHIRE_WALK_STEPS_PER_FRAME=<n>` draws one every `n` steps instead,
//! as a device that can't keep up runs several steps a frame: CI's lavapipe, which draws a frame of this
//! stage in tens of milliseconds, can't draw the walk's 14,000-odd steps twice inside its job's time.
//!
//! The platform's loop must own the process's main thread, so this test supplies its own `main`
//! (`harness = false`). It needs a GPU and cooked content: `cargo xtask content`, then
//! `cargo test -p game_app --features dev --test wins_the_ward -- --ignored`. CI's `App tests` jobs run it on
//! lavapipe, a tier each.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use game_app::{GameApp, GameShow};
use game_flow::flow::{FlowCommand, FlowState, Run};
use game_flow::game::GameSetup;
use game_ui::menus::PlayerSettings;
use sapphire_gameplay::{EncounterPhase, Simulation};
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, Event, Platform, RunOptions, Timestamp, run_with,
};
use sapphire_render::quality::{QualitySettings, TierId};
use sapphire_rhi::wgpu;
use sapphire_stage::{Readback, Stage};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
/// The replay, from the repository's root.
const WALK: &str = "tests/replays/ward-whole-walk.sreplay";
/// Longer than the walk: fifteen minutes of play, the cutscene's held frames among them.
const MOST_STEPS: u32 = 15 * 60 * 60;

/// Draws the stage's next frame into a new texture and reads it back.
fn frame(stage: &mut Stage<GameShow>) -> Result<Vec<u8>, AppInitError> {
    let target = stage.target(WIDTH, HEIGHT, wgpu::TextureUsages::COPY_SRC);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = Readback::new(&stage.gpu, WIDTH, HEIGHT);
    stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
    stage.check_gpu()?;
    readback.read(&stage.gpu).map_err(AppInitError::msg)
}

/// A frame of the app's, with no window: the menus, a step, the view and, if `draw`, a picture.
fn play(stage: &mut Stage<GameShow>, clock: &mut u64, draw: bool) -> Result<(), AppInitError> {
    *clock += 16_666_667;
    stage.show.update_menus(Timestamp::from_nanos(*clock));
    stage.show.follow_input(Timestamp::from_nanos(*clock));
    stage.step();
    stage.show.drive_view(Duration::from_millis(16));
    if draw {
        frame(stage)?;
    }
    Ok(())
}

/// The tiers the walk is played on, in this order.
const TIERS: [TierId; 2] = [TierId::DesktopLow, TierId::MobileLow];

/// The tiers `SAPPHIRE_WALK_TIERS` names, or all of [`TIERS`] when it's unset. A name that isn't one of them is
/// an error, so a misspelt choice can't pass by walking nothing.
fn tiers() -> Result<Vec<TierId>, String> {
    let Ok(names) = std::env::var("SAPPHIRE_WALK_TIERS") else {
        return Ok(TIERS.to_vec());
    };
    let keys: Vec<&str> = TIERS.iter().map(|tier| tier.key()).collect();
    let mut chosen = Vec::new();
    for name in names.split(',').map(str::trim) {
        match TierId::from_key(name).filter(|tier| TIERS.contains(tier)) {
            Some(tier) if !chosen.contains(&tier) => chosen.push(tier),
            Some(_) => {}
            None => {
                return Err(format!(
                    "SAPPHIRE_WALK_TIERS: {name:?} isn't a tier the walk is played on: {}",
                    keys.join(", ")
                ));
            }
        }
    }
    Ok(chosen)
}

/// Steps a frame: `SAPPHIRE_WALK_STEPS_PER_FRAME`, or 1.
fn steps_per_frame() -> u32 {
    std::env::var("SAPPHIRE_WALK_STEPS_PER_FRAME")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .filter(|&steps| steps > 0)
        .unwrap_or(1)
}

fn state(stage: &Stage<GameShow>) -> FlowState {
    stage.show.game().expect("the content names a flow").flow().state()
}

fn simulation(stage: &Stage<GameShow>) -> &Simulation {
    stage.show.shown.fight.combat.as_ref().expect("a run stands").simulation()
}

/// What one tier's walk came to.
struct Walked {
    steps: u32,
    frames: u32,
    tick: u64,
    cutscenes: u32,
    seconds: f32,
}

/// The whole walk on `tier`'s settings, from the title to the victory.
fn walk(platform: &Platform, replay: &Path, tier: TierId) -> Result<Walked, AppInitError> {
    let setup = GameSetup { replay: Some(replay.to_owned()), ..GameSetup::default() };
    let (show, first) = GameShow::from_content(platform.content(), PlayerSettings::default(), setup)?;
    let gpu = sapphire_stage::headless_gpu()?;
    let quality = QualitySettings { tier: Some(tier), ..QualitySettings::default() };
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
    assert_eq!(quality.active().id, tier, "the settings' tier");
    show.stand(combat, scene, content, renderer, gpu, quality.active().id)?;
    assert_eq!(state(&stage), FlowState::Title { stood: Run::New });

    let started = platform.now();
    let every = steps_per_frame();
    let mut clock = 1_000_000_000;
    play(&mut stage, &mut clock, true)?;
    stage.show.choose(FlowCommand::NewGame);
    assert_eq!(state(&stage), FlowState::Playing);
    let (mut steps, mut frames, mut cutscenes, mut was_cutscene) = (1, 1, 0, false);
    while !matches!(state(&stage), FlowState::Victory { .. }) {
        assert!(steps < MOST_STEPS, "never won: {:?} at tick {}", state(&stage), simulation(&stage).tick());
        assert!(!matches!(state(&stage), FlowState::Defeat { .. }), "the player fell");
        let draw = steps.is_multiple_of(every);
        play(&mut stage, &mut clock, draw)?;
        let cutscene = matches!(state(&stage), FlowState::Cutscene { .. });
        cutscenes += u32::from(cutscene && !was_cutscene);
        was_cutscene = cutscene;
        steps += 1;
        frames += u32::from(draw);
    }
    // The win's own frame, whatever the count.
    play(&mut stage, &mut clock, true)?;
    frames += 1;
    let seconds = platform.now().saturating_duration_since(started).as_secs_f32();
    let sim = simulation(&stage);
    let tick = sim.tick();
    let zone = sim.zone().expect("the ward");
    assert!(
        zone.encounters().iter().all(|held| held.phase() == EncounterPhase::Cleared),
        "every encounter's cleared"
    );
    assert!(sapphire_core::alloc::is_installed(), "the stage counts");
    assert_eq!(sim.stats().allocations, 0, "rule R6, over the whole walk");
    let first_uses = stage.renderer.stats().pipelines.first_use_builds;
    assert_eq!(first_uses, 0, "the load missed on {}: {}", tier.name(), stage.renderer.first_use_report());
    // The replay's checksums, beside it: the walk on the stage is the walk the file recorded.
    stage.show.finish().map_err(AppInitError::msg)?;
    Ok(Walked { steps, frames, tick, cutscenes, seconds })
}

/// Does it all in `init`, and the first frame exits.
struct OutsideCaller;

impl App for OutsideCaller {
    const METADATA: AppMetadata = AppMetadata {
        name: "Game Whole Walk Test",
        organization: "Sapphire",
        identifier: "dev.sapphire.game_whole_walk_test",
        version: GameApp::METADATA.version,
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let replay = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(WALK);
        for tier in tiers().map_err(AppInitError::msg)? {
            let walked = walk(platform, &replay, tier)?;
            assert!(walked.cutscenes >= 1, "the court's gate among them, watched");
            eprintln!(
                "wins_the_ward: {}: won at tick {} after {} of the stage's steps, {} frames drawn, in {:.1} s; \
                 first_use=0, step allocations 0",
                tier.name(),
                walked.tick,
                walked.steps,
                walked.frames,
                walked.seconds,
            );
        }
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
        eprintln!("wins_the_ward: needs a GPU and cooked content; run it with --ignored");
        return ExitCode::SUCCESS;
    }
    if let Err(error) = tiers() {
        eprintln!("wins_the_ward: {error}");
        return ExitCode::FAILURE;
    }
    sapphire_core::log::init(Default::default(), None).expect("initialize app test logging");
    if std::env::var_os("SAPPHIRE_CONTENT_DIR").is_none() {
        let cook = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/content/desktop");
        // SAFETY: nothing else runs yet: the platform's loop and its threads start below.
        unsafe { std::env::set_var("SAPPHIRE_CONTENT_DIR", cook) };
    }
    run_with::<OutsideCaller>(RunOptions { background: true, headless: true })
}
