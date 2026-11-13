//! The quality tier in the game, from outside the crates (M4 T3): heat forced on the tier's thermal ladder
//! steps it down at once (its scale, its optional passes, a 30 fps cap the platform's told of), and the tier the
//! ladder then asks for is built off the frame and changed to only while `zone-trip`'s wipe covers the screen,
//! keeping every mesh and the run, with no frame building a pipeline.
//!
//! It starts on Desktop-High, held as a player's choice, and goes to automatic, so the ladder's tier rung is
//! there whatever the device (lavapipe's own tier is Desktop-Low, which has none). Then it does the same from
//! Mobile-High down to Mobile-Low (M4 H9), on a phone's class of device over this one's GPU, where the 30 fps
//! rung is the rate the display paces to rather than a desktop's cap.
//!
//! The platform's loop must own the process's main thread, so this test supplies its own `main`
//! (`harness = false`). It needs a GPU and cooked content: `cargo xtask content`, then
//! `cargo test -p game_app --features dev --test follows_the_ladder -- --ignored`.

#![allow(clippy::print_stdout, reason = "when the tier changed, for a look at CI's log")]

use std::num::NonZeroU32;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use game_app::tiers::{Pacing, TierFollower};
use game_app::{GameApp, GameShow};
use game_combat::combat::CombatOption;
use game_combat::documents::FightDocuments;
use game_flow::boot::Boot;
use game_ui::menus::{MenuDocument, PlayerSettings};
use sapphire_core::target::FormFactor;
use sapphire_core::thermal::{ThermalReading, ThermalState};
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, Event, Platform, RunOptions, SystemInfo, run_with,
};
use sapphire_render::quality::{QualitySettings, TierId, TierSource};
use sapphire_rhi::wgpu;
use sapphire_stage::{Readback, Stage, headless_gpu};

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

/// Does it all in `init`, and the first frame exits.
struct OutsideCaller;

impl App for OutsideCaller {
    const METADATA: AppMetadata = AppMetadata {
        name: "Game Ladder Test",
        organization: "Sapphire",
        identifier: "dev.sapphire.game_ladder_test",
        version: GameApp::METADATA.version,
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        walk(platform, TierId::DesktopHigh)?;
        walk(platform, TierId::MobileHigh)?;
        Ok(Self)
    }

    fn event(&mut self, _: &mut Platform, _: &Event) -> AppControl {
        AppControl::Continue
    }

    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        AppControl::Exit
    }
}

/// Boots on `high`, held as a player's choice, goes to automatic under critical heat, and follows the ladder to
/// the tier below at the second trip's wipe.
fn walk(platform: &Platform, high: TierId) -> Result<(), AppInitError> {
    let class = high.class();
    let low = high.lower().expect("a class's higher tier");
    // A phone's class of device, where the tier's a phone's, on this one's GPU.
    let host = platform.host().as_device(SystemInfo { device_class: class, ..*platform.system_info() });
    let gpu = Arc::new(headless_gpu()?);
    let documents = FightDocuments::read(platform.content()).map_err(AppInitError::msg)?;
    let screens = MenuDocument::read(platform.content()).map_err(AppInitError::msg)?;
    let show = GameShow::new(documents, screens, PlayerSettings::default())?;
    let replay = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/zone-trip.sreplay");
    let option = CombatOption::Replay { path: replay, to_end: false };
    // Booted as the game's window boots it (M4 H2), on the higher tier as a player's choice.
    let held = QualitySettings { tier: Some(high), ..QualitySettings::default() };
    let boot = Boot::start(&host, gpu, FORMAT, held, show, &option)?;
    let (mut stage, _) = boot.run_offscreen((WIDTH, HEIGHT), false)?;
    // Play waits on the title for the fades the boot left to warm there (M4 C7), which no frame builds.
    let mut waited = 0;
    while stage.show.play_waits() {
        assert!(waited < 100_000, "the title's fades never built");
        frame(&mut stage)?;
        waited += 1;
    }
    stage.show.open_fight();
    assert_eq!(stage.quality.active().id, high);
    let character = stage.character;

    // Automatic from here, and the device as hot as it gets.
    let mut tiers = TierFollower::new(&PlayerSettings::default(), class);
    tiers.follow_settings(&PlayerSettings::default(), &mut stage);
    let critical = ThermalReading { state: ThermalState::Critical, ..ThermalReading::default() };
    let mut caps = Vec::new();
    let mut changed_at = None;
    let mut frames = 0u32;
    while tick(&stage) < TICKS {
        stage.step();
        stage.show.drive_view(Duration::from_millis(16));
        let still = stage.show.holds_still();
        if let Some(pacing) = tiers.frame(&mut stage, critical) {
            caps.push(pacing);
        }
        if changed_at.is_none() && !tiers.changes().is_empty() {
            assert!(still, "the tier changed on a frame the wipe didn't cover");
            changed_at = Some((frames, tick(&stage)));
        }
        frame(&mut stage)?;
        frames += 1;
        assert!(frames < 20_000, "{high}: it never got to tick {TICKS}");
        if frames == 1 {
            let state = stage.quality.ladder().state();
            assert!(state.depth > 0, "{high}: critical heat steps the ladder down at once");
            assert!(!state.shed.is_empty(), "and sheds its optional passes");
            assert_eq!(stage.renderer.shed(), state.shed, "in place, on the renderer");
        }
    }

    // A desktop gates its frames at the cap; a phone's display paces to the rate.
    let thirty = NonZeroU32::new(30);
    let capped = |pacing: &Pacing| match class {
        FormFactor::Desktop => pacing.cap == thirty,
        FormFactor::Mobile => pacing.rate == thirty && pacing.cap.is_none(),
    };
    assert!(caps.iter().any(capped), "{high}: the ladder's 30 fps rung reached the pacing: {caps:?}");
    let (at_frame, at_tick) = changed_at.expect("the ladder's tier changed by the second trip");
    assert_eq!(tiers.changes(), [low]);
    assert_eq!(stage.quality.active().id, low);
    assert_eq!(stage.quality.active().source, TierSource::Ladder);
    assert_eq!(stage.character, character, "the character's mesh stayed on the renderer");
    stage.show.shown.fight.check().map_err(AppInitError::msg)?;
    // Exit test 1 (M4 H1, H2): the new tier's renderer was built off the frame, with all the old one had, and
    // no frame waited for it.
    let pipelines = stage.renderer.stats().pipelines;
    let on_frame = (pipelines.frame_thread_builds, pipelines.frame_waits);
    assert_eq!(on_frame, (0, 0), "a frame built or waited: {}", stage.renderer.first_use_report());
    assert_eq!(pipelines.first_use_builds, 0, "a frame built: {}", stage.renderer.first_use_report());
    println!("follows_the_ladder: {frames} frames from {high}; {low} from frame {at_frame}, tick {at_tick}");
    Ok(())
}

fn main() -> ExitCode {
    if !std::env::args().any(|argument| argument == "--ignored") {
        eprintln!("follows_the_ladder: needs a GPU and cooked content; run it with --ignored");
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
