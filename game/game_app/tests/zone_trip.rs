//! The game's show over `zone-trip`'s replay, frame by frame, from outside the crates (M3 W4): the wipe covers
//! the screen from before the zone changes until the new zone's first cells are in, every frame it covers is the
//! same picture, and nothing's built on a frame that the load didn't.
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
use game_ui::menus::PlayerSettings;
use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, Platform, RunOptions, run_with};
use sapphire_render::quality::QualitySettings;
use sapphire_rhi::wgpu;
use sapphire_stage::{Content, Readback, Stage, StageDesc, headless_gpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
/// Past the replay's second entry, at tick 240, and the trip after it.
const TICKS: u64 = 260;

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
        let desc = fight_stage(StageDesc::new(platform.host(), &gpu, FORMAT, QualitySettings::default()));
        let mut content = Content::new(platform.host(), desc.character, desc.sky)?;
        let mut show = GameShow::new(documents, PlayerSettings::default())?;
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
