//! The on-screen interface over the finished frame (spec §5.17, M2 I2): the packet's
//! [`UiDrawList`], as one instanced draw after post and any debug view,
//! under debug draw and the app's overlay. It knows quads and an atlas, never widgets (rule R4).
//!
//! # A frame with no UI costs nothing
//!
//! Nothing here exists until a frame's list has quads and something for them to read: no pipeline,
//! texture or buffer, and no pass in the graph. The first such frame builds the one pipeline, on the calling thread,
//! as a new material variant's are, unless a load's inventory declared it (M3 K1), and uploads the atlas; those stay. A frame with an empty list
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
//! One draw of `n` instances of a four-vertex strip for each run of quads that reads one texture,
//! with the run's quads bound as a vertex buffer at slot 0, so it needs no storage binding, base
//! instance or anything the iOS Simulator lacks. The quads are
//! copied at [`Ui::prepare`] and staged in the frame's uploads pass, so a frame that never runs
//! leaves nothing behind: the next [`Ui::prepare`] writes its own. Past what a buffer holds, quads
//! don't draw and are counted ([`UiStats::quads_over_limit`]).
//!
//! # Glyph pages
//!
//! The list's `page_uploads` go to the queue in [`Ui::prepare`], ahead of the next submit, whether or
//! not this frame's graph runs, so a lost frame loses none. A page is made the first time a list
//! names it. A write has a fixed cost much larger than its bytes', so glyphs side by side go as one
//! ([`UiDrawList::page_bands`]).
//!
//! There is no copy of a page on the CPU, so a renderer can't rebuild its pages as it does the atlas
//! from its `Arc`. It says what it holds instead: [`Ui::page_state`], which a glyph atlas must be
//! handed to start a frame, names this renderer and the last batch it applied. A new renderer or a
//! list that never reached [`Ui::prepare`] makes the atlas start over. A list made for another
//! renderer draws none of its glyphs and writes nothing, and is counted
//! ([`UiStats::stale_page_quads`]), rather than draw another's page layout from empty pages.
//!
//! `ui.wgsl` is a program of its own ([`ShaderFile::Ui`](crate::ShaderFile::Ui)): an edit rebuilds
//! the one pipeline, once built, on the layout it had (`shader_builds`).

use std::collections::BTreeMap;
use std::mem::{offset_of, size_of};
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sapphire_look::{UiAtlas, UiDrawList, UiGlyphStats, UiMask, UiPageBands, UiPageState, UiQuad, UiTreeStats};
use sapphire_rhi::wgpu;

use crate::buffer::{self, GrowableBuffer};
use crate::graph::{FrameGraph, TextureId};
use crate::pipeline_builds::{PipelineBuilds, PipelineFamily};
use crate::shader_builds::Step;
use crate::shaders::{self, Program, ShaderSources};

/// The frame graph pass that draws the interface, for GPU timings.
pub const UI_PASS: &str = "ui";

/// Bytes of a quad, as the vertex stage reads it.
const QUAD_BYTES: u64 = size_of::<UiQuad>() as u64;

/// The next renderer's [`UiPageState::store`]. Never 0, which is no renderer's.
static NEXT_PAGE_STORE: AtomicU64 = AtomicU64::new(1);

/// Quads the buffer holds before it first grows: more than a combat HUD draws.
const INITIAL_QUADS: u64 = 256;

/// The vertex layout of [`UiQuad`], attribute by attribute.
const QUAD_ATTRIBUTES: [wgpu::VertexAttribute; 11] = [
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
    attribute(9, offset_of!(UiQuad, color_end), wgpu::VertexFormat::Unorm8x4),
    attribute(10, offset_of!(UiQuad, gradient), wgpu::VertexFormat::Float32x3),
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
    /// Glyph pages on the GPU.
    pub pages: u32,
    /// Rectangles of glyph pages the last prepared frame wrote, once joined.
    pub page_uploads: u32,
    /// Bytes of them.
    pub page_upload_bytes: u64,
    /// Quads of the last prepared frame's list that didn't draw, because it was made for another
    /// renderer's pages. The frame after it draws them, once the atlas has seen this renderer.
    pub stale_page_quads: u32,
    /// Draws in the last prepared frame.
    pub draws: u32,
    /// What the glyph atlas behind the last prepared frame's list did.
    pub glyphs: UiGlyphStats,
    /// What the retained trees behind it did.
    pub tree: UiTreeStats,
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

/// A glyph page and the group that reads it.
#[derive(Debug)]
struct PageOnGpu {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

/// A run of quads that reads one texture.
#[derive(Clone, Copy, Debug)]
struct UiDraw {
    first: u32,
    count: u32,
    /// The page, or `None` for the atlas.
    page: Option<u16>,
    /// The mask in force, in points.
    mask: Option<[f32; 4]>,
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
    pages: BTreeMap<u16, PageOnGpu>,
    page_size: u32,
}

/// The interface's GPU half. See the module docs.
#[derive(Debug)]
pub(crate) struct Ui {
    device: wgpu::Device,
    color_format: wgpu::TextureFormat,
    builds: PipelineBuilds,
    labels: bool,
    built: Option<Box<Built>>,
    /// This frame's quads, for the uploads pass. Its capacity stays.
    quads: Vec<UiQuad>,
    /// How they draw, in order.
    draws: Vec<UiDraw>,
    uniforms: UiUniforms,
    stats: UiStats,
    /// The largest buffer quads go in; lowered by tests of a full one.
    max_bytes: u64,
    /// This renderer's pages, and the last batch of uploads they took.
    page_state: UiPageState,
    /// This frame's uploads, joined. Its capacity stays.
    bands: UiPageBands,
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
        builds: PipelineBuilds,
        labels: bool,
    ) -> Self {
        Self {
            device: device.clone(),
            color_format,
            builds,
            labels,
            built: None,
            quads: Vec::new(),
            draws: Vec::new(),
            uniforms: UiUniforms::default(),
            stats: UiStats::default(),
            max_bytes: device.limits().max_buffer_size,
            page_state: UiPageState { store: NEXT_PAGE_STORE.fetch_add(1, Ordering::Relaxed), batch: 0 },
            bands: UiPageBands::default(),
        }
    }

    /// What this renderer's pages hold. See the module docs.
    pub(crate) fn page_state(&self) -> UiPageState {
        self.page_state
    }

    /// Whether `list` draws anything.
    fn draws(list: &UiDrawList) -> bool {
        !list.quads.is_empty()
            && (list.atlas.is_some() || !list.page_runs.is_empty())
            && list.viewport[0] > 0.0
            && list.viewport[1] > 0.0
    }

    /// Builds the pipeline and what it reads, if `list` is the first that draws. The caller holds
    /// the error scopes for edited sources (`Renderer::checked_build`).
    pub(crate) fn prepare_pipeline(&mut self, list: &UiDrawList, sources: &ShaderSources) {
        if Self::draws(list) || !list.page_uploads.is_empty() {
            self.build(sources);
        }
    }

    /// Builds the pipeline and what it reads unless they're built: a prewarm's, or the first list that draws or
    /// uploads a glyph page. Atlas quads and glyph pages draw through this one pipeline, so there's nothing else
    /// to declare for either.
    pub(crate) fn build(&mut self, sources: &ShaderSources) {
        if self.built.is_some() {
            return;
        }
        profiling::scope!("Ui::build");
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
        let pipeline = build_pipeline(device, &pipeline_layout, self.color_format, &self.builds, sources);
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
            pages: BTreeMap::new(),
            page_size: 0,
        }));
        self.stats.pipelines = 1;
    }

    /// Takes this frame's list: uploads its atlas if it's new, and copies its quads for the uploads
    /// pass. Allocates nothing once its vector has grown.
    pub(crate) fn prepare(&mut self, queue: &wgpu::Queue, list: &UiDrawList) {
        self.quads.clear();
        self.draws.clear();
        self.stats.quads = 0;
        self.stats.quads_over_limit = 0;
        self.stats.page_uploads = 0;
        self.stats.page_upload_bytes = 0;
        self.stats.stale_page_quads = 0;
        self.stats.draws = 0;
        self.stats.glyphs = list.glyphs;
        self.stats.tree = list.tree;
        // A list that names no page is nobody's, and says nothing about the pages.
        let uses_pages = !list.page_runs.is_empty() || !list.page_uploads.is_empty();
        let ours = list.page_state.store == self.page_state.store;
        if !ours && uses_pages {
            self.stats.stale_page_quads =
                list.page_runs.iter().fold(0u32, |count, run| count.saturating_add(run.count));
            tracing::debug!(
                target: crate::LOG_TARGET,
                quads = self.stats.stale_page_quads,
                "a list made for another renderer's glyph pages"
            );
        }
        let Some(built) = self.built.as_deref_mut() else { return };
        let applied = ours
            && write_pages(&self.device, queue, built, list, &mut self.bands, self.labels).is_some_and(
                |(uploads, bytes)| {
                    self.stats.page_uploads = uploads;
                    self.stats.page_upload_bytes = bytes;
                    self.page_state.batch = list.page_state.batch;
                    true
                },
            );
        self.stats.pages = built.pages.len() as u32;
        if !Self::draws(list) {
            return;
        }
        if let Some(atlas) = list.atlas.as_ref()
            && !built.atlas.as_ref().is_some_and(|on_gpu| Arc::ptr_eq(&on_gpu.source, atlas))
        {
            built.atlas = upload_atlas(&self.device, queue, built, atlas, self.labels);
            self.stats.atlas_bytes = 0;
            if built.atlas.is_some() {
                self.stats.atlas_uploads += 1;
                self.stats.atlas_bytes = atlas.texels.len() as u64;
            }
        }
        let has_atlas = list.atlas.is_some() && built.atlas.is_some();
        if !has_atlas && list.page_runs.is_empty() {
            return;
        }
        let limit = built.quads.max_items(QUAD_BYTES).min(list.quads.len() as u64) as usize;
        built.quads.reserve(&self.device, queue, limit as u64 * QUAD_BYTES, 0);
        self.quads.extend_from_slice(&list.quads[..limit]);
        self.stats.quads = limit as u32;
        self.stats.quads_over_limit = (list.quads.len() - limit) as u32;
        resolve_draws(
            list,
            limit as u32,
            has_atlas,
            |page| applied && built.pages.contains_key(&page),
            &mut self.draws,
        );
        self.stats.draws = self.draws.len() as u32;
        if self.draws.is_empty() {
            self.quads.clear();
        }
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
    pub(crate) fn record<'f>(&'f self, graph: &mut FrameGraph<'f>, target: TextureId, size: (u32, u32)) {
        let Some(built) = self.built.as_deref().filter(|_| !self.quads.is_empty()) else { return };
        let labels = self.labels;
        let draws = self.draws.as_slice();
        // A mask is in points, a scissor in the target's pixels.
        let viewport = self.uniforms.points_to_clip.map(|to_clip| 2.0 / to_clip);
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
            let mut scissor = None;
            for draw in draws {
                let wanted = draw.mask.map(|mask| scissor_of(mask, viewport, size));
                if wanted != scissor {
                    let [x, y, width, height] = wanted.unwrap_or([0, 0, size.0, size.1]);
                    pass.set_scissor_rect(x, y, width, height);
                    scissor = wanted;
                }
                if wanted.is_some_and(|[_, _, width, height]| width == 0 || height == 0) {
                    continue;
                }
                let group = match (draw.page, built.atlas.as_ref()) {
                    (Some(page), _) => &built.pages[&page].bind_group,
                    (None, Some(atlas)) => &atlas.bind_group,
                    (None, None) => continue,
                };
                let start = u64::from(draw.first) * QUAD_BYTES;
                let end = start + u64::from(draw.count) * QUAD_BYTES;
                pass.set_bind_group(0, group, &[]);
                pass.set_vertex_buffer(0, built.quads.buffer().slice(start..end));
                pass.draw(0..4, 0..draw.count);
            }
        });
    }

    /// The pipeline to build from new sources (shader hot reload, off the frame thread:
    /// `shader_builds`), if it was built, for [`Self::set_pipeline`].
    pub(crate) fn rebuild(&self) -> Option<Step<wgpu::RenderPipeline>> {
        let layout = self.built.as_ref()?.pipeline_layout.clone();
        let (device, format, builds) = (self.device.clone(), self.color_format, self.builds.clone());
        Some(Box::new(move |sources, _| build_pipeline(&device, &layout, format, &builds, sources)))
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
    builds: &PipelineBuilds,
    sources: &ShaderSources,
) -> wgpu::RenderPipeline {
    let module = shaders::module(device, Program::Ui, sources);
    builds.render(
        device,
        PipelineFamily::Ui,
        &wgpu::RenderPipelineDescriptor {
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
            cache: builds.cache(),
        },
    )
}

/// Makes the pages `list` names and applies its uploads: the rectangles and bytes written.
fn write_pages(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    built: &mut Built,
    list: &UiDrawList,
    bands: &mut UiPageBands,
    labels: bool,
) -> Option<(u32, u64)> {
    if list.page_runs.is_empty() && list.page_uploads.is_empty() {
        return Some((0, 0));
    }
    let side = list.page_size;
    if !valid_page_uploads(list, device.limits().max_texture_dimension_2d) {
        tracing::warn!(target: crate::LOG_TARGET, side, "invalid glyph page uploads; batch not acknowledged");
        return None;
    }
    if built.page_size != side {
        built.pages.clear();
        built.page_size = side;
    }
    let named = list.page_runs.iter().map(|run| run.page).chain(list.page_uploads.iter().map(|up| up.page));
    for page in named {
        if !built.pages.contains_key(&page) {
            let on_gpu = make_page(device, built, side, labels);
            built.pages.insert(page, on_gpu);
        }
    }
    let left_out = list.page_bands(bands);
    debug_assert_eq!(left_out, 0, "upload bounds checked before mutating pages");
    let (mut written, mut bytes) = (0, 0);
    for band in &bands.bands {
        let (width, height) = (u32::from(band.width), u32::from(band.height));
        if u32::from(band.x) + width > side || u32::from(band.y) + height > side {
            tracing::warn!(target: crate::LOG_TARGET, ?band, "a glyph upload past its page");
            continue;
        }
        let start = band.offset as usize;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &built.pages[&band.page].texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: u32::from(band.x), y: u32::from(band.y), z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &bands.texels[start..start + (width * height) as usize],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        written += 1;
        bytes += u64::from(width * height);
    }
    Some((written, bytes))
}

/// Validate the complete batch before allocating pages or acknowledging any of its writes.
fn valid_page_uploads(list: &UiDrawList, max_side: u32) -> bool {
    let side = list.page_size;
    side > 0
        && side <= max_side
        && list.page_uploads.iter().all(|upload| {
            let (width, height) = (u32::from(upload.width), u32::from(upload.height));
            width > 0
                && height > 0
                && u32::from(upload.x) + width <= side
                && u32::from(upload.y) + height <= side
                && u64::from(upload.offset) + u64::from(width) * u64::from(height)
                    <= list.upload_texels.len() as u64
        })
}

fn make_page(device: &wgpu::Device, built: &Built, side: u32, labels: bool) -> PageOnGpu {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: labels.then_some("ui glyph page"),
        size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: labels.then_some("ui glyph page"),
        layout: &built.group_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: built.uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&built.sampler) },
        ],
    });
    PageOnGpu { texture, bind_group }
}

/// `mask`, in points of `viewport`, as whole pixels of a target of `size` that hold it all: `x, y,
/// width, height`, inside the target as a scissor must be.
fn scissor_of(mask: [f32; 4], viewport: [f32; 2], size: (u32, u32)) -> [u32; 4] {
    let (scale_x, scale_y) = (size.0 as f32 / viewport[0], size.1 as f32 / viewport[1]);
    let x0 = ((mask[0] * scale_x).round().max(0.0) as u32).min(size.0);
    let y0 = ((mask[1] * scale_y).round().max(0.0) as u32).min(size.1);
    let x1 = ((mask[2] * scale_x).round().max(0.0) as u32).clamp(x0, size.0);
    let y1 = ((mask[3] * scale_y).round().max(0.0) as u32).clamp(y0, size.1);
    [x0, y0, x1 - x0, y1 - y0]
}

/// The first `limit` quads of `list` as runs of one texture and one mask each, in order. Quads with
/// nothing to read don't draw.
fn resolve_draws(
    list: &UiDrawList,
    limit: u32,
    has_atlas: bool,
    has_page: impl Fn(u16) -> bool,
    draws: &mut Vec<UiDraw>,
) {
    let mut at = 0;
    let mut masks = Masks { list: &list.masks, next: 0, rect: None };
    let field = |first: u32, end: u32, masks: &mut Masks<'_>, draws: &mut Vec<UiDraw>| {
        if has_atlas && end > first {
            masks.split(first, end, None, draws);
        }
    };
    for run in &list.page_runs {
        let (first, end) = (run.first.min(limit), run.first.saturating_add(run.count).min(limit));
        field(at, first, &mut masks, draws);
        if has_page(run.page) && end > first {
            masks.split(first, end, Some(run.page), draws);
        }
        at = end;
    }
    field(at, limit, &mut masks, draws);
}

/// Where a walk through a list's quads is among its masks. Quads that don't draw still pass theirs.
struct Masks<'a> {
    list: &'a [UiMask],
    next: usize,
    rect: Option<[f32; 4]>,
}

impl Masks<'_> {
    /// Quads `first..end` on one texture, as draws of one mask each.
    fn split(&mut self, first: u32, end: u32, page: Option<u16>, draws: &mut Vec<UiDraw>) {
        let mut at = first;
        while at < end {
            while let Some(mask) = self.list.get(self.next).filter(|mask| mask.first <= at) {
                self.rect = mask.rect;
                self.next += 1;
            }
            let until = self.list.get(self.next).map_or(end, |mask| mask.first.min(end));
            draws.push(UiDraw { first: at, count: until - at, page, mask: self.rect });
            at = until;
        }
    }
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
    use sapphire_look::{UiMask, UiPageRun};

    use super::*;

    #[test]
    fn the_quad_s_layout_is_what_the_vertex_stage_reads() {
        // Holds `UiQuad`, an append-only layout, to the attributes: nothing skipped or overlapped.
        assert_eq!(QUAD_BYTES, 92);
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
        let mut ui =
            Ui::new(gpu.device(), wgpu::TextureFormat::Rgba8UnormSrgb, PipelineBuilds::new(None), false);
        ui.limit_quads(4);
        let atlas = UiAtlas { width: 1, height: 1, spread: 6.0, texels: vec![255] };
        let quad = UiQuad::new([0.0; 2], [1.0, 0.0], [0.0, 1.0], [0.0, 0.0, 1.0, 1.0], [255; 4]);
        let list = UiDrawList {
            viewport: [8.0, 8.0],
            atlas: Some(Arc::new(atlas)),
            quads: vec![quad; 10],
            ..UiDrawList::default()
        };
        ui.prepare_pipeline(&list, &ShaderSources::compiled_in());
        ui.prepare(gpu.queue(), &list);
        assert_eq!((ui.stats().quads, ui.stats().quads_over_limit), (4, 6));
        assert_eq!(ui.uploads().expect("what fits still draws").quads.len(), 4);
    }

    #[test]
    fn runs_of_a_page_split_the_atlas_s_quads_in_order() {
        let quad = UiQuad::new([0.0; 2], [1.0, 0.0], [0.0, 1.0], [0.0; 4], [255; 4]);
        let list = UiDrawList {
            quads: vec![quad; 10],
            page_runs: vec![
                UiPageRun { first: 2, count: 3, page: 0 },
                UiPageRun { first: 5, count: 1, page: 1 },
            ],
            ..UiDrawList::default()
        };
        let resolved = |limit, has_atlas| {
            let mut draws = Vec::new();
            resolve_draws(&list, limit, has_atlas, |page| page < 2, &mut draws);
            draws.iter().map(|d| (d.first, d.count, d.page)).collect::<Vec<_>>()
        };
        assert_eq!(resolved(10, true), [(0, 2, None), (2, 3, Some(0)), (5, 1, Some(1)), (6, 4, None)],);
        assert_eq!(resolved(10, false), [(2, 3, Some(0)), (5, 1, Some(1))], "no atlas, no field quads");
        assert_eq!(resolved(3, true), [(0, 2, None), (2, 1, Some(0))], "a run cut at the buffer's limit");
    }

    #[test]
    fn overflowing_trailing_page_range_does_not_duplicate_or_hide_valid_quads() {
        let quad = UiQuad::new([0.0; 2], [1.0, 0.0], [0.0, 1.0], [0.0; 4], [255; 4]);
        let list = UiDrawList {
            quads: vec![quad; 10],
            page_runs: vec![
                UiPageRun { first: 2, count: 3, page: 0 },
                UiPageRun { first: u32::MAX, count: 1, page: 1 },
            ],
            ..UiDrawList::default()
        };
        let mut draws = Vec::new();
        resolve_draws(&list, 10, true, |_| true, &mut draws);
        assert_eq!(
            draws.iter().map(|draw| (draw.first, draw.count, draw.page)).collect::<Vec<_>>(),
            [(0, 2, None), (2, 3, Some(0)), (5, 5, None)]
        );
        draws.clear();
        resolve_draws(&list, 4, true, |_| true, &mut draws);
        assert_eq!(
            draws.iter().map(|draw| (draw.first, draw.count, draw.page)).collect::<Vec<_>>(),
            [(0, 2, None), (2, 2, Some(0))]
        );
    }

    #[test]
    fn a_mask_splits_a_run_and_outlasts_quads_that_don_t_draw() {
        let quad = UiQuad::new([0.0; 2], [1.0, 0.0], [0.0, 1.0], [0.0; 4], [255; 4]);
        let mask = Some([1.0, 2.0, 3.0, 4.0]);
        let list = UiDrawList {
            quads: vec![quad; 10],
            page_runs: vec![UiPageRun { first: 2, count: 4, page: 0 }],
            masks: vec![UiMask { first: 1, rect: mask }, UiMask { first: 4, rect: None }],
            ..UiDrawList::default()
        };
        let resolved = |has_atlas| {
            let mut draws = Vec::new();
            resolve_draws(&list, 10, has_atlas, |page| page < 1, &mut draws);
            draws.iter().map(|d| (d.first, d.count, d.page, d.mask)).collect::<Vec<_>>()
        };
        assert_eq!(
            resolved(true),
            [
                (0, 1, None, None),
                (1, 1, None, mask),
                (2, 2, Some(0), mask),
                (4, 2, Some(0), None),
                (6, 4, None, None),
            ],
        );
        assert_eq!(
            resolved(false),
            [(2, 2, Some(0), mask), (4, 2, Some(0), None)],
            "quads with nothing to read keep their mask's place",
        );
    }

    #[test]
    fn malformed_page_batches_are_rejected_before_any_page_is_materialized() {
        use sapphire_look::UiPageUpload;
        let mut list = UiDrawList {
            page_size: 16,
            page_uploads: vec![UiPageUpload { page: u16::MAX, x: 0, y: 0, width: 2, height: 2, offset: 0 }],
            upload_texels: vec![255; 4],
            ..UiDrawList::default()
        };
        assert!(valid_page_uploads(&list, 16), "a sparse identifier names just one page");
        list.upload_texels.pop();
        assert!(!valid_page_uploads(&list, 16), "short bytes");
        list.upload_texels.push(255);
        list.page_uploads[0].x = 15;
        assert!(!valid_page_uploads(&list, 16), "outside page");
        list.page_uploads[0].x = 0;
        assert!(!valid_page_uploads(&list, 8), "device limit");
        list.page_size = 0;
        assert!(!valid_page_uploads(&list, 16), "zero side");
    }

    #[test]
    fn a_mask_becomes_whole_pixels_inside_the_target() {
        let viewport = [100.0, 50.0];
        let size = (200, 100);
        assert_eq!(scissor_of([10.0, 5.0, 20.0, 15.0], viewport, size), [20, 10, 20, 20]);
        assert_eq!(scissor_of([-10.0, -5.0, 500.0, 500.0], viewport, size), [0, 0, 200, 100]);
        assert_eq!(scissor_of([20.0, 5.0, 10.0, 15.0], viewport, size), [40, 10, 0, 20], "inside out is empty");
        assert_eq!(scissor_of([900.0, 900.0, 950.0, 950.0], viewport, size), [200, 100, 0, 0]);
    }
}
