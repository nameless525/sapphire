//! Offscreen capture: `--capture <png>`.
//!
//! Renders the sandbox scene after a fixed number of simulation ticks into an offscreen
//! target, writes it as a PNG and exits, without ever creating a window. Run it through
//! `sapphire_platform::run_with` with `RunOptions { background: true }` (as `main` does), so
//! the process never takes focus either. That makes the real app checkable on a desktop
//! someone is using, and it is the hook for golden-image tests later (spec §9).

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::mpsc;

use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, Platform};
use sapphire_render::{RenderSettings, Renderer};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use crate::LOG_TARGET;
use crate::app::SandboxApp;
use crate::options::{CaptureRequest, Options};
use crate::scene::{self, Scene};

/// Captures are sRGB like the swapchain, so they match what the window shows.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The capture "app": all work happens in `init`; the first frame exits.
pub struct CaptureApp;

impl App for CaptureApp {
    const METADATA: AppMetadata = SandboxApp::METADATA;

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let (options, unknown) = Options::parse(platform.args()).map_err(AppInitError::msg)?;
        if !unknown.is_empty() {
            tracing::warn!(target: LOG_TARGET, ?unknown, "ignoring unknown arguments");
        }
        let request = options.capture.ok_or_else(|| AppInitError::msg("CaptureApp needs --capture <png>"))?;

        let instance = GpuInstance::new(&GpuInstanceDesc::default())?;
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None)?;
        let mut renderer = Renderer::new(&gpu, FORMAT, RenderSettings { msaa: options.msaa })?;
        let mesh = scene::load_mesh(platform, scene::CHARACTER_MESH)?;
        let mut scene = Scene::new(renderer.upload_mesh(&mesh)?, mesh.bounds);
        for _ in 0..request.ticks {
            scene.step();
        }

        let pixels = render(&gpu, &mut renderer, &mut scene, &request)?;
        write_png(&request.path, request.width, request.height, &pixels)?;
        tracing::info!(
            target: LOG_TARGET,
            path = %request.path.display(),
            width = request.width,
            height = request.height,
            ticks = request.ticks,
            "captured frame"
        );
        Ok(Self)
    }

    fn event(&mut self, _platform: &mut Platform, _event: &Event) -> AppControl {
        AppControl::Continue
    }

    fn iterate(&mut self, _platform: &mut Platform) -> AppControl {
        AppControl::Exit
    }
}

/// Renders one frame of `scene` and reads it back as tightly packed RGBA8 rows.
fn render(
    gpu: &GpuContext,
    renderer: &mut Renderer,
    scene: &mut Scene,
    request: &CaptureRequest,
) -> Result<Vec<u8>, AppInitError> {
    let (width, height) = (request.width, request.height);
    let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: gpu.label("capture target"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let row_bytes = width * 4;
    let padded_row_bytes = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: gpu.label("capture readback"),
        size: u64::from(padded_row_bytes) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    renderer.prepare(gpu, &scene.frame_view(1.0, width as f32 / height as f32));
    let mut encoder =
        gpu.device().create_command_encoder(&wgpu::CommandEncoderDescriptor { label: gpu.label("capture") });
    renderer.record(&mut encoder, &target.create_view(&wgpu::TextureViewDescriptor::default()));
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row_bytes),
                rows_per_image: Some(height),
            },
        },
        target.size(),
    );
    gpu.queue().submit([encoder.finish()]);

    let (sender, receiver) = mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        // The receiver outlives the poll below, so the send can't fail.
        let _ = sender.send(result);
    });
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
    receiver.recv().map_err(AppInitError::msg)?.map_err(AppInitError::msg)?;
    let mapped = readback.get_mapped_range(..).map_err(AppInitError::msg)?;
    Ok(mapped
        .chunks_exact(padded_row_bytes as usize)
        .flat_map(|row| &row[..row_bytes as usize])
        .copied()
        .collect())
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
