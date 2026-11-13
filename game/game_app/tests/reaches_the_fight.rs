//! A caller outside the crates, with nothing but their public surface: boot the game's show as its window does
//! (M4 H2), wait at the title, press its start, and see a fight go by.
//!
//! The platform's loop must own the process's main thread, so this test supplies its own `main`
//! (`harness = false`). It needs a GPU and cooked content, so it runs only when asked, as an `#[ignore]`d test
//! does: `cargo xtask content`, then `cargo test -p game_app --test reaches_the_fight -- --ignored`. CI's `App
//! tests` job runs it on lavapipe.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use game_app::{GameApp, GameShow};
use game_combat::combat::CombatOption;
use game_combat::documents::FightDocuments;
use game_flow::boot::Boot;
use game_ui::menus::{MenuDocument, PlayerSettings};
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, Event, KeyEvent, Keycode, Modifiers, Platform, RunOptions,
    Scancode, Timestamp, WindowId, run_with,
};
use sapphire_render::quality::QualitySettings;
use sapphire_rhi::wgpu;
use sapphire_stage::anim_graph::GraphDriver;
use sapphire_stage::{FrameLaps, Readback, Stage, headless_gpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;
/// A script that swings: long enough that someone's been hurt.
const STEPS: u32 = 600;
/// More title frames than the fades the boot leaves to warm there take to build, even on lavapipe.
const MOST_WARM_FRAMES: u32 = 100_000;

/// Draws the stage's next frame into a new texture and reads it back.
fn frame(stage: &mut Stage<GameShow>) -> Result<Vec<u8>, AppInitError> {
    let target = stage.target(WIDTH, HEIGHT, wgpu::TextureUsages::COPY_SRC);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = Readback::new(&stage.gpu, WIDTH, HEIGHT);
    stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
    stage.check_gpu()?;
    readback.read(&stage.gpu).map_err(AppInitError::msg)
}

/// How far the simulation has got.
fn tick(stage: &Stage<GameShow>) -> u64 {
    stage.show.shown.fight.combat.as_ref().expect("a fight stands").simulation().tick()
}

/// Does it all in `init`, and the first frame exits.
struct OutsideCaller;

impl App for OutsideCaller {
    const METADATA: AppMetadata = AppMetadata {
        name: "Game Outside Caller Test",
        organization: "Sapphire",
        identifier: "dev.sapphire.game_outside_caller_test",
        version: GameApp::METADATA.version,
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let gpu = Arc::new(headless_gpu()?);
        let documents = FightDocuments::read(platform.content()).map_err(AppInitError::msg)?;
        let screens = MenuDocument::read(platform.content()).map_err(AppInitError::msg)?;
        let show = GameShow::new(documents, screens, PlayerSettings::default())?;
        let option = CombatOption::Fight { script: Some("fight-back".to_owned()), party: false };
        let boot = Boot::start(platform.host(), gpu, FORMAT, QualitySettings::default(), show, &option)?;
        let (mut stage, report) = boot.run_offscreen((WIDTH, HEIGHT), false)?;
        assert!(report.frames > 1, "the boot screen drew while the game built: {report:?}");
        assert_eq!(report.screen_pipelines, 1, "the boot screen draws through the interface's one pipeline");
        // Exit test 1 (M4 H2): from the boot screen's first frame, nothing was built on a frame or waited for.
        let booted = report.pipelines;
        assert_eq!((booted.frame_thread_builds, booted.frame_waits), (0, 0), "{report:?}");
        assert!(booted.warmed_off_frame > 0, "the warm-up built what the game draws: {report:?}");
        let combat = stage.show.shown.fight.combat.as_ref().expect("the boot stood the fight");
        assert!(combat.simulation().overhead_presentation_pair().is_some());

        for _ in 0..30 {
            stage.step();
        }
        let guard_joints = |stage: &Stage<GameShow>| {
            let driver = stage
                .scene
                .world()
                .get::<GraphDriver>(stage.scene.character())
                .expect("the heroine has a graph");
            let pose = driver.player.pose();
            // Hair and clothing have their own secondary motion. Check the guard's body and sword.
            ["J_Bip_C_Hips", "J_Bip_L_Foot", "J_Bip_R_Foot", "J_Bip_R_Hand"].map(|name| {
                let joint = pose.joint_names().iter().position(|candidate| candidate == name).unwrap();
                pose.model()[joint]
            })
        };
        let held_pose = guard_joints(&stage);
        let title = frame(&mut stage)?;
        assert_eq!(tick(&stage), 0, "the title holds the fight");
        let guard = guard_joints(&stage);
        assert_ne!(guard, held_pose, "the title poses the heroine in her guard");

        // Play waits on the title for the fades the boot left to warm there (M4 C7), which no frame builds.
        assert!(stage.show.play_waits(), "the boot left the fades to the title");
        let mut frames = 0;
        while stage.show.play_waits() {
            assert!(frames < MOST_WARM_FRAMES, "the title's fades never built");
            frame(&mut stage)?;
            frames += 1;
        }
        let pipelines = stage.renderer.stats().pipelines;
        let on_frame = (pipelines.first_use_builds, pipelines.frame_thread_builds, pipelines.frame_waits);
        assert_eq!(on_frame, (0, 0, 0), "{}", stage.renderer.first_use_report());

        // What the title's first draw opened has had time to come in; its first row has the focus.
        let press = Event::Key(KeyEvent {
            timestamp: Timestamp::from_nanos(4_000_000_000),
            window: WindowId::from_raw(1),
            scancode: Scancode::RETURN,
            keycode: Keycode(0),
            modifiers: Modifiers::default(),
            down: true,
            repeat: false,
        });
        stage.show.update_menus(Timestamp::from_nanos(3_000_000_000));
        frame(&mut stage)?;
        stage.show.handle_event(&press);
        let left = stage.show.update_menus(Timestamp::from_nanos(5_000_000_000));
        assert!(!left && !stage.show.on_title(), "the title's new game opens the fight");
        let open = frame(&mut stage)?;
        assert_ne!(title, open, "the title goes");

        for _ in 0..STEPS {
            stage.step();
            stage.show.drive_view(Duration::from_millis(16));
        }
        assert_eq!(tick(&stage), u64::from(STEPS), "each step a tick");
        let (values, _) = stage.show.shown.fight.show.hud_values().expect("the fight says how it's going");
        let enemy = values.enemy.expect("an enemy stands there");
        assert!(values.player.hp < 1.0 || enemy.hp < 1.0, "someone's been hurt in {STEPS} ticks");
        assert_ne!(open, frame(&mut stage)?, "the fight moves the picture");
        assert_ne!(guard_joints(&stage), guard, "combat changes the guard pose");

        let fight = &mut stage.show.shown.fight;
        fight.combat.as_mut().expect("a fight stands").restart();
        assert!(fight.follow_step(&mut stage.scene, &mut FrameLaps::default()).started_over);
        frame(&mut stage)?;
        assert_eq!(tick(&stage), 0);
        assert_eq!(guard_joints(&stage), guard, "the first reset frame draws the guard");
        // Exit test 7: from the title through the fight's first seconds, no frame built what the load didn't.
        let first_uses = stage.renderer.stats().pipelines.first_use_builds;
        assert_eq!(first_uses, 0, "the load missed: {}", stage.renderer.first_use_report());
        // Exit test 1 (M4 H1, H2): after the boot screen's first frame, none built a pipeline itself or waited for
        // one; the warm-up built them.
        let pipelines = stage.renderer.stats().pipelines;
        let on_frame = (pipelines.frame_thread_builds, pipelines.frame_waits);
        assert_eq!(on_frame, (0, 0), "a frame built or waited: {}", stage.renderer.first_use_report());
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
        eprintln!("reaches_the_fight: needs a GPU and cooked content; run it with --ignored");
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
