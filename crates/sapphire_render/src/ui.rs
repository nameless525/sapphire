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
//! (which offscreen targets and some surfaces don't declare) or a read of the frame. Each quad's
//! display-encoded colors are decoded as it's copied for the GPU, through an exact table of the 256
//! codes, so an opaque color lands on exactly its code, as the debug overlay's linearized colors do,
//! and every backend blends the same values. The cost is what egui's notes describe: translucent colors
//! read differently than in a display-space blend. A 50% black over white shows as code 188, not
//! 128, so inks that must hold over a bright sky are authored nearly opaque, and an edge's
//! antialiasing is true coverage, which reads light-on-dark a little bolder and dark-on-light a
//! little thinner. An outline around every digit takes care of the second.
//!
//! # Drawing
//!
//! One draw of `n` instances of a four-vertex strip for each run of quads that reads one texture.
//! The frame's quads are one storage buffer that both stages read, and a run's instances come in
//! through a vertex stream of indices sliced at its first quad, so a draw never starts its instances
//! past 0, which the iOS Simulator can't. The quads are
//! copied at [`Ui::prepare`] and staged in the frame's uploads pass, so a frame that never runs
//! leaves nothing behind: the next [`Ui::prepare`] writes its own. Past what a buffer holds, quads
//! don't draw and are counted ([`UiStats::quads_over_limit`]).
//!
//! # What a pixel pays for (M3)
//!
//! A screen of large panels is mostly pixels of fills, so a pixel pays only for what its own quad
//! uses. **Decisions:**
//!
//! - **Only a quad's corner and its index pass between the stages,** and the fragment stage reads the
//!   rest from the buffer. A stage pays for every input it declares on every pixel, read or not, and
//!   a quad's 28 values as flat inputs cost as much as all of its shading.
//! - **A quad reaches the GPU worked out** ([`GpuUiQuad`]): its colors decoded once, here, rather
//!   than by three `pow`s on every pixel, and flags saying which of the slice, field, outline,
//!   gradient, cut and sweep it uses, and what its texels hold ([`UiQuad::texels`]: a field, a glyph
//!   page's coverage, a display face's multi-channel field or an inset's picture), which the shader
//!   branches on rather than compare. Every pixel of a quad agrees, so a branch costs nothing, and a
//!   skipped part is what it would have worked out.
//! - **A solid quad reads no field** ([`is_solid`]): one of the atlas's single-channel field that
//!   reads a single point of it, where
//!   every texel a filtered read blends is full and both its edges have passed, is filled through and
//!   through, and its pixels come out as the field's path makes them. A rectangle and a nine-slice's
//!   middle are, and they're most of a screen.
//!
//! **What it bought** (release, Apple M5, the sandbox's `--bench` with `--ui-game`): the pass's
//! share of the rest of its frame halved on both tiers, on the game's character, settings and title
//! screens as on U4's retired test screen of 719 quads, and every game screen is inside §3.3's
//! 0.3 ms on Desktop-Low at 1080p (spec §5.17 has the rounds). What remains is about half the cost of
//! covering these pixels at all: the same quads writing a constant color take roughly half of
//! today's share.
//! Rejected: computing the field's slope analytically (it moves every edge pixel), a second
//! pipeline for solid quads (a split draw for a branch that costs nothing), and drawing opaque quads
//! unblended (it pays only on GPUs that don't blend in tile memory, and needs the same split).
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
//! # Insets
//!
//! A quad in one of the list's inset runs (M3 U6, `UiDrawList::inset_runs`) reads an inset's picture
//! in place of the atlas, through the same pipeline: its [`UiQuad::texels`] say the texels are a
//! picture (`UI_PICTURE`, a flag on the GPU), linear color the quad's color tints. The pictures are the renderer's own
//! (the `inset` module), each with a group of this layout ([`Ui::picture_group`]); a run naming an
//! inset the frame has no picture for draws nothing.
//!
//! # Display faces' fields
//!
//! A display face's glyphs come cooked as a multi-channel field ([`UiFieldFormat::Multi`]), which a
//! list names in [`UiDrawList::fields`]. Each is an atlas like the HUD's, uploaded the first time a
//! list names it and drawn through the same pipeline and bind group layout: a quad's
//! [`UiQuad::texels`], a flag on the GPU, says how the shader reads what it samples, so a field adds no pipeline and a
//! warm-up declares nothing more for it. One stays on the GPU while anything but this renderer holds
//! its [`Arc`], so a title that comes and goes uploads once, and goes once its face is let go.
//!
//! `ui.wgsl` is a program of its own ([`ShaderFile::Ui`](crate::ShaderFile::Ui)): an edit rebuilds
//! the one pipeline, once built, on the layout it had (`shader_builds`).

use std::collections::BTreeMap;
use std::mem::size_of;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sapphire_look::{
    UI_COVERAGE, UI_FIELD, UI_MULTI_FIELD, UI_PICTURE, UiAtlas, UiDrawList, UiFieldFormat, UiGlyphStats,
    UiMask, UiPageBands, UiPageState, UiQuad, UiSource, UiTreeStats,
};
use sapphire_rhi::wgpu;

use crate::buffer::{self, GrowableBuffer};
use crate::counts::{Counter, PassCounts};
use crate::graph::{FrameGraph, TextureId};
use crate::inset::MAX_INSETS;
use crate::pipeline_builds::{PipelineBuilds, PipelineFamily};
use crate::shader_builds::Step;
use crate::shaders::{self, Program, ShaderSources};

/// The frame graph pass that draws the interface, for GPU timings.
pub const UI_PASS: &str = "ui";

/// Bytes of a quad, as the shader reads it.
const QUAD_BYTES: u64 = size_of::<GpuUiQuad>() as u64;

/// [`GpuUiQuad::flags`]: the quad reads no field, being inside its shape everywhere ([`is_solid`]).
pub(crate) const UI_SOLID: u32 = 1;
/// It's a three-slice: [`UiQuad::slice`] is above 0.
pub(crate) const UI_SLICED: u32 = 1 << 1;
/// Its outline shows: its color has alpha, and it isn't solid.
pub(crate) const UI_OUTLINED: u32 = 1 << 2;
/// Its fill is a gradient.
pub(crate) const UI_GRADIENT: u32 = 1 << 3;
/// It's cut along its width.
pub(crate) const UI_CUT: u32 = 1 << 4;
/// It's swept.
pub(crate) const UI_SWEPT: u32 = 1 << 5;
/// Its texels are coverage, as a glyph page's are ([`UI_COVERAGE`]).
pub(crate) const UI_COVERAGE_TEXELS: u32 = 1 << 6;
/// Its texels are a display face's multi-channel field ([`UI_MULTI_FIELD`]).
pub(crate) const UI_MULTI_TEXELS: u32 = 1 << 7;
/// Its texels are an inset's picture ([`UI_PICTURE`]).
pub(crate) const UI_PICTURE_TEXELS: u32 = 1 << 8;

/// A [`UiQuad`] as the shader reads it, worked out once rather than on each of its pixels: its colors
/// in linear light, and which of its parts it uses. Field by field with `ui.wgsl`'s `UiQuad`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GpuUiQuad {
    pub(crate) origin: [f32; 2],
    pub(crate) axis_x: [f32; 2],
    pub(crate) axis_y: [f32; 2],
    pub(crate) clip: [f32; 2],
    pub(crate) uv: [f32; 4],
    pub(crate) color: [f32; 4],
    pub(crate) color_end: [f32; 4],
    pub(crate) outline_color: [f32; 4],
    /// Its edge, softness, outline and sweep.
    pub(crate) shape: [f32; 4],
    pub(crate) gradient: [f32; 3],
    pub(crate) slice: f32,
    pub(crate) flags: u32,
    pub(crate) padding: [u32; 3],
}

impl GpuUiQuad {
    /// `quad`, its colors decoded through `linear`; not solid, which only its draw can say.
    fn new(quad: &UiQuad, linear: &[f32; 256]) -> Self {
        // Alpha is a share, not a light: only the color's three channels are encoded.
        let decode = |[r, g, b, a]: [u8; 4]| {
            [linear[usize::from(r)], linear[usize::from(g)], linear[usize::from(b)], f32::from(a) / 255.0]
        };
        let mut flags = 0;
        for (on, flag) in [
            (quad.slice > 0.0, UI_SLICED),
            (quad.outline_color[3] != 0, UI_OUTLINED),
            (quad.gradient != [0.0; 3], UI_GRADIENT),
            (quad.clip[0] > 0.0 || quad.clip[1] < 1.0, UI_CUT),
            (quad.sweep > 0.0, UI_SWEPT),
            (quad.texels == UI_COVERAGE, UI_COVERAGE_TEXELS),
            (quad.texels == UI_MULTI_FIELD, UI_MULTI_TEXELS),
            (quad.texels == UI_PICTURE, UI_PICTURE_TEXELS),
        ] {
            if on {
                flags |= flag;
            }
        }
        Self {
            origin: quad.origin,
            axis_x: quad.axis_x,
            axis_y: quad.axis_y,
            clip: quad.clip,
            uv: quad.uv,
            color: decode(quad.color),
            color_end: decode(quad.color_end),
            outline_color: decode(quad.outline_color),
            shape: [quad.edge, quad.softness, quad.outline, quad.sweep],
            gradient: quad.gradient,
            slice: quad.slice,
            flags,
            padding: [0; 3],
        }
    }

    /// Marks it solid: it reads no field, and so shows no outline.
    fn make_solid(&mut self) {
        self.flags = (self.flags | UI_SOLID) & !UI_OUTLINED;
    }
}

/// Each display-encoded code as linear light, exactly: what a quad's colors are decoded through.
fn linear_codes() -> [f32; 256] {
    std::array::from_fn(|code| {
        let display = code as f64 / 255.0;
        let linear = if display <= 0.04045 { display / 12.92 } else { ((display + 0.055) / 1.055).powf(2.4) };
        linear as f32
    })
}

/// How far past an edge a solid quad's field must be, in field: what keeps the shader's ramp at 1
/// whatever its last bits.
const SOLID_MARGIN: f32 = 1e-3;

/// Whether every pixel of `quad` is inside its shape, so its field's path would fill it through:
/// it reads one point of `atlas`, every texel a filtered read there blends is full, so its field is
/// 1 with no slope, and both its fill's edge and its outline's are far enough below 1 that their ramps
/// are 1 at the widest its softness makes them. Only a single-channel field's quad over a
/// single-channel atlas can be: never a glyph's coverage, a display face's field or a picture.
fn is_solid(quad: &UiQuad, atlas: &UiAtlas) -> bool {
    let [u0, v0, u1, v1] = quad.uv;
    let single = quad.texels == UI_FIELD && atlas.format == UiFieldFormat::Single;
    if u0 != u1 || v0 != v1 || !single || atlas.width == 0 || atlas.height == 0 {
        return false;
    }
    // Linear filtering blends the texels around `uv - half a texel`, clamped at the edges.
    let around = |at: f32, side: u32| {
        let first = (at * side as f32 - 0.5).floor();
        let clamp = |texel: f32| texel.clamp(0.0, (side - 1) as f32) as usize;
        [clamp(first), clamp(first + 1.0)]
    };
    let full = around(u0, atlas.width).into_iter().all(|x| {
        around(v0, atlas.height)
            .into_iter()
            .all(|y| atlas.texels.get(y * atlas.width as usize + x) == Some(&255))
    });
    let width = (quad.softness * 2.0).max(1e-4);
    let deepest = quad.edge.max(quad.edge - quad.outline);
    full && (1.0 - deepest) / width >= 0.5 + SOLID_MARGIN
}

/// The next renderer's [`UiPageState::store`]. Never 0, which is no renderer's.
static NEXT_PAGE_STORE: AtomicU64 = AtomicU64::new(1);

/// Quads the buffer holds before it first grows: more than a combat HUD draws.
const INITIAL_QUADS: u64 = 256;

/// Bytes of a quad's index in the stream a draw's instances read.
const INDEX_BYTES: u64 = size_of::<u32>() as u64;

/// The index stream's one attribute: the quad's place in the frame's buffer.
const INDEX_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
    [wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 0, shader_location: 0 }];

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
    /// Display faces' fields on the GPU.
    pub fields: u32,
    /// Their size on the GPU.
    pub field_bytes: u64,
    /// Times a field was uploaded since the renderer was created.
    pub field_uploads: u32,
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
    /// Quads of the last prepared frame that read no field, being inside their shapes throughout.
    pub solid_quads: u32,
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

/// What a run of quads reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UiTexture {
    Atlas,
    Page(u16),
    /// By its place in `Built::fields`.
    Field(u16),
    /// The inset whose picture it reads (M3 U6).
    Inset(u16),
}

/// A run of quads that reads one texture.
#[derive(Clone, Copy, Debug)]
struct UiDraw {
    first: u32,
    count: u32,
    texture: UiTexture,
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
    /// What reads [`Self::quads`], made again when it grows.
    quads_layout: wgpu::BindGroupLayout,
    quads_group: wgpu::BindGroup,
    /// `0, 1, 2, …`, one for each quad [`Self::quads`] holds: what a draw's instances read, sliced at its first.
    indices: wgpu::Buffer,
    atlas: Option<AtlasOnGpu>,
    pages: BTreeMap<u16, PageOnGpu>,
    page_size: u32,
    fields: Vec<AtlasOnGpu>,
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
    quads: Vec<GpuUiQuad>,
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
    /// Where each of this frame's list's fields is in `Built::fields`, if it could be uploaded. Its
    /// capacity stays.
    field_slots: Vec<Option<u16>>,
    /// Each display-encoded code in linear light: [`linear_codes`].
    linear: Box<[f32; 256]>,
}

/// What the uploads pass writes for the interface.
#[derive(Debug)]
pub(crate) struct UiUploads<'a> {
    quads: &'a [GpuUiQuad],
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
            max_bytes: max_quad_bytes(&device.limits()),
            page_state: UiPageState { store: NEXT_PAGE_STORE.fetch_add(1, Ordering::Relaxed), batch: 0 },
            bands: UiPageBands::default(),
            field_slots: Vec::new(),
            linear: Box::new(linear_codes()),
        }
    }

    /// What this renderer's pages hold. See the module docs.
    pub(crate) fn page_state(&self) -> UiPageState {
        self.page_state
    }

    /// Whether `list` draws anything.
    fn draws(list: &UiDrawList) -> bool {
        !list.quads.is_empty()
            && (list.atlas.is_some() || !list.runs.is_empty() || !list.inset_runs.is_empty())
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
        let built = self.maker().make(sources);
        self.adopt(built);
    }

    /// A warm-up's step (`warm_up`): what [`Self::build`] makes, for [`Self::adopt`]. `None` once it's built.
    pub(crate) fn warm(&self) -> Option<Step<WarmedUi>> {
        if self.built.is_some() {
            return None;
        }
        let maker = self.maker();
        Some(Box::new(move |sources, _| maker.make(sources)))
    }

    /// Takes in what a warm-up made, unless it was made meanwhile.
    pub(crate) fn adopt(&mut self, warmed: WarmedUi) {
        if self.built.is_none() {
            self.built = Some(warmed.0);
            self.stats.pipelines = 1;
        }
    }

    fn maker(&self) -> UiMaker {
        UiMaker {
            device: self.device.clone(),
            labels: self.labels,
            color_format: self.color_format,
            builds: self.builds.clone(),
            max_bytes: self.max_bytes,
        }
    }
}

/// What makes the pipeline and what it reads, on any thread ([`Ui::build`], [`Ui::warm`]).
struct UiMaker {
    device: wgpu::Device,
    labels: bool,
    color_format: wgpu::TextureFormat,
    builds: PipelineBuilds,
    max_bytes: u64,
}

/// The pipeline and what it reads, as [`Ui::warm`] made them.
#[derive(Debug)]
pub(crate) struct WarmedUi(Box<Built>);

impl UiMaker {
    fn make(&self, sources: &ShaderSources) -> WarmedUi {
        let maker = self;
        let device = &maker.device;
        let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: maker.labels.then_some("ui"),
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
        let quads_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: maker.labels.then_some("ui quads"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(QUAD_BYTES),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: maker.labels.then_some("ui"),
            bind_group_layouts: &[Some(&group_layout), Some(&quads_layout)],
            immediate_size: 0,
        });
        let pipeline = build_pipeline(device, &pipeline_layout, maker.color_format, &maker.builds, sources);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: maker.labels.then_some("ui"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: maker.labels.then_some("ui uniforms"),
            size: size_of::<UiUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let quads = GrowableBuffer::new(
            device,
            "ui quads",
            wgpu::BufferUsages::STORAGE,
            INITIAL_QUADS * QUAD_BYTES,
            maker.max_bytes,
        );
        let (quads_group, indices) = bind_quads(device, &quads_layout, quads.buffer(), maker.labels);
        WarmedUi(Box::new(Built {
            pipeline,
            pipeline_layout,
            group_layout,
            sampler,
            uniforms,
            quads,
            quads_layout,
            quads_group,
            indices,
            atlas: None,
            pages: BTreeMap::new(),
            page_size: 0,
            fields: Vec::new(),
        }))
    }
}

impl Ui {
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
        self.stats.solid_quads = 0;
        self.stats.glyphs = list.glyphs;
        self.stats.tree = list.tree;
        // A list that names no page is nobody's, and says nothing about the pages.
        let page_runs = || list.runs.iter().filter(|run| matches!(run.source, UiSource::Page(_)));
        let uses_pages = page_runs().next().is_some() || !list.page_uploads.is_empty();
        let ours = list.page_state.store == self.page_state.store;
        if !ours && uses_pages {
            self.stats.stale_page_quads =
                page_runs().fold(0u32, |count, run| count.saturating_add(run.count));
            tracing::debug!(
                target: crate::LOG_TARGET,
                quads = self.stats.stale_page_quads,
                "a list made for another renderer's glyph pages"
            );
        }
        let Some(built) = self.built.as_deref_mut() else { return };
        // What nothing else holds won't be named again.
        built.fields.retain(|field| Arc::strong_count(&field.source) > 1);
        count_fields(&mut self.stats, built);
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
        self.field_slots.clear();
        for field in &list.fields {
            let slot = match built.fields.iter().position(|on_gpu| Arc::ptr_eq(&on_gpu.source, field)) {
                Some(slot) => Some(slot),
                None => upload_atlas(&self.device, queue, built, field, self.labels).map(|on_gpu| {
                    self.stats.field_uploads += 1;
                    built.fields.push(on_gpu);
                    built.fields.len() - 1
                }),
            };
            self.field_slots.push(slot.and_then(|slot| u16::try_from(slot).ok()));
        }
        count_fields(&mut self.stats, built);
        let has_atlas = list.atlas.is_some() && built.atlas.is_some();
        if !has_atlas && list.runs.is_empty() && list.inset_runs.is_empty() {
            return;
        }
        let limit = built.quads.max_items(QUAD_BYTES).min(list.quads.len() as u64) as usize;
        if built.quads.reserve(&self.device, queue, limit as u64 * QUAD_BYTES, 0) {
            (built.quads_group, built.indices) =
                bind_quads(&self.device, &built.quads_layout, built.quads.buffer(), self.labels);
        }
        let linear = &*self.linear;
        self.quads.extend(list.quads[..limit].iter().map(|quad| GpuUiQuad::new(quad, linear)));
        self.stats.quads = limit as u32;
        self.stats.quads_over_limit = (list.quads.len() - limit) as u32;
        let field_slots = &self.field_slots;
        resolve_draws(
            list,
            limit as u32,
            has_atlas,
            |source| match source {
                UiSource::Page(page) => {
                    (applied && built.pages.contains_key(&page)).then_some(UiTexture::Page(page))
                }
                UiSource::Field(at) => {
                    field_slots.get(usize::from(at)).copied().flatten().map(UiTexture::Field)
                }
            },
            &mut self.draws,
        );
        self.stats.draws = self.draws.len() as u32;
        if self.draws.is_empty() {
            self.quads.clear();
        }
        // What reads the atlas may be solid; a glyph page is coverage, and a field or a picture is
        // read whole.
        if let Some(atlas) = list.atlas.as_deref().filter(|_| has_atlas) {
            for draw in self.draws.iter().filter(|draw| draw.texture == UiTexture::Atlas) {
                let range = draw.first as usize..(draw.first + draw.count) as usize;
                for (gpu, quad) in self.quads[range.clone()].iter_mut().zip(&list.quads[range]) {
                    if is_solid(quad, atlas) {
                        gpu.make_solid();
                        self.stats.solid_quads += 1;
                    }
                }
            }
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

    /// A group reading `picture`, an inset's (M3 U6), for [`Self::record`]'s pictures: `None` until the
    /// pipeline is built.
    pub(crate) fn picture_group(&self, picture: &wgpu::TextureView) -> Option<wgpu::BindGroup> {
        let built = self.built.as_deref()?;
        Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: self.labels.then_some("ui inset"),
            layout: &built.group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: built.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(picture) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&built.sampler) },
            ],
        }))
    }

    /// Adds the pass over `target`, if this frame draws. It keeps what's there and blends over it. Inset
    /// runs read `pictures`: by inset, its group ([`Self::picture_group`]) and its picture in the graph.
    /// Its draws count in `counter`, if there is one, a quad of two triangles each.
    pub(crate) fn record<'f>(
        &'f self,
        graph: &mut FrameGraph<'f>,
        target: TextureId,
        size: (u32, u32),
        pictures: [Option<UiPicture<'f>>; MAX_INSETS],
        counter: Option<&'f Counter>,
    ) {
        let Some(built) = self.built.as_deref().filter(|_| !self.quads.is_empty()) else { return };
        let labels = self.labels;
        let draws = self.draws.as_slice();
        // A mask is in points, a scissor in the target's pixels.
        let viewport = self.uniforms.points_to_clip.map(|to_clip| 2.0 / to_clip);
        let mut pass = graph.add_pass(UI_PASS).read(target);
        for picture in pictures.iter().flatten() {
            pass = pass.read(picture.texture);
        }
        pass.write(target).record(move |ctx| {
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
            pass.set_bind_group(1, &built.quads_group, &[]);
            let mut scissor = None;
            let mut counts = PassCounts::default();
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
                let picture = |inset| pictures.iter().flatten().find(|picture| picture.inset == inset);
                let group = match (draw.texture, built.atlas.as_ref()) {
                    (UiTexture::Inset(inset), _) => match picture(inset) {
                        Some(picture) => picture.group,
                        None => continue,
                    },
                    (UiTexture::Page(page), _) => &built.pages[&page].bind_group,
                    (UiTexture::Field(at), _) => &built.fields[usize::from(at)].bind_group,
                    (UiTexture::Atlas, Some(atlas)) => &atlas.bind_group,
                    (UiTexture::Atlas, None) => continue,
                };
                let start = u64::from(draw.first) * INDEX_BYTES;
                let end = start + u64::from(draw.count) * INDEX_BYTES;
                pass.set_bind_group(0, group, &[]);
                pass.set_vertex_buffer(0, built.indices.slice(start..end));
                pass.draw(0..4, 0..draw.count);
                counts = counts.plus(&PassCounts::call(draw.count, 2));
            }
            if let Some(counter) = counter {
                counter.add(&counts);
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
                    array_stride: INDEX_BYTES,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &INDEX_ATTRIBUTES,
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

/// The most bytes of quads a frame can draw: a buffer, bound whole for storage.
fn max_quad_bytes(limits: &wgpu::Limits) -> u64 {
    limits.max_buffer_size.min(limits.max_storage_buffer_binding_size)
}

/// The group that reads `quads`, and the index stream for each quad it holds. The stream never
/// changes, so it's written once, as it's made.
fn bind_quads(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    quads: &wgpu::Buffer,
    labels: bool,
) -> (wgpu::BindGroup, wgpu::Buffer) {
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: labels.then_some("ui quads"),
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: quads.as_entire_binding() }],
    });
    let count = (quads.size() / QUAD_BYTES).max(1);
    let indices = device.create_buffer(&wgpu::BufferDescriptor {
        label: labels.then_some("ui quad indices"),
        size: count * INDEX_BYTES,
        usage: wgpu::BufferUsages::VERTEX,
        mapped_at_creation: true,
    });
    // A lost device makes the buffer invalid, and it can't be mapped: what's made on that device is thrown away
    // with it, so its words needn't be written (M4 H4).
    if let Ok(mut words) = indices.slice(..).get_mapped_range_mut() {
        for index in 0..count as u32 {
            let at = index as usize * INDEX_BYTES as usize;
            words.slice(at..at + INDEX_BYTES as usize).copy_from_slice(&index.to_le_bytes());
        }
    }
    indices.unmap();
    (group, indices)
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
    let named_pages = || {
        list.runs.iter().filter_map(|run| match run.source {
            UiSource::Page(page) => Some(page),
            UiSource::Field(_) => None,
        })
    };
    if named_pages().next().is_none() && list.page_uploads.is_empty() {
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
    let named = named_pages().chain(list.page_uploads.iter().map(|up| up.page));
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

/// The fields on the GPU, for the stats.
fn count_fields(stats: &mut UiStats, built: &Built) {
    stats.fields = built.fields.len() as u32;
    stats.field_bytes = built.fields.iter().map(|field| field.source.texels.len() as u64).sum();
}

/// Whether `list`'s whole batch of page uploads fits its pages and its bytes: checked before any page
/// is made or any of its writes acknowledged.
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

/// An inset's picture, as [`Ui::record`] reads it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UiPicture<'f> {
    pub inset: u16,
    pub group: &'f wgpu::BindGroup,
    pub texture: TextureId,
}

/// The first `limit` quads of `list` as runs of one texture and one mask each, in order: the pages'
/// and fields' runs with each one's source as `texture` finds it, and the insets' runs, which come in
/// order and don't overlap, and the atlas's quads between them. Quads with nothing to read don't draw.
fn resolve_draws(
    list: &UiDrawList,
    limit: u32,
    has_atlas: bool,
    texture: impl Fn(UiSource) -> Option<UiTexture>,
    draws: &mut Vec<UiDraw>,
) {
    let mut at = 0;
    let mut masks = Masks { list: &list.masks, next: 0, rect: None };
    let atlas = |first: u32, end: u32, masks: &mut Masks<'_>, draws: &mut Vec<UiDraw>| {
        if has_atlas && end > first {
            masks.split(first, end, UiTexture::Atlas, draws);
        }
    };
    let runs = list.runs.iter().map(|run| (run.first, run.count, texture(run.source)));
    let insets = list.inset_runs.iter().map(|run| (run.first, run.count, Some(UiTexture::Inset(run.inset))));
    let (mut runs, mut insets) = (runs.peekable(), insets.peekable());
    loop {
        let next = match (runs.peek(), insets.peek()) {
            (Some(run), Some(inset)) if run.0 <= inset.0 => runs.next(),
            (_, Some(_)) => insets.next(),
            (Some(_), None) => runs.next(),
            (None, None) => break,
        };
        let Some((first, count, texture)) = next else { break };
        let (first, end) = (first.min(limit), first.saturating_add(count).min(limit));
        atlas(at, first, &mut masks, draws);
        if let Some(texture) = texture.filter(|_| end > first) {
            masks.split(first, end, texture, draws);
        }
        at = end;
    }
    atlas(at, limit, &mut masks, draws);
}

/// Where a walk through a list's quads is among its masks. Quads that don't draw still pass theirs.
struct Masks<'a> {
    list: &'a [UiMask],
    next: usize,
    rect: Option<[f32; 4]>,
}

impl Masks<'_> {
    /// Quads `first..end` on one texture, as draws of one mask each.
    fn split(&mut self, first: u32, end: u32, texture: UiTexture, draws: &mut Vec<UiDraw>) {
        let mut at = first;
        while at < end {
            while let Some(mask) = self.list.get(self.next).filter(|mask| mask.first <= at) {
                self.rect = mask.rect;
                self.next += 1;
            }
            let until = self.list.get(self.next).map_or(end, |mask| mask.first.min(end));
            draws.push(UiDraw { first: at, count: until - at, texture, mask: self.rect });
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
    let row_bytes = atlas.width * atlas.format.bytes_per_texel();
    let bytes = u64::from(row_bytes) * u64::from(atlas.height);
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
        format: match atlas.format {
            UiFieldFormat::Single => wgpu::TextureFormat::R8Unorm,
            UiFieldFormat::Multi => wgpu::TextureFormat::Rgba8Unorm,
        },
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &atlas.texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(row_bytes),
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
    use std::mem::offset_of;

    use sapphire_look::{UI_COVERAGE, UI_MULTI_FIELD, UI_PICTURE, UiInsetRun, UiMask, UiRun};

    use super::*;

    #[test]
    fn a_gpu_quad_is_whole_rows_of_the_storage_array() {
        // `shaders::tests` holds each field to its place in `ui.wgsl`; here, that the array's stride is
        // the struct's size, a multiple of its 16-byte alignment.
        assert_eq!(QUAD_BYTES, 144);
        assert_eq!(QUAD_BYTES % 16, 0);
        assert_eq!(offset_of!(GpuUiQuad, uv) % 16, 0);
    }

    #[test]
    fn codes_decode_exactly() {
        let linear = linear_codes();
        assert_eq!(linear[0], 0.0);
        assert_eq!(linear[255], 1.0);
        // The linear segment, and the curve: 50% black over white shows as 188 (the module docs).
        assert_eq!(linear[10], (10.0 / 255.0 / 12.92) as f32);
        assert!((linear[188] - 0.5).abs() < 0.003, "{}", linear[188]);
        assert!(linear.windows(2).all(|pair| pair[0] < pair[1]));
    }

    fn quad(uv: [f32; 4]) -> UiQuad {
        UiQuad::new([0.0; 2], [10.0, 0.0], [0.0, 10.0], uv, [200, 100, 50, 255])
    }

    #[test]
    fn a_quad_says_which_parts_it_uses() {
        let linear = linear_codes();
        let plain = GpuUiQuad::new(&quad([0.5; 4]), &linear);
        assert_eq!(plain.flags, 0);
        assert_eq!(plain.color, [linear[200], linear[100], linear[50], 1.0]);
        let mut parts = quad([0.5; 4]);
        parts.slice = 0.25;
        parts.outline_color = [0, 0, 0, 1];
        parts.gradient = [0.0, 0.0, 0.5];
        parts.clip = [0.0, 0.5];
        parts.sweep = 0.75;
        let all = UI_SLICED | UI_OUTLINED | UI_GRADIENT | UI_CUT | UI_SWEPT;
        let mut gpu = GpuUiQuad::new(&parts, &linear);
        assert_eq!(gpu.flags, all);
        assert_eq!(gpu.shape, [parts.edge, parts.softness, parts.outline, 0.75]);
        gpu.make_solid();
        assert_eq!(gpu.flags, (all | UI_SOLID) & !UI_OUTLINED);
        // What its texels hold becomes a flag too, which the shader branches on.
        for (texels, flag) in [
            (UI_COVERAGE, UI_COVERAGE_TEXELS),
            (UI_MULTI_FIELD, UI_MULTI_TEXELS),
            (UI_PICTURE, UI_PICTURE_TEXELS),
        ] {
            let mut read = quad([0.5; 4]);
            read.texels = texels;
            assert_eq!(GpuUiQuad::new(&read, &linear).flags, flag, "texels {texels}");
        }
    }

    #[test]
    fn only_a_point_where_every_blended_texel_is_full_is_solid() {
        // Full but for the corner texel (0, 0), on a 4 × 4 atlas.
        let mut texels = vec![255; 16];
        texels[0] = 200;
        let atlas = UiAtlas { width: 4, height: 4, spread: 6.0, format: UiFieldFormat::Single, texels };
        // Between texels 1 and 2 each way: all four full.
        assert!(is_solid(&quad([0.5, 0.5, 0.5, 0.5]), &atlas));
        // Between texels 0 and 1 each way: the blend reaches (0, 0).
        assert!(!is_solid(&quad([0.25, 0.25, 0.25, 0.25]), &atlas));
        // Past the far edge, clamped onto full texels.
        assert!(is_solid(&quad([1.0, 1.0, 1.0, 1.0]), &atlas));
        // A quad that spans the atlas reads its edges.
        assert!(!is_solid(&quad([0.5, 0.5, 0.75, 0.5]), &atlas));
        // Anything but a single-channel field (a glyph's coverage, a display face's field, a picture),
        // an edge at the field's top, softness that reaches it, or an outline out past it.
        let read = |texels| {
            let mut quad = quad([0.5; 4]);
            quad.texels = texels;
            quad
        };
        let mut high = quad([0.5; 4]);
        high.edge = 0.99999;
        let mut soft = quad([0.5; 4]);
        soft.softness = 0.5;
        let mut outlined = quad([0.5; 4]);
        outlined.outline = -0.6;
        for quad in [read(UI_COVERAGE), read(UI_MULTI_FIELD), read(UI_PICTURE), high, soft, outlined] {
            assert!(!is_solid(&quad, &atlas), "{quad:?}");
        }
        let multi = UiAtlas { format: UiFieldFormat::Multi, texels: vec![255; 64], ..atlas };
        assert!(!is_solid(&quad([0.5; 4]), &multi), "only a single-channel atlas is read for it");
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
        let atlas =
            UiAtlas { width: 1, height: 1, spread: 6.0, format: UiFieldFormat::Single, texels: vec![255] };
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

    /// How a test finds a run's texture: pages under 2 and the first field exist.
    fn known(source: UiSource) -> Option<UiTexture> {
        match source {
            UiSource::Page(page) => (page < 2).then_some(UiTexture::Page(page)),
            UiSource::Field(at) => (at == 0).then_some(UiTexture::Field(7)),
        }
    }

    #[test]
    fn runs_of_a_page_or_a_field_split_the_atlas_s_quads_in_order() {
        use UiTexture::{Atlas, Field, Page};

        let quad = UiQuad::new([0.0; 2], [1.0, 0.0], [0.0, 1.0], [0.0; 4], [255; 4]);
        let list = UiDrawList {
            quads: vec![quad; 12],
            runs: vec![
                UiRun { first: 2, count: 3, source: UiSource::Page(0) },
                UiRun { first: 5, count: 1, source: UiSource::Page(1) },
                UiRun { first: 7, count: 2, source: UiSource::Field(0) },
                UiRun { first: 9, count: 1, source: UiSource::Field(1) },
            ],
            ..UiDrawList::default()
        };
        let resolved = |limit, has_atlas| {
            let mut draws = Vec::new();
            resolve_draws(&list, limit, has_atlas, known, &mut draws);
            draws.iter().map(|d| (d.first, d.count, d.texture)).collect::<Vec<_>>()
        };
        assert_eq!(
            resolved(12, true),
            [
                (0, 2, Atlas),
                (2, 3, Page(0)),
                (5, 1, Page(1)),
                (6, 1, Atlas),
                (7, 2, Field(7)),
                (10, 2, Atlas)
            ],
            "a field that couldn't be uploaded draws nothing, and no other quad in its place",
        );
        assert_eq!(
            resolved(12, false),
            [(2, 3, Page(0)), (5, 1, Page(1)), (7, 2, Field(7))],
            "no atlas, no HUD quads"
        );
        assert_eq!(resolved(3, true), [(0, 2, Atlas), (2, 1, Page(0))], "a run cut at the buffer's limit");
    }

    #[test]
    fn overflowing_trailing_page_range_does_not_duplicate_or_hide_valid_quads() {
        use UiTexture::{Atlas, Page};

        let quad = UiQuad::new([0.0; 2], [1.0, 0.0], [0.0, 1.0], [0.0; 4], [255; 4]);
        let list = UiDrawList {
            quads: vec![quad; 10],
            runs: vec![
                UiRun { first: 2, count: 3, source: UiSource::Page(0) },
                UiRun { first: u32::MAX, count: 1, source: UiSource::Page(1) },
            ],
            ..UiDrawList::default()
        };
        let mut draws = Vec::new();
        resolve_draws(&list, 10, true, known, &mut draws);
        assert_eq!(
            draws.iter().map(|draw| (draw.first, draw.count, draw.texture)).collect::<Vec<_>>(),
            [(0, 2, Atlas), (2, 3, Page(0)), (5, 5, Atlas)]
        );
        draws.clear();
        resolve_draws(&list, 4, true, known, &mut draws);
        assert_eq!(
            draws.iter().map(|draw| (draw.first, draw.count, draw.texture)).collect::<Vec<_>>(),
            [(0, 2, Atlas), (2, 2, Page(0))]
        );
    }

    #[test]
    fn a_mask_splits_a_run_and_outlasts_quads_that_don_t_draw() {
        use UiTexture::{Atlas, Page};

        let quad = UiQuad::new([0.0; 2], [1.0, 0.0], [0.0, 1.0], [0.0; 4], [255; 4]);
        let mask = Some([1.0, 2.0, 3.0, 4.0]);
        let list = UiDrawList {
            quads: vec![quad; 10],
            runs: vec![UiRun { first: 2, count: 4, source: UiSource::Page(0) }],
            masks: vec![UiMask { first: 1, rect: mask }, UiMask { first: 4, rect: None }],
            ..UiDrawList::default()
        };
        let resolved = |has_atlas| {
            let mut draws = Vec::new();
            resolve_draws(&list, 10, has_atlas, known, &mut draws);
            draws.iter().map(|d| (d.first, d.count, d.texture, d.mask)).collect::<Vec<_>>()
        };
        assert_eq!(
            resolved(true),
            [
                (0, 1, Atlas, None),
                (1, 1, Atlas, mask),
                (2, 2, Page(0), mask),
                (4, 2, Page(0), None),
                (6, 4, Atlas, None),
            ],
        );
        assert_eq!(
            resolved(false),
            [(2, 2, Page(0), mask), (4, 2, Page(0), None)],
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
    fn inset_runs_read_their_pictures_between_pages_fields_and_the_atlas() {
        use UiTexture::{Atlas, Field, Inset, Page};
        let quad = UiQuad::new([0.0; 2], [1.0, 0.0], [0.0, 1.0], [0.0; 4], [255; 4]);
        let list = UiDrawList {
            quads: vec![quad; 10],
            runs: vec![
                UiRun { first: 1, count: 2, source: UiSource::Page(0) },
                UiRun { first: 6, count: 1, source: UiSource::Field(0) },
                UiRun { first: 8, count: 1, source: UiSource::Page(5) },
            ],
            inset_runs: vec![UiInsetRun { first: 3, count: 2, inset: 9 }],
            ..UiDrawList::default()
        };
        let mut draws = Vec::new();
        resolve_draws(&list, 10, true, known, &mut draws);
        let resolved: Vec<_> = draws.iter().map(|d| (d.first, d.count, d.texture)).collect();
        assert_eq!(
            resolved,
            [
                (0, 1, Atlas),
                (1, 2, Page(0)),
                (3, 2, Inset(9)),
                (5, 1, Atlas),
                (6, 1, Field(7)),
                (7, 1, Atlas),
                (9, 1, Atlas),
            ],
            "a page with nothing on the GPU draws nothing",
        );
    }

    #[test]
    fn a_mask_becomes_whole_pixels_inside_the_target() {
        let viewport = [100.0, 50.0];
        let size = (200, 100);
        assert_eq!(scissor_of([10.0, 5.0, 20.0, 15.0], viewport, size), [20, 10, 20, 20]);
        assert_eq!(scissor_of([-10.0, -5.0, 500.0, 500.0], viewport, size), [0, 0, 200, 100]);
        assert_eq!(
            scissor_of([20.0, 5.0, 10.0, 15.0], viewport, size),
            [40, 10, 0, 20],
            "inside out is empty"
        );
        assert_eq!(scissor_of([900.0, 900.0, 950.0, 950.0], viewport, size), [200, 100, 0, 0]);
    }
}
