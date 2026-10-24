//! Bind group layout and render pipelines for the M0 passes.

use std::mem::offset_of;
use std::num::NonZeroU64;

use sapphire_assets::mesh::{MeshVertex, ToonMaterialParams};
use sapphire_rhi::wgpu;

use crate::gpu_types::{DrawData, FrameUniforms, InstanceData};
use crate::shaders;

/// Reverse-Z depth: cleared to 0, tested with `Greater`. 32-bit float keeps precision all
/// the way out with an infinite far plane.
pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Vertex buffer slot 0: the mesh pool, laid out as `MeshVertex`.
pub(crate) const MESH_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] = [
    attribute(0, wgpu::VertexFormat::Float32x3, offset_of!(MeshVertex, position)),
    attribute(1, wgpu::VertexFormat::Float32x3, offset_of!(MeshVertex, normal)),
    attribute(2, wgpu::VertexFormat::Float32x3, offset_of!(MeshVertex, outline_normal)),
    attribute(3, wgpu::VertexFormat::Float32x2, offset_of!(MeshVertex, uv)),
    attribute(4, wgpu::VertexFormat::Unorm8x4, offset_of!(MeshVertex, color)),
];

/// Vertex buffer slot 1: the per-draw ID, stepped per instance (see `draw_ids`). The shaders
/// declare it as `@location(5) draw_id: u32`.
pub(crate) const DRAW_ID_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
    [attribute(5, wgpu::VertexFormat::Uint32, 0)];

const fn attribute(location: u32, format: wgpu::VertexFormat, offset: usize) -> wgpu::VertexAttribute {
    wgpu::VertexAttribute { format, offset: offset as u64, shader_location: location }
}

/// Group 0: frame constants plus the persistent scene buffers (see `shaders/frame.wgsl`).
pub(crate) fn scene_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    use wgpu::ShaderStages as S;
    let entry = |binding, visibility, ty, min_size: usize| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(min_size as u64),
        },
        count: None,
    };
    let storage = wgpu::BufferBindingType::Storage { read_only: true };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("scene"),
        entries: &[
            entry(0, S::VERTEX_FRAGMENT, wgpu::BufferBindingType::Uniform, size_of::<FrameUniforms>()),
            entry(1, S::VERTEX, storage, size_of::<InstanceData>()),
            entry(2, S::VERTEX, storage, size_of::<DrawData>()),
            entry(3, S::VERTEX_FRAGMENT, storage, size_of::<ToonMaterialParams>()),
        ],
    })
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

/// The three M0 pipelines, in pass order.
#[derive(Debug)]
pub(crate) struct Pipelines {
    pub sky: wgpu::RenderPipeline,
    pub toon: wgpu::RenderPipeline,
    pub outline: wgpu::RenderPipeline,
}

impl Pipelines {
    pub(crate) fn new(
        device: &wgpu::Device,
        scene_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        sample_count: u32,
        cache: Option<&wgpu::PipelineCache>,
    ) -> Self {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[Some(scene_layout)],
            immediate_size: 0,
        });
        let mesh_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<MeshVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &MESH_VERTEX_ATTRIBUTES,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<u32>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &DRAW_ID_ATTRIBUTES,
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

        use wgpu::CompareFunction::{Always, Greater};
        Self {
            // A fullscreen triangle at the far plane: no vertex buffers, no depth writes.
            sky: build(PassDesc {
                name: "sky",
                source: shaders::SKY,
                vertex_buffers: &[],
                cull: None,
                depth_write: false,
                depth_compare: Always,
            }),
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
        }
    }
}
