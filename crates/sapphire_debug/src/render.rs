//! Drawing the overlay: egui's meshes through `egui-wgpu`, as a frame graph pass over the
//! finished frame.

use egui::epaint::{ClippedPrimitive, textures::TexturesDelta};
use egui_wgpu::{Renderer as EguiRenderer, RendererOptions, ScreenDescriptor};
use sapphire_render::graph::{FrameGraph, TextureId};
use sapphire_rhi::{GpuContext, wgpu};

/// The overlay's GPU half: egui's renderer and the frame it last tessellated.
pub(crate) struct OverlayRenderer {
    renderer: EguiRenderer,
    device: wgpu::Device,
    queue: wgpu::Queue,
    primitives: Vec<ClippedPrimitive>,
    /// Texture changes (the font atlas, mostly) for the next recorded frame.
    textures: TexturesDelta,
    /// Textures to free once the frame that last drew them is recorded.
    to_free: Vec<egui::TextureId>,
    pixels_per_point: f32,
    /// Vertices in the last tessellated frame (rule R9).
    vertices: usize,
}

impl Drop for OverlayRenderer {
    fn drop(&mut self) {
        // Texture changes not yet uploaded die with the renderer that would have uploaded them
        // (epaint asserts that none is dropped by accident).
        self.textures.clear();
    }
}

impl std::fmt::Debug for OverlayRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayRenderer").field("primitives", &self.primitives.len()).finish_non_exhaustive()
    }
}

impl OverlayRenderer {
    /// A renderer drawing into targets of `format`, the frame's output format. With an sRGB
    /// format egui's colors are linearized first, so the hardware's encode restores them and
    /// the overlay shows the colors egui was given.
    pub fn new(gpu: &GpuContext, format: wgpu::TextureFormat) -> Self {
        let options =
            RendererOptions { msaa_samples: 1, depth_stencil_format: None, ..RendererOptions::default() };
        Self {
            renderer: EguiRenderer::new(gpu.device(), format, options),
            device: gpu.device().clone(),
            queue: gpu.queue().clone(),
            primitives: Vec::new(),
            textures: TexturesDelta::default(),
            to_free: Vec::new(),
            pixels_per_point: 1.0,
            vertices: 0,
        }
    }

    /// Takes a finished egui frame: its texture changes and its tessellated shapes.
    pub fn prepare(&mut self, ctx: &egui::Context, output: egui::FullOutput) {
        profiling::scope!("overlay tessellate");
        self.textures.append(output.textures_delta);
        self.pixels_per_point = output.pixels_per_point;
        self.primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        self.vertices = self
            .primitives
            .iter()
            .map(|primitive| match &primitive.primitive {
                egui::epaint::Primitive::Mesh(mesh) => mesh.vertices.len(),
                egui::epaint::Primitive::Callback(_) => 0,
            })
            .sum();
    }

    /// Drops the last frame's shapes, so nothing is drawn until the next [`Self::prepare`].
    pub fn clear(&mut self) {
        self.primitives.clear();
        self.vertices = 0;
    }

    pub fn vertices(&self) -> usize {
        self.vertices
    }

    /// Adds the overlay pass to `graph`, drawing over `target` (after post, so over the final
    /// frame), and uploads the texture changes it needs.
    pub fn record<'f>(&'f mut self, graph: &mut FrameGraph<'f>, target: TextureId) {
        for id in self.to_free.drain(..) {
            self.renderer.free_texture(&id);
        }
        for (id, deltas) in &self.textures.set {
            for delta in deltas {
                self.renderer.update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        self.textures.set.clear();
        self.to_free.extend(self.textures.free.drain());
        if self.primitives.is_empty() {
            return;
        }
        let (width, height) = graph.size(target);
        let pixels_per_point = self.pixels_per_point;
        let Self { renderer, device, queue, primitives, .. } = self;
        let primitives = &primitives[..];
        // Drawing over part of the target keeps the rest, so the pass reads it too.
        graph.add_pass("debug overlay").read(target).write(target).record(move |ctx| {
            let screen = ScreenDescriptor { size_in_pixels: [width, height], pixels_per_point };
            // egui-wgpu returns command buffers only for paint callbacks, which the overlay
            // doesn't use.
            let callbacks = renderer.update_buffers(device, queue, ctx.encoder, primitives, &screen);
            debug_assert!(callbacks.is_empty());
            let mut pass = ctx
                .encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("debug overlay"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: ctx.view(target),
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    timestamp_writes: ctx.timestamp_writes(),
                    ..Default::default()
                })
                // egui-wgpu takes a `'static` pass; this one ends before the encoder is used again.
                .forget_lifetime();
            renderer.render(&mut pass, primitives, &screen);
        });
    }
}
