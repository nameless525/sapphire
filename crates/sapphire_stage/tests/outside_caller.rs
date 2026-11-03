//! A caller outside the crate, with nothing but its public surface: make a stage, step it, draw frames into
//! a texture this test owns and read them back, as the crate's own docs say to.
//!
//! The platform's loop must own the process's main thread, so this test supplies its own `main`
//! (`harness = false`). It needs a GPU and cooked content, so it runs only when asked, as an `#[ignore]`d
//! test does: `cargo xtask content`, then `cargo test -p sapphire_stage --test outside_caller -- --ignored`.
//! CI's `App tests` job runs it on lavapipe.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, Platform, RunOptions, run_with};
use sapphire_render::UiPageState;
use sapphire_render::quality::QualitySettings;
use sapphire_rhi::wgpu;
use sapphire_stage::scene::Scene;
use sapphire_stage::{FrameLaps, Readback, SceneShow, Stage, StageDesc, headless_gpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

/// A show from outside: what a game's fight is to the stage, and how the stage's seam is held.
#[derive(Default)]
struct Counting {
    steps: u32,
    ends: u32,
    animations: u32,
    presentations: u32,
    interfaces: u32,
    pages: Option<UiPageState>,
}

impl SceneShow for Counting {
    fn step(&mut self, _: &mut Scene, _: &mut FrameLaps) {
        self.steps += 1;
    }

    fn end_steps(&mut self, _: &mut Scene, _: f32) {
        self.ends += 1;
    }

    fn animate(&mut self, _: &mut Scene, _: &sapphire_stage::Content, _: f32) {
        self.animations += 1;
    }

    fn begin_interface(&mut self, pages: UiPageState) {
        self.pages = Some(pages);
    }

    fn present(&mut self, _: &mut Scene, _: f32, _: Option<f32>) {
        self.presentations += 1;
    }

    fn draw_interface(&mut self, _: (u32, u32), _: &mut sapphire_render::UiDrawList) {
        self.interfaces += 1;
    }
}

/// Draws the stage's next frame into a new texture and reads it back.
fn frame<S: SceneShow>(stage: &mut Stage<S>) -> Result<Vec<u8>, AppInitError> {
    let target = stage.target(WIDTH, HEIGHT, wgpu::TextureUsages::COPY_SRC);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = Readback::new(&stage.gpu, WIDTH, HEIGHT);
    stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
    stage.check_gpu()?;
    readback.read(&stage.gpu).map_err(AppInitError::msg)
}

/// Does it all in `init`, and the first frame exits.
struct OutsideCaller;

impl App for OutsideCaller {
    const METADATA: AppMetadata = AppMetadata {
        name: "Stage Outside Caller Test",
        organization: "Sapphire",
        identifier: "dev.sapphire.stage_outside_caller_test",
        version: "0.0.0",
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let (instance, gpu) = headless_gpu()?;
        let desc = StageDesc::new(platform, &gpu, FORMAT, QualitySettings::default());
        let mut stage = Stage::new(platform, instance, gpu, desc)?;

        for _ in 0..60 {
            stage.step();
        }
        let first = frame(&mut stage)?;
        assert_eq!(first.len(), (WIDTH * HEIGHT * 4) as usize, "the whole target, without its row padding");
        let colors = first.as_chunks::<4>().0.iter().collect::<BTreeSet<_>>().len();
        assert!(colors > 100, "a drawn scene has more than {colors} colors");

        for _ in 0..60 {
            stage.step();
        }
        assert_ne!(first, frame(&mut stage)?, "stepping moves the picture");

        // The same again with a show on it: it's called at each of a frame's points, and draws the same.
        let (instance, gpu) = headless_gpu()?;
        let desc = StageDesc::new(platform, &gpu, FORMAT, QualitySettings::default());
        let mut shown = Stage::new(platform, instance, gpu, desc)?.with_show(Counting::default());
        for _ in 0..60 {
            shown.step();
        }
        let drawn = frame(&mut shown)?;
        assert_eq!(drawn.len(), first.len());
        let pages = shown.renderer.ui_page_state();
        let Counting { steps, ends, animations, presentations, interfaces, pages: told } = shown.show;
        assert_eq!((steps, ends), (60, 60));
        assert_eq!((animations, presentations, interfaces), (1, 1, 1));
        assert_eq!(told, Some(pages), "a show hears what the renderer's glyph pages hold");
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
        eprintln!("outside_caller: needs a GPU and cooked content; run it with --ignored");
        return ExitCode::SUCCESS;
    }
    if std::env::var_os("SAPPHIRE_CONTENT_DIR").is_none() {
        let cook = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/content/desktop");
        // SAFETY: nothing else runs yet: the platform's loop and its threads start below.
        unsafe { std::env::set_var("SAPPHIRE_CONTENT_DIR", cook) };
    }
    run_with::<OutsideCaller>(RunOptions { background: true, headless: true })
}
