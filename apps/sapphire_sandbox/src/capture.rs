//! `--capture <png>`: renders the sandbox scene after a fixed number of simulation ticks into
//! an offscreen target and writes it as a PNG (see [`crate::offscreen`]).
//!
//! With `--capture-frames` it draws several frames and writes the last, as temporal AA needs: a
//! still scene redrawn while the history converges (`--capture-step 0`), or the scene playing at
//! 60 fps up to the capture's tick (`--capture-step 1`). The jitter follows the renderer's frame
//! count, so every run draws the same frames.
//!
//! Next to the PNG it writes `<name>.adapter.txt` (`backend=`, `adapter=`, `device_type=`,
//! `driver=` lines) naming the GPU that rendered it, so golden-image tests
//! (`cargo xtask golden`) compare against the right backend's reference.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::mpsc;

use sapphire_platform::AppInitError;
use sapphire_render::graph::{FrameGraph, TextureId};
use sapphire_rhi::{GpuContext, wgpu};

use crate::LOG_TARGET;
use crate::offscreen::Stage;
use crate::options::CaptureFrames;

/// Frames drawn before a capture with the debug overlay shown: egui sizes its windows over its
/// first frames, and the frame graph's report and GPU timings come back a frame or two late.
const OVERLAY_WARMUP_FRAMES: u32 = 8;

pub(crate) fn run(
    stage: &mut Stage,
    path: &Path,
    ticks: u32,
    frames: CaptureFrames,
    width: u32,
    height: u32,
) -> Result<(), AppInitError> {
    let first =
        frames.first_tick(ticks).ok_or_else(|| AppInitError::msg("capture frames start before tick 0"))?;
    for _ in 0..first {
        stage.step();
    }
    // Frames before the captured one, `step` ticks apart, ending at `ticks`; with the overlay
    // shown, at least its warm-up, redrawing the same scene state at the end.
    let earlier = (frames.count - 1).max(if stage.overlay_visible() { OVERLAY_WARMUP_FRAMES } else { 0 });
    if earlier > 0 {
        let target = stage.target(width, height, wgpu::TextureUsages::empty());
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        for frame in 0..earlier {
            stage.render(&view, 1.0, |_, _| {})?;
            stage.scene.end_frame();
            stage.gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
            // The frames still to come, the captured one included, each this many ticks later.
            if earlier - frame < frames.count {
                for _ in 0..frames.step {
                    stage.step();
                }
            }
        }
    }
    let pixels = render(stage, width, height)?;
    write_png(path, width, height, &pixels)?;
    write_adapter_info(path, &stage.gpu.caps().adapter)?;
    tracing::info!(
        target: LOG_TARGET,
        path = %path.display(),
        width,
        height,
        ticks,
        frames = frames.count,
        step = frames.step,
        "captured frame"
    );
    Ok(())
}

/// Renders one frame of the scene and reads it back as tightly packed RGBA8 rows.
fn render(stage: &mut Stage, width: u32, height: u32) -> Result<Vec<u8>, AppInitError> {
    let target = stage.target(width, height, wgpu::TextureUsages::COPY_SRC);
    let readback = Readback::new(&stage.gpu, width, height);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
    readback.read(&stage.gpu).map_err(AppInitError::msg)
}

/// A frame read back from the GPU: a buffer the frame's target is copied into, after the
/// frame's passes, then mapped and stripped of its row padding.
pub(crate) struct Readback {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_row_bytes: u32,
}

impl Readback {
    /// For a `width` by `height` target of 4 bytes a pixel.
    pub fn new(gpu: &GpuContext, width: u32, height: u32) -> Self {
        let padded_row_bytes = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: gpu.label("capture readback"),
            size: u64::from(padded_row_bytes) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer, width, height, padded_row_bytes }
    }

    /// Adds the pass that copies `target` here, after every pass before it that writes it.
    /// Passes borrow what they use until the graph has run.
    pub fn record<'f>(&'f self, graph: &mut FrameGraph<'f>, target: TextureId) {
        let (buffer, padded_row_bytes, height) = (&self.buffer, self.padded_row_bytes, self.height);
        graph.add_pass("readback").read(target).side_effect().record(move |ctx| {
            let texture = ctx.texture(target);
            ctx.encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_row_bytes),
                        rows_per_image: Some(height),
                    },
                },
                texture.size(),
            );
        });
    }

    /// Waits for the GPU to finish the frame and returns its pixels, as tightly packed rows of
    /// the target's format.
    pub fn read(&self, gpu: &GpuContext) -> Result<Vec<u8>, String> {
        let (sender, receiver) = mpsc::channel();
        self.buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            // The receiver outlives the poll below, so the send can't fail.
            let _ = sender.send(result);
        });
        gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(|error| error.to_string())?;
        receiver.recv().map_err(|error| error.to_string())?.map_err(|error| error.to_string())?;
        let mapped = self.buffer.get_mapped_range(..).map_err(|error| error.to_string())?;
        let row_bytes = self.width as usize * 4;
        Ok(mapped
            .chunks_exact(self.padded_row_bytes as usize)
            .flat_map(|row| &row[..row_bytes])
            .copied()
            .collect())
    }
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

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), AppInitError> {
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
