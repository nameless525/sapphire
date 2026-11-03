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
use sapphire_render::quality::QualitySettings;
use sapphire_rhi::wgpu;
use sapphire_stage::{Readback, Stage, StageDesc, headless_gpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

/// Draws the stage's next frame into a new texture and reads it back.
fn frame(stage: &mut Stage) -> Result<Vec<u8>, AppInitError> {
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
        let colors = first.chunks_exact(4).collect::<BTreeSet<_>>().len();
        assert!(colors > 100, "a drawn scene has more than {colors} colors");

        for _ in 0..60 {
            stage.step();
        }
        assert_ne!(first, frame(&mut stage)?, "stepping moves the picture");
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
