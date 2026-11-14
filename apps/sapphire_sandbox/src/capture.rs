//! `--capture <png>`: renders the sandbox scene after a fixed number of simulation ticks into
//! an offscreen target and writes it as a PNG (see [`crate::offscreen`]).
//!
//! With `--capture-frames` it draws several frames and writes the last, as temporal AA needs: a
//! still scene redrawn while the history converges (`--capture-step 0`), or the scene playing at
//! 60 fps up to the capture's tick (`--capture-step 1`). The jitter follows the renderer's frame
//! count, so every run draws the same frames. With `--draw-every <n>` only one of those earlier frames in `n` runs
//! its graph, the first among them; the rest are prepared and recorded and lost, and the captured frame always
//! draws. That's for a fight beside the renderer on a software GPU: its picture's history isn't the full run's.
//!
//! With `SAPPHIRE_CAPTURE_SEQUENCE=<dir>` set, every one of those frames is read back and written to `<dir>`
//! as `frame-NNNNN.png`, the captured one last: a sequence for `sapphire_testkit flashes` to judge (M4 A3).
//! `--capture-step 1` makes it the fight at 60 fps; frames `--draw-every` loses aren't written.
//!
//! Next to the PNG it writes `<name>.adapter.txt` (`backend=`, `adapter=`, `device_type=`,
//! `driver=` lines) naming the GPU that rendered it, so golden-image tests
//! (`cargo xtask golden`) compare against the right backend's reference.

use std::fs::File;
use std::io::BufWriter;
use std::num::NonZeroU32;
use std::path::Path;

use game_flow::capture::GameCapture;
use game_flow::game::GameSetup;
use game_flow::progress::{Progress, progress_store};
use game_flow::show::GameShow;
use game_ui::hud::OffscreenDisplay;
use game_ui::menus::PlayerSettings;
use sapphire_platform::{AppInitError, Platform};
use sapphire_render::quality::QualitySettings;
use sapphire_rhi::{GpuContext, wgpu};
use sapphire_save::ProgressStore;

use crate::LOG_TARGET;
use crate::offscreen::Stage;
use crate::options::GameStart;
use game_combat::combat::CaptureFrames;
use sapphire_stage::Readback;
use sapphire_stage::gpu_failure;

/// Frames drawn before a capture with the debug overlay shown: egui sizes its windows over its
/// first frames, and the frame graph's report and GPU timings come back a frame or two late.
const OVERLAY_WARMUP_FRAMES: u32 = 8;

/// A same-executable reference path for validating capture warmup dead-pass elimination.
const HISTORY_ONLY_ENV: &str = "SAPPHIRE_CAPTURE_HISTORY_ONLY";

/// The directory a capture writes each of its frames to, for a flash analysis (M4 A3).
const SEQUENCE_ENV: &str = "SAPPHIRE_CAPTURE_SEQUENCE";

/// Where frame `index` of a sequence goes in `dir`: names that sort in the frames' order.
fn sequence_frame(dir: &Path, index: u32) -> std::path::PathBuf {
    dir.join(format!("frame-{index:05}.png"))
}

fn history_only_requested(value: Option<&std::ffi::OsStr>) -> Result<bool, AppInitError> {
    match value.and_then(std::ffi::OsStr::to_str) {
        None if value.is_none() => Ok(true),
        Some("on") => Ok(true),
        Some("off") => Ok(false),
        _ => Err(AppInitError::msg(format!("{HISTORY_ONLY_ENV} must be on or off"))),
    }
}

/// A tick of a capture: a stage's step, a frame's worth. A frame a cutscene holds the fight for is a frame,
/// so `--capture-ticks` counts it; `through`, for a capture of a replay's end or `--capture-through-trips`
/// (M5 M6), steps on through it, since the end is the replay's last tick (M3 Q3). A zone trip holds the fight
/// until its zone has loaded, its first cells are in and its wipe has covered and cleared, and only a drawn
/// frame moves those on (M3 W4): so a step a trip holds draws one into `view`, its output discarded where it
/// may be. Without it, a capture of `zone-trip`'s end never ended (M3 Z2).
fn step(
    stage: &mut Stage,
    through: bool,
    view: &wgpu::TextureView,
    history_only: bool,
) -> Result<(), AppInitError> {
    loop {
        let held = stage.show.fight.held_steps();
        stage.step();
        if !through || stage.show.fight.held_steps() == held {
            return Ok(());
        }
        if stage.show.fight.show.trip_phase().is_some() {
            stage.render_capture_warmup(view, history_only)?;
            stage.scene.end_frame();
            stage.gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
        }
    }
}

/// `--game`: the game's own show on the content's flow, from `start`, over `progress`'s save if there is one,
/// drawn `ticks` frames with no window on the player's `quality` settings (`--tier` and the rest of the
/// command line's) and written as any capture is (M3 G1). In `language` if one's given, and walking forward
/// into a cutscene if `forward` (M3 Q4), drawing from frame `draw_from` (`GameCapture::draw_from`), its menus
/// and HUD as `display` shows them (M4 S5), and the title's version labelled `version` (0.1.0). It keeps no run's file, and the save only in a scratch directory it
/// takes away after.
pub(crate) fn game(
    platform: &Platform,
    path: &Path,
    (start, progress, forward, language, draw_from, version): (
        GameStart,
        Option<&Path>,
        bool,
        Option<&str>,
        u32,
        String,
    ),
    (ticks, size, display): (u32, (u32, u32), OffscreenDisplay),
    quality: QualitySettings,
) -> Result<(), AppInitError> {
    let scratch = progress.map(Scratch::holding).transpose()?;
    let setup =
        GameSetup { store: scratch.as_ref().map(|scratch| scratch.store.clone()), ..GameSetup::default() };
    let mut settings = PlayerSettings::default();
    if let Some(language) = language {
        language.clone_into(&mut settings.language);
    }
    let (mut show, first) = GameShow::from_content(platform.content(), settings, setup)?;
    show.show_version(version);
    let capture = GameCapture {
        choose: start.command(),
        size,
        ticks,
        at_tick: None,
        quality,
        forward,
        draw_from,
        boot: false,
        display,
    };
    let captured = game_flow::capture::capture(platform.host(), show, &first, &capture, None)?;
    write_png(path, size.0, size.1, &captured.pixels)?;
    write_adapter_info(path, &captured.adapter)?;
    tracing::info!(target: LOG_TARGET, path = %path.display(), ?start, "the game captured");
    Ok(())
}

/// A save of `--progress`'s, in a directory of its own for as long as this lives.
struct Scratch {
    dir: std::path::PathBuf,
    store: ProgressStore<Progress>,
}

impl Scratch {
    /// The save `path` holds, a `Progress` document, written to a new scratch directory's store.
    fn holding(path: &Path) -> Result<Self, AppInitError> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| AppInitError::msg(format!("--progress {}: {error}", path.display())))?;
        let progress: Progress = sapphire_serialize::from_ron(&text)
            .map_err(|error| AppInitError::msg(format!("--progress {}: {error}", path.display())))?;
        let dir = std::env::temp_dir().join(format!("sapphire-sandbox-progress-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let store = progress_store(&dir, |path, bytes| std::fs::write(path, bytes));
        let snapshot = store.snapshot(&progress).map_err(AppInitError::msg)?;
        store.write(&snapshot).map_err(AppInitError::msg)?;
        Ok(Self { dir, store })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub(crate) fn run(
    stage: &mut Stage,
    path: &Path,
    (ticks, through): (u32, bool),
    (frames, draw_every): (CaptureFrames, Option<NonZeroU32>),
    width: u32,
    height: u32,
) -> Result<(), AppInitError> {
    let history_only = history_only_requested(std::env::var_os(HISTORY_ONLY_ENV).as_deref())?;
    let sequence = std::env::var_os(SEQUENCE_ENV).map(std::path::PathBuf::from);
    if let Some(dir) = &sequence {
        std::fs::create_dir_all(dir)?;
    }
    let mut history_only_frames = 0_u32;
    #[cfg(feature = "dev")]
    let mut cell_tally = crate::bench::CellTally::default();
    // Frames whose graph ran, the captured one among them.
    let mut drawn = 1_u32;
    let first =
        frames.first_tick(ticks).ok_or_else(|| AppInitError::msg("capture frames start before tick 0"))?;
    let target = stage.target(width, height, wgpu::TextureUsages::empty());
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    for _ in 0..first {
        step(stage, through, &view, history_only)?;
    }
    // Frames before the captured one, `step` ticks apart, ending at `ticks`; with the overlay
    // shown, at least its warm-up, redrawing the same scene state at the end.
    let earlier = (frames.count - 1).max(if stage.overlay_visible() { OVERLAY_WARMUP_FRAMES } else { 0 });
    if earlier > 0 {
        let draw_every = draw_every.map(NonZeroU32::get);
        for frame in 0..earlier {
            stage.lose_frames = draw_every.is_some_and(|every| frame % every != 0);
            drawn += u32::from(!stage.lose_frames);
            let drawn = match &sequence {
                Some(dir) if !stage.lose_frames => render(stage, width, height)
                    .and_then(|pixels| write_png(&sequence_frame(dir, frame), width, height, &pixels))
                    .map(|()| false),
                _ => stage.render_capture_warmup(&view, history_only),
            };
            #[cfg(feature = "dev")]
            let lost = stage.lose_frames;
            stage.lose_frames = false;
            history_only_frames += u32::from(drawn?);
            #[cfg(feature = "dev")]
            if !lost {
                cell_tally.frame(stage);
                if sequence.is_some() {
                    log_cell_sets(stage, frame);
                }
            }
            stage.scene.end_frame();
            stage.gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
            // The frames still to come, the captured one included, each this many ticks later.
            if earlier - frame < frames.count {
                for _ in 0..frames.step {
                    step(stage, through, &view, history_only)?;
                }
            }
        }
    }
    // Every step has run: a world that isn't the recorded one doesn't become a picture.
    stage.show.fight.check().map_err(AppInitError::msg)?;
    let pixels = render(stage, width, height)?;
    #[cfg(feature = "dev")]
    {
        cell_tally.frame(stage);
        log_cell_sets(stage, earlier);
        cell_tally.log();
    }
    write_capture(&stage.gpu, path, (width, height), &pixels)?;
    if let Some(dir) = &sequence {
        write_png(&sequence_frame(dir, earlier), width, height, &pixels)?;
    }
    stage.show.fight.finish().map_err(AppInitError::msg)?;
    // What the load built counts as prewarmed (`StageDesc::warm_load`): a first use here is what a frame built.
    let builds = stage.renderer.stats().pipelines;
    tracing::info!(
        target: LOG_TARGET,
        path = %path.display(),
        width,
        height,
        ticks,
        frames = frames.count,
        step = frames.step,
        drawn,
        history_only_frames,
        prewarmed = builds.prewarmed,
        first_use = builds.first_use_builds,
        "captured frame"
    );
    Ok(())
}

/// What visible sets by cell hid from frame `frame`'s main view, with `--cell-visibility` (M5 D0's prototype).
#[cfg(feature = "dev")]
fn log_cell_sets(stage: &Stage, frame: u32) {
    let Some(sets) = stage.scene.cell_sets() else { return };
    let stats = stage.renderer.stats().view_culling;
    tracing::info!(
        target: LOG_TARGET,
        frame,
        last = ?sets.last(),
        drawn = stats.drawn,
        hidden_draws = stats.hidden_draws,
        culled = stats.culled,
        "cell sets"
    );
}

/// Writes `pixels` and the adapter's note beside them, unless the GPU has reported an error: then there's
/// no picture, whatever the pixels look like, and a golden can't take one on trust.
fn write_capture(
    gpu: &GpuContext,
    path: &Path,
    (width, height): (u32, u32),
    pixels: &[u8],
) -> Result<(), AppInitError> {
    gpu_failure(gpu).map_err(AppInitError::msg)?;
    write_png(path, width, height, pixels)?;
    write_adapter_info(path, &gpu.caps().adapter)
}

/// Renders one frame of the scene and reads it back as tightly packed RGBA8 rows.
fn render(stage: &mut Stage, width: u32, height: u32) -> Result<Vec<u8>, AppInitError> {
    let target = stage.target(width, height, wgpu::TextureUsages::COPY_SRC);
    let readback = Readback::new(&stage.gpu, width, height);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
    readback.read(&stage.gpu).map_err(AppInitError::msg)
}

/// Whether a `width` by `height` frame can be read back, where a buffer holds at most `largest` bytes.
/// It has to be said before the frame is drawn: past `largest` wgpu only reports a validation error,
/// which says less about why.
pub(crate) fn readback_fits(width: u32, height: u32, largest: u64) -> Result<(), String> {
    let bytes = u128::from(Readback::padded_row_bytes(width)) * u128::from(height);
    if bytes > u128::from(largest) {
        return Err(format!(
            "a {width}x{height} frame takes {bytes} bytes to read back: this device's largest buffer is {largest}"
        ));
    }
    Ok(())
}

/// `<capture>.adapter.txt`: which GPU rendered the capture.
fn write_adapter_info(path: &Path, adapter: &wgpu::AdapterInfo) -> Result<(), AppInitError> {
    let info_path = path.with_extension("adapter.txt");
    let info = format!(
        "backend={}\nadapter={}\ndevice_type={:?}\ndriver={} {}\n",
        adapter.backend.to_str(),
        adapter.name,
        adapter.device_type,
        adapter.driver,
        adapter.driver_info,
    );
    std::fs::write(&info_path, info)
        .map_err(|error| AppInitError::msg(format!("{}: {error}", info_path.display())))
}

pub(crate) fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), AppInitError> {
    let file =
        File::create(path).map_err(|error| AppInitError::msg(format!("{}: {error}", path.display())))?;
    encode_png(BufWriter::new(file), width, height, rgba).map_err(AppInitError::msg)
}

/// Writes `rgba`, tightly packed 8-bit sRGB RGBA rows, as a PNG.
pub(crate) fn encode_png(
    out: impl std::io::Write,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), png::EncodingError> {
    let mut encoder = png::Encoder::new(out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()
}

/// `rgba` (tightly packed 8-bit sRGB rows) as a PNG of at most `max_bytes`, halved as many times
/// as that takes, and how many: live link's captures (`link.rs`), which must fit a frame.
#[cfg(any(test, feature = "dev"))]
pub(crate) fn compress(
    mut width: u32,
    mut height: u32,
    mut rgba: Vec<u8>,
    max_bytes: usize,
) -> Result<(u32, Vec<u8>), String> {
    let mut halved = 0;
    loop {
        let mut png = Vec::new();
        encode_png(&mut png, width, height, &rgba).map_err(|error| error.to_string())?;
        if png.len() <= max_bytes {
            return Ok((halved, png));
        }
        if width < 2 || height < 2 {
            return Err(format!("a {width}x{height} PNG takes {} bytes, more than {max_bytes}", png.len()));
        }
        (width, height, rgba) = halve(width, height, &rgba);
        halved += 1;
    }
}

/// `rgba` at half its size, each pixel the mean of the four it covers (of their encoded values:
/// an image to look at, not to measure); an odd last row or column is dropped.
#[cfg(any(test, feature = "dev"))]
fn halve(width: u32, height: u32, rgba: &[u8]) -> (u32, u32, Vec<u8>) {
    let (half_width, half_height) = (width / 2, height / 2);
    let row = width as usize * 4;
    let mut half = Vec::with_capacity(half_width as usize * half_height as usize * 4);
    for y in 0..half_height as usize {
        for x in 0..half_width as usize {
            let at = 2 * y * row + 2 * x * 4;
            for channel in 0..4 {
                let sum: u32 = [at, at + 4, at + row, at + row + 4]
                    .into_iter()
                    .map(|index| u32::from(rgba[index + channel]))
                    .sum();
                half.push(((sum + 2) / 4) as u8);
            }
        }
    }
    (half_width, half_height, half)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device that holds its errors, as a stage's does.
    fn holding_gpu() -> GpuContext {
        use sapphire_rhi::{GpuContextDesc, GpuInstance, GpuInstanceDesc};
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let desc = GpuContextDesc { hold_errors: true, ..GpuContextDesc::default() };
        GpuContext::new(&instance, &desc, None).expect("device")
    }

    /// A frame whose pixels look fine isn't a picture once the GPU has reported an error, in a build of
    /// any kind. Needs a GPU. Run with `cargo test -p sapphire_sandbox --lib capture -- --ignored`.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_capture_past_a_gpu_error_fails_with_its_text_and_writes_nothing() {
        let gpu = holding_gpu();
        let dir = std::env::temp_dir().join(format!("sapphire_gpu_error_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let (clean, failed) = (dir.join("clean.png"), dir.join("failed.png"));
        let pixels = [255_u8; 16];

        write_capture(&gpu, &clean, (2, 2), &pixels).expect("no error yet");
        assert!(clean.exists() && clean.with_extension("adapter.txt").exists());

        let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("too small"),
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue().write_buffer(&buffer, 0, &[0; 8]);
        let error = write_capture(&gpu, &failed, (2, 2), &pixels).expect_err("past an error");
        let held = gpu.first_error().expect("held");
        assert!(error.to_string().contains(held), "{error}");
        assert!(!failed.exists() && !failed.with_extension("adapter.txt").exists(), "no picture");
        std::fs::remove_dir_all(&dir).expect("removed");
    }

    /// A lost device's frames did nothing, and the errors that follow from it aren't held. Needs a GPU.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_capture_past_a_lost_device_fails() {
        let gpu = holding_gpu();
        assert_eq!(gpu_failure(&gpu), Ok(()));
        gpu.device().destroy();
        let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
        assert_eq!(gpu_failure(&gpu), Err("the GPU device was lost".to_owned()));
    }

    #[test]
    fn a_frame_past_the_largest_buffer_is_refused_by_size() {
        const LARGEST: u64 = 256 << 20;
        assert_eq!(readback_fits(8192, 8192, LARGEST), Ok(()), "exactly the largest");
        let refused = readback_fits(8200, 8200, LARGEST).expect_err("past it");
        assert!(refused.contains("8200x8200") && refused.contains("268435456"), "{refused}");
        assert!(readback_fits(8193, 8187, LARGEST).is_err(), "its rows' padding counts");
        assert!(readback_fits(u32::MAX, u32::MAX, LARGEST).is_err(), "and no overflow on the way");
    }

    #[test]
    fn history_only_switch_is_explicit_and_rejects_mistakes() {
        use std::ffi::OsStr;
        assert!(history_only_requested(None).unwrap());
        assert!(history_only_requested(Some(OsStr::new("on"))).unwrap());
        assert!(!history_only_requested(Some(OsStr::new("off"))).unwrap());
        for value in ["", "ON", "false", "1"] {
            assert!(history_only_requested(Some(OsStr::new(value))).is_err());
        }
    }

    #[test]
    fn halving_averages_each_block_and_drops_an_odd_edge() {
        // 3x2: the third column is dropped.
        let rgba = [
            [0, 0, 0, 255],
            [4, 8, 12, 255],
            [99, 99, 99, 99],
            [8, 16, 24, 255],
            [12, 24, 36, 255],
            [99, 99, 99, 99],
        ]
        .concat();
        assert_eq!(halve(3, 2, &rgba), (1, 1, vec![6, 12, 18, 255]));
    }

    #[test]
    fn captures_too_large_are_halved_until_they_fit() {
        // Noise barely compresses: 512x512 of it takes more than its 1 MiB of pixels as a PNG.
        let mut state = 0x2545_f491_u32;
        let noise: Vec<u8> = (0..512 * 512 * 4)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let (halved, png) = compress(512, 512, noise, 1 << 19).expect("compressed");
        assert_eq!(halved, 1, "256x256 of noise fits half a megabyte");
        assert!(png.len() <= 1 << 19);
        let mut decoder = png::Decoder::new(std::io::Cursor::new(png)).read_info().expect("a PNG");
        let mut pixels = vec![0; decoder.output_buffer_size().expect("a size")];
        let info = decoder.next_frame(&mut pixels).expect("its pixels");
        assert_eq!((info.width, info.height), (256, 256));

        let (halved, _) = compress(4, 4, vec![128; 64], 1 << 19).expect("compressed");
        assert_eq!(halved, 0, "frames that fit are sent whole");
        assert!(compress(1, 1, vec![1; 4], 8).is_err(), "nothing smaller than a pixel");
    }
}
