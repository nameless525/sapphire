//! The game's show over `zone-trip`'s replay, frame by frame, from outside the crates (M3 W4): the wipe covers
//! the screen from before the zone changes until the new zone's first cells are in, every frame it covers is the
//! same picture, and nothing's built on a frame that the load didn't. Then a game's flow on `ground-trip`
//! goes back to the title while its trip's wipe covers, and the title's run stands only once the trip's over,
//! where its scenario starts, rather than being carried through the rest of the trip (M3 Z3f).
//!
//! The platform's loop must own the process's main thread, so this test supplies its own `main`
//! (`harness = false`). It needs a GPU and cooked content: `cargo xtask content`, then
//! `cargo test -p game_app --features dev --test zone_trip -- --ignored`. CI's `App tests` job runs it on lavapipe.

#![allow(clippy::print_stdout, reason = "the trips' frames, for a look at CI's log")]

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use game_app::{GameApp, GameShow};
use game_combat::combat::CombatOption;
use game_combat::documents::FightDocuments;
use game_combat::fight::fight_stage;
use game_combat::fight::trip::TripPhase;
use game_flow::document::FlowDocument;
use game_flow::flow::{FlowCommand, FlowState, Run};
use game_flow::game::{FlowContent, Game, GameSetup};
use game_ui::menus::{MenuDocument, PlayerSettings};
use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, Platform, RunOptions, run_with};
use sapphire_render::quality::QualitySettings;
use sapphire_rhi::wgpu;
use sapphire_stage::{Content, Readback, Stage, StageDesc, headless_gpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
/// Past the replay's second entry, at tick 240, and the trip after it.
const TICKS: u64 = 260;
/// `ground-trip` in the proving ground, which its cart yard's clearing wins.
const FLOW: &str = r#"(type: "sapphire::game::FlowDocument", version: 1, data: (
    scenario: "ground-trip",
    zone: "e8c21831-7d98-4a79-b592-912b806cf3e2",
    beats: [(when: Cleared("5d3a9e21-7c44-4b0e-9f16-0a8ce3b7d214"), then: Victory)],
    music: (title: "waiting", victory: "cleared", defeat: "waiting"),
    holds: (banner: 2.0, defeat: 3.0, victory: 5.0),
))"#;
/// More frames than a trip and a restart's wipe take.
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

fn tick(stage: &Stage<GameShow>) -> u64 {
    stage.show.shown.fight.combat.as_ref().expect("a fight stands").simulation().tick()
}

/// The wipe's state, as of its last frame.
fn wipe_state(stage: &Stage<GameShow>) -> String {
    let wipe = stage.show.shown.wipe.as_ref().expect("a fight that travels has a wipe");
    wipe.tree().state().unwrap_or_default().to_owned()
}

/// What one trip looked like, frame by frame.
#[derive(Default)]
struct Seen {
    /// Frames drawn while the wipe said it covered the screen.
    covered: Vec<Vec<u8>>,
    /// The zone changed under one of them.
    swapped_covered: bool,
    /// Frames while the simulation was held.
    held: u32,
}

/// Does it all in `init`, and the first frame exits.
struct OutsideCaller;

impl App for OutsideCaller {
    const METADATA: AppMetadata = AppMetadata {
        name: "Game Zone Trip Test",
        organization: "Sapphire",
        identifier: "dev.sapphire.game_zone_trip_test",
        version: GameApp::METADATA.version,
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let gpu = headless_gpu()?;
        let documents = FightDocuments::read(platform.content()).map_err(AppInitError::msg)?;
        let screens = MenuDocument::read(platform.content()).map_err(AppInitError::msg)?;
        let desc = fight_stage(StageDesc::new(platform.host(), &gpu, FORMAT, QualitySettings::default()));
        let mut content = Content::new(platform.host(), desc.character, desc.sky)?;
        let mut show = GameShow::new(documents, screens, PlayerSettings::default())?;
        let replay = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/zone-trip.sreplay");
        let option = CombatOption::Replay { path: replay, to_end: false };
        let combat = show.shown.fight.prepare_stock_combat(&option, &mut content)?;
        let mut stage = Stage::new_with_content(gpu, desc, content)?.with_show(show);
        let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
        show.stand(combat, scene, content, renderer, gpu, quality.active().id)?;
        assert!(stage.show.shown.wipe.is_some(), "a fight that goes between zones has a wipe");
        stage.show.open_fight();

        let mut trips: Vec<Seen> = Vec::new();
        let mut zone = stage.scene.zone_on_stage();
        let mut frames = 0;
        while tick(&stage) < TICKS {
            stage.step();
            stage.show.drive_view(Duration::from_millis(16));
            let phase = stage.show.shown.fight.show.trip_phase();
            let covered_before = wipe_state(&stage) == "covered";
            let pixels = frame(&mut stage)?;
            frames += 1;
            assert!(frames < 20_000, "it never got to tick {TICKS}");
            let on_stage = stage.scene.zone_on_stage();
            if let Some(phase) = phase {
                if phase == TripPhase::Covering && trips.last().is_none_or(|seen| !seen.covered.is_empty()) {
                    trips.push(Seen::default());
                }
                let seen = trips.last_mut().expect("one's under way");
                if phase.holds_simulation() {
                    seen.held += 1;
                }
                if covered_before && wipe_state(&stage) == "covered" {
                    seen.covered.push(pixels);
                    seen.swapped_covered |= on_stage != zone;
                } else {
                    assert_eq!(
                        on_stage, zone,
                        "the zone changed on a frame the wipe didn't cover ({phase:?})"
                    );
                }
            } else {
                assert_eq!(on_stage, zone, "the zone changed with no trip");
            }
            zone = on_stage;
        }

        assert_eq!(trips.len(), 2, "two entries by tick {TICKS}");
        for (index, seen) in trips.iter().enumerate() {
            assert!(seen.swapped_covered, "trip {index}: the zone changed under the wipe");
            assert!(seen.covered.len() >= 2, "trip {index}: {} frames covered", seen.covered.len());
            assert!(
                seen.covered.windows(2).all(|pair| pair[0] == pair[1]),
                "trip {index}: every covered frame is the same picture"
            );
        }
        let last = stage.show.shown.fight.show.last_trip().expect("it went");
        assert!(last.error.is_none(), "{last:?}");
        assert!(last.warm, "the hub had the proving ground coming: {last:?}");
        stage.show.shown.fight.check().map_err(AppInitError::msg)?;
        let first_uses = stage.renderer.stats().pipelines.first_use_builds;
        assert_eq!(first_uses, 0, "a trip built what the load didn't: {}", stage.renderer.first_use_report());
        println!(
            "zone_trip: {frames} frames, trips held {:?} frames and covered {:?}",
            trips.iter().map(|seen| seen.held).collect::<Vec<_>>(),
            trips.iter().map(|seen| seen.covered.len()).collect::<Vec<_>>(),
        );
        to_the_title_mid_trip(platform)?;
        Ok(Self)
    }

    fn event(&mut self, _: &mut Platform, _: &Event) -> AppControl {
        AppControl::Continue
    }

    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        AppControl::Exit
    }
}

fn flow_state(stage: &Stage<GameShow>) -> FlowState {
    stage.show.game().expect("a game's show").flow().state()
}

/// A frame of a game's show with no window: a step, the view and a picture.
fn play(stage: &mut Stage<GameShow>) -> Result<(), AppInitError> {
    stage.step();
    stage.show.drive_view(Duration::from_millis(16));
    frame(stage)?;
    Ok(())
}

/// A game's run that goes through the proving ground's exit, and the pause's way back to the title while the
/// trip's wipe covers the screen, before the hub's swapped in: the title's run waits for the trip to end, so
/// it stands at its scenario's start, in no zone it travelled to, and steps from its first tick.
fn to_the_title_mid_trip(platform: &mut Platform) -> Result<(), AppInitError> {
    let gpu = headless_gpu()?;
    let documents = FightDocuments::read(platform.content()).map_err(AppInitError::msg)?;
    let screens = MenuDocument::read(platform.content()).map_err(AppInitError::msg)?;
    let desc = fight_stage(StageDesc::new(platform.host(), &gpu, FORMAT, QualitySettings::default()));
    let mut content = Content::new(platform.host(), desc.character, desc.sky)?;
    let flow = FlowContent {
        document: FlowDocument::from_ron(FLOW).map_err(AppInitError::msg)?,
        registered: Vec::new(),
    };
    let game = Game::new(flow, GameSetup::default()).map_err(AppInitError::msg)?;
    let mut show = GameShow::new(documents, screens, PlayerSettings::default())?.with_game(game);
    let first = show.opening_run().expect("a game's run");
    let combat = show.shown.fight.prepare_stock_combat(&first, &mut content)?;
    let mut stage = Stage::new_with_content(gpu, desc, content)?.with_show(show);
    let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
    show.stand(combat, scene, content, renderer, gpu, quality.active().id)?;
    let ground = stage.scene.zone_on_stage();
    assert!(ground.is_some(), "the run stands in its zone");
    stage.show.choose(FlowCommand::NewGame);
    assert_eq!(flow_state(&stage), FlowState::Playing);
    for _ in 0..10 {
        play(&mut stage)?;
    }

    // Out through the ground's exit, to the hub, which nothing's fetched ahead: back to the title as soon
    // as the trip's wipe covers, while the hub's still to come.
    let Stage { show, scene, content, .. } = &mut stage;
    show.shown.fight.take_exit(scene, content, "to_proving_hub").map_err(AppInitError::msg)?;
    let mut frames = 0;
    while !(wipe_state(&stage) == "covered"
        && stage.show.shown.fight.show.trip_phase().is_some_and(TripPhase::holds_simulation)
        && stage.scene.zone_on_stage() == ground)
    {
        assert!(frames < MOST_FRAMES, "the trip's wipe never covered before the swap");
        play(&mut stage)?;
        frames += 1;
    }
    stage.show.choose(FlowCommand::Pause);
    stage.show.choose(FlowCommand::ToTitle);
    let mut frames = 0;
    while !matches!(flow_state(&stage), FlowState::Title { .. }) {
        assert!(frames < MOST_FRAMES, "never back at the title: {:?}", flow_state(&stage));
        play(&mut stage)?;
        frames += 1;
    }
    assert_eq!(flow_state(&stage), FlowState::Title { stood: Run::Saved });
    assert!(stage.show.shown.fight.show.trip_phase().is_none(), "the trip's over");
    let combat = stage.show.shown.fight.combat.as_ref().expect("the title's run stands");
    let sim = combat.simulation();
    assert_eq!(sim.current_zone(), None, "the title's run is where its scenario starts, not on a trip");
    assert_eq!(sim.tick(), 0, "and hasn't stepped");
    println!(
        "zone_trip: back to the title mid-trip in {frames} frames, the stage {} the ground",
        if stage.scene.zone_on_stage() == ground { "in" } else { "out of" }
    );
    Ok(())
}

fn main() -> ExitCode {
    if !std::env::args().any(|argument| argument == "--ignored") {
        eprintln!("zone_trip: needs a GPU and cooked content; run it with --ignored");
        return ExitCode::SUCCESS;
    }
    if std::env::var_os("SAPPHIRE_CONTENT_DIR").is_none() {
        let cook = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/content/desktop");
        // SAFETY: nothing else runs yet: the platform's loop and its threads start below.
        unsafe { std::env::set_var("SAPPHIRE_CONTENT_DIR", cook) };
    }
    run_with::<OutsideCaller>(RunOptions { background: true, headless: true })
}
