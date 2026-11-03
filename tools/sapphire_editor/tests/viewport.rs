//! Why a viewport pane has two views of one texture, on a real GPU and with no window: what the engine renders
//! through one reaches egui's image with its code values through the other, and an sRGB view comes out too dark.
//! They need a GPU: `cargo test -p sapphire_editor --test viewport -- --ignored --test-threads=1`.
//!
//! A small draw stands in for the renderer, which E3 brings: a ramp covers the values a frame can hold, and it
//! goes through the same pass, formats and views. [`Painter`] does what the editor's window does, against a
//! texture. `egui_kittest`'s own `wgpu` feature would do it too, but it turns on wgpu's `gles` across the
//! workspace's test builds and adds eframe, winit and some 70 crates to the lockfile. Only `handle_delta` is
//! needed from it.

#![allow(clippy::print_stdout, reason = "with --nocapture, its numbers are what it is run for")]

use std::sync::Arc;

use egui::load::SizedTexture;
use egui::mutex::RwLock;
use egui_kittest::{Harness, TestRenderer};
use egui_wgpu::RendererOptions;
use sapphire_editor::shell::{Pane, PaneUi, Shell};
use sapphire_editor::viewport::{VIEWPORT_FORMAT, ViewportTarget, on_pixels};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const RAMP: &str = "
struct Out { @builtin(position) position: vec4f, @location(0) uv: vec2f }

@vertex fn vs(@builtin(vertex_index) index: u32) -> Out {
    let uv = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    return Out(vec4f(uv * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0), 0.0, 1.0), uv);
}

// Linear values, which an sRGB target encodes: every code from dark to light across, a second ramp down.
@fragment fn fs(in: Out) -> @location(0) vec4f {
    return vec4f(in.uv.x, in.uv.y * in.uv.y, 1.0 - in.uv.x, 1.0);
}
";

/// Draws the ramp into a viewport.
struct Engine {
    gpu: GpuContext,
    pipeline: wgpu::RenderPipeline,
}

impl Engine {
    fn new() -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let shader = gpu.device().create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ramp"),
            source: wgpu::ShaderSource::Wgsl(RAMP.into()),
        });
        let pipeline = gpu.device().create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ramp"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(VIEWPORT_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self { gpu, pipeline }
    }

    fn draw(&self, target: &ViewportTarget) {
        let mut encoder =
            self.gpu.device().create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ramp"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.render_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.draw(0..3, 0..1);
        }
        self.gpu.queue().submit([encoder.finish()]);
    }
}

/// egui's renderer on the engine's device: the same adapter, device and queue.
#[derive(Clone)]
struct Painter {
    renderer: Arc<RwLock<egui_wgpu::Renderer>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
}

impl Painter {
    fn new(gpu: &GpuContext, format: wgpu::TextureFormat) -> Self {
        let renderer = egui_wgpu::Renderer::new(gpu.device(), format, RendererOptions::PREDICTABLE);
        Self {
            renderer: Arc::new(RwLock::new(renderer)),
            device: gpu.device().clone(),
            queue: gpu.queue().clone(),
            format,
        }
    }

    fn register(&self, view: &wgpu::TextureView) -> egui::TextureId {
        self.renderer.write().register_native_texture(&self.device, view, wgpu::FilterMode::Nearest)
    }

    /// Points `id` at `view`. The ID stays, so what's already laid out shows the new texture.
    fn update(&self, id: egui::TextureId, view: &wgpu::TextureView) {
        self.renderer.write().update_egui_texture_from_wgpu_texture(
            &self.device,
            view,
            wgpu::FilterMode::Nearest,
            id,
        );
    }

    /// Paints the harness's last pass, and reads it back.
    fn paint(&self, gpu: &GpuContext, harness: &Harness<'_, State>) -> Image {
        let output = harness.output();
        let pixels_per_point = output.pixels_per_point;
        let primitives = harness.ctx.tessellate(output.shapes.clone(), pixels_per_point);
        let size = harness.ctx.content_rect().size() * pixels_per_point;
        let (width, height) = (size.x.round() as u32, size.y.round() as u32);
        let framebuffer = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shell"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = framebuffer.create_view(&wgpu::TextureViewDescriptor::default());
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [width, height], pixels_per_point };
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let mut renderer = self.renderer.write();
        renderer.update_buffers(&self.device, &self.queue, &mut encoder, &primitives, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("shell"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            renderer.render(&mut pass, &primitives, &screen);
        }
        drop(renderer);
        self.queue.submit([encoder.finish()]);
        read_texture(gpu, &framebuffer)
    }
}

/// What a harness asks of a renderer, with no `wgpu` feature: keeping egui's own textures up to date.
impl TestRenderer for Painter {
    fn handle_delta(&mut self, delta: &mut egui::TexturesDelta) {
        let mut renderer = self.renderer.write();
        for (id, deltas) in &delta.set {
            for image in deltas {
                renderer.update_texture(&self.device, &self.queue, *id, image);
            }
        }
        delta.set.clear();
        for id in delta.free.drain() {
            renderer.free_texture(&id);
        }
    }
}

/// A viewport in the shell's Preview pane, filling it as the real one will.
struct Panes {
    texture: egui::TextureId,
    rect: Option<egui::Rect>,
}

impl PaneUi for Panes {
    fn pane_ui(&mut self, ui: &mut egui::Ui, pane: Pane) {
        if pane == Pane::Preview {
            let (rect, _) = on_pixels(ui.max_rect(), ui.pixels_per_point());
            self.rect = Some(rect);
            ui.put(rect, egui::Image::new(SizedTexture::new(self.texture, rect.size())));
        }
    }
}

struct State {
    shell: Shell,
    panes: Panes,
}

fn shell_harness(
    painter: &Painter,
    texture: egui::TextureId,
    (width, height): (f32, f32),
) -> Harness<'static, State> {
    Harness::builder().with_size(egui::vec2(width, height)).renderer(painter.clone()).build_ui_state(
        |ui, state: &mut State| state.shell.ui(ui, &mut state.panes),
        State { shell: Shell::new(), panes: Panes { texture, rect: None } },
    )
}

struct Image {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Image {
    fn at(&self, x: u32, y: u32) -> &[u8] {
        let at = ((y * self.width + x) * 4) as usize;
        &self.pixels[at..at + 4]
    }
}

fn read_texture(gpu: &GpuContext, texture: &wgpu::Texture) -> Image {
    let (width, height) = (texture.width(), texture.height());
    let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    gpu.queue().submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |result| result.expect("maps"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("polls");
    let mapped = buffer.slice(..).get_mapped_range().expect("mapped");
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for line in mapped.chunks(row as usize) {
        pixels.extend_from_slice(&line[..(width * 4) as usize]);
    }
    Image { width, height, pixels }
}

/// The largest difference of a channel, over a viewport's pixels against where they show in the shell, and how
/// many differ at all. Leaves out the rim, where a pane's edge falls between pixels.
fn differences(frame: &Image, shell: &Image, rect: egui::Rect, pixels_per_point: f32) -> (u8, usize) {
    let (left, top) =
        ((rect.min.x * pixels_per_point).round() as u32, (rect.min.y * pixels_per_point).round() as u32);
    let (mut most, mut count) = (0, 0);
    for y in 2..frame.height - 2 {
        for x in 2..frame.width - 2 {
            let (at, from) = (shell.at(x + left, y + top), frame.at(x, y));
            let difference = (0..3).map(|c| from[c].abs_diff(at[c])).max().unwrap_or(0);
            most = most.max(difference);
            count += usize::from(difference > 0);
        }
    }
    (most, count)
}

/// A frame through the sampling view reaches the shell's image with its code values, on an sRGB framebuffer and
/// a plain one.
#[test]
#[ignore = "needs a GPU"]
fn a_frame_shows_in_a_pane_with_its_code_values() {
    let engine = Engine::new();
    for (name, format) in [
        ("sRGB framebuffer", wgpu::TextureFormat::Rgba8UnormSrgb),
        ("plain framebuffer", wgpu::TextureFormat::Rgba8Unorm),
    ] {
        let painter = Painter::new(&engine.gpu, format);
        let probe = ViewportTarget::new(engine.gpu.device(), (64, 64));
        let id = painter.register(&probe.sampled_view);
        let mut harness = shell_harness(&painter, id, (1200.0, 700.0));
        harness.run_steps(2);
        let rect = harness.state().panes.rect.expect("drawn");
        let target = ViewportTarget::new(engine.gpu.device(), on_pixels(rect, 1.0).1);
        painter.update(id, &target.sampled_view);
        engine.draw(&target);
        let frame = read_texture(&engine.gpu, &target.texture);
        let image = painter.paint(&engine.gpu, &harness);
        let (most, count) = differences(&frame, &image, rect, 1.0);
        println!("{name}: pane {rect:?}: {count} pixels differ, by at most {most}");
        // A plain framebuffer takes egui's values as they are, and an sRGB one encodes them after egui decodes
        // them first, to within one code.
        assert!(most <= 1, "{name}: a viewport pixel moved by {most}, {count} in all");
        assert_eq!(
            frame.at(frame.width / 2, frame.height / 2)[3],
            255,
            "opaque, so egui's blending lets nothing through"
        );
    }
}

/// An sRGB view is what egui's docs warn off: it decodes on sampling, egui takes the result as encoded, and the
/// frame comes out dark. The test holds that to the reason for the second view.
#[test]
#[ignore = "needs a GPU"]
fn an_srgb_view_comes_out_too_dark() {
    let engine = Engine::new();
    let painter = Painter::new(&engine.gpu, wgpu::TextureFormat::Rgba8UnormSrgb);
    let probe = ViewportTarget::new(engine.gpu.device(), (64, 64));
    let id = painter.register(&probe.render_view);
    let mut harness = shell_harness(&painter, id, (1200.0, 700.0));
    harness.run_steps(2);
    let rect = harness.state().panes.rect.expect("drawn");
    let target = ViewportTarget::new(engine.gpu.device(), on_pixels(rect, 1.0).1);
    painter.update(id, &target.render_view);
    engine.draw(&target);
    let frame = read_texture(&engine.gpu, &target.texture);
    let image = painter.paint(&engine.gpu, &harness);
    let (most, _) = differences(&frame, &image, rect, 1.0);
    println!("through an sRGB view a pixel moves by {most}");
    assert!(most > 40, "{most}");
}

/// The order an app runs a frame in: egui's pass says where the pane is, the engine draws at that size, and egui
/// paints. So a resized pane shows in that frame, at any scale: a target's pixels are a pane's pixels. The last
/// size puts the pane's edges between pixels, which is why [`on_pixels`] is there: a frame shown where the pane
/// fell, off the grid, moved 58,779 of them by up to 5.
#[test]
#[ignore = "needs a GPU"]
fn a_resize_shows_with_no_latency() {
    let engine = Engine::new();
    for pixels_per_point in [1.0, 2.0, 1.5] {
        let painter = Painter::new(&engine.gpu, wgpu::TextureFormat::Rgba8UnormSrgb);
        let mut target = ViewportTarget::new(engine.gpu.device(), (64, 64));
        let id = painter.register(&target.sampled_view);
        let mut harness = shell_harness(&painter, id, (1200.0, 700.0));
        harness.set_pixels_per_point(pixels_per_point);
        harness.run_steps(2);
        for size in [(1200.0, 700.0), (1400.0, 800.0), (1000.0, 600.0), (1001.0, 601.0)] {
            harness.set_size(egui::vec2(size.0, size.1));
            // One pass says where the pane is, as a window's would.
            harness.step();
            let rect = harness.state().panes.rect.expect("drawn");
            let (_, wanted) = on_pixels(rect, pixels_per_point);
            if target.size() != wanted {
                target = ViewportTarget::new(engine.gpu.device(), wanted);
                painter.update(id, &target.sampled_view);
            }
            engine.draw(&target);
            let frame = read_texture(&engine.gpu, &target.texture);
            // And paints what that pass laid out.
            let image = painter.paint(&engine.gpu, &harness);
            let (most, count) = differences(&frame, &image, rect, pixels_per_point);
            println!(
                "scale {pixels_per_point}, pane {rect:?}, target {wanted:?}: {count} differ, by at most {most}"
            );
            assert!(most <= 1, "scale {pixels_per_point}, window {size:?}: {most}, {count} pixels");
        }
    }
}
