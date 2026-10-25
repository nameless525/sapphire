//! Bind group layouts and render pipelines for the M0 passes.

use std::mem::offset_of;
use std::num::NonZeroU64;

use sapphire_assets::mesh::{MeshVertex, ToonMaterialParams};
use sapphire_rhi::wgpu;

use crate::gpu_types::{DrawData, FrameUniforms, InstanceData, ViewUniforms};
use crate::lights::GpuLight;
use crate::shaders;

/// Reverse-Z depth: cleared to 0, tested with `Greater`. 32-bit float keeps precision all
/// the way out with an infinite far plane.
pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The main pass's color: linear HDR light, unclamped, which post tone maps into the target.
/// Alpha is kept for the character bloom mask (M1 C7).
pub(crate) const SCENE_COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Vertex buffer slot 0: the mesh pool, laid out as `MeshVertex`.
pub(crate) const MESH_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] = [
    attribute(0, wgpu::VertexFormat::Float32x3, offset_of!(MeshVertex, position)),
    attribute(1, wgpu::VertexFormat::Float32x3, offset_of!(MeshVertex, normal)),
    attribute(2, wgpu::VertexFormat::Float32x3, offset_of!(MeshVertex, outline_normal)),
    attribute(3, wgpu::VertexFormat::Float32x2, offset_of!(MeshVertex, uv)),
    attribute(4, wgpu::VertexFormat::Unorm8x4, offset_of!(MeshVertex, color)),
];

/// Vertex buffer slot 1: the draw record, stepped per instance (WGSL `DrawRecord`).
pub(crate) const DRAW_ATTRIBUTES: [wgpu::VertexAttribute; 2] = [
    attribute(5, wgpu::VertexFormat::Uint32, offset_of!(DrawData, instance)),
    attribute(6, wgpu::VertexFormat::Uint32, offset_of!(DrawData, material)),
];

const fn attribute(location: u32, format: wgpu::VertexFormat, offset: usize) -> wgpu::VertexAttribute {
    wgpu::VertexAttribute { format, offset: offset as u64, shader_location: location }
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
    source: &'static str,
    vertex_buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    cull: Option<wgpu::Face>,
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
}

/// The three M0 pipelines.
#[derive(Debug)]
pub(crate) struct Pipelines {
    pub toon: wgpu::RenderPipeline,
    pub outline: wgpu::RenderPipeline,
    pub sky: wgpu::RenderPipeline,
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
        let mesh_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<MeshVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &MESH_VERTEX_ATTRIBUTES,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<DrawData>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &DRAW_ATTRIBUTES,
            }),
        ];
        let color_targets = [Some(wgpu::ColorTargetState {
            format: color_format,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })];

        let build = |pass: PassDesc<'_>| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(pass.name),
                source: wgpu::ShaderSource::Wgsl(pass.source.into()),
            });
            let (vs, fs) = (format!("vs_{}", pass.name), format!("fs_{}", pass.name));
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(pass.name),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
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
                multisample: wgpu::MultisampleState { count: sample_count, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(&fs),
                    compilation_options: Default::default(),
                    targets: &color_targets,
                }),
                multiview_mask: None,
                cache,
            })
        };

        use wgpu::CompareFunction::{Equal, Greater};
        Self {
            toon: build(PassDesc {
                name: "toon",
                source: shaders::TOON,
                vertex_buffers: &mesh_buffers,
                cull: Some(wgpu::Face::Back),
                depth_write: true,
                depth_compare: Greater,
            }),
            // Inverted hull: keep only the back faces of the extruded shell.
            outline: build(PassDesc {
                name: "outline",
                source: shaders::OUTLINE,
                vertex_buffers: &mesh_buffers,
                cull: Some(wgpu::Face::Front),
                depth_write: true,
                depth_compare: Greater,
            }),
            // A fullscreen triangle at the far plane, drawn last: `Equal` against the cleared
            // depth (0) shades only pixels no geometry covered.
            sky: build(PassDesc {
                name: "sky",
                source: shaders::SKY,
                vertex_buffers: &[],
                cull: None,
                depth_write: false,
                depth_compare: Equal,
            }),
        }
    }
}
