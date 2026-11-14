//! The frame's effect meshes (M5 L5): [`VfxFrame::meshes`](sapphire_look::VfxFrame), generated shapes
//! `sapphire_vfx` places and colors, drawn one indexed instance each through `shaders/vfx_mesh.wgsl`. A family of
//! their own ([`PipelineFamily::EffectMeshes`]), apart from the sprites': its own pipelines, uniforms, buffers and
//! pattern array, and the world's frame group ([`FrameGroup::World`](crate::pipelines::FrameGroup)). **Nothing of
//! it exists until a frame has a mesh**, or a load declares them
//! ([`PipelineInventory::effect_meshes`](crate::PipelineInventory)): a frame of sprites alone builds none of it.
//!
//! - **Where they draw:** where sprites draw, by the same rules (the parent module's docs): after the resolve
//!   under temporal AA, testing depth by hand and fading into what's behind; at the end of the main pass
//!   without, against its depth, with no soft fade. They take the sprites' blends: premultiplied, `additive`
//!   between light and a surface that covers. They write no depth, and draw both faces.
//! - **Their order** is the sprites': back to front by their middles' view depth, each mesh just before the
//!   first sprite nearer than its middle, so a mesh and the sprites of its own effect sort together. Ribbons
//!   keep their places among the sprites; a mesh nearer than every sprite draws after them all.
//! - **Unlit:** a mesh's colors are relative to the display, as an emissive sprite's: × 1/exposure.
//! - **Patterns** are `VfxPattern`'s, rasterized here into an array at first use, a mip chain each, sampled
//!   repeating so a scroll never shows a seam. A flipbook reads one cell of its sheet, and the next to blend.
//! - **Distortion (M5 L6) hooks in here:** bit 2 of a mesh's flags (`vfx_mesh.wgsl`'s comment) is kept for a
//!   mesh that offsets what's behind it. L6 adds a second color target to [`MeshBuilder::pipeline`]'s
//!   after-resolve pipelines, an offset texture the after-resolve pass writes beside the composed frame, and
//!   post's fused pass reads it; on phones the main pass's draw writes it. Nothing builds it here.
//! - **Ground decals (M5 L7)** can reuse the rest: a `Card` laid flat is a ground-facing quad, the shape list
//!   ([`VfxMeshShape`]) is where an SDF shape's quad joins, and the document's layer, the order and the draw
//!   point are the same.

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use sapphire_look::vfx::mesh::PATTERN_SIZE;
use sapphire_look::{VfxMesh, VfxMeshShape, VfxMeshVertex, VfxPattern};
use sapphire_rhi::wgpu;
use wgpu::util::DeviceExt as _;

use super::{COLOR_BLEND, COLOR_MAX, MASK_BLEND, VfxMaker, sane, sane_color};
use crate::CameraData;
use crate::bind_cache::CachedBindGroup;
use crate::buffer::{self, GrowableBuffer};
use crate::pipeline_builds::{PipelineBuilds, PipelineFamily};
use crate::pipelines::{DEPTH_FORMAT, SCENE_COLOR_FORMAT};
use crate::shader_builds::Step;
use crate::shaders::{self, BuildOn, Program, ShaderSources};

/// The name of the shader's constant: the pipeline draws over the resolved frame.
const AFTER_RESOLVE_CONSTANT: &str = "VFX_MESH_AFTER_RESOLVE";

/// `GpuVfxMesh::flags`: the erosion is on, and the pattern is read as a flipbook.
const ERODES: u32 = 1;
const FLIPBOOK: u32 = 2;

/// Meshes the buffer holds when first made: past a desktop tier's budget, so it never grows in a fight.
const INITIAL_MESHES: u64 = 32;

/// A mesh (`VfxMesh` in `vfx_mesh.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct GpuVfxMesh {
    world_from_local: [[f32; 4]; 4],
    color: [f32; 3],
    alpha: f32,
    edge_color: [f32; 3],
    edge: f32,
    scroll: [f32; 2],
    tiling: [f32; 2],
    mask_tiling: [f32; 2],
    threshold: f32,
    additive: f32,
    cells: [f32; 4],
    cell_size: [f32; 2],
    blend: f32,
    soft: f32,
    pattern: u32,
    mask: u32,
    flags: u32,
    padding: u32,
}

/// `VfxMeshUniforms` in `vfx_mesh.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(crate) struct GpuVfxMeshUniforms {
    target_size: [f32; 4],
    emissive: f32,
    padding: [f32; 3],
}

/// A tiling's repeats: finite and above 0, at most this many.
const MAX_TILING: f32 = 64.0;

fn tiling(value: glam::Vec2) -> [f32; 2] {
    value.to_array().map(|c| if c.is_finite() && c > 0.0 { c.min(MAX_TILING) } else { 1.0 })
}

impl GpuVfxMesh {
    /// `mesh`, unless where it stands or how it's turned isn't finite: its colors and shares are made safe.
    fn new(mesh: &VfxMesh) -> Option<Self> {
        let scale = mesh.scale.max(Vec3::ZERO).min(Vec3::splat(1e4));
        if !(mesh.position.is_finite() && mesh.rotation.is_finite() && scale.is_finite()) {
            return None;
        }
        let rotation = mesh.rotation.normalize();
        if !rotation.is_finite() {
            return None;
        }
        let world_from_local = Mat4::from_scale_rotation_translation(scale, rotation, mesh.position);
        let mut flags = 0;
        let (mut edge_color, mut edge, mut threshold, mut mask, mut mask_tiling) =
            ([0.0; 3], 0.0, 0.0, VfxPattern::Solid.layer(), [1.0; 2]);
        if let Some(erosion) = mesh.erosion {
            flags |= ERODES;
            edge_color = sane_color(erosion.edge_color);
            edge = sane(erosion.edge, 1.0);
            threshold = sane(erosion.threshold, 1.0);
            mask = erosion.mask.layer();
            mask_tiling = tiling(erosion.tiling);
        }
        let (mut cells, mut cell_size, mut blend) = ([0.0; 4], [1.0; 2], 0.0);
        if let Some(book) = mesh.flipbook.filter(|book| book.columns > 0 && book.rows > 0) {
            flags |= FLIPBOOK;
            let (columns, rows) = (book.columns as f32, book.rows as f32);
            let corner = |frame: u32| {
                let frame = frame.min(book.columns * book.rows - 1);
                [(frame % book.columns) as f32 / columns, (frame / book.columns) as f32 / rows]
            };
            let (now, next) = (corner(book.frame), corner(book.next));
            cells = [now[0], now[1], next[0], next[1]];
            cell_size = [1.0 / columns, 1.0 / rows];
            blend = sane(book.blend, 1.0);
        }
        let scroll = mesh.scroll.to_array().map(|c| if c.is_finite() { c.rem_euclid(1.0) } else { 0.0 });
        Some(Self {
            world_from_local: world_from_local.to_cols_array_2d(),
            color: sane_color(mesh.color),
            alpha: sane(mesh.alpha, 1.0),
            edge_color,
            edge,
            scroll,
            tiling: tiling(mesh.tiling),
            mask_tiling,
            threshold,
            additive: sane(mesh.additive, 1.0),
            cells,
            cell_size,
            blend,
            soft: if mesh.soft.is_finite() && mesh.soft > 0.0 { mesh.soft.min(COLOR_MAX) } else { 1e-3 },
            pattern: mesh.pattern.layer(),
            mask,
            flags,
            padding: 0,
        })
    }
}

/// The patterns at each level of a square of `size`: `VfxPattern::ALL`'s layers, each level a mean of the one
/// above, down to one texel. Generated in code, so no art is involved.
fn generate_patterns(size: u32) -> Vec<Vec<Vec<u8>>> {
    let levels = u32::BITS - size.leading_zeros();
    VfxPattern::ALL
        .map(|pattern| {
            let at = |index: u32| (index as f32 + 0.5) / size as f32;
            let mut mips: Vec<Vec<f32>> = Vec::with_capacity(levels as usize);
            mips.push(
                (0..size)
                    .flat_map(|y| (0..size).map(move |x| (x, y)))
                    .map(|(x, y)| pattern.coverage(at(x), at(y)))
                    .collect(),
            );
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

/// The patterns on the GPU, every level uploaded.
fn upload_patterns(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let levels = generate_patterns(PATTERN_SIZE);
    let array = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vfx mesh patterns"),
        size: wgpu::Extent3d {
            width: PATTERN_SIZE,
            height: PATTERN_SIZE,
            depth_or_array_layers: VfxPattern::ALL.len() as u32,
        },
        mip_level_count: levels[0].len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (layer, mips) in levels.iter().enumerate() {
        for (level, texels) in mips.iter().enumerate() {
            let side = (PATTERN_SIZE >> level).max(1);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &array,
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
    array.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

/// Where a shape's triangles are in the shared buffers.
#[derive(Clone, Copy, Debug, Default)]
struct ShapeRange {
    indices: (u32, u32),
    base_vertex: i32,
}

/// Every shape's triangles in one vertex buffer and one index buffer.
#[derive(Debug)]
struct Shapes {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    ranges: [ShapeRange; VfxMeshShape::ALL.len()],
}

impl Shapes {
    fn new(device: &wgpu::Device) -> Self {
        let (mut vertices, mut indices) = (Vec::<VfxMeshVertex>::new(), Vec::<u16>::new());
        let mut ranges = [ShapeRange::default(); VfxMeshShape::ALL.len()];
        for shape in VfxMeshShape::ALL {
            let geometry = shape.geometry();
            let first = indices.len() as u32;
            ranges[shape.index()] = ShapeRange {
                indices: (first, first + geometry.indices.len() as u32),
                base_vertex: vertices.len() as i32,
            };
            vertices.extend_from_slice(&geometry.vertices);
            indices.extend_from_slice(&geometry.indices);
        }
        // Whole words: a buffer's size is a multiple of four.
        if indices.len() % 2 == 1 {
            indices.push(0);
        }
        let buffer = |label, contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        Self {
            vertices: buffer("vfx mesh shapes", bytemuck::cast_slice(&vertices), wgpu::BufferUsages::VERTEX),
            indices: buffer(
                "vfx mesh shape indices",
                bytemuck::cast_slice(&indices),
                wgpu::BufferUsages::INDEX,
            ),
            ranges,
        }
    }
}

/// What the meshes' pipelines are built with: fixed for a renderer's life.
#[derive(Clone, Debug)]
pub(super) struct MeshBuilder {
    device: wgpu::Device,
    layout: wgpu::PipelineLayout,
    samples: u32,
    after_resolve: bool,
    builds: PipelineBuilds,
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32];

impl MeshBuilder {
    /// As the sprites' `Builder::pipeline`, of triangle lists from the shapes' buffer, both faces.
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
        self.builds.render(
            &self.device,
            PipelineFamily::EffectMeshes,
            &wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&self.layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_vfx_mesh"),
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
                cache: self.builds.cache(),
            },
        )
    }

    fn color(&self, module: &wgpu::ShaderModule) -> wgpu::RenderPipeline {
        let after_resolve = self.after_resolve;
        self.pipeline(module, "vfx mesh", "fs_vfx_mesh", COLOR_BLEND, wgpu::ColorWrites::COLOR, after_resolve)
    }

    fn mask(&self, module: &wgpu::ShaderModule) -> wgpu::RenderPipeline {
        self.pipeline(
            module,
            "vfx mesh mask",
            "fs_vfx_mesh_mask",
            MASK_BLEND,
            wgpu::ColorWrites::ALPHA,
            false,
        )
    }
}

/// The meshes' pipelines built so far, and the module later ones come from.
#[derive(Clone, Debug)]
pub(crate) struct MeshPipelines {
    module: wgpu::ShaderModule,
    pub(super) color: Option<wgpu::RenderPipeline>,
    pub(super) mask: Option<wgpu::RenderPipeline>,
}

impl MeshPipelines {
    pub(super) fn count(&self) -> u32 {
        u32::from(self.color.is_some()) + u32::from(self.mask.is_some())
    }

    fn build_missing(&mut self, builder: &MeshBuilder, mask: bool, on: BuildOn) {
        let mut missing = Vec::new();
        if self.color.is_none() {
            missing.push(false);
        }
        if mask && self.mask.is_none() {
            missing.push(true);
        }
        if missing.is_empty() {
            return;
        }
        let module = &self.module;
        let builds = missing
            .iter()
            .map(|&mask| move || if mask { builder.mask(module) } else { builder.color(module) })
            .collect();
        for (is_mask, pipeline) in missing.iter().zip(on.run(builds)) {
            *(if *is_mask { &mut self.mask } else { &mut self.color }) = Some(pipeline);
        }
    }
}

/// Groups 2 and 3's layouts.
#[derive(Debug)]
struct MeshLayouts {
    buffers: wgpu::BindGroupLayout,
    textures: wgpu::BindGroupLayout,
}

fn layouts(device: &wgpu::Device) -> MeshLayouts {
    use wgpu::ShaderStages as S;
    let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
    let buffers = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("vfx mesh buffers"),
        entries: &[
            // The fragment stage's storage buffers are all taken (`frame.wgsl`).
            entry(
                0,
                S::VERTEX,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<GpuVfxMesh>() as u64),
                },
            ),
            entry(
                1,
                S::VERTEX_FRAGMENT,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<GpuVfxMeshUniforms>() as u64),
                },
            ),
            entry(2, S::FRAGMENT, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
        ],
    });
    let textures = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("vfx mesh textures"),
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
    MeshLayouts { buffers, textures }
}

fn builder(maker: &VfxMaker) -> (MeshBuilder, MeshLayouts) {
    let layouts = layouts(&maker.device);
    let layout = maker.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("vfx mesh"),
        bind_group_layouts: &[
            Some(&maker.frame_layout),
            Some(&maker.view_layout),
            Some(&layouts.buffers),
            Some(&layouts.textures),
        ],
        immediate_size: 0,
    });
    let builder = MeshBuilder {
        device: maker.device.clone(),
        layout,
        samples: maker.samples,
        after_resolve: maker.after_resolve,
        builds: maker.builds.clone(),
    };
    (builder, layouts)
}

/// What a warm-up made of the meshes' first frame: all of it, or the pipelines missing.
#[derive(Debug)]
pub(crate) enum WarmedMeshes {
    Made(Box<MeshGpu>),
    Pipelines(MeshPipelines),
}

/// Everything the meshes make when they're first drawn.
#[derive(Debug)]
pub(crate) struct MeshGpu {
    builder: MeshBuilder,
    pipelines: MeshPipelines,
    shapes: Shapes,
    meshes: GrowableBuffer,
    uniforms: wgpu::Buffer,
    sampler: wgpu::Sampler,
    buffers_layout: wgpu::BindGroupLayout,
    buffers_group: wgpu::BindGroup,
    textures_layout: wgpu::BindGroupLayout,
    patterns: wgpu::TextureView,
    /// Group 3 with a 1×1 depth stand-in nothing reads: the main pass's draws.
    main_group: wgpu::BindGroup,
    /// Group 3 with the scene prepass's depth, the history pair's two.
    depth_groups: CachedBindGroup<2>,
}

fn buffers_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    meshes: &wgpu::Buffer,
    uniforms: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vfx mesh buffers"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: meshes.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

impl MeshGpu {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        builder: MeshBuilder,
        pipelines: MeshPipelines,
        layouts: MeshLayouts,
    ) -> Self {
        let limit = device.limits().max_storage_buffer_binding_size;
        let capacity = INITIAL_MESHES * size_of::<GpuVfxMesh>() as u64;
        let meshes = GrowableBuffer::new(device, "vfx meshes", wgpu::BufferUsages::STORAGE, capacity, limit);
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vfx mesh uniforms"),
            size: size_of::<GpuVfxMeshUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vfx mesh patterns"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let patterns = upload_patterns(device, queue);
        let stand_in = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("vfx mesh depth stand-in"),
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
            label: Some("vfx mesh textures"),
            layout: &layouts.textures,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&patterns) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&stand_in) },
            ],
        });
        let buffers_group = buffers_group(device, &layouts.buffers, meshes.buffer(), &uniforms, &sampler);
        Self {
            builder,
            pipelines,
            shapes: Shapes::new(device),
            meshes,
            uniforms,
            sampler,
            buffers_layout: layouts.buffers,
            buffers_group,
            textures_layout: layouts.textures,
            patterns,
            main_group,
            depth_groups: CachedBindGroup::default(),
        }
    }
}

/// The frame's meshes' share of the effects ([`super::Vfx`] holds it, and what it needs to make its parts).
#[derive(Debug, Default)]
pub(crate) struct VfxMeshes {
    gpu: Option<Box<MeshGpu>>,
    /// This frame's meshes from back to front, their shapes, and their middles' view depths.
    packed: Vec<GpuVfxMesh>,
    shapes: Vec<VfxMeshShape>,
    depths: Vec<f32>,
    /// The sort's keys, kept so a warm frame allocates nothing.
    order: Vec<(f32, u32)>,
    /// Meshes left out because they weren't finite or didn't fit the buffer's limit.
    pub(super) rejected: u32,
}

impl VfxMeshes {
    /// Makes what the first mesh needs and builds its pipelines (`mask` too) that aren't built yet.
    pub(super) fn build(
        &mut self,
        maker: &VfxMaker,
        queue: &wgpu::Queue,
        sources: &ShaderSources,
        mask: bool,
        on: BuildOn,
    ) {
        if self.gpu.is_none() {
            let (builder, layouts) = builder(maker);
            let module = shaders::module(&maker.device, Program::VfxMesh, sources);
            let pipelines = MeshPipelines { module, color: None, mask: None };
            self.gpu = Some(Box::new(MeshGpu::new(&maker.device, queue, builder, pipelines, layouts)));
        }
        let gpu = self.gpu.as_mut().expect("made above");
        gpu.pipelines.build_missing(&gpu.builder, mask, on);
    }

    /// Whether the color pipeline is built, and the mask's.
    pub(super) fn held(&self) -> Option<bool> {
        self.gpu.as_ref().filter(|gpu| gpu.pipelines.color.is_some()).map(|gpu| gpu.pipelines.mask.is_some())
    }

    pub(super) fn warm(
        &self,
        maker: VfxMaker,
        queue: &wgpu::Queue,
        mask: bool,
        on: BuildOn,
    ) -> Option<Step<WarmedMeshes>> {
        if let Some(gpu) = &self.gpu {
            if gpu.pipelines.color.is_some() && (!mask || gpu.pipelines.mask.is_some()) {
                return None;
            }
            let (builder, mut pipelines) = (gpu.builder.clone(), gpu.pipelines.clone());
            return Some(Box::new(move |_, _| {
                pipelines.build_missing(&builder, mask, on);
                WarmedMeshes::Pipelines(pipelines)
            }));
        }
        let queue = queue.clone();
        Some(Box::new(move |sources, _| {
            let (builder, layouts) = builder(&maker);
            let module = shaders::module(&maker.device, Program::VfxMesh, sources);
            let mut pipelines = MeshPipelines { module, color: None, mask: None };
            pipelines.build_missing(&builder, mask, on);
            WarmedMeshes::Made(Box::new(MeshGpu::new(&maker.device, &queue, builder, pipelines, layouts)))
        }))
    }

    pub(super) fn adopt(&mut self, warmed: WarmedMeshes) {
        match (warmed, &mut self.gpu) {
            (WarmedMeshes::Made(made), None) => self.gpu = Some(made),
            (WarmedMeshes::Pipelines(warmed), Some(gpu)) => {
                gpu.pipelines.color = gpu.pipelines.color.take().or(warmed.color);
                gpu.pipelines.mask = gpu.pipelines.mask.take().or(warmed.mask);
            }
            _ => {}
        }
    }

    pub(super) fn pipelines(&self) -> u32 {
        self.gpu.as_ref().map_or(0, |gpu| gpu.pipelines.count())
    }

    pub(super) fn rebuild(&self) -> Option<Step<MeshPipelines>> {
        let gpu = self.gpu.as_ref()?;
        let builder = gpu.builder.clone();
        let (color, mask) = (gpu.pipelines.color.is_some(), gpu.pipelines.mask.is_some());
        Some(Box::new(move |sources, _cancel| {
            let module = shaders::module(&builder.device, Program::VfxMesh, sources);
            MeshPipelines {
                color: color.then(|| builder.color(&module)),
                mask: mask.then(|| builder.mask(&module)),
                module,
            }
        }))
    }

    pub(super) fn set_pipelines(&mut self, pipelines: MeshPipelines) {
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.pipelines = pipelines;
        }
    }

    /// Drops what binds the last frame's depth: a frame with no meshes keeps no view alive.
    pub(super) fn forget_depth(&mut self) {
        if let Some(gpu) = &mut self.gpu {
            gpu.depth_groups = CachedBindGroup::default();
        }
    }

    /// Clears the last frame's meshes.
    pub(super) fn clear(&mut self) {
        self.packed.clear();
        self.shapes.clear();
        self.depths.clear();
        self.order.clear();
        self.rejected = 0;
    }

    /// Orders `meshes` back to front from `camera`, packs them and sizes their buffer. [`Self::build`] first.
    pub(super) fn prepare(
        &mut self,
        meshes: &[VfxMesh],
        camera: &CameraData,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        self.clear();
        let Some(gpu) = self.gpu.as_mut() else { return };
        let size = size_of::<GpuVfxMesh>() as u64;
        let fits = &meshes[..meshes.len().min(gpu.meshes.max_items(size) as usize)];
        self.order.extend(
            fits.iter()
                .enumerate()
                .filter(|(_, mesh)| mesh.position.is_finite())
                .map(|(index, mesh)| (camera.view_depth(mesh.position), index as u32)),
        );
        // Ties keep the frame's order; unstable, so sorting never allocates.
        self.order.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        for &(depth, index) in &self.order {
            let mesh = &fits[index as usize];
            if let Some(packed) = GpuVfxMesh::new(mesh) {
                self.packed.push(packed);
                self.shapes.push(mesh.shape);
                self.depths.push(depth);
            }
        }
        self.rejected = (meshes.len() - self.packed.len()) as u32;
        if gpu.meshes.reserve(device, queue, self.packed.len() as u64 * size, 0) {
            gpu.buffers_group =
                buffers_group(device, &gpu.buffers_layout, gpu.meshes.buffer(), &gpu.uniforms, &gpu.sampler);
        }
    }

    /// The meshes' view depths, back to front.
    pub(super) fn depths(&self) -> &[f32] {
        &self.depths
    }

    pub(super) fn count(&self) -> u32 {
        self.packed.len() as u32
    }

    pub(super) fn bytes(&self) -> u64 {
        (self.packed.len() * size_of::<GpuVfxMesh>()) as u64
    }

    /// This frame's upload and draws, `None` with no meshes to draw. `emissive` is 1 / exposure.
    pub(super) fn split_for_record(
        &mut self,
        output: (u32, u32),
        emissive: f32,
        after_resolve: bool,
        mask: bool,
    ) -> Option<(MeshUpload<'_>, MeshDraws<'_>)> {
        if self.packed.is_empty() {
            return None;
        }
        let gpu = self.gpu.as_mut()?;
        let color = gpu.pipelines.color.as_ref()?;
        let uniforms = GpuVfxMeshUniforms {
            target_size: [
                output.0 as f32,
                output.1 as f32,
                1.0 / output.0.max(1) as f32,
                1.0 / output.1.max(1) as f32,
            ],
            emissive,
            padding: [0.0; 3],
        };
        let upload = MeshUpload {
            uniforms,
            uniforms_buffer: &gpu.uniforms,
            meshes: &self.packed,
            buffer: gpu.meshes.buffer(),
        };
        let shapes = &self.shapes;
        let main = MeshMain {
            color: (!after_resolve).then_some(color),
            mask: gpu.pipelines.mask.as_ref().filter(|_| mask),
            bind: MeshBind {
                buffers: &gpu.buffers_group,
                textures: &gpu.main_group,
                vertices: &gpu.shapes.vertices,
                indices: &gpu.shapes.indices,
                ranges: &gpu.shapes.ranges,
                shapes,
            },
        };
        let after = after_resolve.then_some(MeshAfter {
            color,
            textures_layout: &gpu.textures_layout,
            patterns: &gpu.patterns,
            depth_groups: &mut gpu.depth_groups,
            buffers: &gpu.buffers_group,
            vertices: &gpu.shapes.vertices,
            indices: &gpu.shapes.indices,
            ranges: &gpu.shapes.ranges,
            shapes,
        });
        Some((upload, MeshDraws { main, after }))
    }
}

/// The meshes' uploads.
#[derive(Debug)]
pub(crate) struct MeshUpload<'a> {
    uniforms: GpuVfxMeshUniforms,
    uniforms_buffer: &'a wgpu::Buffer,
    meshes: &'a [GpuVfxMesh],
    buffer: &'a wgpu::Buffer,
}

impl MeshUpload<'_> {
    pub(super) fn write(&self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        buffer::stage(belt, encoder, self.uniforms_buffer, 0, bytemuck::bytes_of(&self.uniforms));
        buffer::stage(belt, encoder, self.buffer, 0, bytemuck::cast_slice(self.meshes));
    }
}

/// What a pass binds to draw meshes: groups 2 and 3, the shapes' buffers, and each mesh's shape.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MeshBind<'a> {
    buffers: &'a wgpu::BindGroup,
    textures: &'a wgpu::BindGroup,
    vertices: &'a wgpu::Buffer,
    indices: &'a wgpu::Buffer,
    ranges: &'a [ShapeRange; VfxMeshShape::ALL.len()],
    shapes: &'a [VfxMeshShape],
}

impl MeshBind<'_> {
    /// Binds `pipeline` and what it reads; groups 0 and 1 stay as they are.
    pub(super) fn bind(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline) {
        pass.set_pipeline(pipeline);
        pass.set_bind_group(2, self.buffers, &[]);
        pass.set_bind_group(3, self.textures, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
    }

    /// Draws mesh `index`, bound.
    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, index: u32) {
        let range = self.ranges[self.shapes[index as usize].index()];
        pass.draw_indexed(range.indices.0..range.indices.1, range.base_vertex, index..index + 1);
    }

    pub(super) fn draw_all(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline) {
        self.bind(pass, pipeline);
        for index in 0..self.shapes.len() as u32 {
            self.draw(pass, index);
        }
    }
}

/// The meshes' draws.
#[derive(Debug)]
pub(crate) struct MeshDraws<'a> {
    pub(super) main: MeshMain<'a>,
    pub(super) after: Option<MeshAfter<'a>>,
}

/// What ends the main pass: the mask where post reads it, and the color without temporal AA.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MeshMain<'a> {
    pub(super) color: Option<&'a wgpu::RenderPipeline>,
    pub(super) mask: Option<&'a wgpu::RenderPipeline>,
    pub(super) bind: MeshBind<'a>,
}

/// What draws over temporal AA's resolve, its depth group made when the pass records.
#[derive(Debug)]
pub(crate) struct MeshAfter<'a> {
    pub(super) color: &'a wgpu::RenderPipeline,
    textures_layout: &'a wgpu::BindGroupLayout,
    patterns: &'a wgpu::TextureView,
    depth_groups: &'a mut CachedBindGroup<2>,
    buffers: &'a wgpu::BindGroup,
    vertices: &'a wgpu::Buffer,
    indices: &'a wgpu::Buffer,
    ranges: &'a [ShapeRange; VfxMeshShape::ALL.len()],
    shapes: &'a [VfxMeshShape],
}

impl MeshAfter<'_> {
    /// What binds the meshes over `depth`, the scene prepass's.
    pub(super) fn bind(
        &mut self,
        device: &wgpu::Device,
        label: Option<&str>,
        depth: &wgpu::TextureView,
    ) -> MeshBind<'_> {
        let textures = self.depth_groups.get(device, label, self.textures_layout, &[self.patterns, depth]);
        MeshBind {
            buffers: self.buffers,
            textures,
            vertices: self.vertices,
            indices: self.indices,
            ranges: self.ranges,
            shapes: self.shapes,
        }
    }
}

/// A step of the color draw: a range of sprite and ribbon instances, or a mesh by its place back to front.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ColorStep {
    Instances(Range<u32>),
    Mesh(u32),
}

/// Plans the color draw: `draws` (the sprites' and ribbons' ranges, `plan_color_draws`'s) with each mesh put
/// just before the first sprite nearer than its middle (see the module docs). `sprite_depths` are the drawn
/// sprites' view depths back to front, and `mesh_depths` the meshes'. With no meshes the steps are the draws.
pub(super) fn plan_steps(
    draws: &[Range<u32>],
    sprite_depths: &[f32],
    mesh_depths: &[f32],
    steps: &mut Vec<ColorStep>,
) {
    steps.clear();
    let sprite_count = sprite_depths.len() as u32;
    // Where each mesh goes: before the sprite at this place.
    let before = |mesh: usize| sprite_depths.partition_point(|&depth| depth >= mesh_depths[mesh]) as u32;
    let push = |steps: &mut Vec<ColorStep>, range: Range<u32>| match steps.last_mut() {
        _ if range.is_empty() => {}
        Some(ColorStep::Instances(last)) if last.end == range.start => last.end = range.end,
        _ => steps.push(ColorStep::Instances(range)),
    };
    let mut mesh = 0;
    for range in draws {
        let mut start = range.start;
        let sprites_end = range.end.min(sprite_count);
        while start < sprites_end {
            while mesh < mesh_depths.len() && before(mesh) <= start {
                steps.push(ColorStep::Mesh(mesh as u32));
                mesh += 1;
            }
            let stop = if mesh < mesh_depths.len() { before(mesh).min(sprites_end) } else { sprites_end };
            push(steps, start..stop);
            start = stop;
        }
        push(steps, start.max(range.start)..range.end);
    }
    steps.extend((mesh as u32..mesh_depths.len() as u32).map(ColorStep::Mesh));
}

#[cfg(test)]
mod tests {
    use glam::{Quat, Vec2};
    use sapphire_look::{VfxErosion, VfxFlipbook};

    use super::*;

    fn mesh() -> VfxMesh {
        VfxMesh {
            shape: VfxMeshShape::Ring,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            color: Vec3::ONE,
            alpha: 1.0,
            additive: 1.0,
            soft: 0.1,
            pattern: VfxPattern::Streaks,
            tiling: Vec2::ONE,
            scroll: Vec2::ZERO,
            erosion: None,
            flipbook: None,
        }
    }

    #[test]
    fn a_mesh_packs_its_cells_and_flags_and_bad_values_are_made_safe() {
        let book = VfxFlipbook { columns: 4, rows: 4, frame: 5, next: 6, blend: 0.25 };
        let erosion = VfxErosion {
            mask: VfxPattern::Noise,
            tiling: Vec2::new(2.0, 0.0),
            threshold: 3.0,
            edge: -1.0,
            edge_color: Vec3::splat(f32::INFINITY),
        };
        let packed = GpuVfxMesh::new(&VfxMesh {
            pattern: VfxPattern::Burst,
            flipbook: Some(book),
            erosion: Some(erosion),
            scroll: Vec2::new(1.25, f32::NAN),
            alpha: 2.0,
            ..mesh()
        })
        .expect("finite");
        assert_eq!(packed.flags, ERODES | FLIPBOOK);
        assert_eq!(packed.cells, [0.25, 0.25, 0.5, 0.25]);
        assert_eq!((packed.cell_size, packed.blend), ([0.25, 0.25], 0.25));
        assert_eq!((packed.threshold, packed.edge, packed.edge_color), (1.0, 0.0, [COLOR_MAX; 3]));
        assert_eq!((packed.mask, packed.mask_tiling), (VfxPattern::Noise.layer(), [2.0, 1.0]));
        assert_eq!((packed.scroll, packed.alpha), ([0.25, 0.0], 1.0));
        assert!(GpuVfxMesh::new(&VfxMesh { position: Vec3::NAN, ..mesh() }).is_none());
        assert!(
            GpuVfxMesh::new(&VfxMesh { rotation: Quat::from_xyzw(0.0, 0.0, 0.0, 0.0), ..mesh() }).is_none()
        );
    }

    #[test]
    fn the_patterns_are_every_layer_down_to_one_texel() {
        let patterns = generate_patterns(16);
        assert_eq!(patterns.len(), VfxPattern::ALL.len());
        assert!(patterns.iter().all(|mips| mips.len() == 5 && mips[4].len() == 1));
        assert!(patterns[VfxPattern::Solid.layer() as usize].iter().flatten().all(|&texel| texel == 255));
    }

    fn steps(draws: &[Range<u32>], sprites: &[f32], meshes: &[f32]) -> Vec<ColorStep> {
        let mut steps = Vec::new();
        plan_steps(draws, sprites, meshes, &mut steps);
        steps
    }

    use ColorStep::{Instances as I, Mesh as M};

    #[test]
    fn with_no_meshes_the_steps_are_the_draws() {
        assert_eq!(
            steps(&[0..3, 5..6, 3..5, 6..9], &[9.0, 5.0, 4.0, 3.0, 2.0, 1.0], &[]),
            [I(0..3), I(5..6), I(3..5), I(6..9)]
        );
        assert_eq!(steps(std::slice::from_ref(&(0..10)), &[3.0, 2.0], &[]), [I(0..10)]);
    }

    #[test]
    fn a_mesh_draws_before_the_first_sprite_nearer_than_it() {
        // Sprites at 9, 5 and 1 metres, then four ribbon points; meshes at 12, 6, 6 and 0.5.
        let sprites = [9.0, 5.0, 1.0];
        assert_eq!(
            steps(std::slice::from_ref(&(0..7)), &sprites, &[12.0, 6.0, 6.0, 0.5]),
            [M(0), I(0..1), M(1), M(2), I(1..7), M(3)]
        );
        // A ribbon's stretch behind the second sprite keeps its place; the mesh goes after it.
        assert_eq!(
            steps(&[0..1, 4..5, 1..4, 5..7], &sprites, &[6.0]),
            [I(0..1), I(4..5), M(0), I(1..4), I(5..7)]
        );
        assert_eq!(steps(&[], &[], &[3.0, 1.0]), [M(0), M(1)]);
    }
}
