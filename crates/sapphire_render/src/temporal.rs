//! Temporal anti-aliasing and upsampling (M1 R7; spec §5.12 "Anti-aliasing and upscaling", look
//! pillar 6: clean, stable edges). Anime frames have two to three times the edge pixels of
//! realistic ones (hard light splits, inverted-hull outlines), so AA carries more of the look,
//! and softening those edges would lose it.
//!
//! # The frame
//!
//! With [`RenderSettings::temporal`](crate::RenderSettings::temporal) on:
//!
//! 1. **Jitter.** Every frame moves the projection by a sub-pixel offset from a Halton(2, 3)
//!    sequence ([`jitter`]), for every pass that draws geometry: the view's matrices carry it
//!    (`ViewUniforms`), so the prepasses, the main pass, its outlines and the sky all rasterize
//!    the same samples. The sequence has [`jitter_phases`] entries, more when the scene is
//!    upsampled (8 × the area ratio, as FSR 2 does), and follows the frame count, so it is the
//!    same in every run.
//! 2. **Velocity stays unjittered.** The scene prepass (M1 R2) compares unjittered clip
//!    positions, so geometry that holds still has exactly zero velocity whatever the jitter. The
//!    scene prepass's depth and velocity, and the character prepass's depth, are the resolve's
//!    inputs, so the frame graph runs the scene prepass under TAA (it culls it otherwise; the
//!    character prepass runs on every tier since M1 C5).
//! 3. **The responsive mask** (`responsive mask` pass, `R8Unorm` at the scene's size): each
//!    pixel's reactivity, the share of the moving history cap it gives up. Characters' (0.5)
//!    where a character (an instance the frame lists as one, since M1 C5; a skinned or morphed
//!    instance before) is the nearest surface, from the character prepass's depth against the
//!    scene prepass's; lines' (1) where an outline hull is, from the scene prepass's outline
//!    mask (M1 C4, P1); 0 elsewhere, and on a fading character's surfaces and lines, which the
//!    masked prepasses mark in the outline mask (M1 C7), so the history keeps every turn of the
//!    proximity fade's dither. That dither turns by [`fade_turn`], its own 16-frame cycle, so
//!    every pixel takes each of its sixteen thresholds once a cycle and the resolve averages
//!    them into the fade.
//! 4. **The resolve** (`taa` pass), a compute pass at the output's size, on HDR scene color
//!    before bloom and the tone curve, in a tonemap-weighted space (see `shaders/temporal.wgsl`,
//!    whose header also says how it is kept fast):
//!    - a reconstruction filter over the 3 × 3 scene pixels around each output pixel, sized in
//!      output pixels, which is also the upsample when the scene is smaller (TAAU);
//!    - variance clipping of the history in YCoCg (Salvi 2016; Playdead's clip toward the mean),
//!      fetched through a five-tap Catmull-Rom filter;
//!    - velocity dilation: a pixel follows the nearest surface of those 3 × 3 when that one is
//!      clearly nearer than its own, so silhouette edges move with the object in front;
//!    - reprojection from depth where no geometry is (the sky): the camera's motion at infinity;
//!    - lines (M1 P1): a hull pixel's velocity is its line's, the silhouette's motion as the
//!      outline prepass extrapolates it (`outline.wgsl`), not that of the hull points it shows,
//!      which slide through the silhouette when a body turns against the view. Pixels with a
//!      line in their 3 × 3 also try the camera's motion alone and none, and keep whichever
//!      reprojection's history matches this frame best (`line_motion`);
//!    - disocclusion rejection: the scene prepass's depth (hulls included) against the farthest
//!      of last frame's depths around where the pixel came from;
//!    - an accumulation weight in the history's alpha, capped lower for moving pixels (or ones
//!      resampled between texels, which thin lines don't survive) and lower again on reactive
//!      ones that move, by their reactivity, with a tighter clip box: a moving character keeps
//!      half the cap, and a moving line none, so it is drawn from the current frame.
//! 5. **Post** then blooms, grades and tone maps the resolved frame at the output's size.
//!
//! The history is a pair of `Rgba16Float` textures at the output's size, the scene prepass's
//! depth a pair at the scene's size, both persistent and imported into each frame's graph: one
//! of each pair is written while the other holds last frame's. The main pass's depth is a
//! transient again (M1 P1), discarded after the pass. The history is dropped
//! ([`HistoryReset`]) when a renderer starts (a new renderer after device loss or a tier
//! change), when the output's size changes, and on a camera cut
//! ([`ViewData::cut`](crate::ViewData::cut)). A render-scale change keeps it: the history is at
//! the output's size, so it stays valid while dynamic resolution moves the scene's, as FSR 2 and
//! DLSS keep theirs (spec §5.12, "As built (M1 R7)").
//!
//! `temporal.wgsl` is a program of its own ([`ShaderFile::Temporal`](crate::ShaderFile)): an
//! edited one rebuilds the mask's and the resolve's pipelines on the layouts they had
//! ([`Renderer::set_shader_sources`](crate::Renderer::set_shader_sources), M1 A8).
//!
//! # Inputs an external upscaler takes instead
//!
//! The resolve reads what DLSS, MetalFX temporal and FSR 2/3 take, at the scene's size: HDR
//! color, depth, velocity in texture coordinates (zero where none is: reproject from depth), the
//! responsive mask (FSR's reactive mask, DLSS's bias-current-color mask, MetalFX's
//! `reactiveMaskTexture`), the jitter in pixels, the exposure and a reset flag, plus the output
//! size (`TemporalInputs`), and nothing else (M1 P1): one of them replaces the `taa` pass behind
//! the same inputs. Outline hulls are in all of them (M1 C4): their depth, their line's velocity
//! (the silhouette's, which serves any upscaler: a hull point's would smear a turning body's
//! line in each of them) and a reactivity of 1, so an upscaler takes a moving line from the
//! current frame as the resolve does. Only `line_motion`'s search among reprojections is the
//! resolve's own; an upscaler does without it.
//!
//! # Later milestones
//!
//! - **External upscalers:** DLSS (`dlss_wgpu`, NVIDIA on Vulkan) and MetalFX temporal behind
//!   `TemporalInputs`, chosen by `GpuCaps` and the tier; FSR 3 or Arm ASR where neither runs.
//! - **Turning lines' stability:** drawn from the current frame, a line on a turning body
//!   changes from frame to frame 2.9 times as much as 4× MSAA's on the GPU tests' sphere (4.2
//!   before M1 P1), as each frame's jitter draws it anew. Reprojected exactly (with no motion,
//!   which is exact for that sphere alone) it would keep its history and change 2.0 times as
//!   much. Nothing in the frame gives a silhouette's exact motion; a closer estimate than the
//!   outline prepass's extrapolation would let moving lines keep some history.
//! - **Transparent surfaces and particles** write the responsive mask (or FSR's
//!   transparency-and-composition mask) as they draw, since they write no velocity.
//! - **The character mask through the resolve:** post (M1 P2) and the effect stack (M1 P3) read
//!   the character mask, scene color's alpha, from the scene color before the resolve, at the
//!   scene's size and jittered, since the history's alpha holds the accumulation weight: right
//!   within a pixel, which they need no better. Only an effect that needs it hard-edged at the
//!   output's size, such as M2's character cut-ins, might carry it through the resolve in a
//!   channel of its own.
//! - **Thin features in still scenes:** where a jitter phase's samples all miss a line's end,
//!   its clip box closes on the background and the pixel flickers once a cycle; FSR 2's locks
//!   protect such pixels.
//! - **Sharpening:** see spec §5.12's R7 note for why the resolve ships without RCAS.
//! - **Mobile:** TAA stays off, MSAA on tile; a spatial upscaler (EASU and RCAS, or MetalFX
//!   spatial) for phones' internal resolutions.

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use sapphire_rhi::{TransientDesc, wgpu};

use crate::bind_cache::CachedBindGroup;
use crate::graph::{FrameGraph, PassContext, TextureId};
use crate::pipelines::{DEPTH_FORMAT, SCENE_COLOR_FORMAT};
use crate::shaders::{self, Program, ShaderSources};

/// The frame graph passes temporal AA adds, the responsive mask's and the resolve's: what the
/// overlay and the bench sum for its GPU time against spec §3.3's budget (0.8 ms on desktop).
pub const PASSES: [&str; 2] = [MASK_PASS, RESOLVE_PASS];
const MASK_PASS: &str = "responsive mask";
const RESOLVE_PASS: &str = "taa";

/// The responsive mask's format: one byte, each pixel's reactivity (FSR's reactive masks are
/// fractional too): 0, characters' or lines' (see the module docs).
pub(crate) const RESPONSIVE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// The history's format: the scene color's, so post reads it as it read scene color.
const HISTORY_FORMAT: wgpu::TextureFormat = SCENE_COLOR_FORMAT;

/// The resolve's workgroups are `RESOLVE_GROUP` × `RESOLVE_GROUP` output pixels (WGSL `GROUP`).
const RESOLVE_GROUP: u32 = 8;

/// The tuning of the resolve. All are Proposals, set against 4× MSAA on the sandbox character
/// and by the GPU tests; spec §5.12's R7 note has the measurements.
mod tuning {
    /// The reconstruction filter, `exp(-k d²)` with `d` in output pixels. Frames count by how
    /// close their samples land, so the converged image is close to this Gaussian over the
    /// scene. k = 4 (σ ≈ 0.35 px) converged nearest a 64-sample reference of the GPU tests and
    /// upsampled best; Unreal's Blackman-Harris fit (k = 2.29, σ ≈ 0.47 px) visibly softened
    /// outlines and light splits, and sharper ones (6, 9) collect too little of each frame to
    /// converge as far.
    pub const FILTER_SHARPNESS: f32 = 4.0;
    /// The history's weight cap in frames' worth of samples, for pixels that hold still: four
    /// jitter cycles. A history starts as a running average after any reset, so the cap costs
    /// no convergence; at 16 the newest of eight phases still moved a converged edge by a few
    /// steps every frame (about 1,000 of 129,600 pixels by more than 2 in the stability test,
    /// against about 130 at 32).
    pub const HISTORY_STILL: f32 = 32.0;
    /// The cap for pixels moving `MOVING_SPEED` output pixels a frame or faster, or by half a
    /// pixel (see the resolve's resampling loss).
    pub const HISTORY_MOVING: f32 = 4.0;
    pub const MOVING_SPEED: f32 = 2.0;
    /// Responsive pixels (characters) moving that fast keep this fraction of the cap: the
    /// responsive mask holds the rest on them, their reactivity.
    pub const RESPONSIVE_HISTORY: f32 = 0.5;
    /// Line pixels (an outline hull's, or beside one) moving that fast keep this fraction: none,
    /// so a moving line is drawn from the current frame (M1 P1), as Blue Protocol keeps TAA's
    /// blur off its lines. On the sandbox character turning at 3 rad/s, against 4× MSAA, this
    /// alone raised the lines' strength from 81% (head) and 83% (figure) at characters' 0.5 to
    /// 89% and 91%; with the line's velocity and `line_motion` they keep 96% and 93%. A still
    /// line keeps the full history. Spec §5.12's P1 note has the measurements. It stays below
    /// `RESPONSIVE_HISTORY`: the resolve tells lines from characters by their higher reactivity.
    pub const OUTLINE_HISTORY: f32 = 0.0;
    const _: () = assert!(OUTLINE_HISTORY < RESPONSIVE_HISTORY);
    /// The clip box's half-width in standard deviations. Narrower boxes clip one-pixel outlines
    /// toward their brighter neighbors: at 1.0 they lost a visible share of their darkness
    /// against 4× MSAA.
    pub const CLIP_GAMMA: f32 = 1.5;
    /// On responsive pixels moving `MOVING_SPEED` or faster.
    pub const RESPONSIVE_CLIP_GAMMA: f32 = 1.0;
    /// How much nearer, as a fraction of its distance, a surface must be to count as another:
    /// to lend a neighbor its motion (velocity dilation), or to make a pixel it covered last
    /// frame disoccluded. Wide enough for one surface's own curvature across a few pixels and
    /// for objects' motion in depth between frames.
    pub const DEPTH_MARGIN: f32 = 0.1;
}

/// The responsive mask's value on characters and on lines: the share of the moving cap each
/// gives up.
const CHARACTER_REACTIVITY: f32 = 1.0 - tuning::RESPONSIVE_HISTORY;
const OUTLINE_REACTIVITY: f32 = 1.0 - tuning::OUTLINE_HISTORY;

/// Jitter phases at native resolution; upsampling multiplies them by the area ratio.
const BASE_PHASES: f32 = 8.0;
/// The longest jitter sequence.
const MAX_PHASES: u32 = 64;

/// The radical inverse of `index` in `base`: the Halton sequence's `index`th element, in [0, 1).
fn halton(mut index: u32, base: u32) -> f32 {
    let (mut fraction, mut value) = (1.0, 0.0);
    while index > 0 {
        fraction /= base as f32;
        value += fraction * (index % base) as f32;
        index /= base;
    }
    value
}

/// The jitter of frame `frame` in a sequence of `phases`, in scene pixels in [-0.5, 0.5), x right
/// and y down: Halton(2, 3) from its second element (the first is 0, 0).
pub fn jitter(frame: u64, phases: u32) -> [f32; 2] {
    let index = (frame % u64::from(phases.max(1))) as u32 + 1;
    [halton(index, 2) - 0.5, halton(index, 3) - 0.5]
}

/// How many jitter phases a scene `input_height` pixels high needs for an output `output_height`
/// high: 8 at native resolution, times the area ratio when upsampling (FSR 2's rule), so each
/// output pixel still gets samples near its center within one sequence; at most 64.
pub fn jitter_phases(input_height: u32, output_height: u32) -> u32 {
    let ratio = output_height as f32 / input_height.max(1) as f32;
    ((BASE_PHASES * ratio * ratio).ceil() as u32).clamp(BASE_PHASES as u32, MAX_PHASES)
}

/// `previous_clip_from_world × clip_from_world⁻¹`: this frame's unjittered clip space to the last
/// frame's, computed in f64 (camera translations far from the origin lose f32 precision in the
/// product). Exactly the identity when the camera held still, so still pixels reproject to
/// themselves.
pub(crate) fn previous_clip_from_clip(previous_clip_from_world: Mat4, clip_from_world: Mat4) -> Mat4 {
    if previous_clip_from_world == clip_from_world {
        return Mat4::IDENTITY;
    }
    (previous_clip_from_world.as_dmat4() * clip_from_world.as_dmat4().inverse()).as_mat4()
}

/// Why the history was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryReset {
    /// The renderer's first frame: a new renderer (startup, device loss, a tier change).
    Start,
    /// The output's size changed.
    Resize,
    /// The camera cut ([`ViewData::cut`](crate::ViewData::cut)).
    Cut,
}

/// Temporal AA's counters (rule R9). The resolve's GPU time is the frame graph's `taa` pass
/// (and `responsive mask`), with F5's pass timings on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TemporalStats {
    /// Whether the renderer resolves temporally ([`RenderSettings::temporal`](crate::RenderSettings::temporal)).
    pub enabled: bool,
    /// The last recorded frame's scene (the resolve's input) and output sizes: they differ when
    /// it upsampled.
    pub input_size: (u32, u32),
    pub output_size: (u32, u32),
    /// Its jitter's position in the sequence, and the sequence's length.
    pub jitter_index: u32,
    pub jitter_phases: u32,
    /// Histories dropped since the renderer was created, the first frame's included.
    pub resets: u32,
    /// Why the last recorded frame dropped its history, if it did.
    pub reset: Option<HistoryReset>,
}

impl TemporalStats {
    /// The last recorded frame's jitter in scene pixels, x right and y down.
    pub fn jitter(&self) -> [f32; 2] {
        jitter(u64::from(self.jitter_index), self.jitter_phases)
    }
}

/// What the resolve reads (see the module docs): the inputs an external upscaler would take in
/// its place. Textures are the frame graph's, at the scene's size.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TemporalInputs {
    /// HDR scene color, linear, before bloom and grading.
    pub color: TextureId,
    /// The scene prepass's depth (reverse-Z).
    pub depth: TextureId,
    /// The scene prepass's velocity, in texture coordinates; 0 where no geometry is.
    pub velocity: TextureId,
    /// The responsive mask: each pixel's reactivity, characters' where a character is the
    /// nearest surface and lines' where an outline hull is.
    pub responsive: TextureId,
}

/// What the responsive mask is made from, at the scene's size.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TemporalMasks {
    /// The character prepass's depth: characters alone.
    pub characters: TextureId,
    /// The scene prepass's outline mask: 1 where an outline hull is the nearest surface.
    pub outlines: TextureId,
}

/// The persistent textures one frame's graph imports (see [`TemporalTextures::import`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct TemporalTargets {
    /// This frame's kept depth, which the next frame reads as its last: the scene prepass's
    /// depth target, outline hulls included.
    pub depth: TextureId,
    /// Last frame's kept depth (this frame's after a reset, unread then).
    previous_depth: TextureId,
    /// Last frame's resolved frame.
    history: TextureId,
    /// This frame's resolved frame, which post reads and the next frame takes as its history.
    pub output: TextureId,
}

/// The resolve's constants (WGSL `TemporalUniforms` in `shaders/temporal.wgsl`, group 0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct TemporalUniforms {
    /// xy: the scene's size in pixels; zw: reciprocals.
    pub input_size: [f32; 4],
    /// xy: the output's size in pixels; zw: reciprocals.
    pub output_size: [f32; 4],
    /// xy: the jitter in scene pixels; z: the filter's sharpness; w: the depth margin.
    pub jitter: [f32; 4],
    /// This frame's unjittered clip space to the last frame's.
    pub previous_clip_from_clip: [[f32; 4]; 4],
    /// x: exposure; y: 1 drops the history; z, w: the history's cap still and moving.
    pub history: [f32; 4],
    /// x: the moving cap's speed; y: characters' reactivity; z, w: clip γ, and responsive.
    pub response: [f32; 4],
    /// x: lines' reactivity; y: the reactivity above which a pixel is a line's; z: 1 when the
    /// camera moved.
    pub outlines: [f32; 4],
}

fn size_and_reciprocal((width, height): (u32, u32)) -> [f32; 4] {
    let (width, height) = (width as f32, height as f32);
    [width, height, width.recip(), height.recip()]
}

/// A texture kept across frames.
#[derive(Debug)]
struct Persistent {
    /// Kept so the texture lives as long as its view.
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

impl Persistent {
    /// A texture its pass writes as `usage` (a storage texture or an attachment), which later
    /// passes sample and tools and tests can copy.
    fn new(
        device: &wgpu::Device,
        label: &'static str,
        (width, height): (u32, u32),
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            // Copyable, as the prepass targets are.
            usage: usage | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { _texture: texture, view, size: (width, height) }
    }
}

/// The history and kept-depth pairs.
#[derive(Debug, Default)]
pub(crate) struct TemporalTextures {
    /// Resolved frames, at the output's size.
    history: [Option<Persistent>; 2],
    /// Depths at the scene's size ([`TemporalTargets::depth`]).
    depth: [Option<Persistent>; 2],
    /// Which of each pair this frame writes; the other holds last frame's.
    write: usize,
    /// Whether the other of each pair holds the last recorded frame's.
    previous: bool,
}

impl TemporalTextures {
    /// Imports this frame's textures into `graph`.
    pub(crate) fn import<'f>(&'f self, graph: &mut FrameGraph<'f>, reset: bool) -> TemporalTargets {
        let read = 1 - self.write;
        let view = |pair: &'f [Option<Persistent>; 2], slot: usize| {
            &pair[slot].as_ref().expect("`begin` makes this frame's textures").view
        };
        let depth = graph.import_texture("kept depth", view(&self.depth, self.write));
        // After a reset nothing reads last frame's depth; this frame's stands in for the binding.
        let previous_depth = match &self.depth[read] {
            Some(previous) if !reset => graph.import_texture("previous kept depth", &previous.view),
            _ => depth,
        };
        TemporalTargets {
            depth,
            previous_depth,
            history: graph.import_texture("history", view(&self.history, read)),
            output: graph.import_texture("resolved", view(&self.history, self.write)),
        }
    }
}

/// The resolve's pipelines, layouts and constants.
#[derive(Debug)]
pub(crate) struct TemporalPasses {
    device: wgpu::Device,
    cache: Option<wgpu::PipelineCache>,
    /// The mask's and the resolve's, which rebuilt pipelines keep.
    pipeline_layouts: [wgpu::PipelineLayout; 2],
    pipelines: TemporalPipelines,
    mask_layout: wgpu::BindGroupLayout,
    resolve_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    /// Group 0: the constants and the sampler.
    shared: wgpu::BindGroup,
    /// Group 1 of each pass, one per write slot: the persistent textures alternate.
    mask_groups: [CachedBindGroup<3>; 2],
    resolve_groups: [CachedBindGroup<7>; 2],
    /// What the last `begin` computed for the uploads.
    uniforms: TemporalUniforms,
}

/// The responsive mask's and the resolve's pipelines, from the `temporal` program.
#[derive(Debug)]
pub(crate) struct TemporalPipelines {
    mask: wgpu::RenderPipeline,
    resolve: wgpu::ComputePipeline,
}

impl TemporalPipelines {
    /// How many there are, for [`Renderer::set_shader_sources`](crate::Renderer::set_shader_sources)'s count.
    pub(crate) const COUNT: u32 = 2;

    /// Both, from `module` on `layouts` (the mask's, then the resolve's).
    fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        layouts: &[wgpu::PipelineLayout; 2],
        cache: Option<&wgpu::PipelineCache>,
    ) -> Self {
        let mask = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(MASK_PASS),
            layout: Some(&layouts[0]),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_fullscreen"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_responsive_mask"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: RESPONSIVE_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache,
        });
        let resolve = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(RESOLVE_PASS),
            layout: Some(&layouts[1]),
            module,
            entry_point: Some("cs_temporal_resolve"),
            // Every thread writes its share of the tile before any reads it, so zeroing it first
            // is wasted work: one thread per workgroup clears it while the rest wait.
            compilation_options: wgpu::PipelineCompilationOptions {
                zero_initialize_workgroup_memory: false,
                ..Default::default()
            },
            cache,
        });
        Self { mask, resolve }
    }
}

/// Temporal AA's persistent state (see the module docs).
#[derive(Debug)]
pub(crate) struct Temporal {
    textures: TemporalTextures,
    passes: TemporalPasses,
    /// Frames begun since the renderer was created: the jitter's index.
    frames: u64,
    stats: TemporalStats,
}

/// What [`Temporal::begin`] decided for a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TemporalFrame {
    /// The projection's jitter in scene pixels, x right and y down.
    pub jitter: [f32; 2],
    /// The proximity fade's dither turn ([`fade_turn`]).
    pub fade_turn: f32,
    pub reset: Option<HistoryReset>,
}

/// Frames in the proximity fade's dither cycle (M1 C7): one per threshold of its 4 × 4 matrix.
pub const FADE_TURNS: u32 = 16;

/// The proximity fade's dither turn in `frame` (M1 C7, `fade_threshold` in `frame.wgsl`): the
/// radical inverse of the frame in a [`FADE_TURNS`]-frame cycle, so over a cycle every pixel
/// takes each of the matrix's sixteen thresholds once, in an order that spreads them evenly, and
/// the history blends them into the fade with no pattern left. Its own cycle rather than the
/// jitter's: the jitter has eight phases at native resolution, and Halton's x over them turns
/// unevenly (its eighth is a sixteenth off), which left a faint screen door.
pub fn fade_turn(frame: u64) -> f32 {
    halton((frame % u64::from(FADE_TURNS)) as u32, 2)
}

impl Temporal {
    /// Temporal AA with its pipelines built from `sources`.
    pub(crate) fn new(
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        sources: &ShaderSources,
    ) -> Self {
        Self {
            textures: TemporalTextures::default(),
            passes: TemporalPasses::new(device, cache, sources),
            frames: 0,
            stats: TemporalStats { enabled: true, ..TemporalStats::default() },
        }
    }

    /// Its pipelines built again from `sources` (see [`TemporalPasses::rebuilt_pipelines`]).
    pub(crate) fn rebuilt_pipelines(&self, sources: &ShaderSources) -> TemporalPipelines {
        self.passes.rebuilt_pipelines(sources)
    }

    pub(crate) fn set_pipelines(&mut self, pipelines: TemporalPipelines) {
        self.passes.pipelines = pipelines;
    }

    /// Starts a frame whose scene is `input` pixels and output `output`: picks the jitter, makes
    /// the persistent textures fit, decides whether the history carries over, and computes the
    /// constants. `cut` is the view's camera cut; `exposure` post's multiplier;
    /// `previous_clip_from_clip` from [`previous_clip_from_clip`].
    pub(crate) fn begin(
        &mut self,
        input: (u32, u32),
        output: (u32, u32),
        cut: bool,
        exposure: f32,
        previous_clip_from_clip: Mat4,
    ) -> TemporalFrame {
        let device = &self.passes.device;
        let textures = &mut self.textures;
        textures.write = 1 - textures.write;
        let resized =
            textures.history.iter().any(|history| history.as_ref().is_none_or(|h| h.size != output));
        if resized {
            let usage = wgpu::TextureUsages::STORAGE_BINDING;
            textures.history = std::array::from_fn(|_| {
                Some(Persistent::new(device, "history", output, HISTORY_FORMAT, usage))
            });
        }
        let depth = &mut textures.depth[textures.write];
        if depth.as_ref().is_none_or(|depth| depth.size != input) {
            let usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
            *depth = Some(Persistent::new(device, "kept depth", input, DEPTH_FORMAT, usage));
        }
        let reset = history_reset(textures.previous, resized, cut);
        textures.previous = true;

        let phases = jitter_phases(input.1, output.1);
        let index = (self.frames % u64::from(phases)) as u32;
        let jitter = jitter(self.frames, phases);
        let fade_turn = fade_turn(self.frames);
        self.frames += 1;

        self.passes.uniforms = TemporalUniforms {
            input_size: size_and_reciprocal(input),
            output_size: size_and_reciprocal(output),
            jitter: [jitter[0], jitter[1], tuning::FILTER_SHARPNESS, tuning::DEPTH_MARGIN],
            previous_clip_from_clip: previous_clip_from_clip.to_cols_array_2d(),
            history: [
                exposure,
                if reset.is_some() { 1.0 } else { 0.0 },
                tuning::HISTORY_STILL,
                tuning::HISTORY_MOVING,
            ],
            response: [
                tuning::MOVING_SPEED,
                CHARACTER_REACTIVITY,
                tuning::CLIP_GAMMA,
                tuning::RESPONSIVE_CLIP_GAMMA,
            ],
            outlines: [
                OUTLINE_REACTIVITY,
                0.5 * (CHARACTER_REACTIVITY + OUTLINE_REACTIVITY),
                if previous_clip_from_clip == Mat4::IDENTITY { 0.0 } else { 1.0 },
                0.0,
            ],
        };
        self.stats = TemporalStats {
            enabled: true,
            input_size: input,
            output_size: output,
            jitter_index: index,
            jitter_phases: phases,
            resets: self.stats.resets + u32::from(reset.is_some()),
            reset,
        };
        TemporalFrame { jitter, fade_turn, reset }
    }

    /// The textures this frame imports, and the passes it records, borrowed apart.
    pub(crate) fn split_for_record(&mut self) -> (&TemporalTextures, &mut TemporalPasses) {
        (&self.textures, &mut self.passes)
    }

    /// The history the last recorded frame wrote, for tests that read its accumulation weight.
    #[cfg(test)]
    pub(crate) fn history(&self) -> &wgpu::Texture {
        &self.textures.history[self.textures.write].as_ref().expect("a frame was recorded")._texture
    }

    pub(crate) fn stats(&self) -> TemporalStats {
        self.stats
    }
}

/// Whether, and why, a frame drops its history: with no previous frame, at a new output size or
/// on a camera cut.
fn history_reset(previous: bool, resized: bool, cut: bool) -> Option<HistoryReset> {
    if !previous {
        Some(HistoryReset::Start)
    } else if resized {
        Some(HistoryReset::Resize)
    } else if cut {
        Some(HistoryReset::Cut)
    } else {
        None
    }
}

impl TemporalPasses {
    fn new(device: &wgpu::Device, cache: Option<&wgpu::PipelineCache>, sources: &ShaderSources) -> Self {
        use wgpu::ShaderStages as S;
        let entry =
            |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
        let texture = |binding, visibility, sample_type| {
            let ty = wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            };
            entry(binding, visibility, ty)
        };
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
        };
        let depth = wgpu::TextureSampleType::Depth;
        // Read through the nearest sampler (or `textureLoad`); the history alone is filtered.
        let loaded = wgpu::TextureSampleType::Float { filterable: false };
        let filtered = wgpu::TextureSampleType::Float { filterable: true };
        let both = S::FRAGMENT | S::COMPUTE;
        let uniforms = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(size_of::<TemporalUniforms>() as u64),
        };
        let linear = wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering);
        let nearest = wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering);
        let shared_layout = layout(
            "temporal",
            &[entry(0, both, uniforms), entry(1, both, linear), entry(2, S::COMPUTE, nearest)],
        );
        let mask_layout = layout(
            MASK_PASS,
            &[
                texture(0, S::FRAGMENT, depth),
                texture(1, S::FRAGMENT, depth),
                texture(2, S::FRAGMENT, loaded),
            ],
        );
        let resolve_layout = layout(
            RESOLVE_PASS,
            &[
                texture(0, S::COMPUTE, loaded),
                texture(1, S::COMPUTE, depth),
                texture(2, S::COMPUTE, loaded),
                texture(3, S::COMPUTE, loaded),
                texture(4, S::COMPUTE, filtered),
                texture(5, S::COMPUTE, depth),
                entry(
                    6,
                    S::COMPUTE,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: HISTORY_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                ),
            ],
        );
        let pipeline_layout = |name: &str, layout: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(name),
                bind_group_layouts: &[Some(&shared_layout), Some(layout)],
                immediate_size: 0,
            })
        };
        let pipeline_layouts =
            [pipeline_layout(MASK_PASS, &mask_layout), pipeline_layout(RESOLVE_PASS, &resolve_layout)];
        let module = shaders::module(device, Program::Temporal, sources);
        let pipelines = TemporalPipelines::new(device, &module, &pipeline_layouts, cache);
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("temporal uniforms"),
            size: size_of::<TemporalUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Both clamp to the edge (the default address mode).
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("temporal linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let nearest = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("temporal nearest clamp"),
            ..Default::default()
        });
        let shared = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("temporal"),
            layout: &shared_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&linear) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&nearest) },
            ],
        });
        Self {
            device: device.clone(),
            cache: cache.cloned(),
            pipeline_layouts,
            pipelines,
            mask_layout,
            resolve_layout,
            uniform_buffer,
            shared,
            mask_groups: Default::default(),
            resolve_groups: Default::default(),
            uniforms: TemporalUniforms::default(),
        }
    }

    /// The pipelines built again from `sources`, on the layouts they had, for
    /// [`Renderer::set_shader_sources`](crate::Renderer::set_shader_sources) to keep if every
    /// rebuilt pipeline is valid.
    fn rebuilt_pipelines(&self, sources: &ShaderSources) -> TemporalPipelines {
        let module = shaders::module(&self.device, Program::Temporal, sources);
        TemporalPipelines::new(&self.device, &module, &self.pipeline_layouts, self.cache.as_ref())
    }

    /// The constants for this frame, and the buffer the frame's uploads write them to.
    pub(crate) fn uniforms(&self) -> (TemporalUniforms, &wgpu::Buffer) {
        (self.uniforms, &self.uniform_buffer)
    }

    /// Adds the frame's two passes: the responsive mask (`inputs.responsive`, from the
    /// character prepass's depth against the scene prepass's, and the outline mask: `masks`),
    /// then the resolve, a compute pass,
    /// from `inputs` and last frame's history and depth into `targets.output`. `slot` is the
    /// frame's write slot ([`TemporalTextures::write_slot`]), which picks the bind groups for
    /// its textures.
    pub(crate) fn record<'f>(
        &'f mut self,
        graph: &mut FrameGraph<'f>,
        masks: TemporalMasks,
        inputs: TemporalInputs,
        targets: TemporalTargets,
        slot: usize,
        labels: bool,
    ) {
        let Self {
            ref device,
            pipelines: TemporalPipelines { mask: ref mask_pipeline, resolve: ref resolve_pipeline },
            ref mask_layout,
            ref resolve_layout,
            ref shared,
            ref mut mask_groups,
            ref mut resolve_groups,
            ..
        } = *self;

        let mask_group = &mut mask_groups[slot];
        let (depth, mask) = (inputs.depth, inputs.responsive);
        let TemporalMasks { characters, outlines } = masks;
        let pass = graph.add_pass(MASK_PASS).read(characters).read(depth).read(outlines);
        pass.write(mask).record(move |ctx| {
            let label = labels.then_some(MASK_PASS);
            let views = [ctx.view(characters), ctx.view(depth), ctx.view(outlines)];
            let group = mask_group.get(device, label, mask_layout, &views);
            draw_fullscreen(ctx, label, mask, mask_pipeline, shared, group);
        });

        let resolve_group = &mut resolve_groups[slot];
        let reads = [
            inputs.color,
            inputs.depth,
            inputs.velocity,
            inputs.responsive,
            targets.history,
            targets.previous_depth,
        ];
        let (width, height) = graph.size(targets.output);
        let mut builder = graph.add_pass(RESOLVE_PASS);
        for texture in reads {
            builder = builder.read(texture);
        }
        builder.write(targets.output).record(move |ctx| {
            let label = labels.then_some(RESOLVE_PASS);
            let [color, depth, velocity, responsive, history, previous_depth] =
                reads.map(|texture| ctx.view(texture));
            let views =
                [color, depth, velocity, responsive, history, previous_depth, ctx.view(targets.output)];
            let group = resolve_group.get(device, label, resolve_layout, &views);
            let mut pass = ctx.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label,
                timestamp_writes: ctx.compute_timestamp_writes(),
            });
            pass.set_pipeline(resolve_pipeline);
            pass.set_bind_group(0, shared, &[]);
            pass.set_bind_group(1, group, &[]);
            pass.dispatch_workgroups(width.div_ceil(RESOLVE_GROUP), height.div_ceil(RESOLVE_GROUP), 1);
        });
    }
}

impl TemporalTextures {
    /// The slot of each pair this frame writes.
    pub(crate) fn write_slot(&self) -> usize {
        self.write
    }
}

/// The responsive mask's transient at the scene's `size`.
pub(crate) fn responsive_desc((width, height): (u32, u32)) -> TransientDesc {
    TransientDesc {
        width,
        height,
        format: RESPONSIVE_FORMAT,
        sample_count: 1,
        // Copyable like the prepass targets, so tests can read it back.
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
    }
}

/// Draws one fullscreen triangle into `output`, every pixel of which it writes.
fn draw_fullscreen(
    ctx: &mut PassContext<'_>,
    label: Option<&str>,
    output: TextureId,
    pipeline: &wgpu::RenderPipeline,
    shared: &wgpu::BindGroup,
    group: &wgpu::BindGroup,
) {
    let timestamp_writes = ctx.timestamp_writes();
    let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: ctx.view(output),
            depth_slice: None,
            resolve_target: None,
            // Every pixel is drawn; clearing is still the cheapest load on tilers.
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        timestamp_writes,
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, shared, &[]);
    pass.set_bind_group(1, group, &[]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod gpu_tests;

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::CameraData;

    #[test]
    fn the_fade_turns_through_every_sixteenth_once_a_cycle() {
        let turns: Vec<f32> = (0..u64::from(FADE_TURNS)).map(fade_turn).collect();
        let mut sixteenths: Vec<u32> = turns.iter().map(|turn| (turn * 16.0) as u32).collect();
        assert!(turns.iter().all(|turn| (turn * 16.0).fract() == 0.0), "{turns:?}");
        sixteenths.sort_unstable();
        assert_eq!(sixteenths, (0..16).collect::<Vec<_>>());
        // Spread: any two frames in a row are half a turn apart, any four a quarter's spacing.
        assert_eq!(turns[..4], [0.0, 0.5, 0.25, 0.75]);
        assert_eq!(fade_turn(u64::from(FADE_TURNS) + 3), turns[3], "a cycle");
    }

    #[test]
    fn jitter_follows_halton_2_3_from_its_second_element() {
        assert_eq!(halton(1, 2), 0.5);
        assert_eq!(halton(6, 2), 0.375);
        assert!((halton(5, 3) - 7.0 / 9.0).abs() < 1e-6);
        assert_eq!(jitter(0, 8), [0.0, 1.0 / 3.0 - 0.5]);
        assert_eq!(jitter(1, 8), [-0.25, 2.0 / 3.0 - 0.5]);
        assert_eq!(jitter(8, 8), jitter(0, 8), "the sequence repeats");
        // Every phase is distinct and inside the pixel.
        for phases in [8, 18, 64] {
            let all: Vec<[f32; 2]> = (0..u64::from(phases)).map(|frame| jitter(frame, phases)).collect();
            for (i, a) in all.iter().enumerate() {
                assert!(a.iter().all(|v| (-0.5..0.5).contains(v)), "{a:?}");
                assert!(all[..i].iter().all(|b| a != b), "phase {i} of {phases} repeats");
            }
            // Its average sits within a sixteenth of a pixel of the center, so the converged
            // image is where the unjittered one would be.
            let mean = all.iter().fold([0.0; 2], |sum, j| [sum[0] + j[0], sum[1] + j[1]]);
            for axis in mean {
                assert!((axis / phases as f32).abs() < 1.0 / 16.0, "{phases}: {mean:?}");
            }
        }
    }

    #[test]
    fn upsampling_lengthens_the_sequence_by_the_area_ratio() {
        assert_eq!(jitter_phases(1080, 1080), 8, "native");
        assert_eq!(jitter_phases(1440, 2160), 18, "1.5× as FSR 2's rule gives");
        assert_eq!(jitter_phases(1080, 2160), 32);
        assert_eq!(jitter_phases(270, 2160), MAX_PHASES, "capped");
        assert_eq!(jitter_phases(0, 2160), MAX_PHASES);
        assert_eq!(jitter_phases(2160, 1080), 8, "never below native's");
    }

    #[test]
    fn a_still_camera_reprojects_to_itself_exactly() {
        let camera = CameraData::look_at(
            Vec3::new(1000.0, 2.0, 5.0),
            Vec3::new(1000.0, 1.0, 0.0),
            Vec3::Y,
            0.8,
            1.7,
            0.05,
        );
        let clip_from_world = camera.proj * camera.view;
        assert_eq!(previous_clip_from_clip(clip_from_world, clip_from_world), Mat4::IDENTITY);
        // A moved camera maps a point's clip position now to where it was.
        let moved = CameraData::look_at(
            Vec3::new(1000.2, 2.0, 5.0),
            Vec3::new(1000.0, 1.0, 0.0),
            Vec3::Y,
            0.8,
            1.7,
            0.05,
        );
        let previous = moved.proj * moved.view;
        let reproject = previous_clip_from_clip(previous, clip_from_world);
        let point = Vec3::new(1000.3, 1.2, -0.5).extend(1.0);
        let (now, then) = (clip_from_world * point, previous * point);
        let mapped = reproject * now;
        let ndc = |clip: glam::Vec4| clip.truncate() / clip.w;
        assert!((ndc(mapped) - ndc(then)).length() < 1e-5, "{:?} vs {:?}", ndc(mapped), ndc(then));
    }

    #[test]
    fn the_history_is_dropped_on_start_resize_and_cuts() {
        assert_eq!(history_reset(false, true, false), Some(HistoryReset::Start));
        assert_eq!(history_reset(true, true, true), Some(HistoryReset::Resize));
        assert_eq!(history_reset(true, false, true), Some(HistoryReset::Cut));
        assert_eq!(history_reset(true, false, false), None);
    }
}
