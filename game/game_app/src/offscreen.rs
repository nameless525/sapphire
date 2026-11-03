//! `--capture <png>`: the game with no window, for a run nobody's watching. Draws its ticks, writes the last
//! frame and exits.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::time::Duration;

use game_combat::fight::fight_stage;
use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, Platform};
use sapphire_render::quality::QualitySettings;
use sapphire_rhi::wgpu;
use sapphire_stage::{Content, Readback, Stage, StageDesc, headless_gpu};
use sapphire_time::TICK_HZ;

use crate::LOG_TARGET;
use crate::app::GameApp;
use crate::options::Options;
use crate::show::GameShow;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Does it all in `init`, and the first frame exits.
pub struct OffscreenGame;

impl App for OffscreenGame {
    const METADATA: AppMetadata = GameApp::METADATA;

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let options = Options::parse(platform.args()).map_err(AppInitError::msg)?;
        let path = options.capture.clone().ok_or_else(|| AppInitError::msg("no --capture"))?;
        let (instance, gpu) = headless_gpu()?;
        let desc = fight_stage(StageDesc::new(platform.host(), &gpu, FORMAT, QualitySettings::default()));
        let mut content = Content::new(platform.host(), desc.character, desc.sky)?;
        let mut show = GameShow::new()?;
        let combat = show.shown.fight.prepare_stock_combat(&options.fight(), &mut content)?;
        let mut stage = Stage::new_with_content(instance, gpu, desc, content)?.with_show(show);
        let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
        show.stand(combat, scene, content, renderer, gpu, quality.active().id)?;
        if options.open_fight {
            show.open_fight();
        }

        let (width, height) = options.size;
        let target = stage.target(width, height, wgpu::TextureUsages::COPY_SRC);
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let frame = Duration::from_secs(1) / TICK_HZ;
        for _ in 1..options.ticks {
            stage.step();
            stage.show.drive_view(frame);
            stage.render(&view, 1.0, |_, _| {})?;
            stage.show.say_first_uses(&stage.renderer);
        }
        if options.ticks > 0 {
            stage.step();
            stage.show.drive_view(frame);
        }
        let readback = Readback::new(&stage.gpu, width, height);
        stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
        stage.check_gpu()?;
        stage.show.say_first_uses(&stage.renderer);
        let pixels = readback.read(&stage.gpu).map_err(AppInitError::msg)?;
        write_png(&path, width, height, &pixels)?;
        stage.show.shown.fight.finish().map_err(AppInitError::msg)?;
        tracing::info!(target: LOG_TARGET, path = %path.display(), "captured");
        Ok(Self)
    }

    fn event(&mut self, _: &mut Platform, _: &Event) -> AppControl {
        AppControl::Continue
    }

    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        AppControl::Exit
    }
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), AppInitError> {
    let file = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(())
}
