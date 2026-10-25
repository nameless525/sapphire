//! The M0 renderer: one render pass with three pipelines (toon, outline, sky).

use std::num::NonZeroU64;

use sapphire_assets::mesh::{MeshAsset, MeshFormatError, MeshVertex};
use sapphire_rhi::{GpuContext, TransientDesc, wgpu};

use crate::buffer::GrowableBuffer;
use crate::gpu_types::{DrawData, FrameUniforms, InstanceData, ViewUniforms};
use crate::graph::{FrameGraph, TextureId};
use crate::mesh_pool::MeshPool;
use crate::pipelines::{DEPTH_FORMAT, Layouts, Pipelines};
use crate::scene::DrawList;
use crate::{CameraData, FrameView, LOG_TARGET, MeshHandle};

const VERTEX_STRIDE: u64 = size_of::<MeshVertex>() as u64;
const DRAW_STRIDE: u64 = size_of::<DrawData>() as u64;

/// Size of each staging buffer the per-frame uploads are written through. The belt keeps a
/// few (one per frame in flight) and reuses them, so it should hold a frame's uploads:
/// constants plus instances and draw records, a few hundred bytes in M0. Bigger frames fall
/// back to a dedicated buffer per upload.
const UPLOAD_CHUNK_SIZE: u64 = 256 * 1024;

/// Multisampling of the main pass. Hull outlines are only a few pixels wide, so they need it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Msaa {
    Off,
    /// Needs device support ([`GpuContext::format_support`]); common on mobile GPUs.
    X2,
    /// The one count WebGPU guarantees for every renderable format.
    #[default]
    X4,
    /// Needs device support.
    X8,
}

impl Msaa {
    pub fn sample_count(self) -> u32 {
        match self {
            Self::Off => 1,
            Self::X2 => 2,
            Self::X4 => 4,
            Self::X8 => 8,
        }
    }
}

/// Renderer options. M1 grows this into the quality-tier table (spec §5.12).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderSettings {
    pub msaa: Msaa,
}

/// Errors from renderer setup and mesh uploads.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("invalid mesh: {0}")]
    InvalidMesh(#[from] MeshFormatError),
    #[error("mesh has {vertices} vertices; at most 2^24 are portable")]
    MeshTooLarge { vertices: usize },
    #[error("mesh pool is out of room for {0}")]
    MeshPoolFull(&'static str),
    #[error("{0:?} can't be rendered to on this device")]
    NotRenderable(wgpu::TextureFormat),
    #[error("{format:?} does not support {samples}x MSAA on this device")]
    UnsupportedMsaa { format: wgpu::TextureFormat, samples: u32 },
}

/// Renderer counters for the stats overlay and Tracy (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    /// Draw calls per mesh pass in the last prepared frame (toon and outline each issue this
    /// many; the sky adds one).
    pub draws: u32,
    /// Instances in the last prepared frame.
    pub instances: u32,
    /// Mesh pool bytes in use: vertices, indices and materials.
    pub mesh_pool_bytes: u64,
    /// GPU buffers reallocated to grow, since the renderer was created. Growth copies data and
    /// rebuilds bind groups, so it should settle after loading.
    pub buffer_growths: u32,
}

/// Draws [`FrameView`]s.
///
/// Each frame is [`prepare`](Self::prepare)d (draw lists and buffer sizing, CPU only) and then
/// [`record`](Self::record)ed as passes in the app's [`FrameGraph`], which the app can add its
/// own passes to (UI, debug, readback) before executing it.
///
/// Per-frame data reaches the GPU through a [`StagingBelt`](wgpu::util::StagingBelt): mapped
/// staging buffers that are reused across frames, copied from at the start of the frame's
/// encoder. `Queue::write_buffer` would instead create a new staging buffer for every call,
/// which dominated the frame's CPU time on Apple GPUs.
#[derive(Debug)]
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    color_format: wgpu::TextureFormat,
    settings: RenderSettings,
    labels: bool,
    layouts: Layouts,
    pipelines: Pipelines,
    frame_uniforms: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    view_uniforms: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    instances: GrowableBuffer,
    /// Draw records, read as instance-step vertex data (vertex slot 1).
    draws: GrowableBuffer,
    meshes: MeshPool,
    scene_bind_group: wgpu::BindGroup,
    draw_list: DrawList,
    uploads: wgpu::util::StagingBelt,
    /// What the last `prepare` computed for `record` to upload and draw.
    prepared: Option<PreparedFrame>,
    stats: RenderStats,
}

#[derive(Debug)]
struct PreparedFrame {
    camera: CameraData,
    constants: FrameUniforms,
}

impl Renderer {
    /// Creates pipelines for `color_format` (the surface's sRGB view format). Fails if the
    /// device can't render to that format, or to depth, at the requested sample count.
    pub fn new(
        gpu: &GpuContext,
        color_format: wgpu::TextureFormat,
        settings: RenderSettings,
    ) -> Result<Self, RenderError> {
        let samples = settings.msaa.sample_count();
        let color = gpu.format_support(color_format);
        if !color.renderable {
            return Err(RenderError::NotRenderable(color_format));
        }
        if !color.supports_samples(samples) || (samples > 1 && !color.multisample_resolve) {
            return Err(RenderError::UnsupportedMsaa { format: color_format, samples });
        }
        let depth = gpu.format_support(DEPTH_FORMAT);
        if !depth.renderable || !depth.supports_samples(samples) {
            return Err(RenderError::UnsupportedMsaa { format: DEPTH_FORMAT, samples });
        }

        let device = gpu.device().clone();
        let limits = device.limits();
        let layouts = Layouts::new(&device);
        let pipelines = Pipelines::new(&device, &layouts, color_format, samples, gpu.pipeline_cache());
        let uniform = |label, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let frame_uniforms = uniform("frame uniforms", size_of::<FrameUniforms>());
        // One view slot in M0. M1 sizes this for every view (main camera, shadow cascades) at
        // a stride of `min_uniform_buffer_offset_alignment` and binds each by offset.
        let view_uniforms = uniform("view uniforms", size_of::<ViewUniforms>());
        let frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &layouts.frame,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_uniforms.as_entire_binding() }],
        });
        let view_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view"),
            layout: &layouts.view,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &view_uniforms,
                    offset: 0,
                    size: NonZeroU64::new(size_of::<ViewUniforms>() as u64),
                }),
            }],
        });
        let storage_limit = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size);
        let instances = GrowableBuffer::new(
            &device,
            "instances",
            wgpu::BufferUsages::STORAGE,
            256 * size_of::<InstanceData>() as u64,
            storage_limit,
        );
        let draws = GrowableBuffer::new(
            &device,
            "draw records",
            wgpu::BufferUsages::VERTEX,
            1024 * DRAW_STRIDE,
            limits.max_buffer_size,
        );
        let meshes = MeshPool::new(&device);
        let scene_bind_group = create_scene_bind_group(&device, &layouts.scene, &instances, &meshes);
        let uploads = wgpu::util::StagingBelt::new(device.clone(), UPLOAD_CHUNK_SIZE);
        tracing::info!(target: LOG_TARGET, ?color_format, msaa = samples, "renderer created");
        Ok(Self {
            device,
            queue: gpu.queue().clone(),
            color_format,
            settings,
            labels: gpu.label("").is_some(),
            layouts,
            pipelines,
            frame_uniforms,
            frame_bind_group,
            view_uniforms,
            view_bind_group,
            instances,
            draws,
            meshes,
            scene_bind_group,
            draw_list: DrawList::default(),
            uploads,
            prepared: None,
            stats: RenderStats::default(),
        })
    }

    /// Validates and uploads a cooked mesh and its materials into the mesh pool.
    pub fn upload_mesh(&mut self, mesh: &MeshAsset) -> Result<MeshHandle, RenderError> {
        let (handle, materials_moved) = self.meshes.upload(&self.device, &self.queue, mesh)?;
        if materials_moved {
            self.scene_bind_group =
                create_scene_bind_group(&self.device, &self.layouts.scene, &self.instances, &self.meshes);
        }
        self.update_pool_stats();
        Ok(handle)
    }

    /// The CPU half of a frame: builds the draw list, computes the frame constants and sizes
    /// the per-frame buffers. Call once per frame, before [`Self::record`], which uploads.
    pub fn prepare(&mut self, gpu: &GpuContext, frame: &FrameView<'_>) {
        profiling::scope!("Renderer::prepare");
        debug_assert!(
            gpu.device() == &self.device,
            "prepare() needs the GpuContext the renderer was created with"
        );
        let (device, queue) = (gpu.device(), gpu.queue());

        self.draw_list.build(self.meshes.records(), frame.instances);
        let list = &self.draw_list;
        // Per-frame data is rewritten whole, so nothing needs to survive a reallocation.
        if self.instances.reserve(device, queue, size_of_val(list.instances.as_slice()) as u64, 0) {
            self.scene_bind_group =
                create_scene_bind_group(device, &self.layouts.scene, &self.instances, &self.meshes);
        }
        self.draws.reserve(device, queue, size_of_val(list.draws.as_slice()) as u64, 0);

        self.prepared = Some(PreparedFrame { camera: frame.camera, constants: FrameUniforms::new(frame) });
        self.stats.draws = list.commands.len() as u32;
        self.stats.instances = list.instances.len() as u32;
        self.update_pool_stats();
    }

    /// The GPU half of a frame: adds the frame's uploads and the main pass to `graph`, drawing
    /// into `target`. Record once per [`Self::prepare`]. The renderer stays borrowed until the
    /// graph has been executed, which submits the uploads and so returns their staging buffers
    /// for reuse.
    ///
    /// `target` must be a 2D texture in the format given to [`Self::new`]. The internal
    /// attachments (MSAA color, depth) are transient graph textures sized from it, so any size
    /// works, including one that changed since the last frame.
    ///
    /// The toon, outline and sky draws share one render pass, so on mobile GPUs the MSAA color
    /// and depth are cleared, resolved and discarded without ever leaving tile memory.
    pub fn record<'f>(&'f mut self, graph: &mut FrameGraph<'f>, target: TextureId) {
        profiling::scope!("Renderer::record");
        debug_assert!(self.prepared.is_some(), "record() needs a prepare() first");
        let Some(PreparedFrame { camera, constants }) = self.prepared.take() else { return };
        let size = graph.size(target);
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        // Cleared on load and resolved or discarded on store, so tile-based GPUs (Apple, Mali,
        // Adreno) can keep them entirely in tile memory.
        let samples = self.settings.msaa.sample_count();
        let attachment = |format| TransientDesc {
            width: size.0,
            height: size.1,
            format,
            sample_count: samples,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TRANSIENT_ATTACHMENT,
        };
        // Without MSAA the pass renders straight into the target.
        let msaa_color =
            (samples > 1).then(|| graph.create_texture("msaa color", attachment(self.color_format)));
        let depth = graph.create_texture("depth", attachment(DEPTH_FORMAT));

        // The uploads need the staging belt exclusively; everything else is shared.
        let Self {
            labels,
            ref pipelines,
            ref frame_uniforms,
            ref frame_bind_group,
            ref view_uniforms,
            ref view_bind_group,
            ref instances,
            ref draws,
            ref meshes,
            ref scene_bind_group,
            ref draw_list,
            ref mut uploads,
            ..
        } = *self;

        // Buffers aren't graph resources yet, so the uploads are a side effect added first:
        // the main pass reads what they copy.
        graph.add_pass("uploads").side_effect().record(move |ctx| {
            upload(uploads, ctx.encoder, frame_uniforms, bytemuck::bytes_of(&constants));
            upload(
                uploads,
                ctx.encoder,
                view_uniforms,
                bytemuck::bytes_of(&ViewUniforms::new(&camera, size)),
            );
            upload(uploads, ctx.encoder, instances.buffer(), bytemuck::cast_slice(&draw_list.instances));
            upload(uploads, ctx.encoder, draws.buffer(), bytemuck::cast_slice(&draw_list.draws));
            uploads.finish_and_recall_on_submit(ctx.encoder);
        });

        let mut main = graph.add_pass("main").write(target).write(depth);
        if let Some(msaa_color) = msaa_color {
            main = main.write(msaa_color);
        }
        main.record(move |ctx| {
            let (view, resolve_target, store) = match msaa_color {
                Some(msaa_color) => (ctx.view(msaa_color), Some(ctx.view(target)), wgpu::StoreOp::Discard),
                None => (ctx.view(target), None, wgpu::StoreOp::Store),
            };
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: labels.then_some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target,
                    // The sky covers every pixel left uncovered; clearing is still the cheapest
                    // load on tilers.
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: ctx.view(depth),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, frame_bind_group, &[]);
            pass.set_bind_group(1, view_bind_group, &[0]);
            pass.set_bind_group(2, scene_bind_group, &[]);

            let commands = &draw_list.commands;
            if !commands.is_empty() {
                pass.set_index_buffer(meshes.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
                // Pass 1: toon opaque. Pass 2: outlines, after opaque so depth rejects the hull
                // wherever the mesh itself covers it.
                for pipeline in [&pipelines.toon, &pipelines.outline] {
                    pass.set_pipeline(pipeline);
                    let mut bound_first_vertex = None;
                    // CPU-driven submission (see the crate docs): offsets select the mesh's
                    // vertices and the draw record, so base vertex and first instance stay 0.
                    for command in commands {
                        if bound_first_vertex != Some(command.first_vertex) {
                            let offset = u64::from(command.first_vertex) * VERTEX_STRIDE;
                            pass.set_vertex_buffer(0, meshes.vertex_buffer().slice(offset..));
                            bound_first_vertex = Some(command.first_vertex);
                        }
                        let record = u64::from(command.draw_id) * DRAW_STRIDE;
                        pass.set_vertex_buffer(1, draws.buffer().slice(record..record + DRAW_STRIDE));
                        pass.draw_indexed(command.indices.clone(), 0, 0..1);
                    }
                }
            }
            // Pass 3: sky, only where no geometry wrote depth.
            pass.set_pipeline(&pipelines.sky);
            pass.draw(0..3, 0..1);
        });
    }

    /// Counters from the last [`Self::prepare`] plus pool totals.
    pub fn stats(&self) -> RenderStats {
        self.stats
    }

    fn update_pool_stats(&mut self) {
        self.stats.mesh_pool_bytes = self.meshes.bytes_used();
        self.stats.buffer_growths = self.meshes.growths() + self.instances.growths() + self.draws.growths();
    }
}

/// Copies `bytes` to the start of `target` through the staging belt. Empty uploads are skipped.
fn upload(
    belt: &mut wgpu::util::StagingBelt,
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::Buffer,
    bytes: &[u8],
) {
    if let Some(size) = wgpu::BufferSize::new(bytes.len() as u64) {
        belt.write_buffer(encoder, target, 0, size).copy_from_slice(bytes);
    }
}

fn create_scene_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    instances: &GrowableBuffer,
    meshes: &MeshPool,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scene"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: instances.buffer().as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: meshes.material_buffer().as_entire_binding() },
        ],
    })
}
