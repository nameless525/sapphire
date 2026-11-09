//! `--capture <png>`: the game with no window, for a run nobody's watching. Draws its ticks
//! (`game_flow::capture`), writes the last frame and exits. `main` runs it on desktop and iOS, and Android's
//! `SDL_main` when the launching intent asks (M3 Z2b), under an activity whose window nothing draws to.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use game_ui::menus::PlayerSettings;
use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, DeviceClass, Event, Platform};

use crate::LOG_TARGET;
use crate::app::GameApp;
use crate::options::Options;

/// Does it all in `init`, and the first frame exits.
pub struct OffscreenGame;

impl App for OffscreenGame {
    const METADATA: AppMetadata = GameApp::METADATA;

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let options = Options::parse(platform.args()).map_err(AppInitError::msg)?;
        let mut path = options.capture.clone().ok_or_else(|| AppInitError::msg("no --capture"))?;
        // A phone's app runs from nowhere it can write: a path with no directory of its own goes under its
        // cache, which `adb shell run-as` and `devicectl` reach (M3 Z2b).
        if path.is_relative() && platform.system_info().device_class != DeviceClass::Desktop {
            path = platform.cache_dir().join(path);
        }
        // A run nobody's playing shows the defaults, whatever whoever's machine this is has chosen.
        let (show, first) = options.show(platform, PlayerSettings::default())?;
        let capture = options.capture();
        let captured = game_flow::capture::capture(platform, show, &first, &capture)?;
        let (width, height) = capture.size;
        write_png(&path, width, height, &captured.pixels)?;
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
