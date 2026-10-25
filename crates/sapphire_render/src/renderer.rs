//! The renderer: a main pass with three pipelines (toon, outline, sky) into HDR scene color,
//! lit by the sun and the frame's local lights, then post-processing into the target.

use std::num::NonZeroU64;

use sapphire_assets::mesh::{ArchivedMesh, MeshError, PositionFormat, VertexShading, VertexSurface};
use sapphire_core::target::FormFactor;
use sapphire_rhi::{GpuContext, TransientDesc, wgpu};

use crate::buffer;
use crate::gpu_types::{DrawData, FrameUniforms, ViewUniforms};
use crate::graph::{FrameGraph, TextureId};
use crate::lights::{LightPath, LightStats, Lights};
use crate::mesh_pool::MeshPool;
use crate::pipelines::{DEPTH_FORMAT, Layouts, Pipelines, SCENE_COLOR_FORMAT, format_index, slot};
use crate::post::PostProcess;
use crate::scene::GpuScene;
use crate::{CameraData, FramePacket, LOG_TARGET, MeshHandle};

const SHADING_STRIDE: u64 = size_of::<VertexShading>() as u64;
const SURFACE_STRIDE: u64 = size_of::<VertexSurface>() as u64;
const DRAW_STRIDE: u64 = size_of::<DrawData>() as u64;

/// Size of each staging buffer the per-frame uploads are written through. The belt keeps a
/// few (one per frame in flight) and reuses them, so it should hold a frame's uploads:
/// constants plus the changed instances, and the draw records after a rebuild, a few hundred
/// bytes in M0. Bigger frames fall back to a dedicated buffer per upload.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderSettings {
    pub msaa: Msaa,
    /// Bloom's half-size levels: 5 runs from ½ to 1/32 of the target (spec §5.12), and quality
    /// tiers cut it on mobile. 0 turns bloom off; more than
    /// [`MAX_BLOOM_LEVELS`](crate::MAX_BLOOM_LEVELS) is clamped.
    pub bloom_levels: u32,
    /// How local lights reach the toon shader: clustered on desktop, per-object lists on
    /// mobile ([`Self::for_device`]). Either runs on every backend.
    pub light_path: LightPath,
    /// The most local lights a frame draws once culled to the view; the nearest are kept
    /// (spec §3.4 budgets 64 on desktop and 8 on mobile). 0 turns local lights off; more than
    /// [`MAX_LIGHTS`](crate::MAX_LIGHTS) is clamped.
    pub max_lights: u32,
}

impl RenderSettings {
    /// The defaults for a class of device, until quality tiers (R6) choose per device:
    /// desktops cluster their lights, and phones and tablets use per-object lists with the
    /// mobile light budget.
    pub fn for_device(class: FormFactor) -> Self {
        match class {
            FormFactor::Desktop => Self::default(),
            FormFactor::Mobile => Self { light_path: LightPath::PerObject, max_lights: 8, ..Self::default() },
        }
    }
}

impl Default for RenderSettings {
    /// The desktop defaults.
    fn default() -> Self {
        Self { msaa: Msaa::default(), bloom_levels: 5, light_path: LightPath::Clustered, max_lights: 64 }
    }
}

/// Errors from renderer setup and mesh uploads.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("invalid mesh: {0}")]
    InvalidMesh(#[from] MeshError),
    #[error("mesh has {vertices} vertices; at most 2^24 are portable")]
    MeshTooLarge { vertices: usize },
    #[error("mesh pool is out of room for {0}")]
    MeshPoolFull(&'static str),
    #[error("{0:?} can't be rendered to on this device")]
    NotRenderable(wgpu::TextureFormat),
    #[error("{0:?} can't be sampled with linear filtering on this device")]
    NotFilterable(wgpu::TextureFormat),
    #[error("{format:?} does not support {samples}x MSAA on this device")]
    UnsupportedMsaa { format: wgpu::TextureFormat, samples: u32 },
}

/// Renderer counters for the stats overlay and Tracy (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    /// Draw calls per mesh pass in the last prepared frame (toon and outline each issue this
    /// many; the sky adds one).
    pub draws: u32,
    /// Instances in the persistent scene after the last prepared frame.
    pub instances: u32,
    /// Instances the last prepared packet added or changed; each is uploaded.
    pub changed_instances: u32,
    /// Times the draw list was rebuilt since the renderer was created: only when instances
    /// are added or removed, change mesh, or meshes are uploaded.
    pub draw_list_builds: u32,
    /// Mesh pool bytes in use: vertices, indices and materials.
    pub mesh_pool_bytes: u64,
    /// GPU buffers reallocated to grow, since the renderer was created. Growth copies data and
    /// rebuilds bind groups, so it should settle after loading.
    pub buffer_growths: u32,
    /// Grading LUTs baked since the renderer was created. Bakes happen only when the grade or
    /// tone curve changes, so this should grow at time-of-day keys and settings changes only.
    pub lut_bakes: u32,
    /// Local lights and their lists in the last prepared frame.
    pub lights: LightStats,
}

/// Draws [`FramePacket`]s.
///
/// Each frame's packet is [`prepare`](Self::prepare)d (its instance deltas applied to the
/// persistent scene and any changed grading LUT baked, CPU only) and then
/// [`record`](Self::record)ed as passes in the app's [`FrameGraph`], which the app can add its
/// own passes to (UI, debug, readback) before executing it.
///
/// Per-frame data reaches the GPU through a [`StagingBelt`](wgpu::util::StagingBelt): mapped
/// staging buffers that are reused across frames, copied from at the start of the frame's
/// encoder. `Queue::write_buffer` would instead create a new staging buffer for every call,
/// which dominated the frame's CPU time on Apple GPUs. Instances upload only when they change.
#[derive(Debug)]
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    settings: RenderSettings,
    labels: bool,
    layouts: Layouts,
    pipelines: Pipelines,
    frame_uniforms: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    view_uniforms: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    /// The frame's local lights, their lists and the cluster pass.
    lights: Lights,
    meshes: MeshPool,
    /// Instances, draw records and the draw list, persistent across frames.
    scene: GpuScene,
    scene_bind_group: wgpu::BindGroup,
    post: PostProcess,
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
    /// Creates pipelines for `color_format`, the target's: the surface's sRGB view format, or
    /// another format that encodes on write. Fails if the device can't render to it, or can't
    /// render, filter and resolve HDR scene color (`Rgba16Float`) and depth at the requested
    /// sample count.
    pub fn new(
        gpu: &GpuContext,
        color_format: wgpu::TextureFormat,
        settings: RenderSettings,
    ) -> Result<Self, RenderError> {
        let samples = settings.msaa.sample_count();
        if !gpu.format_support(color_format).renderable {
            return Err(RenderError::NotRenderable(color_format));
        }
        let scene = gpu.format_support(SCENE_COLOR_FORMAT);
        if !scene.renderable {
            return Err(RenderError::NotRenderable(SCENE_COLOR_FORMAT));
        }
        if !scene.filterable {
            return Err(RenderError::NotFilterable(SCENE_COLOR_FORMAT));
        }
        if !scene.supports_samples(samples) || (samples > 1 && !scene.multisample_resolve) {
            return Err(RenderError::UnsupportedMsaa { format: SCENE_COLOR_FORMAT, samples });
        }
        let depth = gpu.format_support(DEPTH_FORMAT);
        if !depth.renderable || !depth.supports_samples(samples) {
            return Err(RenderError::UnsupportedMsaa { format: DEPTH_FORMAT, samples });
        }
        let post = PostProcess::new(gpu, color_format, settings.bloom_levels)?;

        let device = gpu.device().clone();
        let layouts = Layouts::new(&device);
        let pipelines = Pipelines::new(&device, &layouts, SCENE_COLOR_FORMAT, samples, gpu.pipeline_cache());
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
        let lights = Lights::new(&device, &settings, &frame_uniforms, &layouts.view, gpu.pipeline_cache());
        let frame_bind_group = create_frame_bind_group(&device, &layouts.frame, &frame_uniforms, &lights);
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
        let meshes = MeshPool::new(&device);
        let scene = GpuScene::new(&device);
        let scene_bind_group = create_scene_bind_group(&device, &layouts.scene, &scene, &meshes);
        let uploads = wgpu::util::StagingBelt::new(device.clone(), UPLOAD_CHUNK_SIZE);
        tracing::info!(
            target: LOG_TARGET,
            ?color_format,
            msaa = samples,
            light_path = ?settings.light_path,
            max_lights = settings.max_lights,
            "renderer created"
        );
        Ok(Self {
            device,
            queue: gpu.queue().clone(),
            settings,
            labels: gpu.label("").is_some(),
            layouts,
            pipelines,
            frame_uniforms,
            frame_bind_group,
            view_uniforms,
            view_bind_group,
            lights,
            meshes,
            scene,
            scene_bind_group,
            post,
            uploads,
            prepared: None,
            stats: RenderStats::default(),
        })
    }

    /// Validates and uploads a cooked mesh, read in place ([`sapphire_assets::mesh::Mesh::read`]),
    /// and its materials into the mesh pool. Its streams are copied as they are; all its LODs'
    /// indices are uploaded and LOD0 is drawn. A skinned mesh draws in its bind pose, and its
    /// morph targets at weight 0, until compute skinning (M1 R2).
    ///
    /// The first mesh with f32 positions also builds that format's pipelines.
    pub fn upload_mesh(&mut self, mesh: &ArchivedMesh) -> Result<MeshHandle, RenderError> {
        let (handle, materials_moved) = self.meshes.upload(&self.device, &self.queue, mesh)?;
        self.pipelines.prepare_format(&self.device, mesh.streams.position_format());
        let bounds = mesh.streams.bounds.to_native();
        self.lights.mesh_uploaded(handle, [bounds.min, bounds.max]);
        if materials_moved {
            self.scene_bind_group =
                create_scene_bind_group(&self.device, &self.layouts.scene, &self.scene, &self.meshes);
        }
        self.scene.meshes_changed();
        self.update_pool_stats();
        Ok(handle)
    }

    /// The CPU half of a frame: applies the packet's instance deltas to the persistent scene
    /// (rebuilding the draw list only if the drawn set changed), culls and lists the local
    /// lights, computes the frame constants, sizes the buffers and bakes any grading LUT the
    /// packet's post parameters changed. [`Self::record`] then uploads what changed and draws.
    ///
    /// Every packet must be prepared, in order, including frames that draw nothing (no target,
    /// so no [`Self::record`]): a skipped packet would lose its deltas. Debug builds check the
    /// packets' frame numbers. A new renderer starts with no instances, so its first packet
    /// must add every instance; an app that replaces its renderer (after device loss)
    /// re-extracts everything.
    pub fn prepare(&mut self, gpu: &GpuContext, packet: &FramePacket) {
        profiling::scope!("Renderer::prepare");
        debug_assert!(
            gpu.device() == &self.device,
            "prepare() needs the GpuContext the renderer was created with"
        );
        let (device, queue) = (gpu.device(), gpu.queue());

        if self.scene.prepare(device, queue, packet.frame, &packet.instances, self.meshes.records()) {
            self.scene_bind_group =
                create_scene_bind_group(device, &self.layouts.scene, &self.scene, &self.meshes);
        }
        if self.lights.prepare(device, queue, packet, self.scene.slots()) {
            self.frame_bind_group =
                create_frame_bind_group(device, &self.layouts.frame, &self.frame_uniforms, &self.lights);
        }

        // Without a main view the frame has nothing to draw.
        let lights = self.lights.uniforms();
        self.prepared = packet
            .main_view()
            .map(|view| PreparedFrame { camera: view.camera, constants: FrameUniforms::new(packet, lights) });
        self.post.prepare(queue, &packet.post);
        self.stats.draws = self.scene.draw_count();
        self.stats.instances = self.scene.live_instances();
        self.stats.changed_instances = self.scene.changed_instances();
        self.stats.draw_list_builds = self.scene.draw_list_builds();
        self.stats.lut_bakes = self.post.bakes();
        self.stats.lights = self.lights.stats();
        self.update_pool_stats();
    }

    /// The GPU half of a frame: adds the frame's uploads, the main pass and post-processing to
    /// `graph`, drawing into `target`. Record at most once per [`Self::prepare`], and only
    /// after preparing a packet with a main view. The renderer stays borrowed until the graph
    /// has been executed, which submits the uploads and so returns their staging buffers for
    /// reuse. Scene changes from prepared frames that were never recorded are uploaded by the
    /// next recorded one.
    ///
    /// `target` must be a 2D texture in the format given to [`Self::new`]. The internal
    /// textures (HDR scene color, MSAA color, depth, the bloom chain) are transient graph
    /// textures sized from it, so any size works, including one that changed since the last
    /// frame.
    ///
    /// The toon, outline and sky draws share one render pass that writes linear HDR scene
    /// color, so on mobile GPUs the MSAA color and depth are cleared, resolved and discarded
    /// without ever leaving tile memory. Post then reads the resolved scene color (see the
    /// `post` module). On the clustered light path, frames with local lights first bin them in
    /// a compute pass (see the `lights` module).
    pub fn record<'f>(&'f mut self, graph: &mut FrameGraph<'f>, target: TextureId) {
        profiling::scope!("Renderer::record");
        debug_assert!(self.prepared.is_some(), "record() needs a prepare() of a packet with a main view");
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
        // Without MSAA the pass renders straight into scene color.
        let msaa_color =
            (samples > 1).then(|| graph.create_texture("msaa color", attachment(SCENE_COLOR_FORMAT)));
        let depth = graph.create_texture("depth", attachment(DEPTH_FORMAT));
        let scene_color = graph.create_texture(
            "scene color",
            TransientDesc {
                sample_count: 1,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                ..attachment(SCENE_COLOR_FORMAT)
            },
        );

        // The uploads need the staging belt exclusively; everything else is shared.
        let Self {
            labels,
            ref pipelines,
            ref frame_uniforms,
            ref frame_bind_group,
            ref view_uniforms,
            ref view_bind_group,
            ref mut lights,
            ref meshes,
            ref mut scene,
            ref scene_bind_group,
            ref mut post,
            ref mut uploads,
            ..
        } = *self;
        let (scene_uploads, draw_list, draws) = scene.split_for_record();
        let (light_uploads, cluster_pass) = lights.split_for_record();
        let (post_uniforms, post_buffer) = post.uniforms();
        // A handle, so `post` can be borrowed mutably for its passes below.
        let post_buffer = post_buffer.clone();

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
            scene_uploads.write(uploads, ctx.encoder);
            light_uploads.write(uploads, ctx.encoder);
            upload(uploads, ctx.encoder, &post_buffer, bytemuck::bytes_of(&post_uniforms));
            uploads.finish_and_recall_on_submit(ctx.encoder);
        });
        // Also a side effect: the main pass reads the cluster lists it writes.
        if let Some(cluster_pass) = cluster_pass {
            graph.add_pass("light clusters").side_effect().record(move |ctx| {
                let timestamps = ctx.compute_timestamp_writes();
                cluster_pass.record(ctx.encoder, view_bind_group, labels, timestamps);
            });
        }

        let mut main = graph.add_pass("main").write(scene_color).write(depth);
        if let Some(msaa_color) = msaa_color {
            main = main.write(msaa_color);
        }
        main.record(move |ctx| {
            let (view, resolve_target, store) = match msaa_color {
                Some(msaa_color) => {
                    (ctx.view(msaa_color), Some(ctx.view(scene_color)), wgpu::StoreOp::Discard)
                }
                None => (ctx.view(scene_color), None, wgpu::StoreOp::Store),
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
                timestamp_writes: ctx.timestamp_writes(),
                ..Default::default()
            });
            pass.set_bind_group(0, frame_bind_group, &[]);
            pass.set_bind_group(1, view_bind_group, &[0]);
            pass.set_bind_group(2, scene_bind_group, &[]);

            if draw_list.len() > 0 {
                pass.set_index_buffer(meshes.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
                // Pass 1: toon opaque. Pass 2: outlines, after opaque so depth rejects the hull
                // wherever the mesh itself covers it.
                for outline in [false, true] {
                    for format in PositionFormat::ALL {
                        let commands = &draw_list.commands[format_index(format)];
                        if commands.is_empty() {
                            continue;
                        }
                        let mesh_pipelines =
                            pipelines.mesh(format).expect("built when a mesh in this format was uploaded");
                        pass.set_pipeline(if outline {
                            &mesh_pipelines.outline
                        } else {
                            &mesh_pipelines.toon
                        });
                        let slab = meshes.slab(format);
                        let mut bound_first_vertex = None;
                        // CPU-driven submission (see the crate docs): offsets select the mesh's
                        // streams and the draw record, so base vertex and first instance stay 0.
                        for command in commands {
                            if bound_first_vertex != Some(command.first_vertex) {
                                let vertex = u64::from(command.first_vertex);
                                let positions = slab.positions().slice(vertex * slab.position_stride()..);
                                pass.set_vertex_buffer(slot::POSITIONS, positions);
                                pass.set_vertex_buffer(
                                    slot::SHADING,
                                    slab.shading().slice(vertex * SHADING_STRIDE..),
                                );
                                pass.set_vertex_buffer(
                                    slot::SURFACE,
                                    slab.surface().slice(vertex * SURFACE_STRIDE..),
                                );
                                bound_first_vertex = Some(command.first_vertex);
                            }
                            let record = u64::from(command.draw_id) * DRAW_STRIDE;
                            pass.set_vertex_buffer(
                                slot::DRAW,
                                draws.buffer().slice(record..record + DRAW_STRIDE),
                            );
                            pass.draw_indexed(command.indices.clone(), 0, 0..1);
                        }
                    }
                }
            }
            // Pass 3: sky, only where no geometry wrote depth.
            pass.set_pipeline(&pipelines.sky);
            pass.draw(0..3, 0..1);
        });

        post.record(graph, scene_color, target, labels);
    }

    /// Counters from the last [`Self::prepare`] plus pool totals.
    pub fn stats(&self) -> RenderStats {
        self.stats
    }

    /// The local lights, for tests that read their buffers back.
    #[cfg(test)]
    pub(crate) fn lights(&self) -> &Lights {
        &self.lights
    }

    fn update_pool_stats(&mut self) {
        self.stats.mesh_pool_bytes = self.meshes.bytes_used();
        self.stats.buffer_growths = self.meshes.growths() + self.scene.growths() + self.lights.growths();
    }
}

/// Copies `bytes` to the start of `target` through the staging belt. Empty uploads are skipped.
fn upload(
    belt: &mut wgpu::util::StagingBelt,
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::Buffer,
    bytes: &[u8],
) {
    buffer::stage(belt, encoder, target, 0, bytes);
}

fn create_frame_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    frame_uniforms: &wgpu::Buffer,
    lights: &Lights,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("frame"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: frame_uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: lights.light_buffer().as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: lights.list_buffer().as_entire_binding() },
        ],
    })
}

fn create_scene_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene: &GpuScene,
    meshes: &MeshPool,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scene"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: scene.instance_buffer().as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: meshes.material_buffer().as_entire_binding() },
        ],
    })
}
