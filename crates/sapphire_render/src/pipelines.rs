//! Bind group layouts, vertex layouts and render pipelines for the scene passes.

use std::mem::offset_of;
use std::num::NonZeroU64;

use sapphire_assets::mesh::{PositionFormat, ToonMaterialParams, VertexShading, VertexSurface};
use sapphire_rhi::wgpu;

use crate::gpu_types::{Dequantize, DrawData, FrameUniforms, InstanceData, ViewUniforms};
use crate::lights::GpuLight;
use crate::shaders;

/// Reverse-Z depth: cleared to 0, tested with `Greater`. 32-bit float keeps precision all
/// the way out with an infinite far plane.
pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The main pass's color: linear HDR light, unclamped, which post tone maps into the target.
/// Alpha is kept for the character bloom mask (M1 C7).
pub(crate) const SCENE_COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Vertex buffer slots. Every mesh pass binds the draw record and positions; a position-only
/// pass (shadows, depth prepass) needs nothing more, so the streams it skips come last.
pub(crate) mod slot {
    /// The draw record, stepped per instance.
    pub const DRAW: u32 = 0;
    /// The mesh's position stream.
    pub const POSITIONS: u32 = 1;
    /// Normal and outline normal (rewritten by compute skinning, M1 R2).
    pub const SHADING: u32 = 2;
    /// UV and vertex color.
    pub const SURFACE: u32 = 3;
}

/// Slot 0: the draw record (WGSL `DrawRecord`).
pub(crate) const DRAW_ATTRIBUTES: [wgpu::VertexAttribute; 5] = {
    let dequantize = offset_of!(DrawData, dequantize);
    [
        attribute(5, wgpu::VertexFormat::Uint32, offset_of!(DrawData, instance)),
        attribute(6, wgpu::VertexFormat::Uint32, offset_of!(DrawData, material)),
        attribute(7, wgpu::VertexFormat::Float32x3, dequantize + offset_of!(Dequantize, position_offset)),
        attribute(8, wgpu::VertexFormat::Float32x3, dequantize + offset_of!(Dequantize, position_scale)),
        attribute(9, wgpu::VertexFormat::Float32x4, dequantize + offset_of!(Dequantize, uv_transform)),
    ]
};

/// Slot 1 for quantized positions ([`PositionFormat::Unorm16`]).
pub(crate) const POSITION_UNORM16_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
    [attribute(0, wgpu::VertexFormat::Unorm16x4, 0)];

/// Slot 1 for f32 positions ([`PositionFormat::Float32`]). The shader reads a `vec4`, whose w
/// the vertex fetch fills with 1.
pub(crate) const POSITION_FLOAT32_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
    [attribute(0, wgpu::VertexFormat::Float32x3, 0)];

/// Slot 2: [`VertexShading`].
pub(crate) const SHADING_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    [attribute(1, wgpu::VertexFormat::Snorm16x2, 0), attribute(2, wgpu::VertexFormat::Snorm16x2, 4)];

/// Slot 3: [`VertexSurface`].
pub(crate) const SURFACE_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    [attribute(3, wgpu::VertexFormat::Unorm16x2, 0), attribute(4, wgpu::VertexFormat::Unorm8x4, 4)];

/// The slot 1 attributes for meshes in `format`.
pub(crate) fn position_attributes(format: PositionFormat) -> &'static [wgpu::VertexAttribute] {
    match format {
        PositionFormat::Unorm16 => &POSITION_UNORM16_ATTRIBUTES,
        PositionFormat::Float32 => &POSITION_FLOAT32_ATTRIBUTES,
    }
}

const fn attribute(location: u32, format: wgpu::VertexFormat, offset: usize) -> wgpu::VertexAttribute {
    wgpu::VertexAttribute { format, offset: offset as u64, shader_location: location }
}

/// The vertex buffers of a pass that reads every stream of a mesh in `format`, in slot order.
fn mesh_buffers(format: PositionFormat) -> [Option<wgpu::VertexBufferLayout<'static>>; 4] {
    let vertex = |stride: usize, attributes| {
        Some(wgpu::VertexBufferLayout {
            array_stride: stride as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes,
        })
    };
    [
        Some(wgpu::VertexBufferLayout {
            array_stride: size_of::<DrawData>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &DRAW_ATTRIBUTES,
        }),
        vertex(format.stride(), position_attributes(format)),
        vertex(size_of::<VertexShading>(), &SHADING_ATTRIBUTES),
        vertex(size_of::<VertexSurface>(), &SURFACE_ATTRIBUTES),
    ]
}

/// The bind group layouts of `shaders/frame.wgsl`. Group 3 (material textures) arrives in M1.
#[derive(Debug)]
pub(crate) struct Layouts {
    /// Group 0: frame constants, local lights and their lists.
    pub frame: wgpu::BindGroupLayout,
    /// Group 1: one view, bound with a dynamic offset. The light-cluster pass binds it too.
    pub view: wgpu::BindGroupLayout,
    /// Group 2: instances and materials.
    pub scene: wgpu::BindGroupLayout,
}

impl Layouts {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        use wgpu::ShaderStages as S;
        let buffer =
            |binding, visibility, ty, has_dynamic_offset, min_size: usize| wgpu::BindGroupLayoutEntry {
                binding,
                visibility,
                ty: wgpu::BindingType::Buffer {
                    ty,
                    has_dynamic_offset,
                    min_binding_size: NonZeroU64::new(min_size as u64),
                },
                count: None,
            };
        let uniform = wgpu::BufferBindingType::Uniform;
        let storage = wgpu::BufferBindingType::Storage { read_only: true };
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
        };
        Self {
            frame: layout(
                "frame",
                &[
                    buffer(0, S::VERTEX_FRAGMENT, uniform, false, size_of::<FrameUniforms>()),
                    buffer(1, S::FRAGMENT, storage, false, size_of::<GpuLight>()),
                    buffer(2, S::FRAGMENT, storage, false, size_of::<u32>()),
                ],
            ),
            view: layout(
                "view",
                &[buffer(0, S::VERTEX_FRAGMENT | S::COMPUTE, uniform, true, size_of::<ViewUniforms>())],
            ),
            scene: layout(
                "scene",
                &[
                    buffer(0, S::VERTEX, storage, false, size_of::<InstanceData>()),
                    buffer(1, S::VERTEX_FRAGMENT, storage, false, size_of::<ToonMaterialParams>()),
                ],
            ),
        }
    }
}

/// Fixed-function state that differs between passes. Entry points are `vs_<name>` and
/// `fs_<name>`.
struct PassDesc<'a> {
    name: &'static str,
    module: &'a wgpu::ShaderModule,
    vertex_buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    cull: Option<wgpu::Face>,
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
}

/// The mesh pipelines for one [`PositionFormat`]: the same shaders, with that format's
/// position attribute.
#[derive(Debug)]
pub(crate) struct MeshPipelines {
    pub toon: wgpu::RenderPipeline,
    pub outline: wgpu::RenderPipeline,
}

/// The scene pipelines. Quantized-mesh pipelines are built up front; the f32 ones when the
/// first f32 mesh is uploaded ([`Self::prepare_format`]), so apps that have none never pay for
/// compiling them.
#[derive(Debug)]
pub(crate) struct Pipelines {
    /// Per [`PositionFormat::ALL`] entry.
    mesh: [Option<MeshPipelines>; 2],
    pub sky: wgpu::RenderPipeline,
    target: PipelineTarget,
    toon_module: wgpu::ShaderModule,
    outline_module: wgpu::ShaderModule,
}

/// What every scene pipeline shares, kept to build the f32 ones later.
#[derive(Debug)]
struct PipelineTarget {
    layout: wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
    sample_count: u32,
    cache: Option<wgpu::PipelineCache>,
}

impl Pipelines {
    pub(crate) fn new(
        device: &wgpu::Device,
        layouts: &Layouts,
        color_format: wgpu::TextureFormat,
        sample_count: u32,
        cache: Option<&wgpu::PipelineCache>,
    ) -> Self {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[Some(&layouts.frame), Some(&layouts.view), Some(&layouts.scene)],
            immediate_size: 0,
        });
        let target = PipelineTarget { layout, color_format, sample_count, cache: cache.cloned() };
        let module = |name, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        // A fullscreen triangle at the far plane, drawn last: `Equal` against the cleared
        // depth (0) shades only pixels no geometry covered.
        let sky = target.build(
            device,
            PassDesc {
                name: "sky",
                module: &module("sky", shaders::SKY),
                vertex_buffers: &[],
                cull: None,
                depth_write: false,
                depth_compare: wgpu::CompareFunction::Equal,
            },
        );
        let mut pipelines = Self {
            mesh: [None, None],
            sky,
            target,
            toon_module: module("toon", shaders::TOON),
            outline_module: module("outline", shaders::OUTLINE),
        };
        pipelines.prepare_format(device, PositionFormat::Unorm16);
        pipelines
    }

    /// Builds the mesh pipelines for `format` unless they exist.
    pub(crate) fn prepare_format(&mut self, device: &wgpu::Device, format: PositionFormat) {
        if self.mesh[format_index(format)].is_some() {
            return;
        }
        let buffers = mesh_buffers(format);
        let toon = self.target.build(
            device,
            PassDesc {
                name: "toon",
                module: &self.toon_module,
                vertex_buffers: &buffers,
                cull: Some(wgpu::Face::Back),
                depth_write: true,
                depth_compare: wgpu::CompareFunction::Greater,
            },
        );
        // Inverted hull: keep only the back faces of the extruded shell.
        let outline = self.target.build(
            device,
            PassDesc {
                name: "outline",
                module: &self.outline_module,
                vertex_buffers: &buffers,
                cull: Some(wgpu::Face::Front),
                depth_write: true,
                depth_compare: wgpu::CompareFunction::Greater,
            },
        );
        self.mesh[format_index(format)] = Some(MeshPipelines { toon, outline });
    }

    /// The mesh pipelines for `format`, once [`Self::prepare_format`] built them.
    pub(crate) fn mesh(&self, format: PositionFormat) -> Option<&MeshPipelines> {
        self.mesh[format_index(format)].as_ref()
    }
}

impl PipelineTarget {
    fn build(&self, device: &wgpu::Device, pass: PassDesc<'_>) -> wgpu::RenderPipeline {
        let color_targets = [Some(wgpu::ColorTargetState {
            format: self.color_format,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let (vs, fs) = (format!("vs_{}", pass.name), format!("fs_{}", pass.name));
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(pass.name),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module: pass.module,
                entry_point: Some(&vs),
                compilation_options: Default::default(),
                buffers: pass.vertex_buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: pass.cull,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(pass.depth_write),
                depth_compare: Some(pass.depth_compare),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: self.sample_count, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: pass.module,
                entry_point: Some(&fs),
                compilation_options: Default::default(),
                targets: &color_targets,
            }),
            multiview_mask: None,
            cache: self.cache.as_ref(),
        })
    }
}

/// The index of `format` in [`PositionFormat::ALL`].
pub(crate) fn format_index(format: PositionFormat) -> usize {
    PositionFormat::ALL.iter().position(|&f| f == format).expect("ALL lists every format")
}
