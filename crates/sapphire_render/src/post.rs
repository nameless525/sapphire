//! Post-processing (spec §5.12): from the HDR scene color to the output target.
//!
//! The main pass renders linear HDR light into an `Rgba16Float` scene-color transient
//! ([`SCENE_COLOR_FORMAT`](crate::pipelines::SCENE_COLOR_FORMAT)). Post then adds these frame
//! graph passes, each one fullscreen triangle into a pooled transient:
//!
//! 1. **Bloom prefilter** (½ size): exposure, a soft-knee threshold (the characters' own, M1 P2)
//!    and a Karis average.
//! 2. **Bloom down** (¼ to 1/32): dual-filter downsamples (Bjørge, SIGGRAPH 2015), a 4×4
//!    footprint from five bilinear taps.
//! 3. **Bloom up** (1/16 back to ½): tent upsamples, each replacing `scatter` of the down
//!    level of its size, so wider levels add glow while the total light stays the same.
//! 4. **Post**, the fused full-screen pass: exposure, sharpening, bloom, the lens flare, the
//!    vignette and the grading LUTs (tone curve and grade), then the diffusion glow and the
//!    gradient layers over the graded frame, in one pass writing the output target. It is the
//!    mobile path of spec §3.3, and desktop uses it too.
//!
//! [`RenderSettings::bloom_levels`](crate::RenderSettings::bloom_levels) sets the chain's
//! length (5 reaches 1/32), so quality tiers can shorten it; bloom intensity 0 skips it unless
//! the diffusion glow needs it. Bloom levels are `Rg11b10Ufloat` where the device renders and
//! filters it (half the bandwidth of `Rgba16Float`, which is the fallback).
//!
//! # Render scale (M1 R6)
//!
//! Everything before the fused pass works at the scene's size, so below render scale 1 the
//! bloom chain shrinks with it. The fused pass alone runs at the target's size: at scale 1 it
//! loads scene color texel for texel, exactly as before render scales existed; below it, a
//! uniform branch samples scene color bilinearly at the pixel's position instead, which is the
//! upsample. Bloom is sampled by position either way.
//!
//! Under temporal AA (M1 R7) post reads the resolved frame instead, which is already at the
//! target's size, so the whole chain runs there and the fused pass loads it texel for texel.
//!
//! # Grading
//!
//! The tone curve and grade are baked on the CPU into 32³ LUTs ([`lut`]), only when the
//! frame's [`PostParams`] change them: about 2.6 ms of CPU, half a millisecond of wall time
//! across the job pool (Apple M5, release). Two LUTs blend per pixel (time-of-day keys, zone
//! overrides), and the player's filter strength is baked in as a blend toward the neutral
//! grade. A LUT already baked for either grade is reused, so a key transition costs one bake.
//!
//! # The compositing layers (M1 P2; look pillars 3 and 5)
//!
//! Anime frames are composited like anime cels, and post adds the compositing stage's layers
//! ([`layers`]), all of them in the fused pass on every tier, as phones need:
//!
//! - **The diffusion glow** ([`Diffusion`]): the bloom chain's widest smooth level (the up level
//!   just above its smallest: 1/16 of the chain's input with five levels, 1/8 with Mobile-Low's
//!   four, about the same share of the frame at each tier's size) through a soft threshold of
//!   its own, `1 − exp(−intensity × light)` toward its color, screened over the graded frame's
//!   display values. It holds its light far out where bloom falls off, lifts what is dark
//!   around bright light, and never pushes past white, so it never blows out a face. It costs one
//!   bilinear tap.
//! - **The character mask** ([`CharacterBloom`]): characters bloom with their own threshold and
//!   strength, relative to the world's. The bloom prefilter tells them apart by scene color's
//!   alpha, which holds the world's share of each pixel: the anime character shader writes 0 for
//!   the instances [`FramePacket::characters`](crate::FramePacket::characters) lists, and every
//!   other pass writes 1 (the clear value, the sky, outlines, every surface that isn't a
//!   character's; transparent surfaces keep what is under them), so a world pass needs nothing
//!   new. MSAA's resolve averages it, so edges are partial. The same texture serves every tier:
//!   under temporal AA the resolve's history keeps its weight in alpha, so the prefilter reads
//!   the mask from the frame's own scene color, before the resolve, at the same coordinates (it
//!   is jittered, which the half-size bloom doesn't show). Temporal AA's responsive mask is not
//!   used: it exists only with TAA, marks skinned and morphed instances rather than the frame's
//!   characters (the sandbox's character is neither), and can't carry the fractional values C7's
//!   breathing emission will write into alpha.
//! - **Gradient layers** ([`GradientLayer`], anime compositing's "para"): up to four linear or
//!   radial ramps, each in a blend mode (normal, multiply, screen, overlay, soft light, add),
//!   composited in order over the graded frame's sRGB-encoded values, as compositing tools blend.
//!   Their opacity follows the player's filter strength.
//! - **Lens flares** ([`LensFlare`]): the default lens's elements (`layers::DEFAULT_LENS`: a
//!   ring and a streak on the sun, colored ghosts and a ring along the axis through the middle
//!   of the frame), added to the scene's light before the vignette and the tone curve, so they
//!   are graded with the frame; a pixel outside a box around an element skips it.
//!   **Occlusion is measured on the frame itself,** on every tier:
//!   the fused pass's vertex shader samples five points inside the sun's disc in the post input
//!   and divides their brightness by half the disc's own (`layers::FLARE_VISIBLE_SHARE`), so
//!   characters, the world and clouds in front of the sun dim the flare by how much of the disc
//!   they cover, and a sun off screen draws none. Mobile has no depth to read (its MSAA depth
//!   stays in tile memory and the scene prepass doesn't run), and depth would miss the clouds,
//!   which draw at the far plane. The sun's place on screen comes from the main view and the
//!   sky's disc (`SunOnScreen`), on the CPU.
//! - **The vignette's tint** ([`Vignette::color`]): the corners multiply toward a color rather
//!   than black.
//! - **Sharpening** ([`PostParams::sharpening`], the player's): RCAS (FSR 1's robust
//!   contrast-adaptive sharpening) on exposed light compressed by its luma, five taps a pixel
//!   at the output's resolution (bilinear from a smaller scene, which makes it a bilinear
//!   upscale with sharpening). Off by default: spec §5.12's R7 and P2 notes measure where it
//!   helps and where it doesn't.
//!
//! **Variants.** `post.wgsl`'s pipeline-overridable constants pick the fused pass's variant
//! (`POST_LAYERS`, `POST_SHARPEN`) and the prefilter's (`POST_CHARACTERS`) each frame, from
//! what its parameters draw (`layers::Variant`), so a frame pays for no layer it doesn't draw.
//! Every variant is built with the renderer, so a layer turning on never compiles a pipeline
//! mid-game. Parameters without layers ([`PostParams::plain`], as every timeline from before
//! them resolves to) draw through the plain variants, which are R3's shader code, so they draw
//! exactly the frames they did. The gradient layers and the flare's elements are unrolled
//! calls: wgpu bounds every WGSL loop with a counter, which keeps Metal from unrolling it, and
//! the loops cost about three times as much (temporal AA found the same).
//!
//! # Allocation
//!
//! Bind groups for the pooled textures are cached per pass and rebuilt only when the pool
//! hands that pass a different texture (a resize, a settings change), so steady frames
//! allocate nothing here beyond the frame graph's one boxed closure per pass. Each cache keeps
//! two bind groups (M1 P2): under temporal AA post's input alternates between the history's two
//! textures, which rebuilt the bind groups of every pass reading it each frame (22 allocations a
//! frame on the desktop tiers). wgpu itself
//! allocates about 13 times per render pass (measured on Metal), which is most of post's CPU
//! cost per frame (spec §5.12, "As built (M1 R3)").
//!
//! # Later milestones
//!
//! - **Bloom masks (C7):** an emissive-driven bloom mask; C7's breathing emission writes the
//!   character mask (scene color's alpha) where a character glows.
//! - **The effect stack (P3)** after the layers, with the character mask in the fused pass for
//!   its "keep characters colored" effects; outlines (C4) then write the mask too.
//! - **Lens assets:** per-camera or per-zone flare elements and textures, in place of the
//!   default lens; a moon flare; the flare's visibility smoothed over frames, and light leaks
//!   from the screen's edges keyed to the sun.
//! - **Gradient layers per zone and depth range:** zones override them as any time-of-day
//!   group; a layer applied only beyond a distance needs depth on every tier first.
//! - **Mobile-Low:** the chain starts at ½ of the scene, and the tier's render scale (M1 R6)
//!   already puts the scene at about half the output, so the prefilter covers a sixteenth of
//!   the output's pixels. Starting at ¼ of the scene as well waits for device measurements.
//! - **A sharper upsample:** without temporal AA (the mobile tiers), below render scale 1 the
//!   fused pass upsamples bilinearly, which sharpening can follow. A spatial upscaler (an
//!   FSR1-style EASU, or MetalFX spatial; spec §5.12) replaces it.
//! - **Bloom at the scene's size under TAA upsampling:** the chain now starts at half the
//!   output's size; starting it at half the scene's would save the 4K case its biggest levels,
//!   and keep the bloom's and the diffusion's share of the frame the same at 4K as at 1080p.
//! - **Exposure:** fixed per frame from `PostParams` (later, per time-of-day key); eye
//!   adaptation, if any, is clamped to ±0.5 EV.
//! - **HDR output:** when HDR surfaces arrive (spec §5.11), a second output transform that
//!   stays close to the SDR look, as Genshin's does.
//! - **MSAA resolve:** the hardware resolve averages HDR samples, which can alias where very
//!   bright pixels meet dark outlines; a tonemap-weighted custom resolve on desktop if that
//!   shows once emissives do.
//! - **A hue-stable shoulder:** keeping hue and saturation through the shoulder (AgX-style)
//!   couples the channels near white more than a 32³ table can follow: tried here, it left
//!   ripples of about two display steps along the grey axis. If the look needs it, the fused
//!   pass can evaluate that part analytically and keep the LUT for the grade.
//! - **Dithering** the 8-bit output, if dark gradients band once real content arrives (golden
//!   images render without it).

pub(crate) mod layers;
mod lut;
mod params;

#[cfg(test)]
mod gpu_tests;

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use sapphire_rhi::{GpuContext, TransientDesc, wgpu};

use crate::graph::{FrameGraph, TextureId};
use crate::pipelines::SCENE_COLOR_FORMAT;
use crate::shaders::{self, Program, ShaderSources};
use crate::{CameraData, RenderError, bind_cache};
use layers::{FLARE_ELEMENTS, FLARE_VISIBLE_SHARE, GpuFlareElement, GpuGradient, Layers, Variant};
pub(crate) use layers::{SunDisc, SunOnScreen};
use lut::{LUT_SIZE, LUT_TEXELS, Lut, LutKey, LutSlots};
pub use params::{
    BloomParams, CharacterBloom, ColorGrade, Diffusion, GradientBlend, GradientLayer, GradientShape,
    LensFlare, MAX_GRADIENT_LAYERS, PostParams, ToneCurve, Vignette,
};

/// The longest bloom chain: ½ down to 1/64 of the target.
pub const MAX_BLOOM_LEVELS: u32 = 6;

/// Grading LUTs hold sRGB-encoded display color (see [`lut`]); 10 bits keep the trilinear
/// lookup far below an 8-bit display step.
pub(crate) const LUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgb10a2Unorm;

/// Post's frame graph passes, with the bloom chain's longest: what the overlay and the bench
/// sum for its GPU time against spec §3.3's "Bloom + tonemap + LUT" row (0.7 ms on Desktop-Low,
/// 2.5 ms for Mobile-Low's one fused pass and its bloom).
pub const PASSES: [&str; 2 * MAX_BLOOM_LEVELS as usize] = [
    "bloom prefilter",
    "bloom 1/4",
    "bloom 1/8",
    "bloom 1/16",
    "bloom 1/32",
    "bloom 1/64",
    "bloom up 1/2",
    "bloom up 1/4",
    "bloom up 1/8",
    "bloom up 1/16",
    "bloom up 1/32",
    COMPOSITE_PASS,
];

/// The fused pass's name in the frame graph.
pub const COMPOSITE_PASS: &str = "post";

/// The passes post adds to a frame, by name, in order: the bloom chain of `bloom_levels` levels
/// ([`RenderSettings::bloom_levels`](crate::RenderSettings::bloom_levels)) if it runs
/// ([`PostStats::bloom_chain`]), then the fused pass.
pub fn frame_passes(bloom_levels: u32, bloom_chain: bool) -> impl Iterator<Item = &'static str> {
    let levels = if bloom_chain { bloom_levels.min(MAX_BLOOM_LEVELS) as usize } else { 0 };
    let ups = MAX_BLOOM_LEVELS as usize;
    PASSES[..levels]
        .iter()
        .chain(&PASSES[ups..ups + levels.saturating_sub(1)])
        .chain([&COMPOSITE_PASS])
        .copied()
}

/// The post constants (WGSL `PostUniforms` in `shaders/post.wgsl`, group 0). The fields after
/// `vignette` are the compositing layers' (M1 P2), appended.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct PostUniforms {
    /// x: exposure multiplier; y: bloom intensity; z: LUT blend (0: A alone, 1: B alone);
    /// w: 1 when scene color is smaller than the target (a render scale below 1), else 0.
    pub composite: [f32; 4],
    /// x: threshold; y: knee width (threshold × knee); z: 1 / (4 × knee width); w: scatter.
    pub bloom: [f32; 4],
    /// The LUT shaper: x: 1 / ε; y: 1 / log2(1 + max / ε); z: (size − 1) / size;
    /// w: 0.5 / size.
    pub lut: [f32; 4],
    /// x: vignette intensity; y: start radius; z: 1 / (1 − start).
    pub vignette: [f32; 4],
    /// rgb: the color the vignette's corners go toward, linear.
    pub vignette_color: [f32; 4],
    /// Characters' bloom: x: threshold; y: knee width; z: 1 / (4 × knee width); w: their strength
    /// relative to the world's.
    pub character_bloom: [f32; 4],
    /// rgb: the diffusion glow's color, linear; w: its intensity (0: none).
    pub diffusion: [f32; 4],
    /// The diffusion's soft threshold, as `bloom`'s: x: threshold; y: knee width (half the
    /// threshold); z: 1 / (4 × knee width).
    pub diffusion_threshold: [f32; 4],
    /// x: the frame's aspect (width / height); y: one output pixel's width and z its height, in
    /// texture coordinates; w: sharpening (the RCAS lobe's scale, 0: none).
    pub frame: [f32; 4],
    /// xy: the sun's disc's center in texture coordinates; z: its radius in frame heights (0:
    /// no flare this frame); w: the exposed luminance at which a disc pixel counts as fully
    /// visible.
    pub flare_sun: [f32; 4],
    /// rgb: the lens flare's color × intensity, linear.
    pub flare: [f32; 4],
    /// x: gradient layers in use.
    pub counts: [u32; 4],
    pub gradients: [GpuGradient; MAX_GRADIENT_LAYERS],
    pub flare_elements: [GpuFlareElement; FLARE_ELEMENTS],
}

impl PostUniforms {
    /// Clamps every parameter to its working range, so no frame input can produce NaN.
    fn new(params: &PostParams, layers: &Layers) -> Self {
        let clamp = |value: f32, min: f32, max: f32, fallback: f32| {
            if value.is_finite() { value.clamp(min, max) } else { fallback }
        };
        let bloom = &params.bloom;
        let threshold = clamp(bloom.threshold, 0.0, lut::SHAPER_MAX, 1.0);
        let knee = threshold * clamp(bloom.knee, 0.0, 1.0, 0.0);
        let start = clamp(params.vignette.start, 0.0, 0.99, 0.5);
        let size = LUT_SIZE as f32;
        Self {
            composite: [
                clamp(params.exposure, -16.0, 16.0, 0.0).exp2(),
                clamp(bloom.intensity, 0.0, 16.0, 0.0),
                clamp(params.grade_blend, 0.0, 1.0, 0.0),
                0.0,
            ],
            bloom: [threshold, knee, 1.0 / (4.0 * knee + 1e-5), clamp(bloom.scatter, 0.0, 1.0, 0.5)],
            lut: [1.0 / lut::SHAPER_EPSILON, 1.0 / lut::shaper_range(), (size - 1.0) / size, 0.5 / size],
            vignette: [clamp(params.vignette.intensity, 0.0, 1.0, 0.0), start, 1.0 / (1.0 - start), 0.0],
            vignette_color: layers.vignette_color,
            character_bloom: layers.character_bloom,
            diffusion: layers.diffusion,
            diffusion_threshold: layers.diffusion_threshold,
            frame: [1.0, 0.0, 0.0, layers.sharpening],
            flare_sun: [0.0; 4],
            flare: layers.flare,
            counts: [layers.gradient_count, 0, 0, 0],
            gradients: layers.gradients,
            flare_elements: layers.flare_elements,
        }
    }
}

/// Labels for the bloom passes and textures, by level (½, ¼, …).
const DOWN_NAMES: [&str; MAX_BLOOM_LEVELS as usize] =
    ["bloom 1/2", "bloom 1/4", "bloom 1/8", "bloom 1/16", "bloom 1/32", "bloom 1/64"];
const UP_NAMES: [&str; MAX_BLOOM_LEVELS as usize - 1] =
    ["bloom up 1/2", "bloom up 1/4", "bloom up 1/8", "bloom up 1/16", "bloom up 1/32"];

/// The names of `post.wgsl`'s pipeline-overridable constants: the fused pass's compositing
/// layers and sharpening, and the prefilter's character mask.
pub(crate) const LAYERS_CONSTANT: &str = "POST_LAYERS";
pub(crate) const SHARPEN_CONSTANT: &str = "POST_SHARPEN";
pub(crate) const CHARACTERS_CONSTANT: &str = "POST_CHARACTERS";

/// What post drew in the last prepared frame (rule R9): which compositing layers, and so which
/// variants of its passes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PostStats {
    /// The bloom chain ran: for bloom, or for the diffusion glow.
    pub bloom_chain: bool,
    /// The diffusion glow drew.
    pub diffusion: bool,
    /// Characters bloomed with their own threshold and strength.
    pub character_bloom: bool,
    /// Gradient layers composited.
    pub gradient_layers: u32,
    /// The lens flare was on with the sun in front of the camera (it may still be off screen or
    /// hidden: the fused pass measures that).
    pub lens_flare: bool,
    /// The vignette tinted the corners.
    pub vignette_tint: bool,
    /// Sharpening ran.
    pub sharpening: bool,
}

/// Post's pipelines, layouts and persistent resources, and the per-pass bind group caches.
#[derive(Debug)]
pub(crate) struct PostProcess {
    device: wgpu::Device,
    pipelines: Pipelines,
    /// What the pipelines are built with, kept to build them again from new shader sources.
    shared_layout: wgpu::BindGroupLayout,
    output_format: wgpu::TextureFormat,
    cache: Option<wgpu::PipelineCache>,
    layouts: Layouts,
    uniform_buffer: wgpu::Buffer,
    /// Group 0: the uniforms and the sampler.
    shared: wgpu::BindGroup,
    /// Bound as the bloom and diffusion inputs while bloom is off: one black texel.
    no_bloom: wgpu::TextureView,
    luts: [wgpu::Texture; 2],
    lut_views: [wgpu::TextureView; 2],
    slots: LutSlots<LutKey>,
    /// Bake output, kept so a bake doesn't allocate it.
    texels: Vec<u32>,
    bloom_format: wgpu::TextureFormat,
    bloom_levels: u32,
    bind_groups: BindGroups,
    /// What the last `prepare` decided.
    uniforms: PostUniforms,
    variant: Variant,
    /// The LUT slots grades A and B read.
    bound: [usize; 2],
    bloom: bool,
    bakes: u32,
    stats: PostStats,
}

#[derive(Debug)]
pub(crate) struct Pipelines {
    /// Plain, and with the character mask.
    prefilter: [wgpu::RenderPipeline; 2],
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    /// The fused pass, by [`Variant::composite_index`].
    composite: [wgpu::RenderPipeline; Variant::COMPOSITES],
}

impl Pipelines {
    /// How many there are, for [`Renderer::set_shader_sources`](crate::Renderer::set_shader_sources)'s count.
    pub(crate) const COUNT: u32 = 4 + Variant::COMPOSITES as u32;

    /// The post pipelines, from `module` (`post.wgsl`): bloom passes write `bloom_format`, the
    /// fused composite `output_format`. Every variant is built now, behind the load.
    fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        shared_layout: &wgpu::BindGroupLayout,
        layouts: &Layouts,
        bloom_format: wgpu::TextureFormat,
        output_format: wgpu::TextureFormat,
        cache: Option<&wgpu::PipelineCache>,
    ) -> Self {
        let pipeline = |name: &str,
                        vertex: &str,
                        fragment: &str,
                        layout: &wgpu::BindGroupLayout,
                        format,
                        constants: &[(&str, f64)]| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(name),
                bind_group_layouts: &[Some(shared_layout), Some(layout)],
                immediate_size: 0,
            });
            let compilation_options = wgpu::PipelineCompilationOptions { constants, ..Default::default() };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(name),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vertex),
                    compilation_options: compilation_options.clone(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fragment),
                    compilation_options,
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache,
            })
        };
        let switch = |on: bool| if on { 1.0 } else { 0.0 };
        let fullscreen = "vs_fullscreen";
        let prefilter = [false, true].map(|characters| {
            let constants = [(CHARACTERS_CONSTANT, switch(characters))];
            pipeline(
                "bloom prefilter",
                fullscreen,
                "fs_bloom_prefilter",
                &layouts.two,
                bloom_format,
                &constants,
            )
        });
        // The plain variant is R3's fused pass; the others composite (and measure the flare's
        // visibility once a frame, in their vertex shader).
        let composite = std::array::from_fn(|index| {
            let (layers, sharpen) = (index & 1 != 0, index & 2 != 0);
            let constants = [(LAYERS_CONSTANT, switch(layers)), (SHARPEN_CONSTANT, switch(sharpen))];
            let (vertex, fragment, constants) = match index {
                0 => (fullscreen, "fs_post", &[][..]),
                _ => ("vs_composite", "fs_composite", &constants[..]),
            };
            pipeline(COMPOSITE_PASS, vertex, fragment, &layouts.composite, output_format, constants)
        });
        Self {
            prefilter,
            down: pipeline("bloom down", fullscreen, "fs_bloom_down", &layouts.one, bloom_format, &[]),
            up: pipeline("bloom up", fullscreen, "fs_bloom_up", &layouts.two, bloom_format, &[]),
            composite,
        }
    }
}

/// Group 1 layouts, by the textures a pass reads.
#[derive(Debug)]
struct Layouts {
    /// Down: `texture0`.
    one: wgpu::BindGroupLayout,
    /// Up: `texture0` and `texture1`; the prefilter: its input and the scene color whose alpha
    /// is the character mask.
    two: wgpu::BindGroupLayout,
    /// Composite: both, the two LUTs and the diffusion glow's bloom level.
    composite: wgpu::BindGroupLayout,
}

/// A group 1 bind group, for the most textures a post pass reads.
type CachedBindGroup = bind_cache::CachedBindGroup<5>;

/// One cache per pass position, so each always sees the same layout.
#[derive(Debug, Default)]
struct BindGroups {
    composite: CachedBindGroup,
    prefilter: CachedBindGroup,
    down: [CachedBindGroup; MAX_BLOOM_LEVELS as usize - 1],
    up: [CachedBindGroup; MAX_BLOOM_LEVELS as usize - 1],
}

/// A texture a fullscreen pass samples: one from the frame graph or a persistent one.
#[derive(Clone, Copy)]
enum Input<'f> {
    Graph(TextureId),
    View(&'f wgpu::TextureView),
}

/// What a fullscreen pass draws with.
struct Fullscreen<'f> {
    device: &'f wgpu::Device,
    labels: bool,
    pipeline: &'f wgpu::RenderPipeline,
    shared: &'f wgpu::BindGroup,
    layout: &'f wgpu::BindGroupLayout,
    cache: &'f mut CachedBindGroup,
}

impl PostProcess {
    /// Creates the post pipelines, writing `output_format` (the target's). Fails if the device
    /// can't filter the grading LUTs' format. The scene-color format is the renderer's to
    /// check: it is the main pass's attachment.
    pub(crate) fn new(
        gpu: &GpuContext,
        output_format: wgpu::TextureFormat,
        bloom_levels: u32,
        sources: &ShaderSources,
    ) -> Result<Self, RenderError> {
        if !gpu.format_support(LUT_FORMAT).filterable {
            return Err(RenderError::NotFilterable(LUT_FORMAT));
        }
        // Bloom has no alpha and needs little precision: half the bandwidth where it renders.
        let compact = gpu.format_support(wgpu::TextureFormat::Rg11b10Ufloat);
        let bloom_format = if compact.renderable && compact.filterable {
            wgpu::TextureFormat::Rg11b10Ufloat
        } else {
            SCENE_COLOR_FORMAT
        };
        let (device, cache) = (gpu.device(), gpu.pipeline_cache());
        use wgpu::ShaderStages as S;
        // The fused pass's vertex shader reads its input and the constants too: it measures the
        // sun's visibility for the lens flare once a frame.
        let both = S::VERTEX | S::FRAGMENT;
        let texture = |binding, visibility, dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: dimension,
                multisampled: false,
            },
            count: None,
        };
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
        };
        let flat = wgpu::TextureViewDimension::D2;
        let volume = wgpu::TextureViewDimension::D3;
        let shared_layout = layout(
            "post",
            &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: both,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(size_of::<PostUniforms>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: both,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        );
        let fragment = S::FRAGMENT;
        let layouts = Layouts {
            one: layout("post one texture", &[texture(0, fragment, flat)]),
            two: layout("post two textures", &[texture(0, fragment, flat), texture(1, fragment, flat)]),
            composite: layout(
                "post composite",
                &[
                    texture(0, both, flat),
                    texture(1, fragment, flat),
                    texture(2, fragment, volume),
                    texture(3, fragment, volume),
                    texture(4, fragment, flat),
                ],
            ),
        };

        let pipelines = Pipelines::new(
            device,
            &shaders::module(device, Program::Post, sources),
            &shared_layout,
            &layouts,
            bloom_format,
            output_format,
            cache,
        );

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post uniforms"),
            size: size_of::<PostUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shared = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &shared_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        // wgpu zero-initializes textures, so this one is black without an upload.
        let no_bloom = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("no bloom"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let luts = std::array::from_fn(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("grading LUT"),
                size: wgpu::Extent3d { width: LUT_SIZE, height: LUT_SIZE, depth_or_array_layers: LUT_SIZE },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: LUT_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        let lut_views = [0, 1].map(|slot: usize| luts[slot].create_view(&Default::default()));
        tracing::debug!(target: crate::LOG_TARGET, ?bloom_format, bloom_levels, "post-processing created");
        Ok(Self {
            device: device.clone(),
            pipelines,
            shared_layout,
            output_format,
            cache: cache.cloned(),
            layouts,
            uniform_buffer,
            shared,
            no_bloom,
            luts,
            lut_views,
            slots: LutSlots::default(),
            texels: vec![0; LUT_TEXELS],
            bloom_format,
            bloom_levels: bloom_levels.min(MAX_BLOOM_LEVELS),
            bind_groups: BindGroups::default(),
            uniforms: PostUniforms::default(),
            variant: Variant::default(),
            bound: [0, 0],
            bloom: false,
            bakes: 0,
            stats: PostStats::default(),
        })
    }

    /// The post pipelines built from `sources` instead, for [`Self::set_pipelines`] (shader hot
    /// reload).
    pub(crate) fn rebuilt_pipelines(&self, sources: &ShaderSources) -> Pipelines {
        let module = shaders::module(&self.device, Program::Post, sources);
        let (layouts, cache) = (&self.layouts, self.cache.as_ref());
        Pipelines::new(
            &self.device,
            &module,
            &self.shared_layout,
            layouts,
            self.bloom_format,
            self.output_format,
            cache,
        )
    }

    pub(crate) fn set_pipelines(&mut self, pipelines: Pipelines) {
        self.pipelines = pipelines;
    }

    /// Takes the frame's parameters: bakes the LUTs they need that aren't baked yet and
    /// computes the constants `record` uploads. `camera` is the main view's (the frame's aspect,
    /// which round gradients and flare elements need), and `sun` where it sees the sun's disc,
    /// for the lens flare.
    pub(crate) fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        params: &PostParams,
        camera: Option<&CameraData>,
        sun: Option<SunOnScreen>,
    ) {
        profiling::scope!("PostProcess::prepare");
        let layers = Layers::new(params);
        let mut uniforms = PostUniforms::new(params, &layers);
        let key = |grade| LutKey::new(&params.tone_curve, grade, params.filter_strength);
        let blending = uniforms.composite[2] > 0.0;
        let assignment = self.slots.assign(key(&params.grade_a), blending.then(|| key(&params.grade_b)));
        for slot in [0, 1].into_iter().filter(|&slot| assignment.stale[slot]) {
            let key = self.slots.key(slot).expect("a stale slot was just assigned a key");
            Lut::new(&key).bake(&mut self.texels);
            queue.write_texture(
                self.luts[slot].as_image_copy(),
                bytemuck::cast_slice(&self.texels),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(LUT_SIZE * size_of::<u32>() as u32),
                    rows_per_image: Some(LUT_SIZE),
                },
                self.luts[slot].size(),
            );
            self.bakes += 1;
            tracing::debug!(target: crate::LOG_TARGET, slot, "baked a grading LUT");
        }
        self.bound = assignment.slots;
        let flare_sun = sun.filter(|_| layers.lens_flare());
        if let Some(camera) = camera {
            uniforms.frame[0] = layers::aspect(&camera.proj);
        }
        if let Some(sun) = flare_sun {
            let visible = sun.luminance * uniforms.composite[0] * FLARE_VISIBLE_SHARE;
            uniforms.flare_sun = [sun.uv.x, sun.uv.y, sun.radius, visible];
        }
        self.bloom = self.bloom_levels > 0 && (uniforms.composite[1] > 0.0 || layers.diffusion());
        // The character mask changes only what blooms.
        self.variant = Variant { characters: layers.variant.characters && self.bloom, ..layers.variant };
        self.stats = PostStats {
            bloom_chain: self.bloom,
            diffusion: layers.diffusion() && self.bloom,
            character_bloom: self.variant.characters,
            gradient_layers: layers.gradient_count,
            lens_flare: flare_sun.is_some(),
            vignette_tint: layers.vignette_color[..3] != [0.0; 3],
            sharpening: layers.variant.sharpen,
        };
        self.uniforms = uniforms;
    }

    /// The constants for this frame, and the buffer `record`'s caller uploads them to (with the
    /// frame's other uploads, before any post pass runs). `upsample` says the scene renders
    /// smaller than the target (a render scale below 1), so the final pass filters it up;
    /// `output` is the target's size.
    pub(crate) fn uniforms(
        &self,
        upsample: bool,
        (width, height): (u32, u32),
    ) -> (PostUniforms, &wgpu::Buffer) {
        let mut uniforms = self.uniforms;
        uniforms.composite[3] = if upsample { 1.0 } else { 0.0 };
        uniforms.frame[1] = 1.0 / width.max(1) as f32;
        uniforms.frame[2] = 1.0 / height.max(1) as f32;
        (uniforms, &self.uniform_buffer)
    }

    /// The frame's exposure multiplier (2^EV), which temporal AA weights its resolve by.
    pub(crate) fn exposure(&self) -> f32 {
        self.uniforms.composite[0]
    }

    /// Grading LUTs baked since creation.
    pub(crate) fn bakes(&self) -> u32 {
        self.bakes
    }

    /// What the last prepared frame drew.
    pub(crate) fn stats(&self) -> PostStats {
        self.stats
    }

    /// Adds the bloom chain and the fused composite to `graph`, reading `input` (the main
    /// pass's HDR output, or temporal AA's resolve of it) and writing `target`. `scene_color` is
    /// the main pass's own output, whose alpha is the character mask: `input` itself without
    /// temporal AA.
    pub(crate) fn record<'f>(
        &'f mut self,
        graph: &mut FrameGraph<'f>,
        input: TextureId,
        scene_color: TextureId,
        target: TextureId,
        labels: bool,
    ) {
        let Self {
            ref device,
            ref pipelines,
            ref layouts,
            ref shared,
            ref no_bloom,
            ref lut_views,
            ref mut bind_groups,
            bloom_format,
            bloom_levels,
            bound,
            bloom,
            variant,
            stats,
            ..
        } = *self;
        let BindGroups { ref mut composite, ref mut prefilter, ref mut down, ref mut up } = *bind_groups;
        let pass = |pipeline, layout, cache| Fullscreen { device, labels, pipeline, shared, layout, cache };

        let chain = bloom.then(|| {
            let (width, height) = graph.size(input);
            // Level i is 1 / 2^(i + 1) of the target, rounded up.
            let level = |index: usize| TransientDesc {
                width: width.div_ceil(2 << index),
                height: height.div_ceil(2 << index),
                format: bloom_format,
                sample_count: 1,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            };
            let levels = bloom_levels as usize;
            let mut downs = [input; MAX_BLOOM_LEVELS as usize];
            downs[0] = graph.create_texture(DOWN_NAMES[0], level(0));
            // Only the character mask reads scene color apart from the input: otherwise the
            // input stands in, so the frame's graph is what it was without the mask.
            let mask = if variant.characters { scene_color } else { input };
            add_fullscreen(
                graph,
                PASSES[0],
                [Input::Graph(input), Input::Graph(mask)],
                downs[0],
                pass(&pipelines.prefilter[usize::from(variant.characters)], &layouts.two, prefilter),
            );
            // Level i's caches are `down[i - 1]` and `up[i]`, whatever the chain's length.
            for (index, cache) in (1..levels).zip(down) {
                downs[index] = graph.create_texture(DOWN_NAMES[index], level(index));
                add_fullscreen(
                    graph,
                    DOWN_NAMES[index],
                    [Input::Graph(downs[index - 1])],
                    downs[index],
                    pass(&pipelines.down, &layouts.one, cache),
                );
            }
            // The diffusion glow reads the widest level that is still smooth: the first up level
            // (from the smallest), or the only level of a chain of one.
            let mut lower = downs[levels - 1];
            let mut widest = lower;
            for (index, cache) in (0..levels - 1).zip(up).rev() {
                let upsampled = graph.create_texture(UP_NAMES[index], level(index));
                add_fullscreen(
                    graph,
                    UP_NAMES[index],
                    [Input::Graph(lower), Input::Graph(downs[index])],
                    upsampled,
                    pass(&pipelines.up, &layouts.two, cache),
                );
                if index == levels - 2 {
                    widest = upsampled;
                }
                lower = upsampled;
            }
            (lower, widest)
        });

        let [lut_a, lut_b] = bound.map(|slot| Input::View(&lut_views[slot]));
        let (bloom_input, diffusion_input) = match chain {
            Some((bloom, widest)) if stats.diffusion => (Input::Graph(bloom), Input::Graph(widest)),
            Some((bloom, _)) => (Input::Graph(bloom), Input::View(no_bloom)),
            None => (Input::View(no_bloom), Input::View(no_bloom)),
        };
        add_fullscreen(
            graph,
            COMPOSITE_PASS,
            [Input::Graph(input), bloom_input, lut_a, lut_b, diffusion_input],
            target,
            pass(&pipelines.composite[variant.composite_index()], &layouts.composite, composite),
        );
    }
}

/// Adds a pass that draws one fullscreen triangle into `output`, sampling `inputs` through
/// group 1.
fn add_fullscreen<'f, const N: usize>(
    graph: &mut FrameGraph<'f>,
    name: &'static str,
    inputs: [Input<'f>; N],
    output: TextureId,
    draw: Fullscreen<'f>,
) {
    let mut builder = graph.add_pass(name);
    for input in inputs {
        if let Input::Graph(texture) = input {
            builder = builder.read(texture);
        }
    }
    builder.write(output).record(move |ctx| {
        let Fullscreen { device, labels, pipeline, shared, layout, cache } = draw;
        let views = inputs.map(|input| match input {
            Input::Graph(texture) => ctx.view(texture),
            Input::View(view) => view,
        });
        let label = labels.then_some(name);
        let group = cache.get(device, label, layout, &views);
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
            timestamp_writes: ctx.timestamp_writes(),
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, shared, &[]);
        pass.set_bind_group(1, group, &[]);
        pass.draw(0..3, 0..1);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_clamp_every_parameter() {
        let params = PostParams {
            exposure: f32::NAN,
            bloom: BloomParams { knee: 5.0, scatter: -1.0, ..BloomParams::default() },
            grade_blend: 2.0,
            vignette: Vignette { start: 1.0, color: glam::Vec3::splat(f32::NAN), ..Vignette::default() },
            diffusion: Diffusion { intensity: f32::INFINITY, threshold: -1.0, color: glam::Vec3::ONE },
            sharpening: 7.0,
            ..PostParams::default()
        };
        let uniforms = PostUniforms::new(&params, &Layers::new(&params));
        assert_eq!(uniforms.composite[0], 1.0, "NaN exposure falls back to ×1");
        assert_eq!(uniforms.bloom[1], params.bloom.threshold, "the knee is at most the threshold");
        assert_eq!((uniforms.bloom[3], uniforms.composite[2]), (0.0, 1.0));
        assert!(uniforms.vignette[2].is_finite());
        assert_eq!(uniforms.vignette_color, [0.0; 4], "a non-finite tint is black");
        assert_eq!((uniforms.diffusion[3], uniforms.diffusion_threshold[0]), (0.0, 0.0));
        assert_eq!(uniforms.frame[3], 1.0, "sharpening is at most 1");
        let all = bytemuck::cast_slice::<PostUniforms, f32>(std::slice::from_ref(&uniforms));
        assert!(all.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn the_shader_shaper_matches_the_baked_one() {
        let params = PostParams::default();
        let uniforms = PostUniforms::new(&params, &Layers::new(&params));
        // Above the shaper's maximum the sampler clamps, so the shader needn't.
        for x in [0.0, 0.001, 0.18, 1.0, 10.0, lut::SHAPER_MAX] {
            let shader = (1.0 + x * uniforms.lut[0]).log2() * uniforms.lut[1];
            assert!((shader - lut::shape(x)).abs() < 1e-5, "{x}");
        }
        assert_eq!(lut::shape(2.0 * lut::SHAPER_MAX), 1.0);
    }

    #[test]
    fn the_passes_list_every_post_pass_name() {
        for name in DOWN_NAMES.iter().skip(1).chain(&UP_NAMES) {
            assert!(PASSES.contains(name), "{name}");
        }
        assert!(PASSES.contains(&COMPOSITE_PASS) && PASSES[0] == "bloom prefilter");
        let passes = |levels, chain| frame_passes(levels, chain).collect::<Vec<_>>();
        assert_eq!(passes(5, false), [COMPOSITE_PASS]);
        assert_eq!(passes(0, true), [COMPOSITE_PASS]);
        assert_eq!(passes(1, true), ["bloom prefilter", COMPOSITE_PASS]);
        let four = [
            "bloom prefilter",
            "bloom 1/4",
            "bloom 1/8",
            "bloom 1/16",
            "bloom up 1/2",
            "bloom up 1/4",
            "bloom up 1/8",
        ];
        assert_eq!(passes(4, true)[..7], four);
        assert_eq!(passes(MAX_BLOOM_LEVELS, true).len(), PASSES.len());
    }
}
