//! Post-processing (spec §5.12): from the HDR scene color to the output target.
//!
//! The main pass renders linear HDR light into an `Rgba16Float` scene-color transient
//! ([`SCENE_COLOR_FORMAT`](crate::pipelines::SCENE_COLOR_FORMAT)). Post then adds these frame
//! graph passes, each one fullscreen triangle into a pooled transient:
//!
//! 1. **Bloom prefilter** (½ size): exposure, a soft-knee threshold and a Karis average.
//! 2. **Bloom down** (¼ to 1/32): dual-filter downsamples (Bjørge, SIGGRAPH 2015), a 4×4
//!    footprint from five bilinear taps.
//! 3. **Bloom up** (1/16 back to ½): tent upsamples, each replacing `scatter` of the down
//!    level of its size, so wider levels add glow while the total light stays the same.
//! 4. **Post**, the fused full-screen pass: exposure, bloom, the vignette and the grading LUTs
//!    (tone curve and grade) in one pass, writing the output target. It is the mobile path of
//!    spec §3.3, and desktop uses it too.
//!
//! [`RenderSettings::bloom_levels`](crate::RenderSettings::bloom_levels) sets the chain's
//! length (5 reaches 1/32), so quality tiers can shorten it; bloom intensity 0 skips it.
//! Bloom levels are `Rg11b10Ufloat` where the device renders and filters it (half the
//! bandwidth of `Rgba16Float`, which is the fallback).
//!
//! # Render scale (M1 R6)
//!
//! Everything before the fused pass works at the scene's size, so below render scale 1 the
//! bloom chain shrinks with it. The fused pass alone runs at the target's size: at scale 1 it
//! loads scene color texel for texel, exactly as before render scales existed; below it, a
//! uniform branch samples scene color bilinearly at the pixel's position instead, which is the
//! upsample. Bloom is sampled by position either way.
//!
//! # Grading
//!
//! The tone curve and grade are baked on the CPU into 32³ LUTs ([`lut`]), only when the
//! frame's [`PostParams`] change them: about 2.6 ms of CPU, half a millisecond of wall time
//! across the job pool (Apple M5, release). Two LUTs blend per pixel (time-of-day keys, zone
//! overrides), and the player's filter strength is baked in as a blend toward the neutral
//! grade. A LUT already baked for either grade is reused, so a key transition costs one bake.
//!
//! # Allocation
//!
//! Bind groups for the pooled textures are cached per pass and rebuilt only when the pool
//! hands that pass a different texture (a resize, a settings change), so steady frames
//! allocate nothing here beyond the frame graph's one boxed closure per pass. wgpu itself
//! allocates about 13 times per render pass (measured on Metal), which is most of post's CPU
//! cost per frame (spec §5.12, "As built (M1 R3)").
//!
//! # Later milestones
//!
//! - **Bloom masks (C7, P2):** bloom is driven by brightness alone for now. An emissive-driven
//!   bloom mask and a character bloom mask (written to scene-color alpha, which is kept free
//!   for it) come with the character and post v2 packages.
//! - **Post v2 (P2):** the wide "diffusion" glow from the lowest bloom levels, gradient
//!   ("para") layers, lens flares and RCAS sharpening, all in the fused pass; the effect stack
//!   (P3) after it.
//! - **Mobile-Low:** the chain starts at ½ of the scene, and the tier's render scale (M1 R6)
//!   already puts the scene at about half the output, so the prefilter covers a sixteenth of
//!   the output's pixels. Starting at ¼ of the scene as well waits for device measurements.
//! - **A sharper upsample:** below render scale 1 the fused pass upsamples bilinearly. A
//!   spatial upscaler (an FSR1-style EASU and RCAS port, or MetalFX spatial; spec §5.12) or
//!   TAA's own upsampling (R7) replaces it.
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

mod lut;
mod params;

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use sapphire_rhi::{GpuContext, TransientDesc, wgpu};

use crate::RenderError;
use crate::graph::{FrameGraph, TextureId};
use crate::pipelines::SCENE_COLOR_FORMAT;
use crate::shaders::{self, Program, ShaderSources};
use lut::{LUT_SIZE, LUT_TEXELS, Lut, LutKey, LutSlots};
pub use params::{BloomParams, ColorGrade, PostParams, ToneCurve, Vignette};

/// The longest bloom chain: ½ down to 1/64 of the target.
pub const MAX_BLOOM_LEVELS: u32 = 6;

/// Grading LUTs hold sRGB-encoded display color (see [`lut`]); 10 bits keep the trilinear
/// lookup far below an 8-bit display step.
pub(crate) const LUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgb10a2Unorm;

/// The post constants (WGSL `PostUniforms` in `shaders/post.wgsl`, group 0).
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
}

impl PostUniforms {
    /// Clamps every parameter to its working range, so no frame input can produce NaN.
    fn new(params: &PostParams) -> Self {
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
        }
    }
}

/// Labels for the bloom passes and textures, by level (½, ¼, …).
const DOWN_NAMES: [&str; MAX_BLOOM_LEVELS as usize] =
    ["bloom 1/2", "bloom 1/4", "bloom 1/8", "bloom 1/16", "bloom 1/32", "bloom 1/64"];
const UP_NAMES: [&str; MAX_BLOOM_LEVELS as usize - 1] =
    ["bloom up 1/2", "bloom up 1/4", "bloom up 1/8", "bloom up 1/16", "bloom up 1/32"];

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
    /// Bound as the bloom input while bloom is off: one black texel.
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
    /// The LUT slots grades A and B read.
    bound: [usize; 2],
    bloom: bool,
    bakes: u32,
}

#[derive(Debug)]
pub(crate) struct Pipelines {
    prefilter: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
}

impl Pipelines {
    /// The four post pipelines, from `module` (`post.wgsl`): bloom passes write `bloom_format`,
    /// the fused composite `output_format`.
    fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        shared_layout: &wgpu::BindGroupLayout,
        layouts: &Layouts,
        bloom_format: wgpu::TextureFormat,
        output_format: wgpu::TextureFormat,
        cache: Option<&wgpu::PipelineCache>,
    ) -> Self {
        let pipeline = |name: &str, entry_point: &str, layout: &wgpu::BindGroupLayout, format| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(name),
                bind_group_layouts: &[Some(shared_layout), Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(name),
                layout: Some(&pipeline_layout),
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
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
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
        Self {
            prefilter: pipeline("bloom prefilter", "fs_bloom_prefilter", &layouts.one, bloom_format),
            down: pipeline("bloom down", "fs_bloom_down", &layouts.one, bloom_format),
            up: pipeline("bloom up", "fs_bloom_up", &layouts.two, bloom_format),
            composite: pipeline("post", "fs_post", &layouts.composite, output_format),
        }
    }
}

/// Group 1 layouts, by the textures a pass reads.
#[derive(Debug)]
struct Layouts {
    /// Prefilter and down: `texture0`.
    one: wgpu::BindGroupLayout,
    /// Up: `texture0` and `texture1`.
    two: wgpu::BindGroupLayout,
    /// Composite: both, plus the two LUTs.
    composite: wgpu::BindGroupLayout,
}

/// One cache per pass position, so each always sees the same layout.
#[derive(Debug, Default)]
struct BindGroups {
    composite: CachedBindGroup,
    prefilter: CachedBindGroup,
    down: [CachedBindGroup; MAX_BLOOM_LEVELS as usize - 1],
    up: [CachedBindGroup; MAX_BLOOM_LEVELS as usize - 1],
}

/// A group 1 bind group and the views it was made from.
#[derive(Debug, Default)]
struct CachedBindGroup {
    views: [Option<wgpu::TextureView>; 4],
    bind_group: Option<wgpu::BindGroup>,
}

impl CachedBindGroup {
    /// A bind group of `views` at bindings 0, 1, …, reused while they stay the same.
    fn get(
        &mut self,
        device: &wgpu::Device,
        label: Option<&str>,
        layout: &wgpu::BindGroupLayout,
        views: &[&wgpu::TextureView],
    ) -> &wgpu::BindGroup {
        let current = self.views.iter().zip(views).all(|(cached, view)| cached.as_ref() == Some(*view));
        if !current || self.bind_group.is_none() {
            let entries: [wgpu::BindGroupEntry<'_>; 4] =
                std::array::from_fn(|binding| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: wgpu::BindingResource::TextureView(views[binding.min(views.len() - 1)]),
                });
            self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label,
                layout,
                entries: &entries[..views.len()],
            }));
            for (cached, view) in self.views.iter_mut().zip(views) {
                *cached = Some((*view).clone());
            }
        }
        self.bind_group.as_ref().expect("created above")
    }
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
        let texture = |binding, dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: S::FRAGMENT,
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
                    visibility: S::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(size_of::<PostUniforms>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: S::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        );
        let layouts = Layouts {
            one: layout("post one texture", &[texture(0, flat)]),
            two: layout("post two textures", &[texture(0, flat), texture(1, flat)]),
            composite: layout(
                "post composite",
                &[texture(0, flat), texture(1, flat), texture(2, volume), texture(3, volume)],
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
            bound: [0, 0],
            bloom: false,
            bakes: 0,
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
    /// computes the constants `record` uploads.
    pub(crate) fn prepare(&mut self, queue: &wgpu::Queue, params: &PostParams) {
        profiling::scope!("PostProcess::prepare");
        let uniforms = PostUniforms::new(params);
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
        self.bloom = self.bloom_levels > 0 && uniforms.composite[1] > 0.0;
        self.uniforms = uniforms;
    }

    /// The constants for this frame, and the buffer `record`'s caller uploads them to (with the
    /// frame's other uploads, before any post pass runs). `upsample` says the scene renders
    /// smaller than the target (a render scale below 1), so the final pass filters it up.
    pub(crate) fn uniforms(&self, upsample: bool) -> (PostUniforms, &wgpu::Buffer) {
        let mut uniforms = self.uniforms;
        uniforms.composite[3] = if upsample { 1.0 } else { 0.0 };
        (uniforms, &self.uniform_buffer)
    }

    /// Grading LUTs baked since creation.
    pub(crate) fn bakes(&self) -> u32 {
        self.bakes
    }

    /// Adds the bloom chain and the fused composite to `graph`, reading `scene_color` (the
    /// main pass's HDR output) and writing `target`.
    pub(crate) fn record<'f>(
        &'f mut self,
        graph: &mut FrameGraph<'f>,
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
            ..
        } = *self;
        let BindGroups { ref mut composite, ref mut prefilter, ref mut down, ref mut up } = *bind_groups;
        let pass = |pipeline, layout, cache| Fullscreen { device, labels, pipeline, shared, layout, cache };

        let bloom_texture = bloom.then(|| {
            let (width, height) = graph.size(scene_color);
            // Level i is 1 / 2^(i + 1) of the target, rounded up.
            let level = |index: usize| TransientDesc {
                width: width.div_ceil(2 << index),
                height: height.div_ceil(2 << index),
                format: bloom_format,
                sample_count: 1,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            };
            let levels = bloom_levels as usize;
            let mut downs = [scene_color; MAX_BLOOM_LEVELS as usize];
            downs[0] = graph.create_texture(DOWN_NAMES[0], level(0));
            add_fullscreen(
                graph,
                "bloom prefilter",
                [Input::Graph(scene_color)],
                downs[0],
                pass(&pipelines.prefilter, &layouts.one, prefilter),
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
            let mut lower = downs[levels - 1];
            for (index, cache) in (0..levels - 1).zip(up).rev() {
                let upsampled = graph.create_texture(UP_NAMES[index], level(index));
                add_fullscreen(
                    graph,
                    UP_NAMES[index],
                    [Input::Graph(lower), Input::Graph(downs[index])],
                    upsampled,
                    pass(&pipelines.up, &layouts.two, cache),
                );
                lower = upsampled;
            }
            lower
        });

        let [lut_a, lut_b] = bound.map(|slot| Input::View(&lut_views[slot]));
        let bloom_input = bloom_texture.map_or(Input::View(no_bloom), Input::Graph);
        add_fullscreen(
            graph,
            "post",
            [Input::Graph(scene_color), bloom_input, lut_a, lut_b],
            target,
            pass(&pipelines.composite, &layouts.composite, composite),
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
            vignette: Vignette { start: 1.0, ..Vignette::default() },
            ..PostParams::default()
        };
        let uniforms = PostUniforms::new(&params);
        assert_eq!(uniforms.composite[0], 1.0, "NaN exposure falls back to ×1");
        assert_eq!(uniforms.bloom[1], params.bloom.threshold, "the knee is at most the threshold");
        assert_eq!((uniforms.bloom[3], uniforms.composite[2]), (0.0, 1.0));
        assert!(uniforms.vignette[2].is_finite());
    }

    #[test]
    fn the_shader_shaper_matches_the_baked_one() {
        let uniforms = PostUniforms::new(&PostParams::default());
        // Above the shaper's maximum the sampler clamps, so the shader needn't.
        for x in [0.0, 0.001, 0.18, 1.0, 10.0, lut::SHAPER_MAX] {
            let shader = (1.0 + x * uniforms.lut[0]).log2() * uniforms.lut[1];
            assert!((shader - lut::shape(x)).abs() < 1e-5, "{x}");
        }
        assert_eq!(lut::shape(2.0 * lut::SHAPER_MAX), 1.0);
    }
}
