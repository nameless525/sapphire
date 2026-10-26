//! The renderer: compute skinning, the prepasses, a main pass into HDR scene color (anime
//! character surfaces and outlines per static permutation, the sky, then transparent
//! surfaces), lit by each character's light rig (or the sun) and the frame's local lights, then
//! post-processing into the target.

use std::num::NonZeroU64;

use bevy_reflect::Reflect;
use glam::Mat4;
use sapphire_assets::AssetId;
use sapphire_assets::material::{ArchivedMaterial, MaterialError, MaterialFamily, MaterialFlags};
use sapphire_assets::mesh::{ArchivedMesh, ArchivedSlotMaterial, MeshError, PositionFormat};
use sapphire_rhi::{GpuContext, TransientDesc, wgpu};
use serde::{Deserialize, Serialize};

use crate::buffer;
use crate::characters::Characters;
use crate::draw::{DrawSet, MeshDraws};
use crate::gpu_types::{FrameUniforms, ViewUniforms};
use crate::graph::{FrameGraph, PassContext, TextureId};
use crate::lights::{LightPath, LightStats, Lights};
use crate::materials::{MaterialHandle, Materials};
use crate::mesh_pool::{MeshPool, SubmeshMaterial};
use crate::pipelines::{
    Coverage, DEPTH_FORMAT, Layouts, MeshPass, NORMALS_FORMAT, Part, Pipelines, SCENE_COLOR_FORMAT,
    SurfaceVariant, VELOCITY_FORMAT,
};
use crate::post::PostProcess;
use crate::scene::GpuScene;
use crate::skinning::{Skinning, SkinningStats};
use crate::texture::GpuTexture;
use crate::{CameraData, FramePacket, LOG_TARGET, MeshHandle};

/// Size of each staging buffer the per-frame uploads are written through. The belt keeps a
/// few (one per frame in flight) and reuses them, so it should hold a frame's uploads:
/// constants plus the changed instances, and the draw records after a rebuild, a few hundred
/// bytes in M0. Bigger frames fall back to a dedicated buffer per upload.
const UPLOAD_CHUNK_SIZE: u64 = 256 * 1024;

/// Multisampling of the main pass. Hull outlines are only a few pixels wide, so they need it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
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

/// What a renderer is created with, fixed for its lifetime: the parts of a quality tier that
/// build pipelines and buffers. Apps take them from the active tier
/// ([`QualityTier::render_settings`](crate::quality::QualityTier::render_settings)) and make a new
/// renderer when the tier changes. The render scale is not among them: it can change every frame
/// ([`Renderer::set_render_scale`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderSettings {
    pub msaa: Msaa,
    /// Bloom's half-size levels: 5 runs from ½ to 1/32 of the scene (spec §5.12), and quality
    /// tiers cut it on Mobile-Low. 0 turns bloom off; more than
    /// [`MAX_BLOOM_LEVELS`](crate::MAX_BLOOM_LEVELS) is clamped.
    pub bloom_levels: u32,
    /// How local lights reach the shading: clustered on desktop tiers, per-object lists on
    /// mobile ones. Either runs on every backend.
    pub light_path: LightPath,
    /// The most local lights a frame draws once culled to the view; the nearest are kept
    /// (spec §3.4 budgets 64 on desktop and 8 on mobile). 0 turns local lights off; more than
    /// [`MAX_LIGHTS`](crate::MAX_LIGHTS) is clamped.
    pub max_lights: u32,
}

impl Default for RenderSettings {
    /// The desktop tiers' settings, which every golden image renders with.
    fn default() -> Self {
        Self { msaa: Msaa::default(), bloom_levels: 5, light_path: LightPath::Clustered, max_lights: 64 }
    }
}

/// The smallest render scale: a quarter of the target's width and height.
pub const MIN_RENDER_SCALE: f32 = 0.25;

/// The size the scene renders at for a target of `size` at render `scale`: each side rounded to
/// the nearest pixel, at least 1 and at most the target's. `scale` is clamped to
/// [`MIN_RENDER_SCALE`]..=1 (1 when it isn't finite), so 1 is exactly the target's size.
pub fn scaled_size((width, height): (u32, u32), scale: f32) -> (u32, u32) {
    let scale = clamp_render_scale(scale);
    let side = |pixels: u32| ((pixels as f32 * scale).round() as u32).clamp(1, pixels.max(1));
    (side(width), side(height))
}

fn clamp_render_scale(scale: f32) -> f32 {
    if scale.is_finite() { scale.clamp(MIN_RENDER_SCALE, 1.0) } else { 1.0 }
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
    #[error("invalid material: {0}")]
    InvalidMaterial(#[from] MaterialError),
    #[error("texture slot `{slot}` binds {id}, which the material's textures don't include")]
    MissingTexture { slot: &'static str, id: AssetId },
    #[error(
        "texture slot `{slot}` was cooked against a texture with swizzle `{expected}`, but the texture \
         bound has `{found}`; recook the material with its textures' cook"
    )]
    TextureSwizzle { slot: &'static str, expected: String, found: String },
    #[error("texture slot `{slot}` holds linear data, but the texture bound is sRGB ({format:?})")]
    SrgbData { slot: &'static str, format: wgpu::TextureFormat },
    #[error("{0:?} was not uploaded to this renderer")]
    UnknownMaterial(MaterialHandle),
    #[error("a {from} material can't be updated into a {to} one")]
    MaterialFamilyChanged { from: MaterialFamily, to: MaterialFamily },
    #[error("the {0} family's material buffer is full")]
    MaterialPoolFull(MaterialFamily),
    #[error("the mesh has {slots} material slots, but {handles} materials were given")]
    MaterialCount { slots: usize, handles: usize },
    #[error("{0} materials have no shaders yet, so meshes can't draw with them")]
    NotDrawable(MaterialFamily),
    #[error(
        "material slot {slot:?} names material asset {id}, which wasn't uploaded; the asset server \
         loads it before the mesh, for `upload_material` and then `upload_slot_materials`"
    )]
    UnresolvedMaterial { slot: String, id: AssetId },
}

/// Renderer counters for the stats overlay and Tracy (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    /// Anime character surface draws in the last prepared frame, of every static permutation
    /// (the prepasses draw the opaque and alpha-tested ones again; the sky adds one).
    pub draws: u32,
    /// Of [`Self::draws`], those of alpha-tested materials.
    pub alpha_tested_draws: u32,
    /// Of [`Self::draws`], those of transparent materials, blended after the sky.
    pub transparent_draws: u32,
    /// Of [`Self::draws`], those of face materials (M1 C3), through the face variants.
    pub face_draws: u32,
    /// Outline hulls drawn in the last prepared frame.
    pub outline_draws: u32,
    /// Anime character pipelines built (surface and outline variants, across position
    /// formats): each static permutation's are built when the first draw needs them.
    pub character_pipelines: u32,
    /// Instances in the persistent scene after the last prepared frame.
    pub instances: u32,
    /// Instances the last prepared packet added or changed; each is uploaded.
    pub changed_instances: u32,
    /// Times the draw list was rebuilt since the renderer was created: only when instances
    /// are added or removed, change mesh, meshes are uploaded, or a material edit changes its
    /// static permutation.
    pub draw_list_builds: u32,
    /// Mesh pool bytes in use: vertices (with skin and morph data) and indices.
    pub mesh_pool_bytes: u64,
    /// Materials uploaded, across every family.
    pub materials: u32,
    /// Bytes of every family's material slots.
    pub material_bytes: u64,
    /// Material slots the last prepared frame uploads: those uploaded or updated since the
    /// frame before. A material edit uploads one.
    pub material_uploads: u32,
    /// GPU buffers reallocated to grow, since the renderer was created. Growth copies data and
    /// rebuilds bind groups, so it should settle after loading.
    pub buffer_growths: u32,
    /// Grading LUTs baked since the renderer was created. Bakes happen only when the grade or
    /// tone curve changes, so this should grow at time-of-day keys and settings changes only.
    pub lut_bakes: u32,
    /// Local lights and their lists in the last prepared frame.
    pub lights: LightStats,
    /// Instances the last prepared frame with a view lit with their own light rigs
    /// ([`FramePacket::characters`]); one listed twice counts once.
    pub characters: u32,
    /// Entries of [`FramePacket::characters`] that frame ignored: for instances it doesn't draw,
    /// or with lighting or a face that isn't valid.
    pub ignored_characters: u32,
    /// The size the last recorded frame's scene rendered at: its target's size at the render
    /// scale ([`scaled_size`]).
    pub scene_size: (u32, u32),
    /// Skinned and morphed instances and the skinning they need.
    pub skinning: SkinningStats,
}

/// The prepasses' outputs in a frame's graph, which [`Renderer::record`] returns for passes
/// added after the renderer's to read. Each is the scene's size (the target's at the render
/// scale), single-sampled, and can be sampled and copied.
///
/// A prepass runs only if a later pass reads one of its outputs; the frame graph culls it
/// otherwise, at no GPU cost. Their consumers arrive with rim light and hair shadows (C2, C5),
/// TAA (R7) and outlines under TAA (P1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrepassTargets {
    /// The character prepass: depth of the skinned and morphed instances alone, every tier
    /// (`Depth32Float`, reverse-Z, cleared to 0).
    pub character_depth: TextureId,
    /// The scene prepass: depth of every opaque instance (`Depth32Float`).
    pub depth: TextureId,
    /// The scene prepass: world-space unit normals, octahedral (`Rg16Float`, cleared to 0).
    pub normals: TextureId,
    /// The scene prepass: each pixel's motion since the last drawn frame, in texture
    /// coordinates (x right, y down; it was at `uv - velocity`), from the camera's motion, the
    /// instance's transform and, for skinned and morphed instances, their skinned positions
    /// (`Rg16Float`). Pixels no geometry covers stay 0; TAA reprojects those from depth.
    pub velocity: TextureId,
    /// The main pass's depth, stored for the tests that compare it with the prepasses'.
    #[cfg(test)]
    pub(crate) main_depth: TextureId,
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
    /// The scene's size as a fraction of the target's, clamped as [`scaled_size`] does.
    render_scale: f32,
    labels: bool,
    layouts: Layouts,
    pipelines: Pipelines,
    frame_uniforms: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    view_uniforms: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    /// The frame's local lights, their lists and the cluster pass.
    lights: Lights,
    /// The frame's characters and their light rigs.
    characters: Characters,
    meshes: MeshPool,
    /// Every family's parameter buffer and each material's group 3.
    materials: Materials,
    /// Instances, draw records and the draw list, persistent across frames.
    scene: GpuScene,
    scene_bind_group: wgpu::BindGroup,
    /// Skinned and morphed instances' outputs and the pass that writes them.
    skinning: Skinning,
    post: PostProcess,
    uploads: wgpu::util::StagingBelt,
    /// What the last `prepare` computed for `record` to upload and draw.
    prepared: Option<PreparedFrame>,
    /// The main view's `proj × view` in the last recorded frame, for velocity.
    previous_clip_from_world: Option<Mat4>,
    /// Kept for the pipelines built when meshes arrive.
    pipeline_cache: Option<wgpu::PipelineCache>,
    stats: RenderStats,
    /// Stores the main pass's depth so a test can read it back.
    #[cfg(test)]
    pub(crate) readable_depth: bool,
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
        let characters = Characters::new(&device);
        let frame_bind_group =
            create_frame_bind_group(&device, &layouts.frame, &frame_uniforms, &lights, &characters);
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
        let materials = Materials::new(&device, gpu.queue());
        let scene = GpuScene::new(&device);
        let scene_bind_group = create_scene_bind_group(&device, &layouts.scene, &scene, &materials);
        let skinning = Skinning::new(&device);
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
            render_scale: 1.0,
            labels: gpu.label("").is_some(),
            layouts,
            pipelines,
            frame_uniforms,
            frame_bind_group,
            view_uniforms,
            view_bind_group,
            lights,
            characters,
            meshes,
            materials,
            scene,
            scene_bind_group,
            skinning,
            post,
            uploads,
            prepared: None,
            previous_clip_from_world: None,
            pipeline_cache: gpu.pipeline_cache().cloned(),
            stats: RenderStats::default(),
            #[cfg(test)]
            readable_depth: false,
        })
    }

    /// Validates and uploads a cooked material ([`sapphire_assets::material::Material::read`])
    /// into the next slot of its family's parameter buffer, and binds its textures (group 3),
    /// which `textures` resolves by ID. Each must have the swizzle the material was cooked with.
    /// The slot is the material's for the renderer's lifetime; its block reaches the GPU with
    /// the next recorded frame.
    ///
    /// Every family uploads. Only anime character materials draw so far: the other families'
    /// shaders come with their packages (sky W2, fog cards W3, environment W4, foliage W6).
    pub fn upload_material<'t>(
        &mut self,
        material: &ArchivedMaterial,
        textures: impl Fn(AssetId) -> Option<&'t GpuTexture>,
    ) -> Result<MaterialHandle, RenderError> {
        let (handle, replaced) =
            self.materials.upload(&self.device, &self.queue, &self.layouts.materials, material, textures)?;
        if replaced && material.family == MaterialFamily::AnimeCharacter {
            self.scene_bind_group =
                create_scene_bind_group(&self.device, &self.layouts.scene, &self.scene, &self.materials);
        }
        self.update_pool_stats();
        Ok(handle)
    }

    /// Replaces material `handle` in place with `material`, of the same family: its one slot is
    /// rewritten and uploaded with the next recorded frame, and its textures rebound. Nothing
    /// else is uploaded or rebuilt, so this is how hot reload (A8) and live link (A9) apply a
    /// material edit.
    pub fn update_material<'t>(
        &mut self,
        handle: MaterialHandle,
        material: &ArchivedMaterial,
        textures: impl Fn(AssetId) -> Option<&'t GpuTexture>,
    ) -> Result<(), RenderError> {
        let before = self.materials.record(handle).map(|record| record.flags);
        self.materials.update(&self.device, &self.layouts.materials, handle, material, textures)?;
        let flags = material.flags();
        // Only the anime character family draws, so only its permutations pick pipelines.
        if before != Some(flags) && material.family == MaterialFamily::AnimeCharacter {
            // A new static permutation: its draws move to other pipelines, built for every
            // format already drawn (meshes in another format build theirs when uploaded).
            let variant = SurfaceVariant::of_material(flags);
            for format in PositionFormat::ALL {
                if self.pipelines.mesh(format).is_some() {
                    self.pipelines.prepare_variant(&self.device, format, variant);
                }
            }
            self.scene.meshes_changed();
        }
        Ok(())
    }

    /// Uploads the materials `mesh` embeds, one handle per material slot, for
    /// [`Self::upload_mesh`]. Embedded materials reference no textures (importers make no texture
    /// assets). A slot bound to a material asset fails with [`RenderError::UnresolvedMaterial`]:
    /// [`Self::upload_slot_materials`] resolves those.
    pub fn upload_embedded_materials(
        &mut self,
        mesh: &ArchivedMesh,
    ) -> Result<Vec<MaterialHandle>, RenderError> {
        self.upload_slot_materials(mesh, |_| None)
    }

    /// One material handle per slot of `mesh`, for [`Self::upload_mesh`]: the materials it embeds
    /// are uploaded, and slots bound to material assets take the handles `assets` gives for their
    /// IDs (the asset runtime uploads a mesh's materials before the mesh, M1 A4). A slot `assets`
    /// can't resolve fails with [`RenderError::UnresolvedMaterial`], before anything is uploaded.
    pub fn upload_slot_materials(
        &mut self,
        mesh: &ArchivedMesh,
        assets: impl Fn(AssetId) -> Option<MaterialHandle>,
    ) -> Result<Vec<MaterialHandle>, RenderError> {
        // Checked first: slots are never freed, so a failure halfway would leak the earlier ones.
        for slot in mesh.materials.iter() {
            match &slot.material {
                ArchivedSlotMaterial::Embedded(material) => material.validate()?,
                ArchivedSlotMaterial::Asset(id) if assets(*id).is_none() => {
                    return Err(RenderError::UnresolvedMaterial { slot: slot.name.to_string(), id: *id });
                }
                ArchivedSlotMaterial::Asset(_) => {}
            }
        }
        mesh.materials
            .iter()
            .map(|slot| match &slot.material {
                ArchivedSlotMaterial::Embedded(material) => self.upload_material(material, |_| None),
                ArchivedSlotMaterial::Asset(id) => Ok(assets(*id).expect("resolved above")),
            })
            .collect()
    }

    /// Validates and uploads a cooked mesh, read in place ([`sapphire_assets::mesh::Mesh::read`]),
    /// into the mesh pool, drawing material slot `i` with `materials[i]`. Its streams are copied
    /// as they are; all its LODs' indices are uploaded and LOD0 is drawn. A skinned or morphed
    /// mesh also uploads its skin and morph targets, and each of its instances is drawn from its
    /// own skinning outputs, in its bind pose until a packet poses it
    /// ([`SkinnedInstance`](crate::SkinnedInstance)).
    ///
    /// The first mesh with f32 positions, or skinned or morphed, also builds the f32 pipelines;
    /// the first skinned or morphed one the skinning and character prepass pipelines.
    pub fn upload_mesh(
        &mut self,
        mesh: &ArchivedMesh,
        materials: &[MaterialHandle],
    ) -> Result<MeshHandle, RenderError> {
        let materials = materials
            .iter()
            .map(|&handle| {
                let record = self.materials.record(handle).ok_or(RenderError::UnknownMaterial(handle))?;
                if record.family != MaterialFamily::AnimeCharacter {
                    return Err(RenderError::NotDrawable(record.family));
                }
                Ok(SubmeshMaterial { handle, slot: record.slot })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (handle, moves) = self.meshes.upload(&self.device, &self.queue, mesh, &materials)?;
        let record = &self.meshes.records()[handle.index()];
        let format = record.draw_format();
        self.pipelines.prepare_format(&self.device, format);
        for material in &materials {
            let flags = self.materials.record(material.handle).map_or(MaterialFlags::empty(), |r| r.flags);
            self.pipelines.prepare_variant(&self.device, format, SurfaceVariant::of_material(flags));
        }
        if record.deform().is_some() {
            self.pipelines.prepare_characters(&self.device);
            self.skinning.enable(&self.device, self.pipeline_cache.as_ref());
        }
        if moves.deformable {
            self.skinning.sources_moved();
        }
        let bounds = mesh.streams.bounds.to_native();
        self.lights.mesh_uploaded(handle, [bounds.min, bounds.max]);
        self.scene.meshes_changed();
        self.update_pool_stats();
        Ok(handle)
    }

    /// The CPU half of a frame: applies the packet's instance deltas to the persistent scene
    /// (rebuilding the draw list only if the drawn set changed), keeps its poses for skinning,
    /// culls and lists the local lights, points the characters at their light rigs, computes
    /// the frame constants, sizes the buffers and bakes any grading LUT the packet's post
    /// parameters changed. [`Self::record`] then uploads what changed, skins and draws.
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

        let materials = &self.materials;
        let material_flags =
            |handle: MaterialHandle| materials.record(handle).map_or(MaterialFlags::empty(), |r| r.flags);
        if self.scene.prepare(
            device,
            queue,
            packet.frame,
            &packet.instances,
            self.meshes.records(),
            material_flags,
        ) {
            self.scene_bind_group =
                create_scene_bind_group(device, &self.layouts.scene, &self.scene, &self.materials);
        }
        self.materials.prepare();
        self.skinning.prepare(device, queue, packet, self.scene.slots(), self.meshes.records());
        let lists_replaced = self.lights.prepare(device, queue, packet, self.scene.slots());
        let characters_replaced = self.characters.prepare(device, queue, packet, self.scene.slots_mut());
        if lists_replaced || characters_replaced {
            self.frame_bind_group = create_frame_bind_group(
                device,
                &self.layouts.frame,
                &self.frame_uniforms,
                &self.lights,
                &self.characters,
            );
        }

        // Without a main view the frame has nothing to draw.
        let lights = self.lights.uniforms();
        self.prepared = packet
            .main_view()
            .map(|view| PreparedFrame { camera: view.camera, constants: FrameUniforms::new(packet, lights) });
        self.post.prepare(queue, &packet.post);
        let counts = self.scene.draw_counts();
        let draws = |include: fn(SurfaceVariant) -> bool| -> u32 {
            SurfaceVariant::ALL
                .iter()
                .zip(counts)
                .filter(|&(&variant, _)| include(variant))
                .map(|(_, n)| n)
                .sum()
        };
        self.stats.draws = draws(|_| true);
        self.stats.alpha_tested_draws = draws(|variant| variant.coverage == Coverage::AlphaTest);
        self.stats.transparent_draws = draws(|variant| variant.coverage == Coverage::Transparent);
        self.stats.face_draws = draws(|variant| variant.part == Part::Face);
        self.stats.outline_draws = self.scene.outline_count();
        self.stats.instances = self.scene.live_instances();
        self.stats.changed_instances = self.scene.changed_instances();
        self.stats.draw_list_builds = self.scene.draw_list_builds();
        self.stats.lut_bakes = self.post.bakes();
        self.stats.lights = self.lights.stats();
        self.stats.characters = self.characters.lit();
        self.stats.ignored_characters = self.characters.ignored();
        self.stats.skinning = self.skinning.stats();
        self.stats.material_uploads = self.materials.pending();
        self.update_pool_stats();
    }

    /// The GPU half of a frame: adds the frame's uploads, the skinning pass, the prepasses, the
    /// main pass and post-processing to `graph`, drawing into `target`, and returns the
    /// prepasses' outputs for later passes to read. Record at most once per [`Self::prepare`],
    /// and only after preparing a packet with a main view. The renderer stays borrowed until the
    /// graph has been executed, which submits the uploads and so returns their staging buffers
    /// for reuse. Scene changes and poses from prepared frames that were never recorded are
    /// uploaded by the next recorded one. Returns `None`, adding nothing, without a prepared
    /// view or for an empty target.
    ///
    /// `target` must be a 2D texture in the format given to [`Self::new`]. The internal
    /// textures (HDR scene color, MSAA color, depth, the prepass targets, the bloom chain) are
    /// transient graph textures sized from it at the render scale ([`Self::set_render_scale`]),
    /// so any size works, including one that changed since the last frame.
    ///
    /// The surface, outline and sky draws share one render pass that writes linear HDR scene
    /// color, so on mobile GPUs the MSAA color and depth are cleared, resolved and discarded
    /// without ever leaving tile memory. Post then reads the resolved scene color and, below
    /// scale 1, upsamples it to the target in its final pass (see the `post` module). On the
    /// clustered light path, frames with local lights first bin them in a compute pass (see the
    /// `lights` module); frames that pose skinned or morphed instances skin them first (see the
    /// `skinning` module). The prepasses run only if a later pass reads what they write
    /// ([`PrepassTargets`]).
    pub fn record<'f>(&'f mut self, graph: &mut FrameGraph<'f>, target: TextureId) -> Option<PrepassTargets> {
        profiling::scope!("Renderer::record");
        debug_assert!(self.prepared.is_some(), "record() needs a prepare() of a packet with a main view");
        let PreparedFrame { camera, constants } = self.prepared.take()?;
        let output = graph.size(target);
        if output.0 == 0 || output.1 == 0 {
            return None;
        }
        // The scene renders at the render scale; the views, attachments, prepass targets and
        // bloom follow it, and only post's final pass writes the target's size.
        let size = scaled_size(output, self.render_scale);
        self.stats.scene_size = size;
        self.skinning.begin_record(&self.device, &self.queue, &self.meshes);
        let clip_from_world = camera.proj * camera.view;
        let previous_clip_from_world =
            self.previous_clip_from_world.replace(clip_from_world).unwrap_or(clip_from_world);

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
        let (depth_desc, depth_store) = self.main_depth(attachment(DEPTH_FORMAT));
        let depth = graph.create_texture("depth", depth_desc);
        let scene_color = graph.create_texture(
            "scene color",
            TransientDesc {
                sample_count: 1,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                ..attachment(SCENE_COLOR_FORMAT)
            },
        );
        // Single-sampled, since later passes sample them, and copyable, so tools and tests can
        // read them back.
        let prepass_target = |format| TransientDesc {
            sample_count: 1,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            ..attachment(format)
        };
        let targets = PrepassTargets {
            character_depth: graph.create_texture("character depth", prepass_target(DEPTH_FORMAT)),
            depth: graph.create_texture("prepass depth", prepass_target(DEPTH_FORMAT)),
            normals: graph.create_texture("normals", prepass_target(NORMALS_FORMAT)),
            velocity: graph.create_texture("velocity", prepass_target(VELOCITY_FORMAT)),
            #[cfg(test)]
            main_depth: depth,
        };

        // The uploads need the staging belt exclusively; everything else is shared.
        let Self {
            labels,
            ref pipelines,
            ref frame_uniforms,
            ref frame_bind_group,
            ref view_uniforms,
            ref view_bind_group,
            ref mut lights,
            ref mut characters,
            ref meshes,
            ref mut materials,
            ref mut scene,
            ref scene_bind_group,
            ref skinning,
            ref mut post,
            ref mut uploads,
            ..
        } = *self;
        let (scene_uploads, draw_list, records) = scene.split_for_record();
        let (material_uploads, material_records) = materials.split_for_record();
        let (light_uploads, cluster_pass) = lights.split_for_record();
        let character_uploads = characters.upload();
        let skinning_frame = skinning.frame();
        let mesh_draws = MeshDraws {
            meshes,
            list: draw_list,
            records,
            skinned: skinning.outputs(),
            materials: material_records,
        };
        let (post_uniforms, post_buffer) = post.uniforms(size != output);
        // A handle, so `post` can be borrowed mutably for its passes below.
        let post_buffer = post_buffer.clone();
        let view = ViewUniforms::new(&camera, size, previous_clip_from_world);

        // Buffers aren't graph resources (see the crate docs), so the uploads are a side effect
        // added first: the passes after read what they copy.
        graph.add_pass("uploads").side_effect().record(move |ctx| {
            upload(uploads, ctx.encoder, frame_uniforms, bytemuck::bytes_of(&constants));
            upload(uploads, ctx.encoder, view_uniforms, bytemuck::bytes_of(&view));
            scene_uploads.write(uploads, ctx.encoder);
            material_uploads.write(uploads, ctx.encoder);
            light_uploads.write(uploads, ctx.encoder);
            character_uploads.write(uploads, ctx.encoder);
            if let Some(skinning) = &skinning_frame {
                skinning.upload(uploads, ctx.encoder);
            }
            upload(uploads, ctx.encoder, &post_buffer, bytemuck::bytes_of(&post_uniforms));
            uploads.finish_and_recall_on_submit(ctx.encoder);
        });
        // Also side effects: the passes after read the buffers they write.
        if let Some(skinning) = skinning_frame {
            graph.add_pass("skinning").side_effect().record(move |ctx| {
                skinning.record(ctx.encoder, labels, ctx.compute_timestamp_writes());
            });
        }
        if let Some(cluster_pass) = cluster_pass {
            graph.add_pass("light clusters").side_effect().record(move |ctx| {
                let timestamps = ctx.compute_timestamp_writes();
                cluster_pass.record(ctx.encoder, view_bind_group, labels, timestamps);
            });
        }
        let bind = move |pass: &mut wgpu::RenderPass<'_>| {
            pass.set_bind_group(0, frame_bind_group, &[]);
            pass.set_bind_group(1, view_bind_group, &[0]);
            pass.set_bind_group(2, scene_bind_group, &[]);
            pass.set_index_buffer(meshes.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
        };

        // The prepasses: nothing reads them yet, so the graph culls them unless a pass added
        // after the renderer's does (see `PrepassTargets`).
        graph.add_pass("character prepass").write(targets.character_depth).record(move |ctx| {
            let mut pass = begin_prepass(ctx, "character prepass", targets.character_depth, &[], labels);
            if let Some(pipeline) = pipelines.character_prepass()
                && mesh_draws.has_any_deformed()
            {
                bind(&mut pass);
                pass.set_pipeline(pipeline);
                // Transparent surfaces write no depth.
                for variant in SurfaceVariant::DEPTH_WRITING {
                    mesh_draws.draw_deformed(
                        &mut pass,
                        MeshPass::CharacterPrepass,
                        variant,
                        DrawSet::Surfaces,
                    );
                }
            }
        });
        graph
            .add_pass("scene prepass")
            .write(targets.depth)
            .write(targets.normals)
            .write(targets.velocity)
            .record(move |ctx| {
                let colors = [targets.normals, targets.velocity];
                let mut pass = begin_prepass(ctx, "scene prepass", targets.depth, &colors, labels);
                bind(&mut pass);
                // Opaque and alpha-tested surfaces, through one pipeline per format: the
                // prepass doesn't cut alpha-tested ones out yet (crate docs, "Later
                // milestones"). Transparent surfaces write no depth.
                let prepass = MeshPass::ScenePrepass;
                for format in PositionFormat::ALL {
                    if !SurfaceVariant::DEPTH_WRITING
                        .iter()
                        .any(|&variant| mesh_draws.has_static(format, variant))
                    {
                        continue;
                    }
                    let pipelines =
                        pipelines.mesh(format).expect("built when a mesh in this format was uploaded");
                    pass.set_pipeline(&pipelines.scene_prepass);
                    for variant in SurfaceVariant::DEPTH_WRITING {
                        mesh_draws.draw_static(&mut pass, prepass, format, variant, DrawSet::Surfaces);
                    }
                }
                if SurfaceVariant::DEPTH_WRITING.iter().any(|&variant| mesh_draws.has_deformed(variant)) {
                    let pipelines =
                        pipelines.mesh(PositionFormat::Float32).expect("built with deformable meshes");
                    pass.set_pipeline(&pipelines.scene_prepass);
                    for variant in SurfaceVariant::DEPTH_WRITING {
                        mesh_draws.draw_deformed(&mut pass, prepass, variant, DrawSet::Surfaces);
                    }
                }
            });

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
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: depth_store }),
                    stencil_ops: None,
                }),
                timestamp_writes: ctx.timestamp_writes(),
                ..Default::default()
            });
            bind(&mut pass);
            // Opaque and alpha-tested surfaces, then their outlines, after them so depth rejects
            // the hull wherever a surface covers it, then the sky where no geometry wrote depth,
            // then transparent surfaces over all of it. Each static permutation draws through
            // its own pipeline (a face's outline is its coverage's); skinned and morphed
            // instances draw last in each group, through the f32 pipelines.
            let groups = SurfaceVariant::DEPTH_WRITING
                .map(|variant| (variant, DrawSet::Surfaces))
                .into_iter()
                .chain(SurfaceVariant::DEPTH_WRITING.map(|variant| (variant, DrawSet::Outlines)));
            for (variant, set) in groups {
                draw_main(&mut pass, pipelines, &mesh_draws, variant, set);
            }
            pass.set_pipeline(&pipelines.sky);
            pass.draw(0..3, 0..1);
            for variant in SurfaceVariant::TRANSPARENT {
                draw_main(&mut pass, pipelines, &mesh_draws, variant, DrawSet::Surfaces);
            }
        });

        post.record(graph, scene_color, target, labels);
        Some(targets)
    }

    /// The main pass's depth and how it is stored: kept in tile memory and discarded.
    #[cfg(not(test))]
    fn main_depth(&self, desc: TransientDesc) -> (TransientDesc, wgpu::StoreOp) {
        (desc, wgpu::StoreOp::Discard)
    }

    /// The main pass's depth and how it is stored: stored and copyable when a test reads it.
    #[cfg(test)]
    fn main_depth(&self, desc: TransientDesc) -> (TransientDesc, wgpu::StoreOp) {
        if !self.readable_depth {
            return (desc, wgpu::StoreOp::Discard);
        }
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        (TransientDesc { usage, ..desc }, wgpu::StoreOp::Store)
    }

    /// Counters from the last [`Self::prepare`] plus pool totals, and the last recorded scene
    /// size.
    pub fn stats(&self) -> RenderStats {
        RenderStats { character_pipelines: self.pipelines.character_pipelines(), ..self.stats }
    }

    /// Sets the scene's size as a fraction of the target's width and height, from the next
    /// [`Self::record`]: the hook for quality tiers and dynamic resolution
    /// ([`Quality::render_scale`](crate::quality::Quality::render_scale)). Clamped to
    /// [`MIN_RENDER_SCALE`]..=1, and 1 when it isn't finite.
    ///
    /// At 1 every frame is exactly as without a render scale. Below it, the scene's targets
    /// shrink to [`scaled_size`] and post's final pass upsamples to the target bilinearly; the
    /// pool makes new transients for each new size, so callers should change it in steps
    /// (dynamic resolution moves in 5% steps).
    pub fn set_render_scale(&mut self, scale: f32) {
        self.render_scale = clamp_render_scale(scale);
    }

    /// The render scale the next [`Self::record`] uses.
    pub fn render_scale(&self) -> f32 {
        self.render_scale
    }

    /// The local lights, for tests that read their buffers back.
    #[cfg(test)]
    pub(crate) fn lights(&self) -> &Lights {
        &self.lights
    }

    /// The skinning state, for tests that read its outputs back.
    #[cfg(test)]
    pub(crate) fn skinning(&self) -> &Skinning {
        &self.skinning
    }

    /// The materials, for tests that read their buffers back.
    #[cfg(test)]
    pub(crate) fn materials(&self) -> &Materials {
        &self.materials
    }

    fn update_pool_stats(&mut self) {
        self.stats.mesh_pool_bytes = self.meshes.bytes_used();
        self.stats.materials = self.materials.len();
        self.stats.material_bytes = self.materials.bytes_used();
        self.stats.buffer_growths = self.meshes.growths()
            + self.materials.growths()
            + self.scene.growths()
            + self.lights.growths()
            + self.characters.growths()
            + self.skinning.growths();
    }
}

/// Draws `set` of the main pass's draws of `variant`, static meshes per format and then the
/// skinned and morphed ones, each group through its pipeline: the surface's, or the outline
/// hull's.
fn draw_main(
    pass: &mut wgpu::RenderPass<'_>,
    pipelines: &Pipelines,
    draws: &MeshDraws<'_>,
    variant: SurfaceVariant,
    set: DrawSet,
) {
    let pipeline = |format| {
        let mesh = pipelines.mesh(format).expect("built when a mesh in this format was uploaded");
        let pipeline = match set {
            DrawSet::Surfaces => mesh.character(variant),
            DrawSet::Outlines => mesh.outline(variant),
        };
        pipeline.expect("built when a draw of this permutation was added")
    };
    for format in PositionFormat::ALL {
        if draws.has_static(format, variant) {
            pass.set_pipeline(pipeline(format));
            draws.draw_static(pass, MeshPass::Main, format, variant, set);
        }
    }
    if draws.has_deformed(variant) {
        pass.set_pipeline(pipeline(PositionFormat::Float32));
        draws.draw_deformed(pass, MeshPass::Main, variant, set);
    }
}

/// Begins a prepass's render pass: `depth` and the `colors` cleared, all stored.
fn begin_prepass<'p>(
    ctx: &'p mut PassContext<'_>,
    label: &'static str,
    depth: TextureId,
    colors: &[TextureId],
    labels: bool,
) -> wgpu::RenderPass<'p> {
    let color_attachments: [Option<wgpu::RenderPassColorAttachment<'_>>; 2] = std::array::from_fn(|index| {
        colors.get(index).map(|&color| wgpu::RenderPassColorAttachment {
            view: ctx.view(color),
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })
    });
    let depth_stencil_attachment = Some(wgpu::RenderPassDepthStencilAttachment {
        view: ctx.view(depth),
        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
        stencil_ops: None,
    });
    let timestamp_writes = ctx.timestamp_writes();
    ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: labels.then_some(label),
        color_attachments: &color_attachments[..colors.len()],
        depth_stencil_attachment,
        timestamp_writes,
        ..Default::default()
    })
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
    characters: &Characters,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("frame"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: frame_uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: lights.light_buffer().as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: lights.list_buffer().as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: characters.buffer().as_entire_binding() },
        ],
    })
}

/// Group 2 for the anime character family's pipelines (and the prepasses): instances and that
/// family's materials.
fn create_scene_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene: &GpuScene,
    materials: &Materials,
) -> wgpu::BindGroup {
    let character = materials.buffer(MaterialFamily::AnimeCharacter);
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scene"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: scene.instance_buffer().as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: character.as_entire_binding() },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_sizes_round_per_side_and_stay_in_the_target() {
        assert_eq!(scaled_size((1920, 1080), 1.0), (1920, 1080), "scale 1 is the target's size");
        assert_eq!(scaled_size((1920, 1080), 0.7), (1344, 756));
        // Odd sizes round to the nearest pixel, halves away from zero.
        assert_eq!(scaled_size((641, 361), 0.5), (321, 181));
        assert_eq!(scaled_size((3, 1), 0.5), (2, 1), "never below one pixel");
        assert_eq!(scaled_size((1000, 100), 0.1), (250, 25), "clamped to the minimum scale");
        assert_eq!(scaled_size((640, 360), 1.5), (640, 360), "never above the target");
        assert_eq!(scaled_size((640, 360), f32::NAN), (640, 360));
        assert_eq!(scaled_size((0, 0), 0.5), (1, 1));
    }
}
