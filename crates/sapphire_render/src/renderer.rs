//! The M0 renderer: one render pass with three pipelines (sky, toon, outline).

use sapphire_assets::mesh::{MeshAsset, MeshFormatError};
use sapphire_rhi::{GpuContext, wgpu};

use crate::buffer::GrowableBuffer;
use crate::draw_ids::DrawIdTable;
use crate::gpu_types::{DrawData, FrameUniforms, InstanceData};
use crate::mesh_pool::MeshPool;
use crate::pipelines::{DEPTH_FORMAT, Pipelines, scene_bind_group_layout};
use crate::scene::DrawList;
use crate::{FrameView, LOG_TARGET, MeshHandle};

/// Multisampling of the main pass. Hull outlines are only a few pixels wide, so they need it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Msaa {
    Off,
    /// 4× MSAA, the one count WebGPU guarantees for every renderable format.
    #[default]
    X4,
}

impl Msaa {
    pub fn sample_count(self) -> u32 {
        match self {
            Self::Off => 1,
            Self::X4 => 4,
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
    #[error("mesh pool is out of room for {0}")]
    MeshPoolFull(&'static str),
    #[error("{format:?} does not support {samples}x MSAA on this device")]
    UnsupportedMsaa { format: wgpu::TextureFormat, samples: u32 },
}

/// Draws [`FrameView`]s into a color target.
#[derive(Debug)]
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    color_format: wgpu::TextureFormat,
    settings: RenderSettings,
    scene_layout: wgpu::BindGroupLayout,
    pipelines: Pipelines,
    frame_uniforms: wgpu::Buffer,
    instances: GrowableBuffer,
    draws: GrowableBuffer,
    meshes: MeshPool,
    draw_ids: DrawIdTable,
    scene_bind_group: wgpu::BindGroup,
    targets: Option<RenderTargets>,
    draw_list: DrawList,
}

impl Renderer {
    /// Creates pipelines for `color_format` (the surface's sRGB view format). Call
    /// [`Self::resize`] before the first [`Self::render`].
    pub fn new(
        gpu: &GpuContext,
        color_format: wgpu::TextureFormat,
        settings: RenderSettings,
    ) -> Result<Self, RenderError> {
        let samples = settings.msaa.sample_count();
        for format in [color_format, DEPTH_FORMAT] {
            if !gpu.adapter().get_texture_format_features(format).flags.sample_count_supported(samples) {
                return Err(RenderError::UnsupportedMsaa { format, samples });
            }
        }
        let device = gpu.device().clone();
        let limits = device.limits();
        let scene_layout = scene_bind_group_layout(&device);
        let pipelines = Pipelines::new(&device, &scene_layout, color_format, samples, gpu.pipeline_cache());
        let frame_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame uniforms"),
            size: size_of::<FrameUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
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
            wgpu::BufferUsages::STORAGE,
            1024 * size_of::<DrawData>() as u64,
            storage_limit,
        );
        let meshes = MeshPool::new(&device);
        let draw_ids = DrawIdTable::new(&device, gpu.queue());
        let scene_bind_group =
            create_scene_bind_group(&device, &scene_layout, &frame_uniforms, &instances, &draws, &meshes);
        tracing::info!(target: LOG_TARGET, ?color_format, msaa = samples, "renderer created");
        Ok(Self {
            device,
            queue: gpu.queue().clone(),
            color_format,
            settings,
            scene_layout,
            pipelines,
            frame_uniforms,
            instances,
            draws,
            meshes,
            draw_ids,
            scene_bind_group,
            targets: None,
            draw_list: DrawList::default(),
        })
    }

    /// Validates and uploads a cooked mesh and its materials into the mesh pool.
    pub fn upload_mesh(&mut self, mesh: &MeshAsset) -> Result<MeshHandle, RenderError> {
        let (handle, materials_moved) = self.meshes.upload(&self.device, &self.queue, mesh)?;
        if materials_moved {
            self.rebuild_scene_bind_group();
        }
        Ok(handle)
    }

    /// Resizes the internal targets to the color target's size in pixels. A zero size
    /// disables rendering until the next non-zero resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            self.targets = None;
        } else if self.targets.as_ref().is_none_or(|targets| targets.size != (width, height)) {
            self.targets = Some(RenderTargets::new(
                &self.device,
                (width, height),
                self.color_format,
                self.settings.msaa,
            ));
        }
    }

    /// Records and submits one frame into `target`, which must match the last
    /// [`Self::resize`] and the format given to [`Self::new`].
    ///
    /// The three passes share one hardware render pass, so the MSAA color and depth buffers
    /// are cleared, resolved and discarded without ever leaving tile memory on mobile GPUs.
    pub fn render(&mut self, gpu: &GpuContext, target: &wgpu::TextureView, frame: &FrameView<'_>) {
        profiling::scope!("Renderer::render");
        debug_assert!(
            gpu.device() == &self.device,
            "render() needs the GpuContext the renderer was created with"
        );
        let Some(targets) = &self.targets else {
            return;
        };
        let (device, queue) = (gpu.device(), gpu.queue());

        self.draw_list.build(self.meshes.records(), frame.instances);
        let list = &self.draw_list;
        let instance_bytes: &[u8] = bytemuck::cast_slice(&list.instances);
        let draw_bytes: &[u8] = bytemuck::cast_slice(&list.draws);
        // Per-frame data is rewritten whole, so nothing needs to survive a reallocation.
        let instances_moved = self.instances.reserve(device, queue, instance_bytes.len() as u64, 0);
        let draws_moved = self.draws.reserve(device, queue, draw_bytes.len() as u64, 0);
        self.draw_ids.reserve(device, queue, list.draws.len());
        if instances_moved || draws_moved {
            self.scene_bind_group = create_scene_bind_group(
                device,
                &self.scene_layout,
                &self.frame_uniforms,
                &self.instances,
                &self.draws,
                &self.meshes,
            );
        }
        queue.write_buffer(
            &self.frame_uniforms,
            0,
            bytemuck::bytes_of(&FrameUniforms::new(frame, targets.size)),
        );
        if !list.draws.is_empty() {
            queue.write_buffer(self.instances.buffer(), 0, instance_bytes);
            queue.write_buffer(self.draws.buffer(), 0, draw_bytes);
        }

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let (view, resolve_target, store) = match &targets.msaa_color {
                Some(msaa) => (msaa, Some(target), wgpu::StoreOp::Discard),
                None => (target, None, wgpu::StoreOp::Store),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target,
                    // The sky covers every pixel; clearing is still the cheapest load on tilers.
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.scene_bind_group, &[]);

            // Pass 1: sky gradient.
            pass.set_pipeline(&self.pipelines.sky);
            pass.draw(0..3, 0..1);

            if !list.commands.is_empty() {
                pass.set_vertex_buffer(0, self.meshes.vertex_buffer().slice(..));
                pass.set_index_buffer(self.meshes.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
                // Pass 2: toon opaque. Pass 3: outlines, after opaque so depth rejects the
                // hull wherever the mesh itself covers it.
                for pipeline in [&self.pipelines.toon, &self.pipelines.outline] {
                    pass.set_pipeline(pipeline);
                    // CPU-driven submission: the draw ID comes from the vertex buffer offset,
                    // so base vertex and first instance stay 0 (see `draw_ids`).
                    for command in &list.commands {
                        pass.set_vertex_buffer(1, self.draw_ids.slice(command.draw_id));
                        pass.draw_indexed(command.indices.clone(), 0, 0..1);
                    }
                }
            }
        }
        queue.submit([encoder.finish()]);
    }

    fn rebuild_scene_bind_group(&mut self) {
        self.scene_bind_group = create_scene_bind_group(
            &self.device,
            &self.scene_layout,
            &self.frame_uniforms,
            &self.instances,
            &self.draws,
            &self.meshes,
        );
    }
}

fn create_scene_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    frame_uniforms: &wgpu::Buffer,
    instances: &GrowableBuffer,
    draws: &GrowableBuffer,
    meshes: &MeshPool,
) -> wgpu::BindGroup {
    fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry { binding, resource: buffer.as_entire_binding() }
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scene"),
        layout,
        entries: &[
            entry(0, frame_uniforms),
            entry(1, instances.buffer()),
            entry(2, draws.buffer()),
            entry(3, meshes.material_buffer()),
        ],
    })
}

/// Attachments that live only inside the main pass.
#[derive(Debug)]
struct RenderTargets {
    size: (u32, u32),
    /// `None` without MSAA: the pass then renders straight into the caller's target.
    msaa_color: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
}

impl RenderTargets {
    fn new(device: &wgpu::Device, size: (u32, u32), color_format: wgpu::TextureFormat, msaa: Msaa) -> Self {
        // Transient: cleared on load and resolved or discarded on store, so tile-based GPUs
        // (Apple, Mali, Adreno) can keep them entirely in tile memory.
        let transient = |label, format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: msaa.sample_count(),
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TRANSIENT_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        Self {
            size,
            msaa_color: (msaa != Msaa::Off).then(|| transient("msaa color", color_format)),
            depth: transient("depth", DEPTH_FORMAT),
        }
    }
}
