//! The game's show over `zone-trip`'s replay, frame by frame, from outside the crates (M3 W4), booted as the
//! game's window boots it (M4 H2): the wipe covers the screen from before the zone changes until the new zone's
//! first cells are in, every frame it covers is the same picture, and nothing's built on a frame that the load
//! didn't, or built or waited for on a frame at all. Then a game's flow on `ground-trip`
//! goes back to the title while its trip's wipe covers, and the title's run stands only once the trip's over,
//! where its scenario starts, rather than being carried through the rest of the trip (M3 Z3f), the stage taken
//! back there by a trip of its own under the same wipe (M3 G1b). And a game's run restarted in the hub after its
//! trip there goes home the same way, its run waiting at its first tick until the stage is there (M3 G1b).
//!
//! The platform's loop must own the process's main thread, so this test supplies its own `main`
//! (`harness = false`). It needs a GPU and cooked content: `cargo xtask content`, then
//! `cargo test -p game_app --features dev --test zone_trip -- --ignored`. CI's `App tests` job runs it on lavapipe.

#![allow(clippy::print_stdout, reason = "the trips' frames, for a look at CI's log")]

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use game_app::{GameApp, GameShow};
use game_combat::combat::CombatOption;
use game_combat::documents::FightDocuments;
use game_combat::fight::fight_stage;
use game_combat::fight::trip::TripPhase;
use game_flow::boot::Boot;
use game_flow::document::FlowDocument;
use game_flow::flow::{FlowCommand, FlowState, Run};
use game_flow::game::{FlowContent, Game, GameSetup};
use game_ui::menus::{MenuDocument, PlayerSettings};
use sapphire_gameplay::replay::RunStart;
use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, Platform, RunOptions, run_with};
use sapphire_render::quality::QualitySettings;
use sapphire_rhi::wgpu;
use sapphire_stage::{Content, Readback, Stage, StageDesc, headless_gpu};
use sapphire_time::Stopwatch;

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
    /// The real time its frames took, and the longest of them (M4 H2's measure of a trip's hold).
    took: Duration,
    longest: Duration,
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
        let gpu = Arc::new(headless_gpu()?);
        let documents = FightDocuments::read(platform.content()).map_err(AppInitError::msg)?;
        let screens = MenuDocument::read(platform.content()).map_err(AppInitError::msg)?;
        let show = GameShow::new(documents, screens, PlayerSettings::default())?;
        let replay = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/zone-trip.sreplay");
        let option = CombatOption::Replay { path: replay, to_end: false };
        let boot = Boot::start(platform.host(), gpu, FORMAT, QualitySettings::default(), show, &option)?;
        let (mut stage, report) = boot.run_offscreen((WIDTH, HEIGHT), false)?;
        // Exit test 1 (M4 H2): from the boot screen's first frame, nothing was built on a frame or waited for.
        let booted = report.pipelines;
        assert_eq!((booted.frame_thread_builds, booted.frame_waits), (0, 0), "{report:?}");
        println!(
            "zone_trip: booted in {} frames, {:.0} ms; longest frame {:.1} ms ({}); {} pipelines warmed off the frame",
            report.frames,
            report.warm.as_secs_f64() * 1e3,
            report.longest_frame.as_secs_f64() * 1e3,
            report.longest_in,
            booted.warmed_off_frame,
        );
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
            let drawing = Stopwatch::start();
            let pixels = frame(&mut stage)?;
            let took = drawing.elapsed();
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
                seen.took += took;
                seen.longest = seen.longest.max(took);
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
        // Exit test 1 (M4 H1, H2): the hub's prefetch warmed the proving ground up off the frame, and no frame
        // waited for it.
        let pipelines = stage.renderer.stats().pipelines;
        let on_frame = (pipelines.frame_thread_builds, pipelines.frame_waits);
        assert_eq!(on_frame, (0, 0), "a frame built or waited: {}", stage.renderer.first_use_report());
        println!(
            "zone_trip: {frames} frames, trips held {:?} frames and covered {:?}; their frames took {:?} ms, the \
             longest {:?} ms",
            trips.iter().map(|seen| seen.held).collect::<Vec<_>>(),
            trips.iter().map(|seen| seen.covered.len()).collect::<Vec<_>>(),
            trips.iter().map(|seen| seen.took.as_millis()).collect::<Vec<_>>(),
            trips.iter().map(|seen| format!("{:.1}", seen.longest.as_secs_f64() * 1e3)).collect::<Vec<_>>(),
        );
        to_the_title_mid_trip(platform)?;
        restart_after_a_trip(platform)?;
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
    // The trip out ends in the hub; the title's run stands under the restart's wipe, and the stage goes home by a
    // trip of its own, which that wipe already covers: every frame a trip holds is covered, so nothing flashes
    // between the restart and the trip home, and the zone changes only under it.
    let (mut frames, mut home, mut out_over) = (0, 0, false);
    let mut zone = stage.scene.zone_on_stage();
    while !matches!(flow_state(&stage), FlowState::Title { .. })
        || stage.show.shown.fight.show.trip_phase().is_some()
    {
        assert!(frames < MOST_FRAMES, "never back at the title: {:?}", flow_state(&stage));
        let phase = stage.show.shown.fight.show.trip_phase();
        out_over |= phase.is_none();
        let covered_before = wipe_state(&stage) == "covered";
        play(&mut stage)?;
        frames += 1;
        if let Some(phase) = phase.filter(|phase| phase.holds_simulation()) {
            assert_eq!(wipe_state(&stage), "covered", "a trip holds under the wipe ({phase:?})");
            if out_over {
                home += 1;
            }
        }
        let on_stage = stage.scene.zone_on_stage();
        if on_stage != zone {
            assert!(
                covered_before && wipe_state(&stage) == "covered",
                "the zone changed uncovered ({phase:?})"
            );
        }
        zone = on_stage;
    }
    assert_eq!(flow_state(&stage), FlowState::Title { stood: Run::Saved });
    assert!(home > 0, "the stage went home by a trip");
    assert_eq!(stage.scene.zone_on_stage(), ground, "the title's run stands where its scenario starts");
    let last = stage.show.shown.fight.show.last_trip().expect("it went");
    assert!(last.error.is_none() && Some(last.zone) == ground, "{last:?}");
    let combat = stage.show.shown.fight.combat.as_ref().expect("the title's run stands");
    let sim = combat.simulation();
    assert_eq!(sim.current_zone(), None, "the title's run is where its scenario starts, not on a trip");
    assert_eq!(sim.tick(), 0, "and hasn't stepped");
    let first_uses = stage.renderer.stats().pipelines.first_use_builds;
    assert_eq!(first_uses, 0, "the trips built what the loads didn't: {}", stage.renderer.first_use_report());
    let pipelines = stage.renderer.stats().pipelines;
    let waited = pipelines.frame_waits;
    assert_eq!(waited, 0, "a frame waited for the hub's pipelines: {}", stage.renderer.first_use_report());
    println!("zone_trip: back to the title mid-trip in {frames} frames, and home in {home}");
    Ok(())
}

/// One frame: the steps, then the frame drawn and read back.
struct Drawn {
    /// Where the trip had got before the frame.
    phase: Option<TripPhase>,
    /// The wipe covered the screen before the frame and after it.
    covered: bool,
}

fn step_and_draw(stage: &mut Stage<GameShow>) -> Result<Drawn, AppInitError> {
    let phase = stage.show.shown.fight.show.trip_phase();
    let covered_before = wipe_state(stage) == "covered";
    play(stage)?;
    Ok(Drawn { phase, covered: covered_before && wipe_state(stage) == "covered" })
}

/// A game's run out through the proving ground's exit into the hub, then restarted there as a game's flow
/// restarts one, with no wipe of the flow's over it: the new run stands in the proving ground, and the stage
/// goes back there by a trip of its own, under the trip's wipe, the run waiting at its first tick.
fn restart_after_a_trip(platform: &mut Platform) -> Result<(), AppInitError> {
    let gpu = headless_gpu()?;
    let documents = FightDocuments::read(platform.content()).map_err(AppInitError::msg)?;
    let screens = MenuDocument::read(platform.content()).map_err(AppInitError::msg)?;
    let desc = fight_stage(StageDesc::new(platform.host(), &gpu, FORMAT, QualitySettings::default()));
    let mut content = Content::new(platform.host(), desc.character, desc.sky)?;
    let mut show = GameShow::new(documents, screens, PlayerSettings::default())?;
    let option = CombatOption::Run {
        scenario: "ground-trip".to_owned(),
        start: RunStart::default(),
        script: None,
        keep: None,
    };
    let combat = show.shown.fight.prepare_stock_combat(&option, &mut content)?;
    let mut stage = Stage::new_with_content(gpu, desc, content)?.with_show(show);
    let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
    show.stand(combat, scene, content, renderer, gpu, quality.active().id)?;
    stage.show.open_fight();
    let ground = stage.scene.zone_on_stage().expect("the run stands in the proving ground");

    let Stage { show, scene, content, .. } = &mut stage;
    show.shown.fight.take_exit(scene, content, "to_proving_hub").map_err(AppInitError::msg)?;
    // How long the trip out held, in real time, warming the hub up, which nothing fetched ahead (M4 H2): its
    // wipe holds until the hub's pipelines are built, frames going on meanwhile.
    let (mut frames, mut held, mut longest) = (0, 0, Duration::ZERO);
    let trip = Stopwatch::start();
    while let Some(phase) = stage.show.shown.fight.show.trip_phase() {
        let frame = Stopwatch::start();
        play(&mut stage)?;
        longest = longest.max(frame.elapsed());
        held += u32::from(phase.holds_simulation());
        frames += 1;
        assert!(frames < MOST_FRAMES, "the trip out never ended");
    }
    let warmed = stage.show.shown.fight.show.last_trip().is_some_and(|trip| trip.warm);
    println!(
        "zone_trip: the trip to the hub took {frames} frames, {held} held, in {:.0} ms, longest {:.1} ms; \
         warm ahead: {warmed}",
        trip.elapsed().as_secs_f64() * 1e3,
        longest.as_secs_f64() * 1e3,
    );
    let pipelines = stage.renderer.stats().pipelines;
    let on_frame = (pipelines.frame_thread_builds, pipelines.frame_waits);
    assert_eq!(on_frame, (0, 0), "the trip built or waited: {}", stage.renderer.first_use_report());
    let hub = stage.scene.zone_on_stage();
    assert!(hub.is_some_and(|hub| hub != ground), "the stage went to the hub: {hub:?}");

    let Stage { show, scene, content, .. } = &mut stage;
    let fight = &mut show.shown.fight;
    let combat = fight.prepare_stock_combat(&option, content)?;
    fight.start_run(combat, scene, content).map_err(AppInitError::msg)?;
    assert_eq!(fight.show.trip_phase(), Some(TripPhase::Covering), "the restart takes the stage home");

    let mut zone = hub;
    let (mut held, mut swapped_covered) = (0, false);
    while let Some(phase) = stage.show.shown.fight.show.trip_phase() {
        let drawn = step_and_draw(&mut stage)?;
        frames += 1;
        assert!(frames < MOST_FRAMES, "the trip home never ended");
        let on_stage = stage.scene.zone_on_stage();
        if phase.holds_simulation() {
            held += 1;
            assert_eq!(tick(&stage), 0, "the run waits at its first tick for the stage ({phase:?})");
        }
        if on_stage != zone {
            assert!(drawn.covered, "the zone changed on a frame the wipe didn't cover ({:?})", drawn.phase);
            swapped_covered = true;
        }
        zone = on_stage;
    }
    assert!(swapped_covered, "the stage went home under the wipe");
    assert_eq!(stage.scene.zone_on_stage(), Some(ground), "the restarted run's stage is its own zone");
    let last = stage.show.shown.fight.show.last_trip().expect("it went");
    assert!(last.error.is_none() && last.zone == ground, "{last:?}");
    for _ in 0..10 {
        play(&mut stage)?;
    }
    let combat = stage.show.shown.fight.combat.as_ref().expect("the run stands");
    assert!(combat.simulation().tick() > 0, "the run goes on once the stage is home");
    assert_eq!(combat.simulation().current_zone(), None, "the run's in the zone it started in");
    let first_uses = stage.renderer.stats().pipelines.first_use_builds;
    assert_eq!(
        first_uses,
        0,
        "the trip home built what the loads didn't: {}",
        stage.renderer.first_use_report()
    );
    println!("zone_trip: a restart in the hub went home in {held} held frames");
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
