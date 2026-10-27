//! Shadows (M1 R4; spec §5.12 "Lighting", look pillars 2 and 4): the world key light's cascaded
//! shadow maps, each character's own shadow, and what every surface receives. Anime shadows are
//! clean, stable shapes in colored tones, so this module is built for stability first: no acne,
//! no shimmering edges as the camera or the sun moves, and no noisy self-shadowing on
//! characters.
//!
//! # Who casts and who receives
//!
//! **Decision: the frame's character list classifies.** An instance listed in
//! [`FramePacket::characters`](crate::FramePacket::characters) with a valid entry is a character
//! this frame; every other instance is environment. No flag or layer of its own: the renderer
//! already resolves each instance's character entry per frame (`InstanceData::character`, M1
//! C2), and the shaders branch on that same index, so casting and receiving can never disagree.
//!
//! | | casts into | receives |
//! |---|---|---|
//! | environment | the cascades | the cascades, every character's own shadow |
//! | character | its own shadow only (a map on desktop, a capsule on phones) | the cascades only |
//!
//! So a character never shadows itself, C2's contract for `cast_shadow`: the cascades hold
//! environment casters alone, and a character's own shadow is received only by the environment.
//! Characters don't shadow each other either (optional in the contract; see "Later milestones").
//!
//! - **Character bodies** ([`CharacterBody`](crate::CharacterBody)) group a character's
//!   instances: instances listed with equal bodies are one character, which draws one map or
//!   casts one capsule.
//! - **Faces** never sample a shadow map (C3's contract). A small compute pass (`face shadows`)
//!   samples the cascades over a sphere around each character's head once a frame, in frames
//!   where a face draws, and writes the result into the character's entry
//!   (`CharacterData::shadow`), which the face permutation takes as occlusion: an environment
//!   shadow reaches a face whole.
//!
//! # The cascades
//!
//! `cascades` maps of `resolution` texels, layers of one `Depth32Float` array, reverse-Z (depth 1
//! toward the light, cleared to 0), cover the view out to `distance` ([`ShadowSettings`], per
//! tier: 4 on desktop, 2 on phones). The `fit` module says how they stay still:
//!
//! - **The key light is quantized:** the shadow slope is rounded to 5 mm per meter of height, so
//!   slow time-of-day motion leaves the maps exactly still between steps.
//! - **Stable fits:** each cascade covers the sphere around its slice of the view frustum, whose
//!   size never changes as the camera turns, centered on whole texels of light space, so the
//!   grid stays fixed to the world as the camera moves. Shadow views are never jittered (temporal
//!   AA jitters the main view alone).
//! - **Caching:** the farthest `cached_cascades` are drawn in turns, one a frame (with one, every
//!   other frame), and otherwise sampled as last drawn, with the matrices they were drawn with.
//!   All are drawn again when the quantized light steps, on a camera cut and after a frame
//!   without a view; a cached one early once the view has moved an eighth of its width. A
//!   cascade nothing casts into is drawn once, empty, and then left as it is (under its new fit)
//!   until something does.
//! - **Casters** are the depth-writing surfaces of environment instances (opaque and
//!   alpha-tested, drawn solid; transparent ones cast nothing, as in the prepasses), culled per
//!   cascade by their bounding spheres; skinned and morphed environment instances, whose posed
//!   bounds aren't known yet, go into every cascade. Outline hulls cast nothing. Casters are
//!   drawn two-sided, so single-sided walls cast, with a slope-scaled depth bias
//!   (`CASTER_BIAS`).
//! - **Foliage** (M1 W6) casts through casters of its own (the [`foliage`](crate::foliage)
//!   module's, built with its surfaces): displaced by the wind as the main pass draws it, so
//!   shadows sway with the leaves, and its alpha-tested cards cut out by their base color's
//!   alpha, so a canopy casts its leaves' shapes rather than its cards'. Its leaf cards are
//!   world-space geometry, cast as they stand. Culling pads a foliage instance's sphere by as far
//!   as the frame's wind can carry its vertices (its materials' reach × the wind's strength × its
//!   strongest gust), so a leaf swaying past its cooked bounds stays in the cascade it shades.
//!   **Decision: cached cascades stay cached with foliage in them.** Their foliage casters redraw
//!   on the cascade's turn (every other frame on every tier today), so foliage's shadows there
//!   lag the leaves by up to a frame and move at half the frame rate. The visible effect is
//!   small: at the sample tree's breeze, a frame moves a branch tip at most 1.6 cm and a leaf at
//!   full flutter 3.4 cm (4 and 9 cm at 2.5 times the breeze), against the cached cascades'
//!   texels of about 2.7 and 9.4 cm on Desktop-High at the sandbox's 43° lens, under a tent of
//!   five of them, on plants 15 m and more away. In a gale the far dapple steps at half rate.
//!   Redrawing both cached cascades every frame would bring back all the caster work caching
//!   saves (a fourth cascade a frame on Desktop-High), which far foliage's shadows don't earn.
//! - **Billboards and impostors** (M2) face the light in the shadow passes without code of their
//!   own: their vertex shaders orient cards by the view's axes (`ViewUniforms::view_from_world`),
//!   never by the main camera, and each shadow view carries the light's axes.
//!
//! # Receiving
//!
//! `frame.wgsl` holds the receivers' functions, which every shader that shades by the key light
//! calls: `shadow_visibility()` for environment surfaces, `cascade_visibility()` for characters.
//!
//! - **Selection:** a point takes the first cascade whose square holds it with room for the
//!   filter, so cached cascades (drawn from where the camera was) are selected by what they
//!   really cover; in a cascade's outer tenth it blends into the next. Shadows fade out over the
//!   last tenth of the distance.
//! - **Bias:** the point moves off the surface along its normal by half a texel, plus half the
//!   filter's width where the light grazes it (`sin θ`), so a lit plane never shadows itself
//!   under the filter. Casters carry a slope-scaled raster bias. Together they hold peter-panning
//!   to a few texels (the GPU tests hold both).
//! - **Filter: a tent PCF** (Castaño 2013): 5 × 5 texels in 9 bilinear compare taps on desktop,
//!   3 × 3 in 4 on phones ([`ShadowFilter`]). It is a smooth field with no noise, the same in
//!   every frame and stable under temporal AA; the receivers threshold it (the anime character
//!   shader's light split, W4's terminator), which turns it into a clean edge along a smooth
//!   contour through the texels rather than a staircase. A rotated Poisson disc was rejected: it
//!   needs temporal AA to hide its noise, which phones don't run.
//! - **Tint:** shadows are colored, never black. The visibility enters each receiver's split as
//!   occlusion, so a shadowed point takes the shadow side's tones: the environment shader's
//!   terminator keeps W1's [`FramePacket::shadow_tint`](crate::FramePacket::shadow_tint) of the
//!   key light there (W4), and anime character surfaces go into their authored shade tones (C1).
//!
//! # Characters' own shadows
//!
//! - **Desktop** (`character_shadow`): the characters nearest the camera, at most
//!   [`MAX_CHARACTER_MAPS`], each draw a map of their own: an orthographic frustum from the same
//!   quantized key light around the character's body and its instances' bounds, padded and
//!   texel-snapped, `character_resolution` texels across, a tile of one atlas drawn in one pass.
//!   It reaches 40 m past the character away from the light, fading over the last quarter, and
//!   gives crisp contact shadows under the feet.
//! - **Phones,** and characters past the desktop's maps: the body's capsule casts a soft shadow
//!   analytically (the closest approach of the light's ray from the receiver to the capsule's
//!   axis, against a radius that softens with distance). No map, no pass. At most
//!   [`MAX_CAPSULES`].
//!
//! # Passes and bindings
//!
//! Side-effect passes before the main pass, since the maps are persistent textures outside the
//! frame graph (as the sky cube is): `shadow cascade 0`… for each cascade drawn this frame,
//! `character shadows` (the atlas, when a character draws a map) and `face shadows` (compute,
//! when a face draws). Their GPU time is the shadow row of the stats overlay and of the bench
//! ([`PASSES`]). Receivers read group 0: the
//! `ShadowUniforms` at binding 4 (light space, every cascade's and map's area and depth transform,
//! the capsules), the cascades at 5, the atlas at 6 and a comparison sampler at 7; no
//! storage buffer is added. The caster pipelines bind the view group (each map's view at its own
//! dynamic offset in the view buffer) and the anime character family's scene group (its
//! instances, as the prepasses do), and leave group 0 out, since it holds the maps they draw
//! into. Foliage's casters bind foliage's scene group instead (its instances, its material blocks
//! and the wind) and each draw's group 3, for the alpha test.
//!
//! With shadows off ([`RenderSettings::shadows`](crate::RenderSettings::shadows) `None`) the maps
//! are 1 × 1, no pass runs, and the shaders' `SHADOWS` constant is false, so they compile to what
//! they were without shadows.
//!
//! # Later milestones
//!
//! - **Alpha-tested casters cut out** for the other families (hair cards), as foliage's do (W6).
//! - **Characters shadowing each other** through their maps, which the contract allows; and
//!   posed bounds from the joints, so skinned environment instances are culled per cascade.
//! - **A static far term:** NTE's camera-anchored tiled cache or a baked shadow per time-of-day
//!   phase past the cascades (research notes, world lighting §4), and the cascades' distance and
//!   splits per zone.
//! - **Cheaper receivers:** receiving costs about 0.65 ms of the main pass in the courtyard at
//!   1080p on an M5 (spec §5.12's cost note), more than the passes. A smaller tent in the far
//!   cascades, whose texels are already wide, or a desktop shadow mask: a full-resolution pass
//!   with penumbra classification and screen-space contact shadows.
//! - **Clearing only the atlas tiles drawn:** the whole character atlas clears every frame a
//!   character draws a map (4096 × 2048 on Desktop-High, about 80 µs on an M5).
//! - **Cloud shadows** from the card clouds' coverage (W2), and local-light shadows.
//! - **GPU culling** of casters with the main view's (spec §5.12, "Culling"); then a cascade's
//!   casters can be drawn only where the camera's motion uncovered new ground (NTE's tiles).
//! - **A shadows cvar** (`r.shadows`) for the overlay: a change makes a new renderer, as `r.tier`
//!   does.

pub(crate) mod fit;
mod plan;

use std::num::NonZeroU64;

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use sapphire_assets::mesh::PositionFormat;
use sapphire_rhi::wgpu;
use serde::{Deserialize, Serialize};

use crate::FramePacket;
use crate::buffer;
use crate::characters::GpuCharacter;
use crate::draw::{DrawSet, MeshDraws};
use crate::gpu_types::ViewUniforms;
use crate::graph::FrameGraph;
use crate::mesh_pool::MeshRecord;
use crate::pipelines::{
    Coverage, DEPTH_FORMAT, Layouts, MeshPass, Part, Pipelines, Shading, SurfaceVariant, format_index,
};
use crate::scene::{DrawList, InstanceSlots};
use crate::shaders::{self, Program, ShaderSources};
use plan::{ShadowPlan, WindMargin};

/// The most cascades a tier may have (spec §5.12: 4 on desktop, 2 on phones).
pub const MAX_CASCADES: usize = 4;
/// The most characters that draw a shadow map of their own in a frame, nearest the camera
/// first: the tiles of the desktop's character atlas.
pub const MAX_CHARACTER_MAPS: usize = 8;
/// The most characters that cast a capsule shadow in a frame, nearest the camera first.
pub const MAX_CAPSULES: usize = 16;

/// The character atlas's tiles across and down.
const ATLAS_COLUMNS: u32 = 4;
const ATLAS_ROWS: u32 = MAX_CHARACTER_MAPS as u32 / ATLAS_COLUMNS;

/// The frame graph passes shadows add: what the overlay and the bench sum for their GPU time
/// against spec §3.3's budget (2 ms with 4 cascades on desktop, 3 ms with 2 on phones).
pub const PASSES: [&str; MAX_CASCADES + 2] =
    [CASCADE_PASSES[0], CASCADE_PASSES[1], CASCADE_PASSES[2], CASCADE_PASSES[3], CHARACTER_PASS, FACE_PASS];
const CASCADE_PASSES: [&str; MAX_CASCADES] =
    ["shadow cascade 0", "shadow cascade 1", "shadow cascade 2", "shadow cascade 3"];
const CHARACTER_PASS: &str = "character shadows";
const FACE_PASS: &str = "face shadows";

/// Threads per workgroup of the face pass (`FACE_SHADOW_WORKGROUP` in `shadows.wgsl`).
pub(crate) const FACE_WORKGROUP: u32 = 64;

/// Every caster's raster bias. Reverse-Z: casters move away from the light, more on slopes.
pub(crate) const CASTER_BIAS: wgpu::DepthBiasState =
    wgpu::DepthBiasState { constant: -2, slope_scale: -1.5, clamp: 0.0 };

/// The pipeline-overridable constants of the receivers (`frame.wgsl`).
pub(crate) mod override_constant {
    /// Shadows are on: receivers sample the maps.
    pub const SHADOWS: &str = "SHADOWS";
    /// The tent filter's width in texels, 3 or 5.
    pub const TENT: &str = "SHADOW_TENT";
}

/// The shadows of a quality tier (`tiers.ron`, [`QualityTier`](crate::quality::QualityTier)):
/// spec §5.12's "shadow cascades and resolution" and "character shadow on or off". All are
/// Proposals until M2 measures them (spec §3). The renderer is created with them
/// ([`RenderSettings::shadows`](crate::RenderSettings::shadows)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct ShadowSettings {
    /// Cascades, 1..=[`MAX_CASCADES`]: 4 on desktop, 2 on phones (spec §3.3).
    pub cascades: u32,
    /// Each cascade's map size in texels, a power of two in 256..=8192.
    pub resolution: u32,
    /// How far from the camera the cascades reach, in meters (1..=2000); shadows fade out over
    /// the last tenth. A table without it reads 120.
    #[serde(default = "ShadowSettings::default_distance")]
    pub distance: u32,
    /// The farthest cascades that are cached: drawn in turns, one a frame (every other frame
    /// when there is one), fewer than `cascades`. A table without it caches none.
    #[serde(default)]
    pub cached_cascades: u32,
    /// Characters near the camera draw shadow maps of their own (desktop); otherwise every
    /// character casts its capsule (phones).
    pub character_shadow: bool,
    /// Each character map's size in texels, a power of two in 128..=4096. A table without it
    /// reads 1024.
    #[serde(default = "ShadowSettings::default_character_resolution")]
    pub character_resolution: u32,
    /// How receivers filter the maps. A table without it reads the desktop's.
    #[serde(default)]
    pub filter: ShadowFilter,
}

/// How receivers filter a shadow map: a tent over `N × N` texels (Castaño 2013), smooth and
/// noise-free, which their light split thresholds into a clean edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum ShadowFilter {
    /// 3 × 3 texels in 4 bilinear compare taps: phones.
    Tent3,
    /// 5 × 5 texels in 9 taps: desktop.
    #[default]
    Tent5,
}

impl ShadowFilter {
    /// Texels across the tent.
    pub fn texels(self) -> u32 {
        match self {
            Self::Tent3 => 3,
            Self::Tent5 => 5,
        }
    }
}

impl Default for ShadowSettings {
    /// Desktop-High's.
    fn default() -> Self {
        Self {
            cascades: 4,
            resolution: 2048,
            distance: Self::default_distance(),
            cached_cascades: 2,
            character_shadow: true,
            character_resolution: Self::default_character_resolution(),
            filter: ShadowFilter::Tent5,
        }
    }
}

impl ShadowSettings {
    fn default_distance() -> u32 {
        120
    }

    fn default_character_resolution() -> u32 {
        1024
    }

    /// Checks every knob is in its range, returning the field and what is wrong.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        if !(1..=MAX_CASCADES as u32).contains(&self.cascades) {
            return Err(("shadows.cascades", "must be within 1..=4"));
        }
        if !(self.resolution.is_power_of_two() && (256..=8192).contains(&self.resolution)) {
            return Err(("shadows.resolution", "must be a power of two in 256..=8192"));
        }
        if !(1..=2000).contains(&self.distance) {
            return Err(("shadows.distance", "must be within 1..=2000 meters"));
        }
        if self.cached_cascades >= self.cascades {
            return Err(("shadows.cached_cascades", "must be fewer than the cascades"));
        }
        let texels = self.character_resolution;
        if !(texels.is_power_of_two() && (128..=4096).contains(&texels)) {
            return Err(("shadows.character_resolution", "must be a power of two in 128..=4096"));
        }
        Ok(())
    }

    /// The receivers' pipeline constants for these settings (`SHADOWS`, `SHADOW_TENT`).
    pub(crate) fn constants(&self) -> [(&'static str, f64); 2] {
        [(override_constant::SHADOWS, 1.0), (override_constant::TENT, f64::from(self.filter.texels()))]
    }
}

/// Shadow counters for the stats overlay and Tracy (rule R9), from the last prepared frame with a
/// view. All zero while shadows are off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShadowStats {
    pub enabled: bool,
    /// The tier's cascades and their resolution.
    pub cascades: u32,
    pub resolution: u32,
    /// Cascades drawn this frame, and those sampled as an earlier frame drew them (cached, or
    /// empty with nothing casting into them).
    pub cascades_drawn: u32,
    pub cascades_cached: u32,
    /// Characters that drew a map of their own this frame, and those that cast a capsule.
    pub character_maps: u32,
    pub capsules: u32,
    /// Characters casting nothing of their own this frame: past the shadow distance, or past
    /// the maps and capsules there are.
    pub uncast_characters: u32,
    /// Draws in this frame's shadow passes: every cascade drawn and every character map.
    pub caster_draws: u32,
    /// Environment draws culled from the cascades drawn, one for each cascade they missed.
    pub culled_draws: u32,
    /// Times the quantized key light stepped since the renderer was created; each draws every
    /// cascade again.
    pub light_steps: u32,
}

/// One cascade as receivers read it (WGSL `ShadowCascade`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuCascade {
    /// xy: the square's center in light space; z: texture coordinates per meter, 1 / its width;
    /// w: one texel, in meters.
    pub area: [f32; 4],
    /// xy: reverse-Z depth of light-space z, `z × x + y`; z: texels across; w: 0.
    pub depth: [f32; 4],
}

/// One character's map as receivers read it (WGSL `CharacterShadowMap`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuCharacterMap {
    /// As [`GpuCascade::area`].
    pub area: [f32; 4],
    /// xy: as [`GpuCascade::depth`]; z: the light-space z where its reach ends; w: 1 / the
    /// length it fades out over before that.
    pub depth: [f32; 4],
    /// xy: the tile's corner in the atlas's texture coordinates; zw: its size in them.
    pub tile: [f32; 4],
}

/// One character's capsule as receivers read it (WGSL `ShadowCapsule`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuCapsule {
    /// xyz: the body's base, world space; w: its radius.
    pub base: [f32; 4],
    /// xyz: its top; w: 0.
    pub top: [f32; 4],
}

/// Everything receivers read about the frame's shadows (WGSL `ShadowUniforms`, group 0 binding
/// 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct ShadowUniforms {
    /// Light space's axes in the world: x, y, and z toward the key light as shadows are cast
    /// (quantized). w: 0.
    pub light_x: [f32; 4],
    pub light_y: [f32; 4],
    pub light_z: [f32; 4],
    /// x: cascades; y: character maps; z: capsules; w: character entries the face pass writes.
    pub counts: [u32; 4],
    /// xyz: the main camera's position; w: how far the cascades reach from it, in meters.
    pub camera: [f32; 4],
    /// xy: one texel of the character atlas in its texture coordinates; z: a tile's texels
    /// across; w: 0.
    pub atlas: [f32; 4],
    pub cascades: [GpuCascade; MAX_CASCADES],
    pub characters: [GpuCharacterMap; MAX_CHARACTER_MAPS],
    pub capsules: [GpuCapsule; MAX_CAPSULES],
}

impl Default for ShadowUniforms {
    fn default() -> Self {
        Self::zeroed()
    }
}

/// The shadow pipelines: the caster's per position format, and the face pass's.
#[derive(Debug)]
pub(crate) struct ShadowPipelines {
    /// Per [`PositionFormat::ALL`] entry, built with the first mesh drawn in the format.
    casters: [Option<wgpu::RenderPipeline>; 2],
    face: wgpu::ComputePipeline,
}

impl ShadowPipelines {
    /// How many there are, for [`Renderer::set_shader_sources`](crate::Renderer::set_shader_sources)'s count.
    pub(crate) fn count(&self) -> u32 {
        1 + self.casters.iter().flatten().count() as u32
    }
}

/// The textures shadows draw into and receivers sample.
#[derive(Debug)]
struct ShadowTextures {
    /// The cascades' layers, each a view to draw into.
    layers: Vec<wgpu::TextureView>,
    /// Every cascade, as receivers sample them.
    cascades: wgpu::TextureView,
    /// The character atlas, drawn into and sampled whole.
    atlas: wgpu::TextureView,
    atlas_size: (u32, u32),
}

/// The renderer's shadows: settings, maps, pipelines and each frame's plan.
#[derive(Debug)]
pub(crate) struct Shadows {
    settings: Option<ShadowSettings>,
    textures: ShadowTextures,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    /// Built while shadows are on.
    pipelines: Option<ShadowPipelines>,
    caster_layout: wgpu::PipelineLayout,
    /// The face pass's layout: group 0 the maps and uniforms, group 1 the character buffer,
    /// read-write.
    face_layouts: [wgpu::BindGroupLayout; 2],
    face_layout: wgpu::PipelineLayout,
    face_maps: wgpu::BindGroup,
    face_entries: Option<wgpu::BindGroup>,
    /// Character entries the face pass writes this frame; 0 skips it.
    entries: u32,
    /// A face draws this frame: only faces read what the face pass writes.
    faces: bool,
    plan: ShadowPlan,
    pipeline_cache: Option<wgpu::PipelineCache>,
}

impl Shadows {
    /// Shadows with `settings` (`None`: off, with 1 × 1 maps and no pipelines), built from
    /// `sources`.
    pub(crate) fn new(
        device: &wgpu::Device,
        layouts: &Layouts,
        settings: Option<ShadowSettings>,
        cache: Option<&wgpu::PipelineCache>,
        sources: &ShaderSources,
    ) -> Self {
        let textures = ShadowTextures::new(device, settings.as_ref());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow comparison"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // Reverse-Z: a receiver at or nearer the light than the nearest caster is lit.
            compare: Some(wgpu::CompareFunction::GreaterEqual),
            ..Default::default()
        });
        // Zeroed at creation: no cascades, maps or capsules until a frame says otherwise.
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow uniforms"),
            size: size_of::<ShadowUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let caster_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow caster"),
            // Group 0 holds the maps the casters draw into, so they leave it out. Group 2 is the
            // anime character family's scene group, which the prepasses bind for every family's
            // draws too: casters read only its instances.
            bind_group_layouts: &[None, Some(&layouts.view), Some(&layouts.scene[0])],
            immediate_size: 0,
        });
        let face_layouts = face_layouts(device);
        let face_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(FACE_PASS),
            bind_group_layouts: &[Some(&face_layouts[0]), Some(&face_layouts[1])],
            immediate_size: 0,
        });
        let face_maps = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(FACE_PASS),
            layout: &face_layouts[0],
            entries: &[
                wgpu::BindGroupEntry { binding: 4, resource: uniforms.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&textures.cascades),
                },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let mut shadows = Self {
            settings,
            textures,
            sampler,
            uniforms,
            pipelines: None,
            caster_layout,
            face_layouts,
            face_layout,
            face_maps,
            face_entries: None,
            entries: 0,
            faces: false,
            plan: ShadowPlan::default(),
            pipeline_cache: cache.cloned(),
        };
        if settings.is_some() {
            shadows.pipelines = Some(shadows.build_pipelines(device, sources, &[PositionFormat::Unorm16]));
        }
        shadows
    }

    /// Views the shadow passes draw through, after the renderer's own in the view buffer: one per
    /// cascade and per character map.
    pub(crate) fn views(&self) -> u32 {
        self.settings.map_or(0, |settings| {
            settings.cascades + if settings.character_shadow { MAX_CHARACTER_MAPS as u32 } else { 0 }
        })
    }

    /// Builds the caster pipeline for meshes in `format` unless it exists (shadows on only).
    pub(crate) fn prepare_format(
        &mut self,
        device: &wgpu::Device,
        format: PositionFormat,
        sources: &ShaderSources,
    ) {
        let built =
            self.pipelines.as_ref().is_none_or(|pipelines| pipelines.casters[format_index(format)].is_some());
        if built {
            return;
        }
        let module = shaders::module(device, Program::Shadows, sources);
        let caster = self.build_caster(device, &module, format);
        if let Some(pipelines) = &mut self.pipelines {
            pipelines.casters[format_index(format)] = Some(caster);
        }
    }

    /// The face pass's pipeline and the caster pipelines for `formats`, from `sources`.
    fn build_pipelines(
        &self,
        device: &wgpu::Device,
        sources: &ShaderSources,
        formats: &[PositionFormat],
    ) -> ShadowPipelines {
        let module = shaders::module(device, Program::Shadows, sources);
        let mut casters = [None, None];
        for &format in formats {
            casters[format_index(format)] = Some(self.build_caster(device, &module, format));
        }
        let constants = [(override_constant::SHADOWS, 1.0)];
        let face = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(FACE_PASS),
            layout: Some(&self.face_layout),
            module: &module,
            entry_point: Some("cs_face_shadow"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &constants,
                ..Default::default()
            },
            cache: self.pipeline_cache.as_ref(),
        });
        ShadowPipelines { casters, face }
    }

    fn build_caster(
        &self,
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        format: PositionFormat,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow caster"),
            layout: Some(&self.caster_layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_shadow_caster"),
                compilation_options: Default::default(),
                buffers: &MeshPass::Shadow.buffers(format),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                // Two-sided: single-sided walls and cards cast too.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: CASTER_BIAS,
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview_mask: None,
            cache: self.pipeline_cache.as_ref(),
        })
    }

    /// Its pipelines built again from `sources` (hot reload), on the layouts they had: the face
    /// pass's and every caster built so far. `None` while shadows are off.
    pub(crate) fn rebuilt_pipelines(
        &self,
        device: &wgpu::Device,
        sources: &ShaderSources,
    ) -> Option<ShadowPipelines> {
        let pipelines = self.pipelines.as_ref()?;
        let mut formats = [PositionFormat::Unorm16; 2];
        let mut count = 0;
        for format in PositionFormat::ALL {
            if pipelines.casters[format_index(format)].is_some() {
                formats[count] = format;
                count += 1;
            }
        }
        Some(self.build_pipelines(device, sources, &formats[..count]))
    }

    pub(crate) fn set_pipelines(&mut self, pipelines: ShadowPipelines) {
        self.pipelines = Some(pipelines);
    }

    /// Group 0's shadow bindings for the scene passes: the uniforms, the cascades, the atlas and
    /// the comparison sampler (bindings 4 to 7).
    pub(crate) fn frame_entries(&self) -> [wgpu::BindGroupEntry<'_>; 4] {
        [
            wgpu::BindGroupEntry { binding: 4, resource: self.uniforms.as_entire_binding() },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&self.textures.cascades),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&self.textures.atlas),
            },
            wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&self.sampler) },
        ]
    }

    /// Points the face pass at the character buffer: when shadows start and whenever the buffer
    /// is replaced.
    pub(crate) fn set_character_buffer(&mut self, device: &wgpu::Device, buffer: &wgpu::Buffer) {
        if self.settings.is_none() {
            return;
        }
        self.face_entries = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(FACE_PASS),
            layout: &self.face_layouts[1],
            entries: &[wgpu::BindGroupEntry { binding: 1, resource: buffer.as_entire_binding() }],
        }));
    }

    /// Plans the frame's shadows from `packet` (CPU only; see [`ShadowPlan::update`]): `slots`
    /// has the packet's instance deltas applied, `meshes` are the pool's records, `wind_margin`
    /// how far the frame's wind carries each one's vertices past its bounds, `list` the draw list,
    /// `entries` the character buffer's entries this frame.
    pub(crate) fn prepare(
        &mut self,
        packet: &FramePacket,
        slots: &InstanceSlots,
        meshes: &[MeshRecord],
        wind_margin: &WindMargin<'_>,
        list: &DrawList,
        entries: u32,
    ) {
        let Some(settings) = self.settings else { return };
        self.entries = entries;
        let face = Shading::Character(Part::Face);
        self.faces =
            SurfaceVariant::ALL.iter().any(|&variant| variant.shading == face && list.len(variant) > 0);
        self.plan.update(
            &settings,
            packet,
            slots,
            meshes,
            wind_margin,
            list,
            entries,
            self.textures.atlas_size,
        );
    }

    /// Counters from the last frame with a view.
    pub(crate) fn stats(&self) -> ShadowStats {
        self.plan.stats
    }

    /// The shadows' share of a frame's uploads pass: the receivers' uniforms, and the view of
    /// every map drawn this frame at its place in `views` (`first` views from the start, one
    /// every `stride` bytes).
    pub(crate) fn upload(
        &self,
        belt: &mut wgpu::util::StagingBelt,
        encoder: &mut wgpu::CommandEncoder,
        views: &wgpu::Buffer,
        first: u32,
        stride: u32,
    ) {
        if self.settings.is_none() || !self.plan.active {
            return;
        }
        buffer::stage(belt, encoder, &self.uniforms, 0, bytemuck::bytes_of(&self.plan.uniforms));
        let cascades = self.plan.drawn.iter().enumerate().filter_map(|(index, fit)| Some((index, (*fit)?)));
        let maps = self.plan.maps.iter().enumerate().map(|(index, fit)| (MAX_CASCADES + index, *fit));
        for (index, fit) in cascades.chain(maps) {
            let camera = fit.camera();
            // Never jittered; and nothing reads a shadow view's velocity.
            let view = ViewUniforms::new(
                &camera,
                (fit.resolution, fit.resolution),
                camera.proj * camera.view,
                [0.0; 2],
            );
            let offset = u64::from(stride) * u64::from(first + self.view_index(index));
            buffer::stage(belt, encoder, views, offset, bytemuck::bytes_of(&view));
        }
    }

    /// The view of cascade `index` (below [`MAX_CASCADES`]), or of character map `index −
    /// MAX_CASCADES`, among the shadows' views.
    fn view_index(&self, index: usize) -> u32 {
        let cascades = self.settings.map_or(0, |settings| settings.cascades);
        if index < MAX_CASCADES { index as u32 } else { cascades + (index - MAX_CASCADES) as u32 }
    }

    /// Adds the frame's shadow passes to `graph`, side effects to run before the main pass: each
    /// cascade drawn, the character maps and the face pass. `draws` are the frame's mesh draws;
    /// the shadows' views are `first` views into the view buffer (`stride` bytes apart), bound
    /// through `view_group`; `casters` holds the scene groups that bind the instances and
    /// foliage's casters.
    #[expect(clippy::too_many_arguments, reason = "the frame's shared bindings, all borrowed for the graph")]
    pub(crate) fn record<'f>(
        &'f self,
        graph: &mut FrameGraph<'f>,
        draws: MeshDraws<'f>,
        view_group: &'f wgpu::BindGroup,
        casters: CasterBindings<'f>,
        first: u32,
        stride: u32,
        labels: bool,
    ) {
        let Some(pipelines) = &self.pipelines else { return };
        if !self.plan.active {
            return;
        }
        let plan = &self.plan.casters;
        let offset = move |index: usize| stride * (first + self.view_index(index));
        for (index, fit) in self.plan.drawn.iter().enumerate() {
            if fit.is_none() {
                continue;
            }
            let layer = &self.textures.layers[index];
            let bit = 1u8 << index;
            graph.add_pass(CASCADE_PASSES[index]).side_effect().record(move |ctx| {
                let timestamps = ctx.timestamp_writes();
                let mut pass =
                    begin_depth_pass(ctx.encoder, CASCADE_PASSES[index], layer, timestamps, labels);
                pass.set_bind_group(1, view_group, &[offset(index)]);
                pass.set_index_buffer(draws.meshes.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
                draw_casters(&mut pass, pipelines, casters, &draws, |slot| {
                    plan.get(slot as usize).is_some_and(|caster| caster.cascades & bit != 0)
                });
            });
        }
        if let Some(settings) = self.settings.filter(|_| !self.plan.maps.is_empty()) {
            let (tile, maps) = (settings.character_resolution, self.plan.maps.len());
            graph.add_pass(CHARACTER_PASS).side_effect().record(move |ctx| {
                let timestamps = ctx.timestamp_writes();
                let atlas = &self.textures.atlas;
                let mut pass = begin_depth_pass(ctx.encoder, CHARACTER_PASS, atlas, timestamps, labels);
                pass.set_index_buffer(draws.meshes.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
                for map in 0..maps {
                    let (x, y) = ((map as u32 % ATLAS_COLUMNS) * tile, (map as u32 / ATLAS_COLUMNS) * tile);
                    pass.set_viewport(x as f32, y as f32, tile as f32, tile as f32, 0.0, 1.0);
                    pass.set_scissor_rect(x, y, tile, tile);
                    pass.set_bind_group(1, view_group, &[offset(MAX_CASCADES + map)]);
                    draw_casters(&mut pass, pipelines, casters, &draws, |slot| {
                        plan.get(slot as usize).is_some_and(|caster| caster.map == Some(map as u8))
                    });
                }
            });
        }
        if let Some(entries) = self.face_entries.as_ref().filter(|_| self.entries > 0 && self.faces) {
            let workgroups = self.entries.div_ceil(FACE_WORKGROUP);
            let face = &pipelines.face;
            let maps = &self.face_maps;
            graph.add_pass(FACE_PASS).side_effect().record(move |ctx| {
                let timestamp_writes = ctx.compute_timestamp_writes();
                let mut pass = ctx.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: labels.then_some(FACE_PASS),
                    timestamp_writes,
                });
                pass.set_pipeline(face);
                pass.set_bind_group(0, maps, &[]);
                pass.set_bind_group(1, entries, &[]);
                pass.dispatch_workgroups(workgroups, 1, 1);
            });
        }
    }
}

/// Begins a depth-only render pass into `view`, cleared to 0 (reverse-Z's farthest) and stored.
fn begin_depth_pass<'p>(
    encoder: &'p mut wgpu::CommandEncoder,
    label: &'static str,
    view: &'p wgpu::TextureView,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'p>>,
    labels: bool,
) -> wgpu::RenderPass<'p> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: labels.then_some(label),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
            stencil_ops: None,
        }),
        timestamp_writes,
        ..Default::default()
    })
}

/// What the caster pipelines bind beyond the view: the scene groups and foliage's pipelines.
#[derive(Clone, Copy)]
pub(crate) struct CasterBindings<'f> {
    /// The anime character family's scene group: the shared casters read its instances.
    pub scene_group: &'f wgpu::BindGroup,
    /// Foliage's scene group: its instances, material blocks and the wind.
    pub foliage_group: &'f wgpu::BindGroup,
    /// The mesh pipelines, which hold foliage's casters.
    pub mesh: &'f Pipelines,
}

/// Draws the depth-writing surfaces of the instances `include` takes, by slot: static meshes per
/// format, then skinned and morphed ones, through the shared caster pipelines, then foliage
/// through its own.
fn draw_casters(
    pass: &mut wgpu::RenderPass<'_>,
    pipelines: &ShadowPipelines,
    bindings: CasterBindings<'_>,
    draws: &MeshDraws<'_>,
    include: impl Fn(u32) -> bool + Copy,
) {
    pass.set_bind_group(2, bindings.scene_group, &[]);
    for format in PositionFormat::ALL {
        let Some(pipeline) = &pipelines.casters[format_index(format)] else { continue };
        if !SurfaceVariant::SHARED_DEPTH.iter().any(|&variant| draws.has_static(format, variant)) {
            continue;
        }
        pass.set_pipeline(pipeline);
        for variant in SurfaceVariant::SHARED_DEPTH {
            draws.draw_static_where(pass, MeshPass::Shadow, format, variant, DrawSet::Surfaces, include);
        }
    }
    let float = &pipelines.casters[format_index(PositionFormat::Float32)];
    if let Some(pipeline) = float
        && SurfaceVariant::SHARED_DEPTH.iter().any(|&variant| draws.has_deformed(variant))
    {
        pass.set_pipeline(pipeline);
        for variant in SurfaceVariant::SHARED_DEPTH {
            draws.draw_deformed_where(pass, MeshPass::Shadow, variant, DrawSet::Surfaces, include);
        }
    }
    // Foliage, static meshes only, through its casters per coverage.
    let mut foliage_bound = false;
    for format in PositionFormat::ALL {
        let Some(mesh) = bindings.mesh.mesh(format) else { continue };
        for coverage in Coverage::DEPTH_WRITING {
            let variant = SurfaceVariant::foliage(coverage);
            let Some(pipeline) = mesh.foliage_caster(coverage) else { continue };
            if !draws.has_static(format, variant) {
                continue;
            }
            if !foliage_bound {
                pass.set_bind_group(2, bindings.foliage_group, &[]);
                foliage_bound = true;
            }
            pass.set_pipeline(pipeline);
            draws.draw_static_where(
                pass,
                MeshPass::FoliageShadow,
                format,
                variant,
                DrawSet::Surfaces,
                include,
            );
        }
    }
}

impl ShadowTextures {
    /// The maps for `settings`: `cascades` layers of `resolution`² and, on desktop, an atlas of
    /// [`MAX_CHARACTER_MAPS`] tiles of `character_resolution`²; 1 × 1 where there is nothing to
    /// draw, so the frame's bind group is complete with shadows off.
    fn new(device: &wgpu::Device, settings: Option<&ShadowSettings>) -> Self {
        let depth = |label, width, height, layers| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: layers },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let (resolution, layers) =
            settings.map_or((1, 1), |settings| (settings.resolution, settings.cascades));
        let cascades = depth("shadow cascades", resolution, resolution, layers);
        let layer_views = (0..layers)
            .map(|layer| {
                cascades.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("shadow cascade"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let sampled = cascades.create_view(&wgpu::TextureViewDescriptor {
            label: Some("shadow cascades"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let atlas_size = match settings {
            Some(settings) if settings.character_shadow => {
                (ATLAS_COLUMNS * settings.character_resolution, ATLAS_ROWS * settings.character_resolution)
            }
            _ => (1, 1),
        };
        let atlas = depth("character shadows", atlas_size.0, atlas_size.1, 1);
        Self {
            layers: layer_views,
            cascades: sampled,
            atlas: atlas.create_view(&wgpu::TextureViewDescriptor::default()),
            atlas_size,
        }
    }
}

/// The face pass's bind group layouts: group 0 the shadow uniforms, the cascades and the
/// comparison sampler (at `frame.wgsl`'s bindings 4, 5 and 7), group 1 the character buffer,
/// read-write, at binding 1 (`shadows.wgsl`).
fn face_layouts(device: &wgpu::Device) -> [wgpu::BindGroupLayout; 2] {
    let compute = wgpu::ShaderStages::COMPUTE;
    let maps = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("face shadow maps"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: compute,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(size_of::<ShadowUniforms>() as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: compute,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 7,
                visibility: compute,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ],
    });
    let entries = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("face shadow entries"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: compute,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(size_of::<GpuCharacter>() as u64),
            },
            count: None,
        }],
    });
    [maps, entries]
}
