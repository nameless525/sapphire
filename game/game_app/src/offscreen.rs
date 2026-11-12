//! `--capture <png>`: the game with no window, for a run nobody's watching. Draws its ticks
//! (`game_flow::capture`), writes the last frame and exits. `main` runs it on desktop and iOS, and Android's
//! `SDL_main` when the launching intent asks (M3 Z2b), under an activity whose window nothing draws to.
//!
//! With `--patch-dir` it patches as the window does (M4 P4): what's staged there installed first, then the
//! boot's update from the build's host (or `--update-host`'s, in a dev build), and the launch confirmed once
//! the frame's drawn. Exit test 2's lane (`cargo xtask patch rehearse`) runs it so.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use game_flow::boot::BootUpdate;
use game_flow::update::Patches;
use game_ui::menus::PlayerSettings;
use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, Platform};

use crate::LOG_TARGET;
use crate::app::GameApp;
use crate::options::Options;

/// Does it all in `init`, and the first frame exits.
pub struct OffscreenGame;

impl App for OffscreenGame {
    const METADATA: AppMetadata = GameApp::METADATA;

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let options = Options::parse(platform.args()).map_err(AppInitError::msg)?;
        let path = options.capture.clone().ok_or_else(|| AppInitError::msg("no --capture"))?;
        let path = crate::options::writable(path, platform);
        let mut patches = options.patch_setup(platform).map(|setup| Patches::open(platform.host(), setup));
        let host = patches.as_ref().map_or(platform.host(), Patches::host).clone();
        // A run nobody's playing shows the defaults, whatever whoever's machine this is has chosen.
        let (show, first) = options.show(platform, host.content(), PlayerSettings::default())?;
        let update = patches.as_ref().and_then(Patches::check).map(|update| BootUpdate {
            update,
            remake: options.make_show(platform, PlayerSettings::default()),
        });
        let capture = options.capture();
        #[cfg(not(feature = "playtest"))]
        let captured = game_flow::capture::capture(&host, show, &first, &capture, update)?;
        #[cfg(feature = "playtest")]
        let captured = crate::playtest::capture(
            platform,
            &host,
            (show, &first),
            &capture,
            update,
            options.save_dir.as_deref(),
        )?;
        if let Some(patches) = &mut patches {
            patches.reopen();
            patches.confirm_launch();
        }
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
