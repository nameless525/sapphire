//! The on-screen interface over the finished frame (spec §5.17, M2 I2): the packet's
//! [`UiDrawList`], as one instanced draw after post and any debug view,
//! under debug draw and the app's overlay. It knows quads and an atlas, never widgets (rule R4).
//!
//! # A frame with no UI costs nothing
//!
//! Nothing here exists until a frame's list has quads and an atlas: no pipeline, texture or buffer,
//! and no pass in the graph. The first such frame builds the one pipeline, on the calling thread,
//! as a new material variant's are, and uploads the atlas; those stay. A frame with an empty list
//! after that adds no pass either. `tests/gpu/ui.rs` holds both by counting pipelines.
//!
//! # sRGB
//!
//! The target encodes on write ([`Renderer::new`](crate::Renderer::new)), as post assumes, so the
//! hardware blends in linear light and encodes the result. **Decision: keep that blend and author
//! around it,** rather than blend in display space, which would need the target's non-sRGB view
//! (which offscreen targets and some surfaces don't declare) or a read of the frame. The shader
//! decodes each quad's display-encoded colors, so an opaque color lands on exactly its code, as the
//! debug overlay's linearized colors do. The cost is what egui's notes describe: translucent colors
//! read differently than in a display-space blend. A 50% black over white shows as code 188, not
//! 128, so inks that must hold over a bright sky are authored nearly opaque, and an edge's
//! antialiasing is true coverage, which reads light-on-dark a little bolder and dark-on-light a
//! little thinner. An outline around every digit takes care of the second.
//!
//! # Drawing
//!
//! One draw of `n` instances of a four-vertex strip, with the quads in a vertex buffer at slot 0, so
//! it needs no storage binding, base instance or anything the iOS Simulator lacks. The quads are
//! copied at [`Ui::prepare`] and staged in the frame's uploads pass, so a frame that never runs
//! leaves nothing behind: the next [`Ui::prepare`] writes its own. Past what a buffer holds, quads
//! don't draw and are counted ([`UiStats::quads_over_limit`]).
//!
//! `ui.wgsl` is a program of its own ([`ShaderFile::Ui`](crate::ShaderFile::Ui)): an edit rebuilds
//! the one pipeline, once built, on the layout it had (`shader_builds`).

use std::mem::{offset_of, size_of};
use std::num::NonZeroU64;
use std::sync::Arc;

use sapphire_look::{UiAtlas, UiDrawList, UiQuad};
use sapphire_rhi::wgpu;

use crate::buffer::{self, GrowableBuffer};
use crate::graph::{FrameGraph, TextureId};
use crate::shader_builds::Step;
use crate::shaders::{self, Program, ShaderSources};

/// The frame graph pass that draws the interface, for GPU timings.
pub const UI_PASS: &str = "ui";

/// Bytes of a quad, as the vertex stage reads it.
const QUAD_BYTES: u64 = size_of::<UiQuad>() as u64;

/// Quads the buffer holds before it first grows: more than a combat HUD draws.
const INITIAL_QUADS: u64 = 256;

/// The vertex layout of [`UiQuad`], attribute by attribute.
const QUAD_ATTRIBUTES: [wgpu::VertexAttribute; 9] = [
    attribute(0, offset_of!(UiQuad, origin), wgpu::VertexFormat::Float32x2),
    attribute(1, offset_of!(UiQuad, axis_x), wgpu::VertexFormat::Float32x2),
    attribute(2, offset_of!(UiQuad, axis_y), wgpu::VertexFormat::Float32x2),
    attribute(3, offset_of!(UiQuad, uv), wgpu::VertexFormat::Float32x4),
    attribute(4, offset_of!(UiQuad, color), wgpu::VertexFormat::Unorm8x4),
    attribute(5, offset_of!(UiQuad, outline_color), wgpu::VertexFormat::Unorm8x4),
    // `edge`, `softness`, `outline` and `sweep`, side by side.
    attribute(6, offset_of!(UiQuad, edge), wgpu::VertexFormat::Float32x4),
    attribute(7, offset_of!(UiQuad, clip), wgpu::VertexFormat::Float32x2),
    attribute(8, offset_of!(UiQuad, slice), wgpu::VertexFormat::Float32),
];

const fn attribute(shader_location: u32, offset: usize, format: wgpu::VertexFormat) -> wgpu::VertexAttribute {
    wgpu::VertexAttribute { format, offset: offset as u64, shader_location }
}

/// What the interface drew and built, for the stats overlay and Tracy (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiStats {
    /// Quads the last prepared frame draws.
    pub quads: u32,
    /// Quads the last prepared frame left out, past what a buffer holds.
    pub quads_over_limit: u32,
    /// Pipelines built: 0 until a frame draws quads, then 1.
    pub pipelines: u32,
    /// The atlas's size on the GPU, its one level.
    pub atlas_bytes: u64,
    /// Times an atlas was uploaded since the renderer was created.
    pub atlas_uploads: u32,
}

/// `ui.wgsl`'s uniforms.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct UiUniforms {
    points_to_clip: [f32; 2],
    padding: [f32; 2],
}

/// An uploaded atlas and the group that reads it.
struct AtlasOnGpu {
    source: Arc<UiAtlas>,
    bind_group: wgpu::BindGroup,
}

impl std::fmt::Debug for AtlasOnGpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AtlasOnGpu").field("source", &self.source).finish_non_exhaustive()
    }
}

/// What exists once a frame has drawn quads.
#[derive(Debug)]
struct Built {
    pipeline: wgpu::RenderPipeline,
    pipeline_layout: wgpu::PipelineLayout,
    group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    quads: GrowableBuffer,
    atlas: Option<AtlasOnGpu>,
}

/// The interface's GPU half. See the module docs.
#[derive(Debug)]
pub(crate) struct Ui {
    device: wgpu::Device,
    color_format: wgpu::TextureFormat,
    cache: Option<wgpu::PipelineCache>,
    labels: bool,
    built: Option<Box<Built>>,
    /// This frame's quads, for the uploads pass. Its capacity stays.
    quads: Vec<UiQuad>,
    uniforms: UiUniforms,
    stats: UiStats,
    /// The largest buffer quads go in; lowered by tests of a full one.
    max_bytes: u64,
}

/// What the uploads pass writes for the interface.
#[derive(Debug)]
pub(crate) struct UiUploads<'a> {
    quads: &'a [UiQuad],
    buffer: &'a wgpu::Buffer,
    uniform_buffer: &'a wgpu::Buffer,
    uniforms: UiUniforms,
}

impl UiUploads<'_> {
    pub(crate) fn write(&self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        buffer::stage(belt, encoder, self.buffer, 0, bytemuck::cast_slice(self.quads));
        buffer::stage(belt, encoder, self.uniform_buffer, 0, bytemuck::bytes_of(&self.uniforms));
    }
}

impl Ui {
    pub(crate) fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        cache: Option<wgpu::PipelineCache>,
        labels: bool,
    ) -> Self {
        Self {
            device: device.clone(),
            color_format,
            cache,
            labels,
            built: None,
            quads: Vec::new(),
            uniforms: UiUniforms::default(),
            stats: UiStats::default(),
            max_bytes: device.limits().max_buffer_size,
        }
    }

    /// Whether `list` draws anything.
    fn draws(list: &UiDrawList) -> bool {
        !list.quads.is_empty() && list.atlas.is_some() && list.viewport[0] > 0.0 && list.viewport[1] > 0.0
    }

    /// Builds the pipeline and what it reads, if `list` is the first that draws. The caller holds
    /// the error scopes for edited sources (`Renderer::checked_build`).
    pub(crate) fn prepare_pipeline(&mut self, list: &UiDrawList, sources: &ShaderSources) {
        if self.built.is_some() || !Self::draws(list) {
            return;
        }
        profiling::scope!("Ui::prepare_pipeline");
        let device = &self.device;
        let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: self.labels.then_some("ui"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(size_of::<UiUniforms>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: self.labels.then_some("ui"),
            bind_group_layouts: &[Some(&group_layout)],
            immediate_size: 0,
        });
        let pipeline =
            build_pipeline(device, &pipeline_layout, self.color_format, self.cache.as_ref(), sources);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: self.labels.then_some("ui"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: self.labels.then_some("ui uniforms"),
            size: size_of::<UiUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let quads = GrowableBuffer::new(
            device,
            "ui quads",
            wgpu::BufferUsages::VERTEX,
            INITIAL_QUADS * QUAD_BYTES,
            self.max_bytes,
        );
        self.built = Some(Box::new(Built {
            pipeline,
            pipeline_layout,
            group_layout,
            sampler,
            uniforms,
            quads,
            atlas: None,
        }));
        self.stats.pipelines = 1;
    }

    /// Takes this frame's list: uploads its atlas if it's new, and copies its quads for the uploads
    /// pass. Allocates nothing once its vector has grown.
    pub(crate) fn prepare(&mut self, queue: &wgpu::Queue, list: &UiDrawList) {
        self.quads.clear();
        self.stats.quads = 0;
        self.stats.quads_over_limit = 0;
        if !Self::draws(list) {
            return;
        }
        let (Some(built), Some(atlas)) = (self.built.as_deref_mut(), list.atlas.as_ref()) else { return };
        if !built.atlas.as_ref().is_some_and(|on_gpu| Arc::ptr_eq(&on_gpu.source, atlas)) {
            built.atlas = upload_atlas(&self.device, queue, built, atlas, self.labels);
            if built.atlas.is_some() {
                self.stats.atlas_uploads += 1;
                self.stats.atlas_bytes = atlas.texels.len() as u64;
            }
        }
        if built.atlas.is_none() {
            return;
        }
        let limit = built.quads.max_items(QUAD_BYTES).min(list.quads.len() as u64) as usize;
        built.quads.reserve(&self.device, queue, limit as u64 * QUAD_BYTES, 0);
        self.quads.extend_from_slice(&list.quads[..limit]);
        self.stats.quads = limit as u32;
        self.stats.quads_over_limit = (list.quads.len() - limit) as u32;
        self.uniforms = UiUniforms {
            points_to_clip: [2.0 / list.viewport[0], 2.0 / list.viewport[1]],
            padding: [0.0; 2],
        };
    }

    /// What the uploads pass writes, if this frame draws.
    pub(crate) fn uploads(&self) -> Option<UiUploads<'_>> {
        let built = self.built.as_deref().filter(|_| !self.quads.is_empty())?;
        Some(UiUploads {
            quads: &self.quads,
            buffer: built.quads.buffer(),
            uniform_buffer: &built.uniforms,
            uniforms: self.uniforms,
        })
    }

    /// Adds the pass over `target`, if this frame draws. It keeps what's there and blends over it.
    pub(crate) fn record<'f>(&'f self, graph: &mut FrameGraph<'f>, target: TextureId) {
        let Some(built) = self.built.as_deref().filter(|_| !self.quads.is_empty()) else { return };
        let Some(atlas) = built.atlas.as_ref() else { return };
        let (labels, count) = (self.labels, self.quads.len() as u32);
        let bytes = u64::from(count) * QUAD_BYTES;
        graph.add_pass(UI_PASS).read(target).write(target).record(move |ctx| {
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: labels.then_some(UI_PASS),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ctx.view(target),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                timestamp_writes: ctx.timestamp_writes(),
                ..Default::default()
            });
            pass.set_pipeline(&built.pipeline);
            pass.set_bind_group(0, &atlas.bind_group, &[]);
            pass.set_vertex_buffer(0, built.quads.buffer().slice(..bytes));
            pass.draw(0..4, 0..count);
        });
    }

    /// The pipeline to build from new sources (shader hot reload, off the frame thread:
    /// `shader_builds`), if it was built, for [`Self::set_pipeline`].
    pub(crate) fn rebuild(&self) -> Option<Step<wgpu::RenderPipeline>> {
        let layout = self.built.as_ref()?.pipeline_layout.clone();
        let (device, format, cache) = (self.device.clone(), self.color_format, self.cache.clone());
        Some(Box::new(move |sources, _| build_pipeline(&device, &layout, format, cache.as_ref(), sources)))
    }

    /// Whether the pipeline is built: with the first frame that draws quads.
    pub(crate) fn pipeline_built(&self) -> bool {
        self.built.is_some()
    }

    pub(crate) fn set_pipeline(&mut self, pipeline: wgpu::RenderPipeline) {
        if let Some(built) = &mut self.built {
            built.pipeline = pipeline;
        }
    }

    pub(crate) fn stats(&self) -> UiStats {
        self.stats
    }

    /// Lowers the quads' buffer limit, for tests of a full one. Before the first frame that draws.
    #[cfg(test)]
    pub(crate) fn limit_quads(&mut self, quads: u64) {
        self.max_bytes = quads * QUAD_BYTES;
    }
}

fn build_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    cache: Option<&wgpu::PipelineCache>,
    sources: &ShaderSources,
) -> wgpu::RenderPipeline {
    let module = shaders::module(device, Program::Ui, sources);
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(UI_PASS),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_ui"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: QUAD_BYTES,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &QUAD_ATTRIBUTES,
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_ui"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                // The shader's color is premultiplied, so a fill, its outline and their edge
                // compose in one quad.
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache,
    })
}

/// `atlas` as a texture and the group that reads it, or `None` if its size doesn't describe its
/// bytes or passes what the device holds: then nothing draws, rather than a validation error.
fn upload_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    built: &Built,
    atlas: &Arc<UiAtlas>,
    labels: bool,
) -> Option<AtlasOnGpu> {
    let bytes = u64::from(atlas.width) * u64::from(atlas.height);
    let side = device.limits().max_texture_dimension_2d;
    if bytes == 0 || bytes != atlas.texels.len() as u64 || atlas.width > side || atlas.height > side {
        tracing::warn!(target: crate::LOG_TARGET, ?atlas, "a UI atlas that can't be uploaded");
        return None;
    }
    let size = wgpu::Extent3d { width: atlas.width, height: atlas.height, depth_or_array_layers: 1 };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: labels.then_some("ui atlas"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &atlas.texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(atlas.width),
            rows_per_image: Some(atlas.height),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: labels.then_some("ui"),
        layout: &built.group_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: built.uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&built.sampler) },
        ],
    });
    Some(AtlasOnGpu { source: Arc::clone(atlas), bind_group })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quad_s_layout_is_what_the_vertex_stage_reads() {
        // Holds `UiQuad`, an append-only layout, to the attributes: nothing skipped or overlapped.
        assert_eq!(QUAD_BYTES, 76);
        let mut end = 0;
        for attribute in QUAD_ATTRIBUTES {
            assert_eq!(attribute.offset, end, "location {}", attribute.shader_location);
            end += attribute.format.size();
        }
        assert_eq!(end, QUAD_BYTES);
    }

    /// Needs a GPU: `cargo test -p sapphire_render --lib ui:: -- --ignored`.
    #[test]
    #[ignore = "needs a GPU"]
    fn quads_past_the_buffer_s_limit_are_left_out_and_counted() {
        use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc};

        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let mut ui = Ui::new(gpu.device(), wgpu::TextureFormat::Rgba8UnormSrgb, None, false);
        ui.limit_quads(4);
        let atlas = UiAtlas { width: 1, height: 1, spread: 6.0, texels: vec![255] };
        let quad = UiQuad::new([0.0; 2], [1.0, 0.0], [0.0, 1.0], [0.0, 0.0, 1.0, 1.0], [255; 4]);
        let list = UiDrawList { viewport: [8.0, 8.0], atlas: Some(Arc::new(atlas)), quads: vec![quad; 10] };
        ui.prepare_pipeline(&list, &ShaderSources::compiled_in());
        ui.prepare(gpu.queue(), &list);
        assert_eq!((ui.stats().quads, ui.stats().quads_over_limit), (4, 6));
        assert_eq!(ui.uploads().expect("what fits still draws").quads.len(), 4);
    }
}
