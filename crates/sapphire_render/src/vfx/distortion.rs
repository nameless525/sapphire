//! The frame's bends (M5 L6): [`VfxFrame::distortions`](sapphire_look::VfxFrame), effect mesh layers that move
//! the picture behind them rather than add light, a shockwave's ring or a fire's haze. Each draws its shape,
//! pattern, scroll, erosion and fades through `shaders/vfx_distortion.wgsl`, one indexed instance each, into a
//! buffer of screen offsets ([`PASS`], [`FORMAT`]), and post's fused pass reads the frame through it (`post`'s
//! distorted composites, `post_distortion.wgsl`).
//!
//! - **The buffer** is a share of post's input: half its width and height on the desktop tiers, a quarter on
//!   Mobile-High ([`DistortionResolution`]), two half floats a texel, cleared to 0, each bend added in. An offset
//!   is in frame heights, so a bend moves the picture by as much of the frame at any output size or render scale.
//!   Bends are smooth, so post reads it filtered and a smaller buffer loses little.
//! - **Depth:** over temporal AA's frame each fragment is tested against the scene prepass's depth by hand, which
//!   that frame keeps, and fades into the surface behind it over its `soft` metres, as an effect mesh does: a
//!   wall in front of a haze isn't bent. Without temporal AA (Mobile-High, and a desktop tier run with MSAA)
//!   there's no depth to read and nothing is tested: what stands in is post's character mask, which no bend ever
//!   moves, so characters in front of a bend stay exact, and the rare world surface in front of one bends with it.
//! - **Near the camera** a bend fades out by its fragment's view depth, none nearer than half a metre and whole
//!   past a metre and a half (the shader's `VFX_DISTORTION_NEAR_*`), so a column or dome the camera passes through
//!   doesn't bend the whole screen.
//! - **Unjittered:** the buffer is drawn through the main view without temporal AA's jitter, as the frame post
//!   reads is resolved, and the same frame twice draws the same offsets.
//! - **Costs nothing while off:** Mobile-Low has none ([`DistortionResolution::Off`]): no pipeline, no target, no
//!   pass, whatever the effects ask. A tier with it builds its pipeline only for a frame with a bend or a load that
//!   declares them ([`PipelineInventory::distortion`](crate::PipelineInventory)), and a frame without a bend adds
//!   no pass and no texture. Shapes and patterns are its own, made with the pipeline, so a library whose meshes
//!   only bend makes none of the effect meshes' part. A tier change (M4 T3) builds what the tier left had asked
//!   for, not only what it built: Mobile-Low keeps a load's declaration, so the tier a player changes back to
//!   from it has the bends' pipelines, with post's distorted composites, before its first bend.
//! - **Counted** in post's counts (B4's `Counted::Post`): a call of one instance per bend, with its shape's
//!   triangles, as the effect meshes' draws are.

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use sapphire_look::{VfxBend, VfxDistortion, VfxMeshShape, VfxMeshVertex};
use sapphire_rhi::{TransientDesc, wgpu};
use serde::{Deserialize, Serialize};

use super::mesh::{self, GpuVfxMesh, Shapes, VERTEX_ATTRIBUTES};
use super::{VfxMaker, sane};
use crate::CameraData;
use crate::bind_cache::CachedBindGroup;
use crate::buffer::{self, GrowableBuffer};
use crate::counts::PassCounts;
use crate::graph::{PassContext, TextureId};
use crate::pipeline_builds::{PipelineBuilds, PipelineFamily};
use crate::pipelines::DEPTH_FORMAT;
use crate::shader_builds::Step;
use crate::shaders::{self, BuildOn, Program, ShaderSources};

/// The frame graph's pass, whose time is the bends' share of spec §3.3's "Particles, distortion".
pub const PASS: &str = "distortion";

/// The offsets' format: two half floats, in frame heights.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;

/// The name of the shader's constant: each fragment tests the scene prepass's depth.
const DEPTH_CONSTANT: &str = "VFX_DISTORTION_DEPTH";

/// `GpuVfxDistortion::flags`.
const ERODES: u32 = 1;
const SHIMMER: u32 = 2;
const FLIPBOOK: u32 = 4;

/// The most a bend moves the picture, in frame heights: `sapphire_vfx::MAX_BEND`, which documents are held to.
pub const MAX_BEND: f32 = 0.1;

/// Bends the buffer holds when first made: past every tier's cap, so it never grows in a fight.
const INITIAL_BENDS: u64 = 16;

/// How large the offsets are against post's input (M5 L6, [`RenderSettings::distortion`](crate::RenderSettings)).
/// Off, the default, has none: no pipeline, target or pass, whatever a frame's effects ask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum DistortionResolution {
    #[default]
    Off,
    /// Half post's input's width and height: the desktop tiers'.
    Half,
    /// A quarter: Mobile-High's.
    Quarter,
}

impl DistortionResolution {
    /// Whether it draws bends at all.
    pub const fn any(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// The offsets' size for post's input of `size`, rounded up: `None` when off.
    pub const fn size_for(self, (width, height): (u32, u32)) -> Option<(u32, u32)> {
        match self {
            Self::Off => None,
            Self::Half => Some((width.div_ceil(2), height.div_ceil(2))),
            Self::Quarter => Some((width.div_ceil(4), height.div_ceil(4))),
        }
    }
}

/// A bend (`VfxDistortion` in `vfx_distortion.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct GpuVfxDistortion {
    world_from_local: [[f32; 4]; 4],
    scroll: [f32; 2],
    tiling: [f32; 2],
    mask_tiling: [f32; 2],
    threshold: f32,
    edge: f32,
    cells: [f32; 4],
    cell_size: [f32; 2],
    blend: f32,
    strength: f32,
    alpha: f32,
    soft: f32,
    pattern: u32,
    mask: u32,
    flags: u32,
    padding: [u32; 3],
}

/// `VfxDistortionUniforms` in `vfx_distortion.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(crate) struct GpuVfxDistortionUniforms {
    clip_from_world: [[f32; 4]; 4],
    target_size: [f32; 4],
    frame: [f32; 4],
}

impl GpuVfxDistortion {
    /// `distortion`, unless its mesh isn't finite where it stands or how it's turned: as an effect mesh packs,
    /// its strength held to [`MAX_BEND`].
    fn new(distortion: &VfxDistortion) -> Option<Self> {
        let mesh = GpuVfxMesh::new(&distortion.mesh)?;
        let mut flags = 0;
        if mesh.flags & mesh::ERODES != 0 {
            flags |= ERODES;
        }
        if mesh.flags & mesh::FLIPBOOK != 0 {
            flags |= FLIPBOOK;
        }
        if distortion.bend == VfxBend::Shimmer {
            flags |= SHIMMER;
        }
        let strength =
            if distortion.strength.is_finite() { sane(distortion.strength, MAX_BEND) } else { 0.0 };
        Some(Self {
            world_from_local: mesh.world_from_local,
            scroll: mesh.scroll,
            tiling: mesh.tiling,
            mask_tiling: mesh.mask_tiling,
            threshold: mesh.threshold,
            edge: mesh.edge,
            cells: mesh.cells,
            cell_size: mesh.cell_size,
            blend: mesh.blend,
            strength,
            alpha: mesh.alpha,
            soft: mesh.soft,
            pattern: mesh.pattern,
            mask: mesh.mask,
            flags,
            padding: [0; 3],
        })
    }
}

/// What the pipeline is built with: fixed for a renderer's life.
#[derive(Clone, Debug)]
pub(super) struct DistortionBuilder {
    device: wgpu::Device,
    layout: wgpu::PipelineLayout,
    /// Over temporal AA's frame, testing the scene prepass's depth.
    depth: bool,
    builds: PipelineBuilds,
}

impl DistortionBuilder {
    /// Adds each bend's offset into the target: triangle lists from the shapes' buffer, both faces, no depth
    /// attachment.
    fn pipeline(&self, module: &wgpu::ShaderModule) -> wgpu::RenderPipeline {
        let constants = [(DEPTH_CONSTANT, if self.depth { 1.0 } else { 0.0 })];
        let compilation_options =
            wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() };
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        self.builds.render(
            &self.device,
            PipelineFamily::Distortion,
            &wgpu::RenderPipelineDescriptor {
                label: Some("vfx distortion"),
                layout: Some(&self.layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_vfx_distortion"),
                    compilation_options: compilation_options.clone(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<VfxMeshVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &VERTEX_ATTRIBUTES,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_vfx_distortion"),
                    compilation_options,
                    targets: &[Some(wgpu::ColorTargetState {
                        format: FORMAT,
                        blend: Some(wgpu::BlendState { color: add, alpha: add }),
                        write_mask: wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN,
                    })],
                }),
                multiview_mask: None,
                cache: self.builds.cache(),
            },
        )
    }
}

/// The pipeline.
#[derive(Clone, Debug)]
pub(crate) struct DistortionPipeline {
    pipeline: wgpu::RenderPipeline,
}

/// Groups 0 and 1's layouts.
#[derive(Debug)]
struct DistortionLayouts {
    buffers: wgpu::BindGroupLayout,
    textures: wgpu::BindGroupLayout,
}

fn layouts(device: &wgpu::Device) -> DistortionLayouts {
    use wgpu::ShaderStages as S;
    let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
    let buffers = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("vfx distortion buffers"),
        entries: &[
            entry(
                0,
                S::VERTEX_FRAGMENT,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<GpuVfxDistortionUniforms>() as u64),
                },
            ),
            entry(
                1,
                S::VERTEX,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<GpuVfxDistortion>() as u64),
                },
            ),
            entry(2, S::FRAGMENT, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            entry(3, S::FRAGMENT, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering)),
        ],
    });
    let textures = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("vfx distortion textures"),
        entries: &[
            entry(
                0,
                S::FRAGMENT,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
            ),
            entry(
                1,
                S::FRAGMENT,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            ),
        ],
    });
    DistortionLayouts { buffers, textures }
}

fn builder(maker: &VfxMaker) -> (DistortionBuilder, DistortionLayouts) {
    let layouts = layouts(&maker.device);
    let layout = maker.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("vfx distortion"),
        bind_group_layouts: &[Some(&layouts.buffers), Some(&layouts.textures)],
        immediate_size: 0,
    });
    let builder = DistortionBuilder {
        device: maker.device.clone(),
        layout,
        depth: maker.after_resolve,
        builds: maker.builds.clone(),
    };
    (builder, layouts)
}

/// What a warm-up made of the bends' first frame.
#[derive(Debug)]
pub(crate) struct WarmedDistortion(Box<DistortionGpu>);

/// Everything the bends make when they're first drawn.
#[derive(Debug)]
pub(crate) struct DistortionGpu {
    builder: DistortionBuilder,
    pipeline: DistortionPipeline,
    shapes: Shapes,
    bends: GrowableBuffer,
    uniforms: wgpu::Buffer,
    sampler: wgpu::Sampler,
    point: wgpu::Sampler,
    buffers_layout: wgpu::BindGroupLayout,
    buffers_group: wgpu::BindGroup,
    textures_layout: wgpu::BindGroupLayout,
    patterns: wgpu::TextureView,
    /// Group 1 with a 1×1 depth stand-in nothing reads: without temporal AA.
    flat_group: wgpu::BindGroup,
    /// Group 1 with the scene prepass's depth, the history pair's two.
    depth_groups: CachedBindGroup<2>,
}

fn buffers_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    bends: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
    point: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vfx distortion buffers"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: bends.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(point) },
        ],
    })
}

impl DistortionGpu {
    /// Makes the bends' resources and builds their pipeline from `sources`.
    fn new(maker: &VfxMaker, queue: &wgpu::Queue, sources: &ShaderSources) -> Self {
        let device = &maker.device;
        let (builder, layouts) = builder(maker);
        let module = shaders::module(device, Program::VfxDistortion, sources);
        let pipeline = DistortionPipeline { pipeline: builder.pipeline(&module) };
        let limit = device.limits().max_storage_buffer_binding_size;
        let capacity = INITIAL_BENDS * size_of::<GpuVfxDistortion>() as u64;
        let bends =
            GrowableBuffer::new(device, "vfx distortions", wgpu::BufferUsages::STORAGE, capacity, limit);
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vfx distortion uniforms"),
            size: size_of::<GpuVfxDistortionUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vfx distortion patterns"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let point = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vfx distortion depth"),
            ..Default::default()
        });
        let patterns = mesh::upload_patterns(device, queue);
        let stand_in = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("vfx distortion depth stand-in"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let flat_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vfx distortion textures"),
            layout: &layouts.textures,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&patterns) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&stand_in) },
            ],
        });
        let buffers_group =
            buffers_group(device, &layouts.buffers, &uniforms, bends.buffer(), &sampler, &point);
        Self {
            builder,
            pipeline,
            shapes: Shapes::new(device),
            bends,
            uniforms,
            sampler,
            point,
            buffers_layout: layouts.buffers,
            buffers_group,
            textures_layout: layouts.textures,
            patterns,
            flat_group,
            depth_groups: CachedBindGroup::default(),
        }
    }
}

/// The frame's bends' share of the effects ([`super::Vfx`] holds it).
#[derive(Debug, Default)]
pub(crate) struct VfxDistortions {
    /// The tier's buffer: off makes nothing, ever.
    resolution: DistortionResolution,
    gpu: Option<Box<DistortionGpu>>,
    /// Bends were asked for ([`Self::build`]), by an inventory or a frame, on this tier or not: what a tier
    /// changed to builds them for (M4 T3), so a tier without bends hands their declaration on to one with.
    asked: bool,
    /// This frame's bends, in the frame's order (they add, so order changes nothing), and their shapes.
    packed: Vec<GpuVfxDistortion>,
    shapes: Vec<VfxMeshShape>,
    /// The main view's, unjittered, and its near plane.
    clip_from_world: [[f32; 4]; 4],
    near: f32,
    /// Bends left out because they weren't finite or didn't fit the buffer's limit.
    pub(super) rejected: u32,
}

impl VfxDistortions {
    pub(super) fn new(resolution: DistortionResolution) -> Self {
        Self { resolution, ..Self::default() }
    }

    /// The tier's buffer.
    pub(super) fn resolution(&self) -> DistortionResolution {
        self.resolution
    }

    /// Whether a frame of `bends` draws any: the tier has them and one packs, by the test [`Self::prepare`]
    /// packs them by. A frame's are within its budget's cap, far under the buffer's limit.
    pub(super) fn draw(&self, bends: &[VfxDistortion]) -> bool {
        self.resolution.any() && bends.iter().any(|bend| GpuVfxDistortion::new(bend).is_some())
    }

    /// Makes what the first bend needs and builds its pipeline, unless made, on a tier with bends.
    pub(super) fn build(&mut self, maker: &VfxMaker, queue: &wgpu::Queue, sources: &ShaderSources) {
        self.asked = true;
        if self.gpu.is_none() && self.resolution.any() {
            self.gpu = Some(Box::new(DistortionGpu::new(maker, queue, sources)));
        }
    }

    /// Whether bends were asked for: what another tier builds before it takes these ones' place, though a tier
    /// without bends built nothing of them.
    pub(super) fn held(&self) -> bool {
        self.asked
    }

    /// What [`Self::build`] would make, as a warm-up's step: `None` when made, or off.
    pub(super) fn warm(
        &self,
        maker: VfxMaker,
        queue: &wgpu::Queue,
        on: BuildOn,
    ) -> Option<Step<WarmedDistortion>> {
        if self.gpu.is_some() || !self.resolution.any() {
            return None;
        }
        let queue = queue.clone();
        // One pipeline: `on` spreads nothing, so it builds where the step runs.
        let _ = on;
        Some(Box::new(move |sources, _| {
            WarmedDistortion(Box::new(DistortionGpu::new(&maker, &queue, sources)))
        }))
    }

    pub(super) fn adopt(&mut self, warmed: WarmedDistortion) {
        if self.gpu.is_none() {
            self.gpu = Some(warmed.0);
        }
    }

    pub(super) fn pipelines(&self) -> u32 {
        u32::from(self.gpu.is_some())
    }

    pub(super) fn rebuild(&self) -> Option<Step<DistortionPipeline>> {
        let builder = self.gpu.as_ref()?.builder.clone();
        Some(Box::new(move |sources, _cancel| {
            let module = shaders::module(&builder.device, Program::VfxDistortion, sources);
            DistortionPipeline { pipeline: builder.pipeline(&module) }
        }))
    }

    pub(super) fn set_pipelines(&mut self, pipeline: DistortionPipeline) {
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.pipeline = pipeline;
        }
    }

    /// Drops what binds the last frame's depth: a frame with no bends keeps no view alive.
    pub(super) fn forget_depth(&mut self) {
        if let Some(gpu) = &mut self.gpu {
            gpu.depth_groups = CachedBindGroup::default();
        }
    }

    pub(super) fn clear(&mut self) {
        self.packed.clear();
        self.shapes.clear();
        self.rejected = 0;
    }

    /// Packs `bends` and sizes their buffer, through `camera`. [`Self::build`] first.
    pub(super) fn prepare(
        &mut self,
        bends: &[VfxDistortion],
        camera: &CameraData,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        self.clear();
        let Some(gpu) = self.gpu.as_mut() else { return };
        let size = size_of::<GpuVfxDistortion>() as u64;
        let fits = &bends[..bends.len().min(gpu.bends.max_items(size) as usize)];
        for bend in fits {
            if let Some(packed) = GpuVfxDistortion::new(bend) {
                self.packed.push(packed);
                self.shapes.push(bend.mesh.shape);
            }
        }
        self.rejected = (bends.len() - self.packed.len()) as u32;
        self.clip_from_world = (camera.proj * camera.view).to_cols_array_2d();
        self.near = camera.proj.w_axis.z;
        if gpu.bends.reserve(device, queue, self.packed.len() as u64 * size, 0) {
            gpu.buffers_group = buffers_group(
                device,
                &gpu.buffers_layout,
                &gpu.uniforms,
                gpu.bends.buffer(),
                &gpu.sampler,
                &gpu.point,
            );
        }
    }

    pub(super) fn count(&self) -> u32 {
        self.packed.len() as u32
    }

    pub(super) fn bytes(&self) -> u64 {
        (self.packed.len() * size_of::<GpuVfxDistortion>()) as u64
    }

    /// This frame's upload and draw for post's input of `input` (its size): `None` with no bends to draw.
    pub(super) fn split_for_record(
        &mut self,
        input: (u32, u32),
    ) -> Option<(DistortionUpload<'_>, DistortionDraw<'_>)> {
        if self.packed.is_empty() {
            return None;
        }
        let size = self.resolution.size_for(input)?;
        let gpu = self.gpu.as_mut()?;
        let uniforms = GpuVfxDistortionUniforms {
            clip_from_world: self.clip_from_world,
            target_size: [
                size.0 as f32,
                size.1 as f32,
                1.0 / size.0.max(1) as f32,
                1.0 / size.1.max(1) as f32,
            ],
            frame: [input.0 as f32 / input.1.max(1) as f32, self.near, 0.0, 0.0],
        };
        let upload = DistortionUpload {
            uniforms,
            uniforms_buffer: &gpu.uniforms,
            bends: &self.packed,
            buffer: gpu.bends.buffer(),
        };
        let draw = DistortionDraw {
            size,
            pipeline: &gpu.pipeline.pipeline,
            depth: gpu.builder.depth,
            buffers: &gpu.buffers_group,
            flat_group: &gpu.flat_group,
            textures_layout: &gpu.textures_layout,
            patterns: &gpu.patterns,
            depth_groups: &mut gpu.depth_groups,
            shapes: &gpu.shapes,
            drawn: &self.shapes,
        };
        Some((upload, draw))
    }
}

/// The bends' uploads.
#[derive(Debug)]
pub(crate) struct DistortionUpload<'a> {
    uniforms: GpuVfxDistortionUniforms,
    uniforms_buffer: &'a wgpu::Buffer,
    bends: &'a [GpuVfxDistortion],
    buffer: &'a wgpu::Buffer,
}

impl DistortionUpload<'_> {
    pub(super) fn write(&self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        buffer::stage(belt, encoder, self.uniforms_buffer, 0, bytemuck::bytes_of(&self.uniforms));
        buffer::stage(belt, encoder, self.buffer, 0, bytemuck::cast_slice(self.bends));
    }
}

/// The bends' draw into the offsets, its depth group made when the pass records.
#[derive(Debug)]
pub(crate) struct DistortionDraw<'a> {
    size: (u32, u32),
    pipeline: &'a wgpu::RenderPipeline,
    depth: bool,
    buffers: &'a wgpu::BindGroup,
    flat_group: &'a wgpu::BindGroup,
    textures_layout: &'a wgpu::BindGroupLayout,
    patterns: &'a wgpu::TextureView,
    depth_groups: &'a mut CachedBindGroup<2>,
    shapes: &'a Shapes,
    drawn: &'a [VfxMeshShape],
}

impl DistortionDraw<'_> {
    /// The offsets' texture.
    pub(crate) fn desc(&self) -> TransientDesc {
        TransientDesc {
            width: self.size.0,
            height: self.size.1,
            format: FORMAT,
            sample_count: 1,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        }
    }

    /// Whether it reads the scene prepass's depth, which the pass then reads.
    pub(crate) fn reads_depth(&self) -> bool {
        self.depth
    }

    /// Clears `offsets` and adds every bend into it, `depth` the scene prepass's where it reads one. What it
    /// drew: a call of one instance per bend.
    pub(crate) fn record(
        self,
        ctx: &mut PassContext<'_>,
        device: &wgpu::Device,
        offsets: TextureId,
        depth: Option<TextureId>,
        labels: bool,
    ) -> PassCounts {
        let label = labels.then_some(PASS);
        let textures = match depth.filter(|_| self.depth) {
            Some(depth) => {
                let depth = ctx.view(depth);
                self.depth_groups.get(device, label, self.textures_layout, &[self.patterns, depth])
            }
            None => self.flat_group,
        };
        let timestamp_writes = ctx.timestamp_writes();
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.view(offsets),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            timestamp_writes,
            ..Default::default()
        });
        pass.set_pipeline(self.pipeline);
        pass.set_bind_group(0, self.buffers, &[]);
        pass.set_bind_group(1, textures, &[]);
        pass.set_vertex_buffer(0, self.shapes.vertices.slice(..));
        pass.set_index_buffer(self.shapes.indices.slice(..), wgpu::IndexFormat::Uint16);
        let mut counts = PassCounts::default();
        for (index, shape) in self.drawn.iter().enumerate() {
            let range = self.shapes.ranges[shape.index()];
            let index = index as u32;
            pass.draw_indexed(range.indices.0..range.indices.1, range.base_vertex, index..index + 1);
            counts = counts.plus(&PassCounts::call(1, u64::from(range.indices.1 - range.indices.0) / 3));
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use glam::{Quat, Vec2, Vec3};
    use sapphire_look::{VfxErosion, VfxMesh, VfxPattern};

    use super::*;

    fn bend(strength: f32) -> VfxDistortion {
        VfxDistortion {
            mesh: VfxMesh {
                shape: VfxMeshShape::Ring,
                position: Vec3::new(0.0, 1.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
                color: Vec3::ZERO,
                alpha: 0.5,
                additive: 1.0,
                soft: 0.2,
                pattern: VfxPattern::Noise,
                tiling: Vec2::ONE,
                scroll: Vec2::ZERO,
                erosion: Some(VfxErosion {
                    mask: VfxPattern::Streaks,
                    tiling: Vec2::ONE,
                    threshold: 0.3,
                    edge: 0.1,
                    edge_color: Vec3::ONE,
                }),
                flipbook: None,
            },
            bend: VfxBend::Shimmer,
            strength,
        }
    }

    #[test]
    fn a_bend_packs_its_flags_and_its_strength_is_held_to_the_most_a_document_may_ask() {
        let packed = GpuVfxDistortion::new(&bend(0.02)).expect("finite");
        assert_eq!(packed.flags, ERODES | SHIMMER);
        assert_eq!((packed.strength, packed.alpha, packed.threshold), (0.02, 0.5, 0.3));
        assert_eq!(GpuVfxDistortion::new(&bend(5.0)).expect("finite").strength, MAX_BEND);
        assert_eq!(GpuVfxDistortion::new(&bend(f32::NAN)).expect("finite").strength, 0.0);
        let outward = VfxDistortion { bend: VfxBend::Outward, ..bend(0.01) };
        assert_eq!(GpuVfxDistortion::new(&outward).expect("finite").flags, ERODES);
        let lost = VfxDistortion { mesh: VfxMesh { position: Vec3::NAN, ..bend(0.01).mesh }, ..bend(0.01) };
        assert!(GpuVfxDistortion::new(&lost).is_none());
    }

    #[test]
    fn the_buffer_is_a_share_of_post_s_input_rounded_up_and_none_while_off() {
        assert_eq!(DistortionResolution::Off.size_for((1920, 1080)), None);
        assert_eq!(DistortionResolution::Half.size_for((1920, 1080)), Some((960, 540)));
        assert_eq!(DistortionResolution::Half.size_for((1281, 721)), Some((641, 361)));
        assert_eq!(DistortionResolution::Quarter.size_for((1281, 721)), Some((321, 181)));
        assert_eq!(DistortionResolution::default(), DistortionResolution::Off);
    }
}
