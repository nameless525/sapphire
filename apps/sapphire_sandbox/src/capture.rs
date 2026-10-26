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
    let row_bytes = width * 4;
    let padded_row_bytes = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let gpu = &stage.gpu;
    let readback = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: gpu.label("capture readback"),
        size: u64::from(padded_row_bytes) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    // Passes borrow what they use until the graph has run.
    let readback = &readback;
    stage.render(&view, 1.0, |graph, target| {
        graph.add_pass("readback").read(target).side_effect().record(move |ctx| {
            let texture = ctx.texture(target);
            ctx.encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_row_bytes),
                        rows_per_image: Some(height),
                    },
                },
                texture.size(),
            );
        });
    })?;
    read_back(&stage.gpu, readback, padded_row_bytes as usize, row_bytes as usize)
}

/// Maps `readback` after the GPU has written it and strips the row padding.
fn read_back(
    gpu: &GpuContext,
    readback: &wgpu::Buffer,
    padded_row_bytes: usize,
    row_bytes: usize,
) -> Result<Vec<u8>, AppInitError> {
    let (sender, receiver) = mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        // The receiver outlives the poll below, so the send can't fail.
        let _ = sender.send(result);
    });
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
    receiver.recv().map_err(AppInitError::msg)?.map_err(AppInitError::msg)?;
    let mapped = readback.get_mapped_range(..).map_err(AppInitError::msg)?;
    Ok(mapped.chunks_exact(padded_row_bytes).flat_map(|row| &row[..row_bytes]).copied().collect())
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
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut writer = encoder.write_header().map_err(AppInitError::msg)?;
    writer.write_image_data(rgba).map_err(AppInitError::msg)?;
    writer.finish().map_err(AppInitError::msg)?;
    Ok(())
}
