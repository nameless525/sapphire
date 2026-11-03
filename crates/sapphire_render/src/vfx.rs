//! The frame's effects (M2 E2, spec §5.16): [`FramePacket::vfx`](crate::FramePacket), plain sprites
//! and ribbon points `sapphire_vfx` resolves (rule R4), drawn instanced. `shaders/vfx.wgsl` has the
//! shading. **Nothing here exists until a frame has effects:** no buffers, map or pipelines, no
//! pass and no draw (a GPU test counts them), and everything else about such a frame is what it was.
//! [`Renderer::prewarm_vfx`](crate::Renderer::prewarm_vfx) builds them ahead, so a game's first hit
//! doesn't compile its pipelines on the frame it lands.
//!
//! # Where they draw
//!
//! Effects are the frame's last world draw, **after every transparent surface and fog card**:
//! they're near and bright where mist is far and faint, and a spark behind a sheet of mist still
//! shows through it, as light does. On both paths below they come before post, so debug draw's
//! lines, which follow post, stay on top of them, as the overlay stays on top of those. Depending
//! on the tier:
//!
//! - **Without temporal AA** (phones, MSAA in tile memory) they end the main pass: its depth test,
//!   no new pass, and the MSAA target keeps them. Nothing can read that depth, so nothing fades
//!   softly into a surface: quads cut where they meet one, as fog cards do there, and the shapes'
//!   soft edges and dust's low opacity carry it.
//! - **Under temporal AA** (the desktop tiers) they draw **after the resolve**, in a pass of their
//!   own (the frame graph's [`PASS`]) over a copy of the resolved frame, before bloom and the tone
//!   curve. The history must never hold them: the resolve's clip keeps a moving spark's first frame
//!   at a thirty-third of its light over a still world, whose zero velocity also takes away the
//!   responsive mask's say (it scales the moving cap), and a trail's new stretches sit still once
//!   drawn, so no true velocity helps. Reactivity would cost the world behind them its history, and
//!   each trail edge's crispness to the jitter. Drawn after the resolve they ghost not at all, cost
//!   the world nothing and stay crisp at the output's size, upsampled or not. They read the scene
//!   prepass's depth by hand there, four texels at a time, which is also what makes them soft.
//!
//! # Scene color's alpha
//!
//! Alpha 0 marks characters for post (M1 P2). A spark over a character should bloom as a spark
//! anywhere does, not by skin's higher threshold and lower strength, so wherever an effect covers a
//! pixel it raises the world's share by its coverage. Blending premultiplied light needs alpha for
//! itself, so the mask is a draw of its own, writing alpha alone, at the end of the main pass on
//! every tier (`VfxGpu`'s mask pipeline), only where post reads the mask; under temporal AA that's the
//! scene color post reads the mask from, and on phones the color draw leaves alpha alone.
//!
//! # Light
//!
//! An emissive color is relative to the display: the vertex stage divides it by the frame's
//! exposure, so a core is as bright against the tone curve at noon as at midnight, and only a lit
//! quad's light (dust) follows the hour, as an albedo under the frame's ambient and key light. Each
//! ends through the fog as `fogged` has it, per vertex, since a particle is small and a frame stacks
//! many: an added light takes the fog's transmittance alone, as what's behind it has its fog
//! already, and whatever covers what's behind brings the fog's own light for that share.
//!
//! # Order
//!
//! One draw of `sprites + ribbon points` instances, sprites back to front, sorted here each frame
//! (premultiplied quads that cover need it, and light doesn't mind), then ribbons. The sort and the
//! upload are the frame's only per-effect CPU work here; the simulation's is `sapphire_vfx`'s, and
//! §3.2 budgets the two of them together.
//!
//! Sprites sort by their middles' depth along the view, not their distance from the eye: a quad
//! that faces the camera whole lies in a plane of one depth, and off to the side a nearer one's
//! middle can be the farther from the eye (review REN-001). A streak, turned about its axis, lies
//! across depths, so for it this is a good guess, and two that cross have no one right order.
//!
//! # Debug views
//!
//! The views replay the main pass's meshes, and effects aren't meshes, so they don't show there.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use sapphire_look::{VfxFrame, VfxLight, VfxRibbonPoint, VfxShape, VfxSprite, VfxStats};
use sapphire_rhi::{TransientDesc, wgpu};

use crate::CameraData;
use crate::bind_cache::CachedBindGroup;
use crate::buffer::{self, GrowableBuffer};
use crate::graph::{PassContext, TextureId};
use crate::pipelines::{DEPTH_FORMAT, SCENE_COLOR_FORMAT};
use crate::shader_builds::Step;
use crate::shaders::{self, BuildOn, Program, ShaderSources};

/// The frame graph's pass under temporal AA, whose time is effects' GPU cost, against spec §3.3's
/// "Particles, distortion" (1.2 ms on Desktop-Low). Elsewhere they draw inside the main pass, and
/// comparing that pass's time with and without them is the only measure.
pub const PASS: &str = "vfx";

/// The name of the shader's constant: the pipeline draws over the resolved frame.
const AFTER_RESOLVE_CONSTANT: &str = "VFX_AFTER_RESOLVE";

/// The shapes' map, a side. The largest a shape draws is a hit's flash, about a third of a frame
/// at a blade's reach on a close camera.
const MAP_SIZE: u32 = 128;

/// The brightest color a quad may hold, in scene light: well above anything a document means, and
/// far from the half floats' limit once multiplied by coverage.
const COLOR_MAX: f32 = 1024.0;

/// What each instance buffer holds when first made: a duel's worth, so few frames grow them.
const INITIAL_ITEMS: u64 = 512;

/// What the frame's effects drew (rule R9), and what the simulation that made them holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VfxRenderStats {
    /// Sprites drawn in the last prepared frame.
    pub sprites: u32,
    /// Ribbon points drawn in the last prepared frame.
    pub ribbon_points: u32,
    /// Sprites and points left out because they weren't finite or didn't fit their buffer's limit.
    pub rejected: u32,
    /// Pipelines built: 0 until a frame has effects, or [`Renderer::prewarm_vfx`](crate::Renderer::prewarm_vfx).
    pub pipelines: u32,
    /// Bytes of instance data the last prepared frame uploads.
    pub bytes: u64,
    /// Whether they draw after temporal AA's resolve, in [`PASS`], rather than in the main pass.
    pub after_resolve: bool,
    /// What the simulation reported with the frame.
    pub simulation: VfxStats,
}

/// A quad (`VfxSprite` in `vfx.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct GpuVfxSprite {
    position: [f32; 3],
    half_width: f32,
    axis: [f32; 3],
    rotation: f32,
    color: [f32; 3],
    alpha: f32,
    half_length: f32,
    additive: f32,
    soft: f32,
    flags: u32,
}

/// A point of a ribbon (`VfxRibbonPoint` in `vfx.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct GpuVfxRibbonPoint {
    base: [f32; 3],
    ends: u32,
    tip: [f32; 3],
    core_position: f32,
    color: [f32; 3],
    alpha: f32,
    core: [f32; 3],
    core_half_width: f32,
    glow_reach: f32,
    additive: f32,
    soft: f32,
    padding: f32,
}

/// `VfxUniforms` in `vfx.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(crate) struct GpuVfxUniforms {
    target_size: [f32; 4],
    sprite_count: u32,
    emissive: f32,
    padding: [f32; 2],
}

/// `GpuVfxSprite::flags`' bit for a lit quad (`VFX_LIT`).
const LIT: u32 = 256;

/// `value` when it's finite and not negative, else 0, and at most `max`.
fn sane(value: f32, max: f32) -> f32 {
    value.max(0.0).min(max)
}

fn sane_color(color: Vec3) -> [f32; 3] {
    color.max(Vec3::ZERO).min(Vec3::splat(COLOR_MAX)).to_array()
}

impl GpuVfxSprite {
    /// `sprite`, unless where it stands isn't finite: its colors and sizes are made safe, so nothing
    /// a document or a bad step wrote can reach bloom as an infinity.
    fn new(sprite: &VfxSprite) -> Option<Self> {
        if !sprite.position.is_finite() {
            return None;
        }
        let axis = if sprite.axis.is_finite() { sprite.axis } else { Vec3::ZERO };
        let rotation = if sprite.rotation.is_finite() { sprite.rotation } else { 0.0 };
        let lit = if sprite.light == VfxLight::Lit { LIT } else { 0 };
        Some(Self {
            position: sprite.position.to_array(),
            half_width: sane(sprite.half_width, f32::MAX),
            axis: axis.to_array(),
            rotation,
            color: sane_color(sprite.color),
            alpha: sane(sprite.alpha, 1.0),
            half_length: sane(sprite.half_length, f32::MAX),
            additive: sane(sprite.additive, 1.0),
            soft: sane(sprite.soft, f32::MAX),
            flags: sprite.shape.layer() | lit,
        })
    }
}

impl GpuVfxRibbonPoint {
    /// `point`, unless its rails aren't finite: its colors are made safe as a sprite's are.
    fn new(point: &VfxRibbonPoint) -> Option<Self> {
        if !point.base.is_finite() || !point.tip.is_finite() {
            return None;
        }
        Some(Self {
            base: point.base.to_array(),
            ends: u32::from(point.ends),
            tip: point.tip.to_array(),
            core_position: sane(point.core_position, 1.0),
            color: sane_color(point.color),
            alpha: sane(point.alpha, 1.0),
            core: sane_color(point.core),
            core_half_width: sane(point.core_half_width, 1.0),
            glow_reach: sane(point.glow_reach, f32::MAX).max(1e-4),
            additive: sane(point.additive, 1.0),
            soft: sane(point.soft, f32::MAX),
            padding: 0.0,
        })
    }
}

/// The map of the shapes' coverage at `level` of a square of `size`: `VfxShape::ALL`'s layers, each
/// a mean of the level above, down to one texel. Generated in code, so no art is involved.
fn generate_map(size: u32) -> Vec<Vec<Vec<u8>>> {
    let levels = u32::BITS - size.leading_zeros();
    VfxShape::ALL
        .map(|shape| {
            let mut mips: Vec<Vec<f32>> = Vec::with_capacity(levels as usize);
            let at = |index: u32| (index as f32 + 0.5) / size as f32;
            let top = (0..size)
                .flat_map(|y| (0..size).map(move |x| (x, y)))
                .map(|(x, y)| shape.coverage(at(x), at(y)))
                .collect();
            mips.push(top);
            for level in 1..levels {
                let side = (size >> level).max(1) as usize;
                let above = &mips[level as usize - 1];
                let mut mip = Vec::with_capacity(side * side);
                for y in 0..side {
                    for x in 0..side {
                        let texel = |dx: usize, dy: usize| above[(2 * y + dy) * (2 * side) + 2 * x + dx];
                        mip.push((texel(0, 0) + texel(1, 0) + texel(0, 1) + texel(1, 1)) * 0.25);
                    }
                }
                mips.push(mip);
            }
            mips.into_iter()
                .map(|mip| mip.into_iter().map(|value| (value * 255.0).round() as u8).collect())
                .collect()
        })
        .to_vec()
}

/// What pipelines are built with: fixed for a renderer's life.
#[derive(Clone, Debug)]
struct Builder {
    device: wgpu::Device,
    layout: wgpu::PipelineLayout,
    /// The main pass's sample count.
    samples: u32,
    /// Effects draw after temporal AA's resolve.
    after_resolve: bool,
    cache: Option<wgpu::PipelineCache>,
}

/// What blends premultiplied light over what's behind, and writes the color alone: its source alpha
/// belongs to the blend.
const COLOR_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

/// What raises scene color's alpha by the coverage: the mask draw's.
const MASK_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

impl Builder {
    /// A pipeline of `module`: `fragment`, over the resolved frame if `after_resolve` (one sample,
    /// no depth), otherwise in the main pass, at its sample count against its depth.
    fn pipeline(
        &self,
        module: &wgpu::ShaderModule,
        label: &'static str,
        fragment: &'static str,
        blend: wgpu::BlendState,
        write_mask: wgpu::ColorWrites,
        after_resolve: bool,
    ) -> wgpu::RenderPipeline {
        let constants = [(AFTER_RESOLVE_CONSTANT, if after_resolve { 1.0 } else { 0.0 })];
        let compilation_options =
            wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() };
        let depth = (!after_resolve).then(|| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: Default::default(),
        });
        let samples = if after_resolve { 1 } else { self.samples };
        self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_vfx"),
                compilation_options: compilation_options.clone(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: depth,
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(fragment),
                compilation_options,
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_COLOR_FORMAT,
                    blend: Some(blend),
                    write_mask,
                })],
            }),
            multiview_mask: None,
            cache: self.cache.as_ref(),
        })
    }

    fn color(&self, module: &wgpu::ShaderModule) -> wgpu::RenderPipeline {
        let (write, resolve) = (wgpu::ColorWrites::COLOR, self.after_resolve);
        self.pipeline(module, "vfx", "fs_vfx", COLOR_BLEND, write, resolve)
    }

    /// Always in the main pass, where scene color's alpha is.
    fn mask(&self, module: &wgpu::ShaderModule) -> wgpu::RenderPipeline {
        self.pipeline(module, "vfx mask", "fs_vfx_mask", MASK_BLEND, wgpu::ColorWrites::ALPHA, false)
    }
}

/// The pipelines built so far, and the module later ones come from: what [`Vfx::rebuild`] builds.
#[derive(Clone, Debug)]
pub(crate) struct VfxPipelines {
    module: wgpu::ShaderModule,
    color: Option<wgpu::RenderPipeline>,
    mask: Option<wgpu::RenderPipeline>,
}

impl VfxPipelines {
    /// How many are built, for the shader builds' count.
    pub(crate) fn count(&self) -> u32 {
        u32::from(self.color.is_some()) + u32::from(self.mask.is_some())
    }
}

/// Everything effects make when they're first drawn.
#[derive(Debug)]
struct VfxGpu {
    builder: Builder,
    pipelines: VfxPipelines,
    sprites: GrowableBuffer,
    ribbon: GrowableBuffer,
    uniforms: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// Group 2's layout: the instance buffers, the uniforms and the sampler.
    buffers_layout: wgpu::BindGroupLayout,
    /// Group 2, made again when a buffer grows.
    buffers_group: wgpu::BindGroup,
    /// Group 3's layout: the map and the depth it reads.
    textures_layout: wgpu::BindGroupLayout,
    map: wgpu::TextureView,
    /// Group 3 with a 1×1 depth stand-in nothing reads: the main pass's draws.
    main_group: wgpu::BindGroup,
    /// Group 3 with the scene prepass's depth, the history pair's two.
    depth_groups: CachedBindGroup<2>,
}

/// What groups 2 and 3 are laid out as.
#[derive(Debug)]
struct Layouts {
    buffers: wgpu::BindGroupLayout,
    textures: wgpu::BindGroupLayout,
}

impl Layouts {
    fn new(device: &wgpu::Device) -> Self {
        use wgpu::ShaderStages as S;
        let storage = |binding, size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            // The fragment stage's storage buffers are all taken (`frame.wgsl`).
            visibility: S::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(size as u64),
            },
            count: None,
        };
        let buffers = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vfx buffers"),
            entries: &[
                storage(0, size_of::<GpuVfxSprite>()),
                storage(1, size_of::<GpuVfxRibbonPoint>()),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: S::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<GpuVfxUniforms>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: S::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let textures = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vfx textures"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: S::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: S::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        Self { buffers, textures }
    }
}

/// The shapes' map, on the GPU, every level uploaded.
fn upload_map(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let levels = generate_map(MAP_SIZE);
    let mip_level_count = levels[0].len() as u32;
    let map = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vfx shapes"),
        size: wgpu::Extent3d {
            width: MAP_SIZE,
            height: MAP_SIZE,
            depth_or_array_layers: VfxShape::ALL.len() as u32,
        },
        mip_level_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (layer, mips) in levels.iter().enumerate() {
        for (level, texels) in mips.iter().enumerate() {
            let side = (MAP_SIZE >> level).max(1);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &map,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(side),
                    rows_per_image: Some(side),
                },
                wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
            );
        }
    }
    map.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

impl VfxGpu {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        builder: Builder,
        pipelines: VfxPipelines,
        layouts: Layouts,
    ) -> Self {
        let limit = device.limits().max_storage_buffer_binding_size;
        let storage = |label, item: usize| {
            let capacity = INITIAL_ITEMS * item as u64;
            GrowableBuffer::new(device, label, wgpu::BufferUsages::STORAGE, capacity, limit)
        };
        let sprites = storage("vfx sprites", size_of::<GpuVfxSprite>());
        let ribbon = storage("vfx ribbon", size_of::<GpuVfxRibbonPoint>());
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vfx uniforms"),
            size: size_of::<GpuVfxUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vfx shapes"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let map = upload_map(device, queue);
        let stand_in = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("vfx depth stand-in"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let main_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vfx textures"),
            layout: &layouts.textures,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&map) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&stand_in) },
            ],
        });
        let buffers_group =
            buffers_group(device, &layouts.buffers, sprites.buffer(), ribbon.buffer(), &uniforms, &sampler);
        Self {
            builder,
            pipelines,
            sprites,
            ribbon,
            uniforms,
            sampler,
            buffers_layout: layouts.buffers,
            buffers_group,
            textures_layout: layouts.textures,
            map,
            main_group,
            depth_groups: CachedBindGroup::default(),
        }
    }
}

fn buffers_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sprites: &wgpu::Buffer,
    ribbon: &wgpu::Buffer,
    uniforms: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vfx buffers"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: sprites.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: ribbon.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

/// The frame's effects' share of the renderer.
#[derive(Debug)]
pub(crate) struct Vfx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    frame_layout: wgpu::BindGroupLayout,
    view_layout: wgpu::BindGroupLayout,
    samples: u32,
    after_resolve: bool,
    cache: Option<wgpu::PipelineCache>,
    gpu: Option<Box<VfxGpu>>,
    /// This frame's sprites from back to front, and the sort's keys, kept across frames so a warm
    /// frame allocates nothing.
    sprites: Vec<GpuVfxSprite>,
    order: Vec<(f32, u32)>,
    points: Vec<GpuVfxRibbonPoint>,
    /// The reciprocal of the frame's exposure.
    emissive: f32,
    /// The frame's post reads scene color's alpha.
    mask_read: bool,
    stats: VfxRenderStats,
}

impl Vfx {
    /// Holds what later frames need to make its resources, and makes none of them: a renderer that
    /// never draws an effect never pays for one.
    pub(crate) fn new(
        gpu: &sapphire_rhi::GpuContext,
        frame_layout: &wgpu::BindGroupLayout,
        view_layout: &wgpu::BindGroupLayout,
        samples: u32,
        after_resolve: bool,
    ) -> Self {
        Self {
            device: gpu.device().clone(),
            queue: gpu.queue().clone(),
            frame_layout: frame_layout.clone(),
            view_layout: view_layout.clone(),
            samples,
            after_resolve,
            cache: gpu.pipeline_cache().cloned(),
            gpu: None,
            sprites: Vec::new(),
            order: Vec::new(),
            points: Vec::new(),
            emissive: 1.0,
            mask_read: false,
            stats: VfxRenderStats { after_resolve, ..VfxRenderStats::default() },
        }
    }

    /// Makes what the first effect needs, and the pipelines it draws through (`mask` too, if post
    /// reads scene color's alpha), that aren't built yet. A load-time cost where it can be
    /// ([`Renderer::prewarm_vfx`](crate::Renderer::prewarm_vfx)): on the frame thread otherwise.
    pub(crate) fn build(&mut self, sources: &ShaderSources, mask: bool, on: BuildOn) {
        if self.gpu.is_none() {
            let device = &self.device;
            let layouts = Layouts::new(device);
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("vfx"),
                bind_group_layouts: &[
                    Some(&self.frame_layout),
                    Some(&self.view_layout),
                    Some(&layouts.buffers),
                    Some(&layouts.textures),
                ],
                immediate_size: 0,
            });
            let builder = Builder {
                device: device.clone(),
                layout,
                samples: self.samples,
                after_resolve: self.after_resolve,
                cache: self.cache.clone(),
            };
            let module = shaders::module(device, Program::Vfx, sources);
            let pipelines = VfxPipelines { module, color: None, mask: None };
            self.gpu = Some(Box::new(VfxGpu::new(device, &self.queue, builder, pipelines, layouts)));
        }
        let gpu = self.gpu.as_mut().expect("made above");
        let builder = &gpu.builder;
        let module = &gpu.pipelines.module;
        let mut missing: Vec<bool> = Vec::new();
        if gpu.pipelines.color.is_none() {
            missing.push(false);
        }
        if mask && gpu.pipelines.mask.is_none() {
            missing.push(true);
        }
        if missing.is_empty() {
            return;
        }
        let builds = missing
            .iter()
            .map(|&mask| move || if mask { builder.mask(module) } else { builder.color(module) })
            .collect();
        let built = on.run(builds);
        for (is_mask, pipeline) in missing.iter().zip(built) {
            let slot = if *is_mask { &mut gpu.pipelines.mask } else { &mut gpu.pipelines.color };
            *slot = Some(pipeline);
        }
        self.stats.pipelines = gpu.pipelines.count();
    }

    /// The CPU half of a frame: orders the frame's effects, back to front from `camera`, sizes their
    /// buffers and builds what they first need from `sources`. `exposure` is the frame's multiplier,
    /// and `mask` whether post reads scene color's alpha. Empty, it costs this call alone.
    pub(crate) fn prepare(
        &mut self,
        frame: &VfxFrame,
        camera: Option<&CameraData>,
        exposure: f32,
        mask: bool,
        sources: &ShaderSources,
        on: BuildOn,
    ) {
        profiling::scope!("Vfx::prepare");
        self.sprites.clear();
        self.points.clear();
        self.order.clear();
        self.stats.sprites = 0;
        self.stats.ribbon_points = 0;
        self.stats.rejected = 0;
        self.stats.bytes = 0;
        self.stats.simulation = frame.stats;
        let Some(camera) = camera.filter(|_| !frame.is_empty()) else { return };
        self.build(sources, mask, on);
        let gpu = self.gpu.as_mut().expect("just built");
        self.emissive = 1.0 / exposure.max(1e-6);
        self.mask_read = mask;

        // Whatever would pass a buffer's limit doesn't draw (review RA-004): the simulation's caps
        // are far below it.
        let sprite_size = size_of::<GpuVfxSprite>() as u64;
        let point_size = size_of::<GpuVfxRibbonPoint>() as u64;
        let max_sprites = gpu.sprites.max_items(sprite_size) as usize;
        let max_points = gpu.ribbon.max_items(point_size) as usize;
        let sprites = &frame.sprites[..frame.sprites.len().min(max_sprites)];
        self.order.extend(
            sprites
                .iter()
                .enumerate()
                .filter(|(_, sprite)| sprite.position.is_finite())
                .map(|(index, sprite)| (camera.view_depth(sprite.position), index as u32)),
        );
        // Ties keep the frame's order; unstable, so sorting never allocates.
        self.order.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        self.sprites
            .extend(self.order.iter().filter_map(|&(_, index)| GpuVfxSprite::new(&sprites[index as usize])));
        let points = &frame.ribbon_points[..frame.ribbon_points.len().min(max_points)];
        self.points.extend(points.iter().filter_map(GpuVfxRibbonPoint::new));
        // A list cut short, or missing its last point, still ends.
        if let Some(last) = self.points.last_mut() {
            last.ends = 1;
        }
        self.stats.sprites = self.sprites.len() as u32;
        self.stats.ribbon_points = self.points.len() as u32;
        let total = frame.sprites.len() + frame.ribbon_points.len();
        self.stats.rejected = (total - self.sprites.len() - self.points.len()) as u32;
        let (sprite_bytes, point_bytes) =
            (self.sprites.len() as u64 * sprite_size, self.points.len() as u64 * point_size);
        self.stats.bytes = sprite_bytes + point_bytes;
        let grew = gpu.sprites.reserve(&self.device, &self.queue, sprite_bytes, 0)
            | gpu.ribbon.reserve(&self.device, &self.queue, point_bytes, 0);
        if grew {
            gpu.buffers_group = buffers_group(
                &self.device,
                &gpu.buffers_layout,
                gpu.sprites.buffer(),
                gpu.ribbon.buffer(),
                &gpu.uniforms,
                &gpu.sampler,
            );
        }
    }

    /// Whether this frame draws effects.
    fn drawn(&self) -> bool {
        self.gpu.is_some() && !(self.sprites.is_empty() && self.points.is_empty())
    }

    /// The frame's uploads and draws, for [`Renderer::record`](crate::Renderer::record): `None` when
    /// it has no effects. `output` is the target's size.
    pub(crate) fn split_for_record(&mut self, output: (u32, u32)) -> Option<(VfxUpload<'_>, VfxDraw<'_>)> {
        if !self.drawn() {
            return None;
        }
        let gpu = self.gpu.as_mut()?;
        let uniforms = GpuVfxUniforms {
            target_size: [
                output.0 as f32,
                output.1 as f32,
                1.0 / output.0.max(1) as f32,
                1.0 / output.1.max(1) as f32,
            ],
            sprite_count: self.sprites.len() as u32,
            emissive: self.emissive,
            padding: [0.0; 2],
        };
        let upload = VfxUpload {
            uniforms,
            uniforms_buffer: &gpu.uniforms,
            sprites: &self.sprites,
            sprites_buffer: gpu.sprites.buffer(),
            points: &self.points,
            points_buffer: gpu.ribbon.buffer(),
        };
        let instances = (self.sprites.len() + self.points.len()) as u32;
        let color = gpu.pipelines.color.as_ref()?;
        let main = VfxMainDraw {
            color: (!self.after_resolve).then_some(color),
            mask: gpu.pipelines.mask.as_ref().filter(|_| self.mask_read),
            buffers: &gpu.buffers_group,
            textures: &gpu.main_group,
            instances,
        };
        let after = self.after_resolve.then_some(VfxAfterResolve {
            device: &self.device,
            color,
            buffers: &gpu.buffers_group,
            textures_layout: &gpu.textures_layout,
            map: &gpu.map,
            depth_groups: &mut gpu.depth_groups,
            instances,
        });
        Some((upload, VfxDraw { main, after }))
    }

    /// The pipelines built so far, to build anew from new sources (`shader_builds`). `None` until
    /// the first effect: nothing to build.
    pub(crate) fn rebuild(&self) -> Option<Step<VfxPipelines>> {
        let gpu = self.gpu.as_ref()?;
        let builder = gpu.builder.clone();
        let (color, mask) = (gpu.pipelines.color.is_some(), gpu.pipelines.mask.is_some());
        Some(Box::new(move |sources, _cancel| {
            let module = shaders::module(&builder.device, Program::Vfx, sources);
            VfxPipelines {
                color: color.then(|| builder.color(&module)),
                mask: mask.then(|| builder.mask(&module)),
                module,
            }
        }))
    }

    /// How many pipelines it has built, which a build off the frame thread checks it rebuilt.
    pub(crate) fn pipelines_built(&self) -> u32 {
        self.stats.pipelines
    }

    pub(crate) fn set_pipelines(&mut self, pipelines: VfxPipelines) {
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.pipelines = pipelines;
            self.stats.pipelines = gpu.pipelines.count();
        }
    }

    pub(crate) fn stats(&self) -> VfxRenderStats {
        self.stats
    }
}

/// The frame's effects' uploads.
#[derive(Debug)]
pub(crate) struct VfxUpload<'a> {
    uniforms: GpuVfxUniforms,
    uniforms_buffer: &'a wgpu::Buffer,
    sprites: &'a [GpuVfxSprite],
    sprites_buffer: &'a wgpu::Buffer,
    points: &'a [GpuVfxRibbonPoint],
    points_buffer: &'a wgpu::Buffer,
}

impl VfxUpload<'_> {
    pub(crate) fn write(&self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        buffer::stage(belt, encoder, self.uniforms_buffer, 0, bytemuck::bytes_of(&self.uniforms));
        buffer::stage(belt, encoder, self.sprites_buffer, 0, bytemuck::cast_slice(self.sprites));
        buffer::stage(belt, encoder, self.points_buffer, 0, bytemuck::cast_slice(self.points));
    }
}

/// The frame's effects' draws.
#[derive(Debug)]
pub(crate) struct VfxDraw<'a> {
    /// What ends the main pass.
    pub main: VfxMainDraw<'a>,
    /// What draws over temporal AA's resolve, in [`PASS`]: `None` without temporal AA.
    pub after: Option<VfxAfterResolve<'a>>,
}

/// What ends the main pass: the mask, where post reads it, and the color without temporal AA.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VfxMainDraw<'a> {
    color: Option<&'a wgpu::RenderPipeline>,
    mask: Option<&'a wgpu::RenderPipeline>,
    buffers: &'a wgpu::BindGroup,
    textures: &'a wgpu::BindGroup,
    instances: u32,
}

impl VfxMainDraw<'_> {
    /// Groups 0 and 1 stay as the main pass bound them.
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(2, self.buffers, &[]);
        pass.set_bind_group(3, self.textures, &[]);
        for pipeline in [self.mask, self.color].into_iter().flatten() {
            pass.set_pipeline(pipeline);
            pass.draw(0..4, 0..self.instances);
        }
    }
}

/// What draws over temporal AA's resolve.
#[derive(Debug)]
pub(crate) struct VfxAfterResolve<'a> {
    device: &'a wgpu::Device,
    color: &'a wgpu::RenderPipeline,
    buffers: &'a wgpu::BindGroup,
    textures_layout: &'a wgpu::BindGroupLayout,
    map: &'a wgpu::TextureView,
    depth_groups: &'a mut CachedBindGroup<2>,
    instances: u32,
}

/// The pass's textures.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AfterResolveTargets {
    /// Temporal AA's resolve, read.
    pub resolved: TextureId,
    /// Where it's copied and the effects draw, which post reads instead.
    pub composed: TextureId,
    /// The scene prepass's depth.
    pub depth: TextureId,
}

impl VfxAfterResolve<'_> {
    /// Copies the resolved frame and draws over it. `frame` and `view` are groups 0 and 1.
    pub(crate) fn record(
        self,
        ctx: &mut PassContext<'_>,
        targets: AfterResolveTargets,
        frame: &wgpu::BindGroup,
        view: &wgpu::BindGroup,
        labels: bool,
    ) {
        let (resolved, composed) = (ctx.texture(targets.resolved), ctx.texture(targets.composed));
        ctx.encoder.copy_texture_to_texture(
            resolved.as_image_copy(),
            composed.as_image_copy(),
            resolved.size(),
        );
        let label = labels.then_some("vfx");
        let depth = ctx.view(targets.depth);
        let textures = self.depth_groups.get(self.device, label, self.textures_layout, &[self.map, depth]);
        let timestamp_writes = ctx.timestamp_writes();
        let attachment = ctx.view(targets.composed);
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: labels.then_some(PASS),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: attachment,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            timestamp_writes,
            ..Default::default()
        });
        pass.set_pipeline(self.color);
        pass.set_bind_group(0, frame, &[]);
        pass.set_bind_group(1, view, &[0]);
        pass.set_bind_group(2, self.buffers, &[]);
        pass.set_bind_group(3, textures, &[]);
        pass.draw(0..4, 0..self.instances);
    }
}

/// The texture the pass draws in: the resolved frame's size and format.
pub(crate) fn composed_desc(size: (u32, u32)) -> TransientDesc {
    TransientDesc {
        width: size.0,
        height: size.1,
        format: SCENE_COLOR_FORMAT,
        sample_count: 1,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST,
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;

    fn sprite() -> VfxSprite {
        VfxSprite {
            position: Vec3::ZERO,
            axis: Vec3::ZERO,
            rotation: 0.0,
            half_width: 0.1,
            half_length: 0.1,
            color: Vec3::ONE,
            alpha: 1.0,
            additive: 1.0,
            soft: 0.02,
            shape: VfxShape::Star,
            light: VfxLight::Lit,
        }
    }

    #[test]
    fn the_map_is_every_shape_down_to_one_texel() {
        let map = generate_map(8);
        assert_eq!(map.len(), VfxShape::ALL.len());
        for mips in &map {
            let sides: Vec<usize> = mips.iter().map(Vec::len).collect();
            assert_eq!(sides, [64, 16, 4, 1]);
        }
        // Each level holds the mean of the level above, so the ring's and streak's thin strokes
        // fade with distance rather than shimmer.
        let top: f32 = map[0][0].iter().map(|&texel| f32::from(texel)).sum::<f32>() / 64.0;
        assert!((f32::from(map[0][3][0]) - top).abs() <= 2.0, "{} against {top}", map[0][3][0]);
    }

    #[test]
    fn a_sprite_packs_its_shape_and_light_into_its_flags() {
        let gpu = GpuVfxSprite::new(&sprite()).expect("finite");
        assert_eq!(gpu.flags, VfxShape::Star.layer() | LIT);
        let emissive =
            GpuVfxSprite::new(&VfxSprite { light: VfxLight::Emissive, ..sprite() }).expect("finite");
        assert_eq!(emissive.flags, VfxShape::Star.layer());
    }

    #[test]
    fn bad_values_reach_the_shaders_safe() {
        let bad = VfxSprite {
            axis: Vec3::NAN,
            rotation: f32::INFINITY,
            half_width: f32::NAN,
            half_length: -1.0,
            color: Vec3::new(f32::NAN, f32::INFINITY, -1.0),
            alpha: f32::NAN,
            additive: 2.0,
            soft: f32::NAN,
            ..sprite()
        };
        let gpu = GpuVfxSprite::new(&bad).expect("where it stands is finite");
        assert_eq!(gpu.axis, [0.0; 3]);
        assert_eq!(gpu.rotation, 0.0);
        assert_eq!(gpu.half_width, 0.0);
        assert_eq!(gpu.half_length, 0.0);
        assert_eq!(gpu.color, [0.0, COLOR_MAX, 0.0]);
        assert_eq!(gpu.alpha, 0.0);
        assert_eq!(gpu.additive, 1.0);
        assert_eq!(gpu.soft, 0.0);
        assert!(GpuVfxSprite::new(&VfxSprite { position: Vec3::NAN, ..sprite() }).is_none());
        let point = VfxRibbonPoint {
            base: Vec3::ZERO,
            tip: Vec3::Y,
            color: Vec3::NAN,
            alpha: 1.0,
            core: Vec3::INFINITY,
            core_position: 0.9,
            core_half_width: 0.05,
            glow_reach: 0.0,
            additive: 1.0,
            soft: 0.02,
            ends: true,
        };
        let gpu = GpuVfxRibbonPoint::new(&point).expect("finite rails");
        assert_eq!(gpu.color, [0.0; 3]);
        assert_eq!(gpu.core, [COLOR_MAX; 3]);
        assert_eq!(gpu.glow_reach, 1e-4, "it divides by this");
        assert_eq!(gpu.ends, 1);
        assert!(GpuVfxRibbonPoint::new(&VfxRibbonPoint { tip: Vec3::NAN, ..point }).is_none());
    }
}
