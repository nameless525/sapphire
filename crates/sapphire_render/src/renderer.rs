//! The renderer: compute skinning, the prepasses, the sky cube's face, the shadow maps, the
//! ambient pass (the probe grid's relight and GTAO), a main pass into HDR scene color (anime
//! character, environment and foliage surfaces per surface variant, the characters' outlines,
//! their eyes through hair, the sky, then transparent surfaces and fog cards), lit by each
//! character's light rig (or, for the world, the sun) under the frame's shadows, ambient light,
//! local lights and fog, then post-processing into the target.

use std::future::Future;
use std::num::NonZeroU64;
use std::task::{Context, Poll, Waker};

use bevy_reflect::Reflect;
use glam::Mat4;
use sapphire_assets::AssetId;
use sapphire_assets::material::{ArchivedMaterial, MaterialError, MaterialFamily, MaterialFlags};
use sapphire_assets::mesh::{ArchivedMesh, ArchivedSlotMaterial, MeshError, PositionFormat};
use sapphire_assets::probes::ArchivedProbeGrid;
use sapphire_rhi::{GpuContext, TransientDesc, wgpu};
use serde::{Deserialize, Serialize};

use crate::ambient::{
    Ambient, AmbientOcclusion, AmbientPipelines, AmbientStats, GtaoInputs, ReflectionSettings,
};
use crate::buffer;
use crate::character_prepass::{CharacterPrepassStats, CharacterPrepassTargets};
use crate::characters::Characters;
use crate::draw::{DrawSet, MeshDraws};
use crate::environment::EnvironmentSettings;
use crate::fog::{FogCards, FogStats, GpuFog};
use crate::foliage::{self, FoliageSettings, FoliageWind};
use crate::gpu_types::{FrameUniforms, ViewUniforms};
use crate::graph::{FrameGraph, PassContext, TextureId};
use crate::lights::{LightPath, LightStats, Lights};
use crate::materials::{MaterialHandle, Materials};
use crate::mesh_pool::{MeshPool, MeshRecord, PoolMoves, SubmeshMaterial};
use crate::outlines::OutlineCull;
use crate::pipelines::{
    Coverage, DEPTH_FORMAT, FOG_CARD_DEPTH_BINDING, FOLIAGE_WIND_BINDING, Layouts, MESH_FAMILIES, MeshPass,
    MeshPipelines, NORMALS_FORMAT, OUTLINES_FORMAT, Part, Pipelines, SCENE_COLOR_FORMAT, Shading,
    SurfaceVariant, VELOCITY_FORMAT,
};
use crate::post::{self, PostEffectSettings, PostProcess, PostStats, SunOnScreen};
use crate::scene::GpuScene;
use crate::shaders::{self, Program, ShaderError, ShaderSources};
use crate::shadows::{CasterBindings, ShadowSettings, ShadowStats, Shadows};
use crate::skinning::{Skinning, SkinningStats};
use crate::sky::{self, Sky, SkyPipelines, SkySettings, SkyStats, cube_face_camera};
use crate::temporal::{self, Temporal, TemporalInputs, TemporalMasks, TemporalPipelines, TemporalStats};
use crate::texture::GpuTexture;
use crate::{CameraData, FramePacket, LOG_TARGET, MeshHandle};

/// The first shadow view in the view buffer: after the main view and the sky cube's faces.
const FIRST_SHADOW_VIEW: u32 = 1 + sky::CUBE_FACES;

/// Size of each staging buffer the per-frame uploads are written through. The belt keeps a
/// few (one per frame in flight) and reuses them, so it should hold a frame's uploads:
/// constants plus the changed instances, and the draw records after a rebuild, a few hundred
/// bytes in M0. Bigger frames fall back to a dedicated buffer per upload.
const UPLOAD_CHUNK_SIZE: u64 = 256 * 1024;

/// Multisampling of the main pass. Hull outlines are only a few pixels wide, so they need it,
/// or temporal AA ([`RenderSettings::temporal`]).
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
    /// Temporal AA and upsampling (M1 R7, the [`temporal`](crate::temporal) module): the
    /// projection jitters, and a resolve accumulates the frames at the target's size, which
    /// also upsamples a scene drawn below render scale 1. The desktop tiers turn it on with MSAA
    /// off; it composes with MSAA, which then resolves first.
    pub temporal: bool,
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
    /// The sky cube's size and update rate, and the cirrus layer (see the `sky` module).
    pub sky: SkySettings,
    /// The environment shader's specular model (see the `environment` module).
    pub environment: EnvironmentSettings,
    /// The key light's shadows (M1 R4, the [`shadows`](crate::shadows) module): its cascades
    /// and each character's own shadow. `None` draws none: no maps, no passes, and shaders
    /// compiled as they were without shadows.
    pub shadows: Option<ShadowSettings>,
    /// What the screen effects spend (M1 P3, the [`post`](crate::post) module): the radial
    /// blur's taps per pixel.
    pub post_effects: PostEffectSettings,
    /// Foliage's wind levels, alpha-to-coverage under MSAA and translucency (M1 W6, the
    /// [`foliage`](crate::foliage) module).
    pub foliage: FoliageSettings,
    /// Ambient occlusion (M1 W5, the [`ambient`](crate::ambient) module): capsule AO from the
    /// characters' bodies, and GTAO. Off draws none: no passes, and shaders compiled as they were
    /// without it.
    pub ambient_occlusion: AmbientOcclusion,
    /// The reflection cube a probe grid's environment specular reads (M1 W5): the sky cube's
    /// prefiltered mips. Made only with environment specular on.
    pub reflections: ReflectionSettings,
}

impl Default for RenderSettings {
    /// The desktop tiers' settings with 4× MSAA instead of temporal AA, and without shadows or
    /// ambient occlusion: what the golden scenes from before M1 R7 pin (`--aa msaa`, `--shadows
    /// off` since M1 R4 and `--ao off` since W5) and most GPU tests draw with, since a single
    /// frame shows them completely.
    fn default() -> Self {
        Self {
            msaa: Msaa::default(),
            temporal: false,
            bloom_levels: 5,
            light_path: LightPath::Clustered,
            max_lights: 64,
            sky: SkySettings::default(),
            environment: EnvironmentSettings::default(),
            shadows: None,
            post_effects: PostEffectSettings::default(),
            foliage: FoliageSettings::default(),
            ambient_occlusion: AmbientOcclusion::OFF,
            reflections: ReflectionSettings::default(),
        }
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
    #[error("{0:?} was not uploaded to this renderer")]
    UnknownMesh(MeshHandle),
    #[error("a {from} material can't be updated into a {to} one")]
    MaterialFamilyChanged { from: MaterialFamily, to: MaterialFamily },
    #[error("the {0} family's material buffer is full")]
    MaterialPoolFull(MaterialFamily),
    #[error("the mesh has {slots} material slots, but {handles} materials were given")]
    MaterialCount { slots: usize, handles: usize },
    #[error("meshes don't draw with {0} materials")]
    NotDrawable(MaterialFamily),
    #[error(
        "the mesh has foliage materials but no foliage stream; cook it with foliage import settings \
         (`sapphire_pipeline::bake::foliage`)"
    )]
    NoFoliageStream,
    #[error("foliage materials draw on static meshes only, and this mesh is skinned or morphed")]
    DeformedFoliage,
    #[error(
        "material slot {slot:?} names material asset {id}, which wasn't uploaded; the asset server \
         loads it before the mesh, for `upload_material` and then `upload_slot_materials`"
    )]
    UnresolvedMaterial { slot: String, id: AssetId },
    #[error("the probe grid's {bricks} bricks don't fit this device's atlas, buffers or dispatches")]
    ProbeGridTooLarge { bricks: u32 },
}

/// The character prepass's name in the frame graph (M1 C5): the characters' depth and IDs, on
/// every tier.
pub const CHARACTER_PREPASS: &str = "character prepass";
/// The scene prepass's name in the frame graph (M1 R2): every opaque instance's depth, normals
/// and velocity, where something reads them (temporal AA).
pub const SCENE_PREPASS: &str = "scene prepass";
/// The prepasses, which spec §3.3's "Culling + prepass" row budgets.
pub const PREPASSES: [&str; 2] = [CHARACTER_PREPASS, SCENE_PREPASS];
/// The main pass's name in the frame graph: every family's opaque, alpha-tested and transparent
/// surfaces, their outlines and the sky, forward lit. The overlay and the bench time it against
/// spec §3.3's "Forward opaque" row (3.5 ms on Desktop-Low, 8 ms on Mobile-Low).
pub const MAIN_PASS: &str = "main";

/// Renderer counters for the stats overlay and Tracy (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    /// Surface draws in the last prepared frame, of every material family and surface variant
    /// (the prepasses draw the opaque and alpha-tested ones again; the sky adds one).
    pub draws: u32,
    /// Of [`Self::draws`], those of alpha-tested materials.
    pub alpha_tested_draws: u32,
    /// Of [`Self::draws`], those of transparent materials, blended after the sky.
    pub transparent_draws: u32,
    /// Of [`Self::draws`], those of face materials (M1 C3), through the face variants.
    pub face_draws: u32,
    /// Of [`Self::draws`], those of hair materials (M1 C5), through the hair variants.
    pub hair_draws: u32,
    /// Of [`Self::draws`], those of eye materials (M1 C5), through the eye variants.
    pub eye_draws: u32,
    /// Of [`Self::draws`], those of environment materials (M1 W4), through the environment
    /// shader.
    pub environment_draws: u32,
    /// Of [`Self::draws`], those of foliage materials (M1 W6), through the foliage shader.
    pub foliage_draws: u32,
    /// Instances with at least one foliage draw in the last prepared frame.
    pub foliage_instances: u32,
    /// Outline hulls drawn in the last prepared frame (in the main pass, and in the scene
    /// prepass when it runs).
    pub outline_draws: u32,
    /// Outline hulls the last prepared frame didn't draw: their instances were past their
    /// materials' LOD cutoff (M1 C4, `OutlineCurve::cutoff`).
    pub outlines_culled: u32,
    /// Anime character pipelines built (surface and outline variants, across position
    /// formats): each static permutation's are built when the first draw needs them.
    pub character_pipelines: u32,
    /// Environment pipelines built (surface variants across position formats), likewise.
    pub environment_pipelines: u32,
    /// Foliage pipelines built (surface variants, and their scene prepass and shadow casters,
    /// across position formats), likewise.
    pub foliage_pipelines: u32,
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
    /// The character prepass's draws, and what reads it: faces with the bangs' shadow, eyes
    /// through hair (M1 C5).
    pub character_prepass: CharacterPrepassStats,
    /// The size the last recorded frame's scene rendered at: its target's size at the render
    /// scale ([`scaled_size`]).
    pub scene_size: (u32, u32),
    /// Skinned and morphed instances and the skinning they need.
    pub skinning: SkinningStats,
    /// The sky's layers and its cube.
    pub sky: SkyStats,
    /// Times new shader sources rebuilt pipelines since the renderer was created
    /// ([`Renderer::set_shader_sources`]).
    pub shader_rebuilds: u32,
    /// Temporal AA's jitter, sizes and history resets; all zero when it is off.
    pub temporal: TemporalStats,
    /// The compositing layers post drew (M1 P2).
    pub post: PostStats,
    /// The fog's layers and the fog cards drawn (M1 W3).
    pub fog: FogStats,
    /// The shadows' cascades, character maps, capsules and caster draws; all zero when they are
    /// off.
    pub shadows: ShadowStats,
    /// The ambient probe grid, the reflection cube and ambient occlusion (M1 W5).
    pub ambient: AmbientStats,
}

/// What [`Renderer::set_shader_sources`] rebuilt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShaderUpdate {
    /// Shader modules built from changed files.
    pub programs: u32,
    /// Pipelines built from them: every one the renderer had built from their old versions.
    pub pipelines: u32,
}

/// The prepasses' outputs in a frame's graph, which [`Renderer::record`] returns for passes
/// added after the renderer's to read. Each is the scene's size (the target's at the render
/// scale), single-sampled, and can be sampled and copied.
///
/// The scene prepass runs only if a later pass reads one of its outputs; the frame graph culls
/// it otherwise, at no GPU cost. Temporal AA (M1 R7) reads it. The character prepass (M1 C5) runs
/// every frame: the main pass reads it for the rim, the bangs' shadow and eyes through hair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrepassTargets {
    /// The character prepass: depth of the characters the frame lists alone, opaque and
    /// alpha-tested surfaces, every tier (`Depth32Float`, reverse-Z, cleared to 0). Persistent:
    /// the renderer's own, imported.
    pub character_depth: TextureId,
    /// The character prepass: each pixel's character ID (`R16Uint`, cleared to 0): its entry in
    /// the frame's character buffer in bits 0 to 13 and its part in bits 14 and 15 (0 body, 1
    /// face, 2 hair, 3 eye or brow; `AnimeCharacterParams::SURFACE_*`).
    pub character_ids: TextureId,
    /// The scene prepass: depth of every opaque instance and its outline hull (`Depth32Float`),
    /// bit for bit the main pass's. Under temporal AA it is one of a pair of persistent
    /// textures, since the next frame reads it as its last.
    pub depth: TextureId,
    /// The scene prepass: world-space unit normals, octahedral (`Rg16Float`, cleared to 0); a
    /// hull's are its outline normals.
    pub normals: TextureId,
    /// The scene prepass: each pixel's motion since the last drawn frame, in texture
    /// coordinates (x right, y down; it was at `uv - velocity`), from the camera's motion, the
    /// instance's transform and, for skinned and morphed instances, their skinned positions
    /// (`Rg16Float`). Pixels no geometry covers stay 0; TAA reprojects those from depth. It
    /// ignores the jitter, so geometry that holds still has exactly 0. Outline hulls write their
    /// own (M1 C4).
    pub velocity: TextureId,
    /// The scene prepass: 1 where an outline hull is the nearest surface, else 0 (`R8Unorm`, M1
    /// C4). Temporal AA's responsive mask takes it in, so lines in motion are drawn from the
    /// current frame (M1 P1).
    pub outlines: TextureId,
    /// With temporal AA, the responsive mask (`R8Unorm`): each pixel's reactivity, the share of
    /// the moving history cap it gives up: characters' where a character is the nearest
    /// surface, from the two prepasses' depths, and lines' (all of it) where an outline hull
    /// is, from [`Self::outlines`]; 0 elsewhere. The resolve keeps less history there and
    /// clips it tighter, and reprojects lines by the silhouette (M1 P1); an external upscaler
    /// takes it as its reactive mask.
    pub responsive: Option<TextureId>,
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
    /// The scale on materials' outline LOD cutoffs ([`Self::set_outline_distance`]).
    outline_distance: f32,
    labels: bool,
    layouts: Layouts,
    pipelines: Pipelines,
    frame_uniforms: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    /// Every view's uniforms, one per [`Self::view_stride`]: the main view first, then the sky
    /// cube's six faces (written once, at creation).
    view_uniforms: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    /// Bytes between views in [`Self::view_uniforms`]: a view's size, aligned for dynamic offsets.
    view_stride: u32,
    /// The sky's pipelines, uniforms, default material and cube.
    sky: Sky,
    /// The frame's local lights, their lists and the cluster pass.
    lights: Lights,
    /// The frame's characters and their light rigs.
    characters: Characters,
    /// The character prepass's depth and IDs, which group 0 binds (M1 C5).
    character_prepass: CharacterPrepassTargets,
    /// Scratch for counting the characters with the bangs' shadow.
    character_scratch: Vec<u32>,
    /// The key light's shadow maps, their passes and each frame's plan.
    shadows: Shadows,
    /// The wind foliage sways in, this frame's and the last drawn one's.
    wind: FoliageWind,
    /// The ambient probe grid, the reflection cube, GTAO and their passes (M1 W5).
    ambient: Ambient,
    meshes: MeshPool,
    /// Every family's parameter buffer and each material's group 3.
    materials: Materials,
    /// Instances, draw records and the draw list, persistent across frames.
    scene: GpuScene,
    /// Which draws' outline hulls the frame draws, by their LOD cutoff.
    outlines: OutlineCull,
    /// Fog cards' depth and their order each frame.
    fog_cards: FogCards,
    /// Group 2 per [`MESH_FAMILIES`] entry: the instances and that family's materials.
    scene_bind_groups: [wgpu::BindGroup; MESH_FAMILIES.len()],
    /// Skinned and morphed instances' outputs and the pass that writes them.
    skinning: Skinning,
    post: PostProcess,
    /// Temporal AA's history and passes, with [`RenderSettings::temporal`].
    temporal: Option<Temporal>,
    uploads: wgpu::util::StagingBelt,
    /// What the last `prepare` computed for `record` to upload and draw.
    prepared: Option<PreparedFrame>,
    /// The main view's unjittered `proj × view` in the last recorded frame, for velocity.
    previous_clip_from_world: Option<Mat4>,
    /// Kept for the pipelines built when meshes arrive.
    pipeline_cache: Option<wgpu::PipelineCache>,
    /// The shader sources every pipeline is built from, including those built later.
    shaders: ShaderSources,
    /// `shaders` aren't the compiled-in ones, so pipelines built later are checked
    /// (`Renderer::checked_build`).
    edited_shaders: bool,
    stats: RenderStats,
    /// Stores the main pass's depth so a test can read it back.
    #[cfg(test)]
    pub(crate) readable_depth: bool,
}

#[derive(Debug)]
struct PreparedFrame {
    camera: CameraData,
    /// The view cut in this frame, or in a prepared frame that was never recorded since.
    cut: bool,
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
        let shaders = ShaderSources::compiled_in();
        let post =
            PostProcess::new(gpu, color_format, settings.bloom_levels, settings.post_effects, &shaders)?;

        let device = gpu.device().clone();
        let layouts = Layouts::new(&device);
        let pipelines = Pipelines::new(
            &device,
            &layouts,
            SCENE_COLOR_FORMAT,
            samples,
            settings.environment,
            settings.foliage,
            gpu.pipeline_cache(),
            &shaders,
            settings.shadows.as_ref(),
            settings.temporal,
            crate::ambient::constants(&settings.ambient_occlusion, None),
        );
        let fog_cards = FogCards::new(&device, settings.temporal);
        let uniform = |label, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let frame_uniforms = uniform("frame uniforms", size_of::<FrameUniforms>());
        // Every view at a stride of `min_uniform_buffer_offset_alignment`, each bound by offset:
        // the main camera, then the sky cube's faces, then the shadow cascades and character
        // maps (M1 R4).
        let view_stride = (size_of::<ViewUniforms>() as u32)
            .next_multiple_of(device.limits().min_uniform_buffer_offset_alignment);
        let mut shadows = Shadows::new(&device, &layouts, settings.shadows, gpu.pipeline_cache(), &shaders);
        let views = FIRST_SHADOW_VIEW + shadows.views();
        let view_uniforms = uniform("view uniforms", (view_stride * views) as usize);
        for face in 0..sky::CUBE_FACES {
            let camera = cube_face_camera(face as usize);
            let size = (settings.sky.cube_size, settings.sky.cube_size);
            // Never jittered: the cube is sampled by direction, and only the main view's samples
            // are accumulated by temporal AA.
            let view = ViewUniforms::new(&camera, size, camera.proj * camera.view, [0.0; 2]);
            let offset = u64::from(view_stride * (1 + face));
            gpu.queue().write_buffer(&view_uniforms, offset, bytemuck::bytes_of(&view));
        }
        let lights =
            Lights::new(&device, &settings, &frame_uniforms, &layouts.view, gpu.pipeline_cache(), &shaders);
        let characters = Characters::new(&device, settings.ambient_occlusion.capsules);
        shadows.set_character_buffer(&device, characters.buffer());
        let character_prepass = CharacterPrepassTargets::new(&device);
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
        let mut materials = Materials::new(&device, gpu.queue());
        let sky = Sky::new(gpu, &layouts, &mut materials, settings.sky, samples, &shaders)?;
        // Reflections only where environment specular reads them.
        let reflections = (settings.environment.specular != crate::EnvironmentSpecular::Off)
            .then_some(settings.reflections);
        let mut ambient = Ambient::new(
            &device,
            gpu.queue(),
            &layouts,
            &frame_uniforms,
            sky.cube_view(),
            settings.ambient_occlusion,
            settings.environment.probes,
            settings.temporal,
            reflections,
            gpu.pipeline_cache(),
            &shaders,
        );
        ambient.set_character_buffer(characters.buffer());
        let frame_bind_group = create_frame_bind_group(
            &device,
            &layouts.frame,
            FrameBindings {
                uniforms: &frame_uniforms,
                lights: &lights,
                characters: &characters,
                shadows: &shadows,
                character_prepass: &character_prepass,
                ambient: &ambient,
            },
        );
        let scene = GpuScene::new(&device);
        let wind = FoliageWind::new(&device);
        let scene_bind_groups =
            create_scene_bind_groups(&device, &layouts, &scene, &materials, &fog_cards, &wind);
        let skinning = Skinning::new(&device);
        let temporal = settings.temporal.then(|| Temporal::new(&device, gpu.pipeline_cache(), &shaders));
        let uploads = wgpu::util::StagingBelt::new(device.clone(), UPLOAD_CHUNK_SIZE);
        tracing::info!(
            target: LOG_TARGET,
            ?color_format,
            msaa = samples,
            temporal = settings.temporal,
            light_path = ?settings.light_path,
            max_lights = settings.max_lights,
            environment_specular = ?settings.environment.specular,
            shadows = ?settings.shadows,
            foliage = ?settings.foliage,
            ambient_occlusion = ?settings.ambient_occlusion,
            "renderer created"
        );
        Ok(Self {
            device,
            queue: gpu.queue().clone(),
            settings,
            render_scale: 1.0,
            outline_distance: 1.0,
            labels: gpu.label("").is_some(),
            layouts,
            pipelines,
            frame_uniforms,
            frame_bind_group,
            view_uniforms,
            view_bind_group,
            view_stride,
            sky,
            lights,
            characters,
            character_prepass,
            character_scratch: Vec::new(),
            shadows,
            wind,
            ambient,
            meshes,
            materials,
            scene,
            outlines: OutlineCull::default(),
            fog_cards,
            scene_bind_groups,
            skinning,
            post,
            temporal,
            uploads,
            prepared: None,
            previous_clip_from_world: None,
            pipeline_cache: gpu.pipeline_cache().cloned(),
            shaders,
            edited_shaders: false,
            stats: RenderStats {
                temporal: TemporalStats { enabled: settings.temporal, ..TemporalStats::default() },
                ..RenderStats::default()
            },
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
    /// Every family uploads. Anime character, environment, foliage and fog card materials draw on
    /// meshes, and a sky material draws the sky when a packet names it
    /// ([`FramePacket::sky_material`]).
    pub fn upload_material<'t>(
        &mut self,
        material: &ArchivedMaterial,
        textures: impl Fn(AssetId) -> Option<&'t GpuTexture>,
    ) -> Result<MaterialHandle, RenderError> {
        let (handle, replaced) =
            self.materials.upload(&self.device, &self.queue, &self.layouts.materials, material, textures)?;
        if replaced && MESH_FAMILIES.contains(&material.family) {
            self.scene_bind_groups = create_scene_bind_groups(
                &self.device,
                &self.layouts,
                &self.scene,
                &self.materials,
                &self.fog_cards,
                &self.wind,
            );
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
        // Only the families meshes draw with pick pipelines by their flags.
        if before != Some(flags)
            && let Some(variant) = SurfaceVariant::of_material(material.family, flags)
        {
            // A new static permutation: its draws move to other pipelines, built for every
            // format already drawn (meshes in another format build theirs when uploaded); and
            // eyes through hair, if it draws through now.
            let through_hair = flags.contains(MaterialFlags::THROUGH_HAIR);
            self.checked_build(|renderer| {
                for format in PositionFormat::ALL {
                    if renderer.pipelines.mesh(format).is_some() {
                        renderer.pipelines.prepare_variant(&renderer.device, format, variant);
                        if through_hair {
                            renderer.pipelines.prepare_through_hair(&renderer.device, format, variant);
                        }
                    }
                }
            });
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
    /// into the mesh pool, drawing material slot `i` with `materials[i]`: anime character,
    /// environment or foliage materials, mixed freely within a mesh and across them (each submesh
    /// draws through its family's shader). Its streams are copied as they are; all its LODs'
    /// indices are uploaded and LOD0 is drawn. A skinned or morphed mesh also uploads its skin and
    /// morph targets, and each of its instances is drawn from its own skinning outputs, in its
    /// bind pose until a packet poses it ([`SkinnedInstance`](crate::SkinnedInstance)). Foliage
    /// materials need a static mesh cooked with its foliage stream
    /// ([`RenderError::NoFoliageStream`], [`RenderError::DeformedFoliage`]; see the
    /// [`foliage`](crate::foliage) module).
    ///
    /// The first mesh with f32 positions, or skinned or morphed, also builds the f32 pipelines;
    /// the first skinned or morphed one the skinning and character prepass pipelines.
    pub fn upload_mesh(
        &mut self,
        mesh: &ArchivedMesh,
        materials: &[MaterialHandle],
    ) -> Result<MeshHandle, RenderError> {
        let materials = self.submesh_materials(materials)?;
        let families = materials.iter().filter_map(|material| self.materials.record(material.handle));
        check_foliage(mesh, families.map(|record| record.family))?;
        let (handle, moves) = self.meshes.upload(&self.device, &self.queue, mesh, &materials)?;
        self.prepare_mesh(handle, &materials, moves);
        self.scene.meshes_changed();
        self.update_pool_stats();
        Ok(handle)
    }

    /// Replaces mesh `handle` in place with `mesh`, as [`Self::upload_slot_materials`] and
    /// [`Self::upload_mesh`] would upload it, under the same handle: instances draw the new mesh
    /// from the next frame, skinned ones from their bind pose until posed again. This is how hot
    /// reload (A8) and live link (A9) apply a changed mesh. The materials the new mesh embeds
    /// rewrite, slot by slot, the material slots the old mesh's embedded ones had where the family
    /// matches, rather than taking new ones; slots bound to material assets take `assets`'
    /// handles, as in [`Self::upload_slot_materials`]. The old streams' room in the mesh pool is
    /// given back. Fails, changing nothing, if the mesh or a material it embeds is invalid or a
    /// slot can't be resolved; a mesh pool too full for the new mesh fails after the materials of
    /// slots that need new ones were uploaded, as [`Self::upload_mesh`] does after
    /// [`Self::upload_slot_materials`].
    pub fn update_mesh(
        &mut self,
        handle: MeshHandle,
        mesh: &ArchivedMesh,
        assets: impl Fn(AssetId) -> Option<MaterialHandle>,
    ) -> Result<(), RenderError> {
        /// What a slot draws with: a material asset's handle, one of the old mesh's embedded
        /// materials rewritten, or a new one.
        enum Plan<'m> {
            Asset(MaterialHandle),
            Rewrite(MaterialHandle, &'m ArchivedMaterial),
            Upload(&'m ArchivedMaterial),
        }
        let old_slots =
            &self.meshes.records().get(handle.index()).ok_or(RenderError::UnknownMesh(handle))?.slots;
        mesh.validate()?;
        // Every slot is resolved and checked before anything is uploaded or rewritten.
        let mut plans = Vec::with_capacity(mesh.materials.len());
        let mut families = Vec::with_capacity(mesh.materials.len());
        for (index, slot) in mesh.materials.iter().enumerate() {
            plans.push(match &slot.material {
                ArchivedSlotMaterial::Asset(id) => {
                    let handle = assets(*id).ok_or_else(|| RenderError::UnresolvedMaterial {
                        slot: slot.name.to_string(),
                        id: *id,
                    })?;
                    self.submesh_materials(&[handle])?;
                    families.extend(self.materials.record(handle).map(|record| record.family));
                    Plan::Asset(handle)
                }
                ArchivedSlotMaterial::Embedded(material) => {
                    material.validate()?;
                    if !MESH_FAMILIES.contains(&material.family) {
                        return Err(RenderError::NotDrawable(material.family));
                    }
                    families.push(material.family);
                    // Embedded materials name no textures (importers make no texture assets), so
                    // rewriting one after the pool has the new mesh can't fail.
                    let mut slots = material.family.texture_slots().iter().zip(material.textures.iter());
                    if let Some((desc, id)) =
                        slots.find_map(|(desc, binding)| Some((desc, binding.texture()?)))
                    {
                        return Err(RenderError::MissingTexture { slot: desc.name, id });
                    }
                    let reusable =
                        old_slots.get(index).filter(|old| old.embedded).map(|old| old.material.handle);
                    match reusable {
                        Some(old)
                            if self.materials.record(old).is_some_and(|r| r.family == material.family) =>
                        {
                            Plan::Rewrite(old, material)
                        }
                        _ => Plan::Upload(material),
                    }
                }
            });
        }
        check_foliage(mesh, families)?;
        let mut handles = Vec::with_capacity(plans.len());
        for plan in &plans {
            handles.push(match *plan {
                Plan::Asset(handle) | Plan::Rewrite(handle, _) => handle,
                Plan::Upload(material) => self.upload_material(material, |_| None)?,
            });
        }
        let materials = self.submesh_materials(&handles)?;
        let moves = self.meshes.replace(&self.device, &self.queue, handle, mesh, &materials)?;
        for plan in plans {
            if let Plan::Rewrite(handle, material) = plan {
                self.update_material(handle, material, |_| None)?;
            }
        }
        self.prepare_mesh(handle, &materials, moves);
        self.lights.mesh_replaced();
        self.scene.mesh_replaced(handle);
        self.update_pool_stats();
        Ok(())
    }

    /// The draw-side record of each of `materials`, which must be this renderer's and of a family
    /// meshes draw with.
    fn submesh_materials(&self, materials: &[MaterialHandle]) -> Result<Vec<SubmeshMaterial>, RenderError> {
        materials
            .iter()
            .map(|&handle| {
                let record = self.materials.record(handle).ok_or(RenderError::UnknownMaterial(handle))?;
                if !MESH_FAMILIES.contains(&record.family) {
                    return Err(RenderError::NotDrawable(record.family));
                }
                Ok(SubmeshMaterial { handle, slot: record.slot })
            })
            .collect()
    }

    /// Builds what mesh `handle`, just placed in the pool with `materials`, draws with: the
    /// pipelines of its position format (the prepasses' among them) and its materials'
    /// permutations and eyes through hair, its format's shadow caster, and for a skinned or
    /// morphed mesh the skinning pipeline.
    fn prepare_mesh(&mut self, handle: MeshHandle, materials: &[SubmeshMaterial], moves: PoolMoves) {
        self.checked_build(|renderer| renderer.build_mesh_pipelines(handle, materials, moves));
    }

    /// [`Self::prepare_mesh`]'s pipelines, built from the current shader sources.
    fn build_mesh_pipelines(&mut self, handle: MeshHandle, materials: &[SubmeshMaterial], moves: PoolMoves) {
        let record = &self.meshes.records()[handle.index()];
        let format = record.draw_format();
        self.pipelines.prepare_format(&self.device, format);
        self.shadows.prepare_format(&self.device, format, &self.shaders);
        for material in materials {
            let Some(record) = self.materials.record(material.handle) else { continue };
            let Some(variant) = SurfaceVariant::of_material(record.family, record.flags) else { continue };
            let through_hair = record.flags.contains(MaterialFlags::THROUGH_HAIR);
            self.pipelines.prepare_variant(&self.device, format, variant);
            if through_hair {
                self.pipelines.prepare_through_hair(&self.device, format, variant);
            }
        }
        if record.deform().is_some() {
            self.skinning.enable(&self.device, self.pipeline_cache.as_ref(), &self.shaders);
        }
        if moves.deformable {
            self.skinning.sources_moved();
        }
    }

    /// Builds the renderer's shaders from `sources` from now on (hot reload, live link; see
    /// `shaders` in the crate docs): every program with a changed file is checked with naga,
    /// then its module and every pipeline built from its old version are built again, and
    /// replace the old ones from the next recorded frame, each on the layout it had, so every
    /// bind group stays valid (a rebuilt sky also draws its whole cube again that frame).
    /// Pipelines built later (new permutations, formats, skinning) use them too. Unchanged
    /// programs keep what they have.
    ///
    /// If any program fails, nothing changes and the error says which file and line, or which
    /// pipelines wgpu refused: the renderer goes on drawing with the shaders it has. The work is
    /// synchronous, on this thread, so the frame that applies an edit takes longer.
    pub fn set_shader_sources(&mut self, sources: &ShaderSources) -> Result<ShaderUpdate, ShaderError> {
        profiling::scope!("Renderer::set_shader_sources");
        let changed: Vec<Program> =
            Program::ALL.into_iter().filter(|&program| self.shaders.differs(sources, program)).collect();
        if changed.is_empty() {
            return Ok(ShaderUpdate::default());
        }
        for &program in &changed {
            shaders::validate(program, sources)?;
        }
        // naga accepted every program; wgpu still checks them against the layouts and vertex
        // buffers the renderer builds pipelines with. Error scopes catch what it refuses, which
        // would otherwise be an uncaptured (and in debug builds fatal) GPU error.
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let device = &self.device;
        let cache = self.pipeline_cache.as_ref();
        let mut pipelines = 0;
        let mesh = changed.iter().any(|program| program.is_mesh()).then(|| {
            let (mesh, built) = self.pipelines.rebuilt(device, sources, &changed);
            pipelines += built;
            mesh
        });
        let sky = changed.contains(&Program::Sky).then(|| self.sky.rebuilt_pipelines(device, sources));
        pipelines += if sky.is_some() { SkyPipelines::COUNT } else { 0 };
        let post = changed.contains(&Program::Post).then(|| self.post.rebuilt_pipelines(sources));
        pipelines += if post.is_some() { post::Pipelines::COUNT } else { 0 };
        let temporal = changed
            .contains(&Program::Temporal)
            .then(|| self.temporal.as_ref().map(|temporal| temporal.rebuilt_pipelines(sources)))
            .flatten();
        pipelines += if temporal.is_some() { TemporalPipelines::COUNT } else { 0 };
        let clusters = changed
            .contains(&Program::Clusters)
            .then(|| self.lights.rebuilt_cluster_pipeline(device, sources))
            .flatten();
        let skinning = changed
            .contains(&Program::Skinning)
            .then(|| self.skinning.rebuilt_pipeline(device, cache, sources))
            .flatten();
        pipelines += u32::from(clusters.is_some()) + u32::from(skinning.is_some());
        let shadows = changed
            .contains(&Program::Shadows)
            .then(|| self.shadows.rebuilt_pipelines(device, sources))
            .flatten();
        pipelines += shadows.as_ref().map_or(0, |shadows| shadows.count());
        // Only those built so far: none without a grid or GTAO.
        let ambient = changed.contains(&Program::Ambient).then(|| self.ambient.rebuilt_pipelines(sources));
        pipelines += ambient.as_ref().map_or(0, AmbientPipelines::count);
        let error = scope_error(internal.pop()).or(scope_error(validation.pop()));
        if let Some(error) = error {
            let programs = changed.iter().map(|program| program.name()).collect::<Vec<_>>().join(", ");
            return Err(ShaderError::Pipeline { programs, message: error.to_string() });
        }
        if let Some(mesh) = mesh {
            self.pipelines = mesh;
        }
        if let Some(sky) = sky {
            self.sky.set_pipelines(sky);
        }
        if let Some(post) = post {
            self.post.set_pipelines(post);
        }
        if let (Some(pipelines), Some(temporal)) = (temporal, self.temporal.as_mut()) {
            temporal.set_pipelines(pipelines);
        }
        if let Some(clusters) = clusters {
            self.lights.set_cluster_pipeline(clusters);
        }
        if let Some(skinning) = skinning {
            self.skinning.set_pipeline(skinning);
        }
        if let Some(shadows) = shadows {
            self.shadows.set_pipelines(shadows);
        }
        if let Some(ambient) = ambient {
            self.ambient.set_pipelines(ambient);
        }
        self.shaders = sources.clone();
        self.edited_shaders = !sources.is_compiled_in();
        self.stats.shader_rebuilds += 1;
        let update = ShaderUpdate { programs: changed.len() as u32, pipelines };
        tracing::info!(target: LOG_TARGET, programs = update.programs, pipelines = update.pipelines, "shaders rebuilt");
        Ok(update)
    }

    /// Runs `build`, which may build pipelines from the current shader sources. Edited sources
    /// were checked against the pipelines built when they were applied, not against those built
    /// later (another permutation, f32 positions, the skinning pass): if wgpu refuses one of
    /// those, the renderer goes back to the compiled-in shaders, rebuilding every pipeline from
    /// them, rather than draw with an invalid one, and logs why.
    fn checked_build<R>(&mut self, build: impl FnOnce(&mut Self) -> R) -> R {
        if !self.edited_shaders {
            return build(self);
        }
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let result = build(self);
        if let Some(error) = scope_error(internal.pop()).or(scope_error(validation.pop())) {
            tracing::error!(
                target: LOG_TARGET,
                %error,
                "a pipeline built from the edited shaders failed; going back to the compiled-in shaders"
            );
            if let Err(error) = self.set_shader_sources(&ShaderSources::compiled_in()) {
                tracing::error!(target: LOG_TARGET, %error, "the compiled-in shaders didn't build");
            }
        }
        result
    }

    /// The shader sources the renderer builds with: compiled in, until
    /// [`Self::set_shader_sources`] replaced them.
    pub fn shader_sources(&self) -> &ShaderSources {
        &self.shaders
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
        let material_kind = |handle: MaterialHandle| {
            materials
                .record(handle)
                .map_or((MaterialFamily::AnimeCharacter, MaterialFlags::empty()), |r| (r.family, r.flags))
        };
        if self.scene.prepare(
            device,
            queue,
            packet.frame,
            &packet.instances,
            self.meshes.records(),
            material_kind,
        ) {
            self.scene_bind_groups = create_scene_bind_groups(
                device,
                &self.layouts,
                &self.scene,
                &self.materials,
                &self.fog_cards,
                &self.wind,
            );
        }
        self.materials.prepare();
        self.wind.prepare(packet);
        self.skinning.prepare(device, queue, packet, self.scene.slots(), self.meshes.records());
        let lists_replaced =
            self.lights.prepare(device, queue, packet, self.scene.slots(), self.meshes.records());
        let characters_replaced = self.characters.prepare(device, queue, packet, self.scene.slots_mut());
        if characters_replaced {
            self.shadows.set_character_buffer(device, self.characters.buffer());
            self.ambient.set_character_buffer(self.characters.buffer());
        }
        if lists_replaced || characters_replaced {
            self.rebuild_frame_bind_group();
        }
        let entries = self.characters.entries();
        self.ambient.prepare(packet, entries);
        // How far the frame's wind carries a foliage mesh past its bounds: its materials' reach.
        let materials = &self.materials;
        let wind_margin = |record: &MeshRecord| -> f32 {
            if record.foliage.is_none() {
                return 0.0;
            }
            let reach = record
                .slots
                .iter()
                .filter_map(|slot| materials.foliage_wind(slot.material.handle))
                .map(foliage::wind_reach)
                .fold(0.0, f32::max);
            foliage::wind_margin(reach, &packet.wind)
        };
        self.shadows.prepare(
            packet,
            self.scene.slots(),
            self.meshes.records(),
            &wind_margin,
            self.scene.draw_list(),
            entries,
        );

        // Without a main view the frame has nothing to draw. A cut in a frame that was prepared
        // but never recorded still applies to the next one recorded.
        let lights = self.lights.uniforms();
        let unrecorded_cut = self.prepared.as_ref().is_some_and(|prepared| prepared.cut);
        self.prepared = packet.main_view().map(|view| PreparedFrame {
            camera: view.camera,
            cut: view.cut || unrecorded_cut,
            constants: FrameUniforms::new(packet, lights, self.outline_distance),
        });
        if let Some(view) = packet.main_view() {
            let materials = &self.materials;
            let params = |slot| materials.anime_character(slot);
            let (list, slots) = (self.scene.draw_list(), self.scene.slots());
            let meshes = self.meshes.records();
            self.outlines.prepare(&view.camera, self.outline_distance, list, slots, meshes, params);
            self.fog_cards.sort(view.camera.position, list, slots, meshes);
        }
        self.sky.prepare(packet, &self.materials);
        // The lens flare follows the sun's disc as the main view sees it.
        let camera = packet.main_view().map(|view| view.camera);
        let sun = camera.zip(self.sky.sun_disc()).and_then(|(camera, disc)| SunOnScreen::new(&camera, disc));
        self.post.prepare(queue, &packet.post, camera.as_ref(), sun);
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
        self.stats.face_draws = draws(|variant| variant.shading == Shading::Character(Part::Face));
        self.stats.hair_draws = draws(|variant| variant.shading == Shading::Character(Part::Hair));
        self.stats.eye_draws = draws(|variant| variant.shading == Shading::Character(Part::Eye));
        self.stats.environment_draws = draws(|variant| variant.shading == Shading::Environment);
        self.stats.fog.cards = draws(|variant| variant == SurfaceVariant::FOG_CARD);
        self.stats.fog.layers = GpuFog::new(&packet.fog).layers();
        self.stats.foliage_draws = draws(|variant| variant.shading == Shading::Foliage);
        self.stats.foliage_instances = self.scene.draw_list().foliage_instances;
        let outlines = self.outlines.counts();
        (self.stats.outline_draws, self.stats.outlines_culled) = (outlines.drawn, outlines.culled);
        self.stats.instances = self.scene.live_instances();
        self.stats.changed_instances = self.scene.changed_instances();
        self.stats.draw_list_builds = self.scene.draw_list_builds();
        self.stats.lut_bakes = self.post.bakes();
        self.stats.post = self.post.stats();
        self.stats.lights = self.lights.stats();
        self.stats.characters = self.characters.lit();
        self.stats.ignored_characters = self.characters.ignored();
        let characters = &self.characters;
        self.stats.character_prepass = CharacterPrepassStats::count(
            self.scene.draw_list(),
            |slot| characters.entry(slot),
            &self.materials,
            &mut self.character_scratch,
        );
        self.stats.skinning = self.skinning.stats();
        self.stats.shadows = self.shadows.stats();
        self.stats.ambient = self.ambient.stats();
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
    /// textures (HDR scene color, MSAA color, depth, the scene prepass's targets, the bloom
    /// chain) are transient graph textures sized from it at the render scale
    /// ([`Self::set_render_scale`]), so any size works, including one that changed since the
    /// last frame. Temporal AA's history and kept depth, and the character prepass's depth and
    /// IDs, are persistent textures, made again when the sizes change.
    ///
    /// The surface, outline and sky draws share one render pass that writes linear HDR scene
    /// color, so on mobile GPUs the MSAA color and depth are cleared, resolved and discarded
    /// without ever leaving tile memory. Post then reads the resolved scene color and, below
    /// scale 1, upsamples it to the target in its final pass (see the `post` module). Under
    /// temporal AA the projection is jittered, and the [`temporal`](crate::temporal) resolve
    /// accumulates the scene at the target's size before post reads it. On the clustered light
    /// path, frames with local lights first bin them in a compute pass (see the `lights`
    /// module); frames that pose skinned or morphed instances skin them first (see the
    /// `skinning` module). The character prepass runs every frame, before the main pass, which
    /// reads it; the scene prepass only if a later pass reads what it writes
    /// ([`PrepassTargets`]), as temporal AA does.
    pub fn record<'f>(&'f mut self, graph: &mut FrameGraph<'f>, target: TextureId) -> Option<PrepassTargets> {
        profiling::scope!("Renderer::record");
        debug_assert!(self.prepared.is_some(), "record() needs a prepare() of a packet with a main view");
        let PreparedFrame { camera, cut, constants } = self.prepared.take()?;
        let output = graph.size(target);
        if output.0 == 0 || output.1 == 0 {
            return None;
        }
        // The scene renders at the render scale; the views, attachments, prepass targets and
        // bloom follow it, and only post's final pass (or temporal AA's resolve, which
        // upsamples) writes the target's size.
        let size = scaled_size(output, self.render_scale);
        self.stats.scene_size = size;
        self.skinning.begin_record(&self.device, &self.queue, &self.meshes);
        // The character prepass's targets follow the scene's size, and GTAO's (new textures for a
        // new size): group 0 binds both.
        let prepass_resized = self.character_prepass.resize(&self.device, size);
        if self.ambient.fit(size) || prepass_resized {
            self.rebuild_frame_bind_group();
        }
        // Unjittered: velocity and reprojection compare the camera's motion alone. After a cut
        // the camera counts as unmoved.
        let clip_from_world = camera.proj * camera.view;
        let previous_clip_from_world = match self.previous_clip_from_world.replace(clip_from_world) {
            Some(previous) if !cut => previous,
            _ => clip_from_world,
        };
        let temporal_frame = self.temporal.as_mut().map(|temporal| {
            let reproject = temporal::previous_clip_from_clip(previous_clip_from_world, clip_from_world);
            temporal.begin(size, output, cut, self.post.exposure(), reproject)
        });
        if let Some(temporal) = &self.temporal {
            self.stats.temporal = temporal.stats();
        }
        let jitter = temporal_frame.map_or([0.0; 2], |frame| frame.jitter);
        let temporal_reset = temporal_frame.is_some_and(|frame| frame.reset.is_some());
        // The wind, and the last drawn frame's for velocity, which a cut holds still as it does
        // the camera.
        let wind_uniforms = self.wind.record(cut);

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

        // The uploads need the staging belt exclusively; everything else is shared.
        let Self {
            labels,
            ref pipelines,
            ref frame_uniforms,
            ref frame_bind_group,
            ref view_uniforms,
            ref view_bind_group,
            view_stride,
            ref mut sky,
            ref mut lights,
            ref mut characters,
            ref character_prepass,
            ref shadows,
            ref wind,
            ref mut ambient,
            ref meshes,
            ref mut materials,
            ref mut scene,
            ref outlines,
            ref mut fog_cards,
            ref scene_bind_groups,
            ref layouts,
            ref device,
            ref skinning,
            ref mut post,
            ref mut temporal,
            ref mut uploads,
            ..
        } = *self;
        // Temporal AA keeps the scene prepass's depth, outline hulls included (M1 C4), for the
        // next frame's disocclusion test: the scene prepass draws into one of a persistent pair
        // it imports along with its history. The main pass's depth stays in tile memory.
        let temporal = temporal.as_mut().map(Temporal::split_for_record);
        let temporal_targets = temporal.as_ref().map(|(textures, _)| textures.import(graph, temporal_reset));
        let depth = graph.create_texture("depth", depth_desc);
        let (character_depth, character_ids) = character_prepass.import(graph);
        let targets = PrepassTargets {
            character_depth,
            character_ids,
            depth: match temporal_targets {
                Some(temporal) => temporal.depth,
                None => graph.create_texture("prepass depth", prepass_target(DEPTH_FORMAT)),
            },
            normals: graph.create_texture("normals", prepass_target(NORMALS_FORMAT)),
            velocity: graph.create_texture("velocity", prepass_target(VELOCITY_FORMAT)),
            outlines: graph.create_texture("outline mask", prepass_target(OUTLINES_FORMAT)),
            responsive: temporal
                .is_some()
                .then(|| graph.create_texture("responsive mask", temporal::responsive_desc(size))),
            #[cfg(test)]
            main_depth: depth,
        };
        // The buffers fog cards' depth group binds, by handle: the splits below borrow their owners.
        let card_buffers =
            (scene.instance_buffer().clone(), materials.buffer(MaterialFamily::FogCard).clone());
        let (scene_uploads, draw_list, records) = scene.split_for_record();
        let (material_uploads, material_records) = materials.split_for_record();
        let (sky_uniforms, sky_buffer, sky_draw, sky_cube) = sky.record(material_records);
        let ambient = ambient.split_for_record(sky_cube.as_ref().map_or(0..0, |cube| cube.faces()));
        let ambient_upload = ambient.upload;
        let (light_uploads, cluster_pass) = lights.split_for_record();
        let (character_uploads, character_entries) = characters.split_for_record();
        let skinning_frame = skinning.frame();
        let mesh_draws = MeshDraws {
            meshes,
            list: draw_list,
            records,
            skinned: skinning.outputs(),
            materials: material_records,
            hulls: outlines.visible(),
        };
        // Temporal AA's resolve writes the target's size, so post upsamples only without it.
        let (post_uniforms, post_buffer) = post.uniforms(temporal.is_none() && size != output, output);
        // Handles, so `post` and `temporal` can be borrowed mutably for their passes below.
        let post_buffer = post_buffer.clone();
        let temporal_upload = temporal.as_ref().map(|(_, passes)| {
            let (uniforms, buffer) = passes.uniforms();
            (uniforms, buffer.clone())
        });
        let view = ViewUniforms::new(&camera, size, previous_clip_from_world, jitter);

        // Buffers aren't graph resources (see the crate docs), so the uploads are a side effect
        // added first: the passes after read what they copy.
        graph.add_pass("uploads").side_effect().record(move |ctx| {
            upload(uploads, ctx.encoder, frame_uniforms, bytemuck::bytes_of(&constants));
            upload(uploads, ctx.encoder, view_uniforms, bytemuck::bytes_of(&view));
            upload(uploads, ctx.encoder, sky_buffer, bytemuck::bytes_of(&sky_uniforms));
            upload(uploads, ctx.encoder, wind.buffer(), bytemuck::bytes_of(&wind_uniforms));
            scene_uploads.write(uploads, ctx.encoder);
            material_uploads.write(uploads, ctx.encoder);
            light_uploads.write(uploads, ctx.encoder);
            character_uploads.write(uploads, ctx.encoder);
            if let Some(skinning) = &skinning_frame {
                skinning.upload(uploads, ctx.encoder);
            }
            upload(uploads, ctx.encoder, &post_buffer, bytemuck::bytes_of(&post_uniforms));
            if let Some((uniforms, buffer)) = &temporal_upload {
                upload(uploads, ctx.encoder, buffer, bytemuck::bytes_of(uniforms));
            }
            shadows.upload(uploads, ctx.encoder, view_uniforms, FIRST_SHADOW_VIEW, view_stride);
            if let Some(ambient_upload) = &ambient_upload {
                ambient_upload.write(uploads, ctx.encoder);
            }
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
        // The sky cube's face (all six on the first frame): persistent, so a side effect too,
        // before the main pass, whose readers (W4, W5) then see this frame's sky.
        if let Some(cube) = sky_cube {
            graph.add_pass("sky cube").side_effect().record(move |ctx| {
                let timestamps = ctx.timestamp_writes();
                let offset = |face: u32| view_stride * (1 + face);
                cube.record(ctx.encoder, frame_bind_group, view_bind_group, offset, labels, timestamps);
            });
        }
        // The shadow maps (M1 R4): persistent too, so side effects before the main pass. Their
        // casters bind the anime character family's scene group, as the prepasses do, and
        // foliage's casters foliage's (M1 W6).
        let foliage_group = Shading::Foliage.scene_group();
        let casters = CasterBindings {
            scene_group: &scene_bind_groups[0],
            foliage_group: &scene_bind_groups[foliage_group],
            mesh: pipelines,
        };
        shadows.record(graph, mesh_draws, view_bind_group, casters, FIRST_SHADOW_VIEW, view_stride, labels);
        // Group 2 starts as the anime character family's, which the prepasses bind for every
        // family's draws but foliage's (they read no material); the main pass binds each family's
        // in turn.
        let bind = move |pass: &mut wgpu::RenderPass<'_>| {
            pass.set_bind_group(0, frame_bind_group, &[]);
            pass.set_bind_group(1, view_bind_group, &[0]);
            pass.set_bind_group(2, &scene_bind_groups[0], &[]);
            pass.set_index_buffer(meshes.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
        };

        // The character prepass (M1 C5): the main pass reads it through group 0 and temporal AA
        // through the graph; its targets are imported, so it always runs. The characters the
        // frame lists alone, their anime character surfaces that write depth (transparent ones
        // don't): an unlisted instance (a skinned prop, an anime-shaded pedestal) stays out of
        // what the rim, the bangs' shadow, eyes through hair and the responsive mask read. Its
        // layout leaves group 0, which binds its targets, out.
        graph.add_pass(CHARACTER_PREPASS).write(targets.character_depth).write(targets.character_ids).record(
            move |ctx| {
                let colors = [targets.character_ids];
                let mut pass =
                    begin_prepass(ctx, CHARACTER_PREPASS, targets.character_depth, &colors, labels);
                pass.set_bind_group(1, view_bind_group, &[0]);
                pass.set_bind_group(2, &scene_bind_groups[0], &[]);
                pass.set_index_buffer(meshes.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
                let listed =
                    |slot: u32| character_entries.get(slot as usize).is_some_and(|&entry| entry != 0);
                let prepass = MeshPass::CharacterPrepass;
                for format in PositionFormat::ALL {
                    let Some(mesh) = pipelines.mesh(format) else { continue };
                    let variants = SurfaceVariant::CHARACTER_DEPTH_WRITING;
                    if variants.iter().any(|&variant| mesh_draws.has_static(format, variant)) {
                        pass.set_pipeline(&mesh.character_prepass);
                        for variant in variants {
                            mesh_draws.draw_static_where(
                                &mut pass,
                                prepass,
                                format,
                                variant,
                                DrawSet::Surfaces,
                                listed,
                            );
                        }
                    }
                }
                let deformed = pipelines.mesh(PositionFormat::Float32);
                if let Some(mesh) = deformed
                    && SurfaceVariant::CHARACTER_DEPTH_WRITING
                        .iter()
                        .any(|&variant| mesh_draws.has_deformed(variant))
                {
                    pass.set_pipeline(&mesh.character_prepass);
                    for variant in SurfaceVariant::CHARACTER_DEPTH_WRITING {
                        mesh_draws.draw_deformed_where(
                            &mut pass,
                            prepass,
                            variant,
                            DrawSet::Surfaces,
                            listed,
                        );
                    }
                }
            },
        );
        graph
            .add_pass(SCENE_PREPASS)
            .write(targets.depth)
            .write(targets.normals)
            .write(targets.velocity)
            .write(targets.outlines)
            .record(move |ctx| {
                let colors = [targets.normals, targets.velocity, targets.outlines];
                let mut pass = begin_prepass(ctx, SCENE_PREPASS, targets.depth, &colors, labels);
                bind(&mut pass);
                // Opaque and alpha-tested surfaces of every family but foliage, through one
                // pipeline per format: the prepass doesn't cut their alpha-tested ones out yet
                // (crate docs, "Later milestones"). Transparent surfaces write no depth.
                let prepass = MeshPass::ScenePrepass;
                for format in PositionFormat::ALL {
                    if !SurfaceVariant::SHARED_DEPTH
                        .iter()
                        .any(|&variant| mesh_draws.has_static(format, variant))
                    {
                        continue;
                    }
                    let pipelines =
                        pipelines.mesh(format).expect("built when a mesh in this format was uploaded");
                    pass.set_pipeline(&pipelines.scene_prepass);
                    for variant in SurfaceVariant::SHARED_DEPTH {
                        mesh_draws.draw_static(&mut pass, prepass, format, variant, DrawSet::Surfaces);
                    }
                }
                if SurfaceVariant::SHARED_DEPTH.iter().any(|&variant| mesh_draws.has_deformed(variant)) {
                    let pipelines =
                        pipelines.mesh(PositionFormat::Float32).expect("built with deformable meshes");
                    pass.set_pipeline(&pipelines.scene_prepass);
                    for variant in SurfaceVariant::SHARED_DEPTH {
                        mesh_draws.draw_deformed(&mut pass, prepass, variant, DrawSet::Surfaces);
                    }
                }
                // Foliage (M1 W6) through its own pipelines per coverage, on its scene group (the
                // wind) and each draw's group 3: displaced by this frame's wind, with velocity
                // from the last drawn frame's, and its alpha-tested cards cut out as the main pass
                // cuts them.
                let mut foliage_bound = false;
                for format in PositionFormat::ALL {
                    let Some(mesh) = pipelines.mesh(format) else { continue };
                    for variant in SurfaceVariant::FOLIAGE {
                        if !mesh_draws.has_static(format, variant) {
                            continue;
                        }
                        let pipeline =
                            mesh.foliage_prepass(variant.coverage).expect("built with the variant's surface");
                        if !foliage_bound {
                            pass.set_bind_group(2, &scene_bind_groups[foliage_group], &[]);
                            foliage_bound = true;
                        }
                        pass.set_pipeline(pipeline);
                        let foliage = MeshPass::FoliagePrepass;
                        mesh_draws.draw_static(&mut pass, foliage, format, variant, DrawSet::Surfaces);
                    }
                }
                if foliage_bound {
                    pass.set_bind_group(2, &scene_bind_groups[0], &[]);
                }
                // Then the outline hulls, after every surface as in the main pass and with the
                // same depths, writing their own velocity (M1 C4): temporal AA reprojects a line
                // by its own motion. Every outlined variant shares one pipeline per format.
                let hulls = MeshPass::OutlinePrepass;
                for format in PositionFormat::ALL {
                    let Some(pipeline) = pipelines.mesh(format).and_then(MeshPipelines::outline_prepass)
                    else {
                        continue;
                    };
                    if SurfaceVariant::OUTLINED.iter().any(|&variant| mesh_draws.has_static(format, variant))
                    {
                        pass.set_pipeline(pipeline);
                        for variant in SurfaceVariant::OUTLINED {
                            mesh_draws.draw_static(&mut pass, hulls, format, variant, DrawSet::Outlines);
                        }
                    }
                }
                let deformed_hulls =
                    pipelines.mesh(PositionFormat::Float32).and_then(MeshPipelines::outline_prepass);
                if let Some(pipeline) = deformed_hulls
                    && SurfaceVariant::OUTLINED.iter().any(|&variant| mesh_draws.has_deformed(variant))
                {
                    pass.set_pipeline(pipeline);
                    for variant in SurfaceVariant::OUTLINED {
                        mesh_draws.draw_deformed(&mut pass, hulls, variant, DrawSet::Outlines);
                    }
                }
            });
        // The ambient pass (M1 W5), a side effect before the main pass, which reads what it
        // writes: with a probe grid, the sky's projection and the reflection cube's prefilter,
        // which read this frame's face, the grid's relight and the characters' ambient; with GTAO,
        // GTAO from the scene prepass, which its reads make run, without the characters the
        // character prepass marks.
        if let Some(pass) = ambient.pass {
            let inputs = GtaoInputs {
                depth: targets.depth,
                normals: targets.normals,
                outlines: targets.outlines,
                characters: targets.character_depth,
            };
            pass.add(graph, view_bind_group, inputs, labels);
        }

        // Fog cards fade against the scene prepass's depth where the frame has it.
        let card_depth_fade = fog_cards.depth_fade();
        let (card_draws, card_groups) = fog_cards.split_for_record();
        let card_depth = (card_depth_fade && !card_draws.is_empty()).then_some(targets.depth);
        let mut main = graph
            .add_pass(MAIN_PASS)
            .read(targets.character_depth)
            .read(targets.character_ids)
            .write(scene_color)
            .write(depth);
        if let Some(msaa_color) = msaa_color {
            main = main.write(msaa_color);
        }
        if let Some(card_depth) = card_depth {
            main = main.read(card_depth);
        }
        main.record(move |ctx| {
            // Fog cards' scene group, with the depth they fade against where they read the
            // prepass's: made before the pass begins, since the depth's view borrows the context.
            let card_family = SurfaceVariant::FOG_CARD.shading.scene_group();
            let card_group = match card_depth {
                Some(card_depth) => {
                    let (instances, materials) = &card_buffers;
                    let layout = &layouts.scene[card_family];
                    card_groups.get(device, layout, instances, materials, ctx.view(card_depth))
                }
                None => &scene_bind_groups[card_family],
            };
            let (view, resolve_target, store) = match msaa_color {
                Some(msaa_color) => {
                    (ctx.view(msaa_color), Some(ctx.view(scene_color)), wgpu::StoreOp::Discard)
                }
                None => (ctx.view(scene_color), None, wgpu::StoreOp::Store),
            };
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: labels.then_some(MAIN_PASS),
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
            // Opaque and alpha-tested surfaces (characters before the world in each coverage, so
            // depth rejects the world behind them), then the characters' outlines, after them so
            // depth rejects the hull wherever a surface covers it, then eyes through the bangs
            // over the hair (M1 C5), then the sky where no geometry wrote depth, then transparent
            // surfaces over all of it. Each surface variant draws through its own pipeline (a
            // face's outline is its coverage's), binding its family's scene group when the
            // family changes; skinned and morphed instances draw last in each group, through the
            // f32 pipelines.
            let groups = SurfaceVariant::DEPTH_WRITING
                .map(|variant| (variant, DrawSet::Surfaces))
                .into_iter()
                .chain(SurfaceVariant::OUTLINED.map(|variant| (variant, DrawSet::Outlines)))
                .chain(SurfaceVariant::THROUGH_HAIR.map(|variant| (variant, DrawSet::ThroughHair)));
            let mut scene_group = Some(0);
            for (variant, set) in groups {
                draw_main(
                    &mut pass,
                    pipelines,
                    &mesh_draws,
                    scene_bind_groups,
                    &mut scene_group,
                    variant,
                    set,
                );
            }
            sky_draw.draw(&mut pass);
            // The sky's pipeline bound its own groups 2 and 3; mesh draws bind their family's
            // group 2 again, and group 3 per draw.
            let mut scene_group = None;
            for variant in SurfaceVariant::TRANSPARENT {
                draw_main(
                    &mut pass,
                    pipelines,
                    &mesh_draws,
                    scene_bind_groups,
                    &mut scene_group,
                    variant,
                    DrawSet::Surfaces,
                );
            }
            // Fog cards last, back to front (M1 W3), their scene group holding the depth they
            // fade against.
            if !card_draws.is_empty() {
                pass.set_bind_group(2, card_group, &[]);
                let pipeline = |format| {
                    let mesh = pipelines.mesh(format).expect("built when a mesh in this format was uploaded");
                    mesh.surface(SurfaceVariant::FOG_CARD).expect("built when a fog card draw was added")
                };
                mesh_draws.draw_ordered(
                    &mut pass,
                    MeshPass::Main,
                    SurfaceVariant::FOG_CARD,
                    card_draws,
                    pipeline,
                );
            }
        });

        // Temporal AA resolves the HDR scene into the target's size before bloom and grading.
        let resolved = match (temporal, temporal_targets, targets.responsive) {
            (Some((textures, passes)), Some(temporal_targets), Some(responsive)) => {
                let inputs = TemporalInputs {
                    color: scene_color,
                    depth: targets.depth,
                    velocity: targets.velocity,
                    responsive,
                };
                let masks = TemporalMasks { characters: targets.character_depth, outlines: targets.outlines };
                let slot = textures.write_slot();
                passes.record(graph, masks, inputs, temporal_targets, slot, labels);
                temporal_targets.output
            }
            _ => scene_color,
        };
        post.record(graph, resolved, scene_color, target, labels);
        Some(targets)
    }

    /// The main pass's depth, a transient, and how it is stored: kept in tile memory and
    /// discarded.
    #[cfg(not(test))]
    fn main_depth(&self, desc: TransientDesc) -> (TransientDesc, wgpu::StoreOp) {
        (desc, wgpu::StoreOp::Discard)
    }

    /// The main pass's depth, a transient, and how it is stored: stored and copyable when a test
    /// reads it.
    #[cfg(test)]
    fn main_depth(&self, desc: TransientDesc) -> (TransientDesc, wgpu::StoreOp) {
        if !self.readable_depth {
            return (desc, wgpu::StoreOp::Discard);
        }
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        (TransientDesc { usage, ..desc }, wgpu::StoreOp::Store)
    }

    /// Counters from the last [`Self::prepare`] plus pool totals, and the last recorded scene
    /// size, sky cube faces and ambient textures.
    pub fn stats(&self) -> RenderStats {
        RenderStats {
            character_pipelines: self.pipelines.built(MaterialFamily::AnimeCharacter),
            environment_pipelines: self.pipelines.built(MaterialFamily::Environment),
            foliage_pipelines: self.pipelines.built(MaterialFamily::Foliage),
            sky: self.sky.stats(),
            fog: FogStats { card_pipelines: self.pipelines.built(MaterialFamily::FogCard), ..self.stats.fog },
            ambient: self.ambient.stats(),
            ..self.stats
        }
    }

    /// Uploads a zone's ambient probe grid ([`sapphire_assets::probes::ProbeGrid::read`], M1
    /// W5), replacing the grid the renderer had (a hot reload hands the new one here), or removes
    /// it (`None`). The next frame relights all of it, later ones as the light changes; the
    /// environment shader reads it in place of the ambient hemisphere where it reaches, and
    /// characters read their ambient from it (see the [`ambient`](crate::ambient) module). The
    /// environment's pipelines are built again with the grid's constants on (or off), and the
    /// first grid builds the ambient pass's light pipelines and the reflection cube, on this
    /// thread: a load-time cost.
    ///
    /// Fails, keeping no grid, if the grid is larger than the device's atlas, buffers or
    /// dispatches allow.
    pub fn set_probe_grid(&mut self, grid: Option<&ArchivedProbeGrid>) -> Result<(), RenderError> {
        profiling::scope!("Renderer::set_probe_grid");
        let result = self
            .checked_build(|renderer| renderer.ambient.set_grid(&renderer.queue, grid, &renderer.shaders));
        if result.is_err() {
            self.ambient.set_grid(&self.queue, None, &self.shaders).expect("removing a grid can't fail");
        }
        self.rebuild_frame_bind_group();
        let constants = self.ambient.constants();
        self.checked_build(|renderer| renderer.pipelines.set_ambient_constants(&renderer.device, constants));
        // Entries the character pass wrote for the old grid go back to the CPU's.
        self.characters.invalidate();
        result
    }

    /// Rebuilds group 0 after a buffer or texture in it was replaced.
    fn rebuild_frame_bind_group(&mut self) {
        self.frame_bind_group = create_frame_bind_group(
            &self.device,
            &self.layouts.frame,
            FrameBindings {
                uniforms: &self.frame_uniforms,
                lights: &self.lights,
                characters: &self.characters,
                shadows: &self.shadows,
                character_prepass: &self.character_prepass,
                ambient: &self.ambient,
            },
        );
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

    /// Scales every material's outline LOD cutoff (M1 C4, `OutlineCurve::cutoff`) from the next
    /// [`Self::prepare`]: below 1 lines stop nearer, fading out over the cutoff's last quarter as
    /// always; 0 draws no hulls at all. The hook for the `r.outline_distance` cvar
    /// ([`Quality::outline_distance`](crate::quality::Quality::outline_distance)), for tuning and
    /// profiling. Clamped to at least 0, and 1 when it isn't finite.
    pub fn set_outline_distance(&mut self, scale: f32) {
        self.outline_distance = if scale.is_finite() { scale.max(0.0) } else { 1.0 };
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

    /// Temporal AA's state, for tests that read its history back.
    #[cfg(test)]
    pub(crate) fn temporal(&self) -> Option<&Temporal> {
        self.temporal.as_ref()
    }

    /// The materials, for tests that read their buffers back.
    #[cfg(test)]
    pub(crate) fn materials(&self) -> &Materials {
        &self.materials
    }

    /// The sky, for tests that read its cube back.
    #[cfg(test)]
    pub(crate) fn sky(&self) -> &Sky {
        &self.sky
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
/// hull's. Binds the variant's family's scene group first unless `bound` says it is bound
/// already, and only if the variant has draws.
fn draw_main(
    pass: &mut wgpu::RenderPass<'_>,
    pipelines: &Pipelines,
    draws: &MeshDraws<'_>,
    scene_bind_groups: &[wgpu::BindGroup],
    bound: &mut Option<usize>,
    variant: SurfaceVariant,
    set: DrawSet,
) {
    if !draws.has_any(variant) {
        return;
    }
    let group = variant.shading.scene_group();
    if *bound != Some(group) {
        pass.set_bind_group(2, &scene_bind_groups[group], &[]);
        *bound = Some(group);
    }
    // Eyes through hair are only for the draws whose material draws through: a face or eye
    // variant with none has no pipeline for it, and nothing to draw.
    let pipeline = |format| {
        let mesh = pipelines.mesh(format).expect("built when a mesh in this format was uploaded");
        match set {
            DrawSet::Surfaces => {
                Some(mesh.surface(variant).expect("built when a draw of this variant was added"))
            }
            DrawSet::Outlines => {
                Some(mesh.outline(variant).expect("built when a draw of this variant was added"))
            }
            DrawSet::ThroughHair => mesh.through_hair(variant),
        }
    };
    // Foliage reads its own stream too.
    let kind = variant.shading.main_pass();
    for format in PositionFormat::ALL {
        if draws.has_static(format, variant)
            && let Some(pipeline) = pipeline(format)
        {
            pass.set_pipeline(pipeline);
            draws.draw_static(pass, kind, format, variant, set);
        }
    }
    if draws.has_deformed(variant)
        && let Some(pipeline) = pipeline(PositionFormat::Float32)
    {
        pass.set_pipeline(pipeline);
        draws.draw_deformed(pass, kind, variant, set);
    }
}

/// Checks that a mesh drawing with materials of `families` can draw foliage: only a static mesh
/// with a foliage stream can (see the [`foliage`](crate::foliage) module).
fn check_foliage(
    mesh: &ArchivedMesh,
    families: impl IntoIterator<Item = MaterialFamily>,
) -> Result<(), RenderError> {
    if !families.into_iter().any(|family| family == MaterialFamily::Foliage) {
        return Ok(());
    }
    if mesh.skin.is_some() || !mesh.morph_targets.is_empty() {
        return Err(RenderError::DeformedFoliage);
    }
    if mesh.streams.foliage_bytes().is_empty() {
        return Err(RenderError::NoFoliageStream);
    }
    Ok(())
}

/// Begins a prepass's render pass: `depth` and the `colors` cleared, all stored.
fn begin_prepass<'p>(
    ctx: &'p mut PassContext<'_>,
    label: &'static str,
    depth: TextureId,
    colors: &[TextureId],
    labels: bool,
) -> wgpu::RenderPass<'p> {
    let color_attachments: [Option<wgpu::RenderPassColorAttachment<'_>>; 3] = std::array::from_fn(|index| {
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

/// The error an error scope caught: `pop`'s future, which native backends resolve at once. Only
/// WebGPU resolves it later, and the engine has no web build (spec §1.3).
fn scope_error(pop: impl Future<Output = Option<wgpu::Error>>) -> Option<wgpu::Error> {
    let mut pop = std::pin::pin!(pop);
    match pop.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(error) => error,
        Poll::Pending => None,
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

/// What group 0 binds, by owner (the crate docs' binding table).
struct FrameBindings<'a> {
    uniforms: &'a wgpu::Buffer,
    lights: &'a Lights,
    characters: &'a Characters,
    shadows: &'a Shadows,
    character_prepass: &'a CharacterPrepassTargets,
    ambient: &'a Ambient,
}

fn create_frame_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    bindings: FrameBindings<'_>,
) -> wgpu::BindGroup {
    let FrameBindings { uniforms: frame_uniforms, lights, characters, shadows, character_prepass, ambient } =
        bindings;
    let [shadow_uniforms, cascades, atlas, sampler] = shadows.frame_entries();
    let [character_depth, character_ids, character_sampler] = character_prepass.frame_entries();
    let [ambient_uniforms, probe_atlas, bricks, reflection, occlusion, ambient_sampler] =
        ambient.frame_entries();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("frame"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: frame_uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: lights.light_buffer().as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: lights.list_buffer().as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: characters.buffer().as_entire_binding() },
            shadow_uniforms,
            cascades,
            atlas,
            sampler,
            character_depth,
            character_ids,
            character_sampler,
            ambient_uniforms,
            probe_atlas,
            bricks,
            reflection,
            occlusion,
            ambient_sampler,
        ],
    })
}

/// Group 2 per [`MESH_FAMILIES`] entry, for that family's pipelines: the instances and the
/// family's materials, for fog cards the stand-in for the depth they fade against (frames that
/// read the prepass's bind a group of their own, `fog::CardGroupCache`), and for foliage the wind.
/// The shared prepass and caster pipelines bind the
/// anime character family's.
fn create_scene_bind_groups(
    device: &wgpu::Device,
    layouts: &Layouts,
    scene: &GpuScene,
    materials: &Materials,
    fog_cards: &FogCards,
    wind: &FoliageWind,
) -> [wgpu::BindGroup; MESH_FAMILIES.len()] {
    std::array::from_fn(|group| {
        let family = MESH_FAMILIES[group];
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: scene.instance_buffer().as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: materials.buffer(family).as_entire_binding() },
        ];
        match family {
            MaterialFamily::FogCard => entries.push(wgpu::BindGroupEntry {
                binding: FOG_CARD_DEPTH_BINDING,
                resource: wgpu::BindingResource::TextureView(fog_cards.stand_in()),
            }),
            MaterialFamily::Foliage => {
                let resource = wind.buffer().as_entire_binding();
                entries.push(wgpu::BindGroupEntry { binding: FOLIAGE_WIND_BINDING, resource });
            }
            _ => {}
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(family.name()),
            layout: &layouts.scene[group],
            entries: &entries,
        })
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
