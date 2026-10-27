//! The extract boundary (spec §4.6, rule R4): the plain data the renderer draws from.
//!
//! Each frame the app's extract systems fill one [`FramePacket`] from the simulation, and the
//! renderer reads only that. A packet holds `Copy` data and owned vectors, never references
//! into the simulation or gameplay types, so it can be handed to render workers while the next
//! frame simulates (§4.2).
//!
//! - **Views and scene constants** are rewritten whole every frame; they are small. The scene
//!   constants are the frame's look: the key light, the sun and moon, the sky gradient and the
//!   sky's layers over it (discs, stars, clouds), ambient light, fog, the shadow tint, the
//!   character rig's key light, the day/night factor, the emission scale and post-processing.
//!   Time of day resolves all of them from one set of keys (`sapphire_world::tod`, M1 W1; look
//!   pillar 1); the renderer sees only the values. The weather's wetness ([`Weather`]) joins
//!   them, dry until weather fills it, and the wind foliage sways in ([`Wind`], M1 W6), calm
//!   until the world sends one.
//! - **Local lights** are rewritten whole every frame too: a frame has a few dozen at most
//!   (spec §3.4), and the renderer culls and lists them per frame anyway.
//! - **Characters** are rewritten whole every frame as well: each [`CharacterInstance`] gives an
//!   instance its character light rig ([`CharacterLighting`], M1 C2), resolved by the world for
//!   this frame's camera and time of day, which changes whenever either moves, and its face
//!   ([`CharacterFace`], M1 C3): the head's pose and the expression weights, which animation
//!   moves.
//! - **Instances are deltas.** The renderer keeps every instance in a persistent GPU scene, so a
//!   packet carries only the instances added, changed or removed since the previous one. The
//!   renderer must therefore see every packet, in order (see [`crate::Renderer::prepare`]).
//! - **Poses are deltas too.** A [`SkinnedInstance`] gives a skinned or morphed instance a new
//!   pose: a range of [`FramePacket::palettes`] (its skinning palette) and of
//!   [`FramePacket::morph_weights`]. An animated character sends one every frame; one that holds
//!   still (hitstop, pause) sends none and keeps its last pose, which costs nothing to draw.
//!
//! Packets live in a [`FramePacketRing`] of three, the in-flight depth of the pipelined frame:
//! one being extracted, one being rendered, one spare so neither side waits. A packet's vectors
//! keep their capacity when the ring reuses it, so a warm frame allocates nothing for its
//! packet. What the packet gains in later milestones is listed on [`FramePacket`].

use std::ops::Range;

use glam::camera::rh::{proj::directx, view};
use glam::{Affine3A, Mat4, Quat, Vec3};

use crate::{MaterialHandle, MeshHandle, PostParams, ProbeKeys};

/// Packets in a [`FramePacketRing`]: one extracting, one rendering, one spare.
const RING_SIZE: usize = 3;

/// Everything the renderer needs to draw one frame.
///
/// # Later milestones
///
/// The packet grows with the systems that feed it (spec §4.6): VFX spawns and emitter
/// parameters, the UI draw list, and debug draws (dev builds). Shadow cascades (R4) and UI 3D
/// insets add views after the main camera.
///
/// One scene constant is carried for the package that will read it and drawn from nothing yet:
/// [`Self::weather`] (the wet surface model). Each type's docs name its readers.
#[derive(Clone, Debug, PartialEq)]
pub struct FramePacket {
    /// Numbers packets in extraction order. Consecutive packets differ by one, which is how
    /// the renderer checks that it saw them all.
    pub frame: u64,
    /// What to draw from. The first view is the main camera; with none, the frame draws
    /// nothing (no target this frame), though its instance deltas still apply.
    pub views: Vec<ViewData>,
    /// The world's key light, which shades the scene and (with R4) casts its shadows: the sun
    /// by day and the moon at night, blended through twilight by time of day.
    pub sun: DirectionalLight,
    pub sky: SkyGradient,
    /// The sun and the moon themselves, and the turn of the star field: where the sky draws
    /// its discs and stars (W2).
    pub celestial: Celestial,
    /// What the sky draws over its gradient (W2): the sun's and moon's discs and glows, the
    /// stars and the clouds' light. The default draws nothing over the gradient.
    pub sky_layers: SkyLayers,
    /// The zone's sky material
    /// ([`MaterialFamily::Sky`](sapphire_assets::material::MaterialFamily)): the discs' sizes,
    /// the moon's face, star twinkle and density, the Milky Way, and the cloud and cirrus maps
    /// with their scale and drift. `None`, or a handle that isn't an uploaded sky material, draws
    /// with [`SkyMaterial::default`](sapphire_assets::material::SkyMaterial): default disc sizes,
    /// even stars and no clouds.
    pub sky_material: Option<MaterialHandle>,
    pub ambient: AmbientLight,
    /// Which keys of the ambient probe grid's bake
    /// ([`Renderer::set_probe_grid`](crate::Renderer::set_probe_grid)) this frame's bounce light
    /// blends, resolved by time of day from the frame's hour (M1 W5). The default blends none: a
    /// grid then lends sky light alone.
    pub probe_keys: ProbeKeys,
    /// Distance and height fog over the world and the sky (M1 W3), which fog cards take their
    /// light from. The default has none.
    pub fog: Fog,
    /// Linear RGB the key light keeps inside cast shadows, so shadows are colored rather than
    /// black (look pillar 4): black is a physical shadow, a dim blue-violet the anime one. Read
    /// by the environment shader (W4) on the shadow side of its terminator and in cast shadow
    /// (R4's, which enter its terminator), times each material's own tint. Characters in cast
    /// shadow take their materials' authored shade colors instead (C1).
    pub shadow_tint: Vec3,
    /// The weather's wetness inputs. The default is dry.
    pub weather: Weather,
    /// The wind foliage sways in (M1 W6). The default is calm.
    pub wind: Wind,
    /// Time of day's curve for the character rig's key light: the world resolves each
    /// character's [`CharacterLighting`] from it, so the renderer reads it only through
    /// [`Self::characters`].
    pub character_light: CharacterLight,
    /// 0 by day, 1 at night: the anime character shader's day/night ramp blend (C1; spec §5.12
    /// "Ramps"), and anything else that has a day and a night look. The frame uniforms carry it
    /// as `day_night`.
    pub day_night: f32,
    /// How much of the world's authored emission shows (time of day's emission key, M1 W1's
    /// keyed emission): the environment shader multiplies its materials' emission by it, but
    /// for the share each keeps steady at every hour (the material's
    /// [`emission_steady`](sapphire_assets::material::EnvironmentMaterial::emission_steady)), so
    /// lanterns and lit windows can rest pale by day and glow at night under one exposure. 1,
    /// the default, draws emission as authored, as does a value that isn't finite; a negative
    /// one draws none. Characters' emission never follows it: their glow is part of their
    /// design at any hour.
    pub emission_scale: f32,
    /// Presentation time in seconds, read from the owning clock (rule R2), for animation:
    /// cloud drift and star twinkle. It is never wrapped: the renderer reduces it as each use
    /// needs, in `f64`, so motion keeps its precision in long sessions and never jumps. Shaders
    /// get it wrapped to an hour (the frame uniforms' `time`), for periodic effects.
    pub time_seconds: f64,
    /// Exposure, bloom, tone curve, grade and vignette, and the compositing layers over them
    /// (M1 P2: the diffusion glow, characters' bloom, gradient layers, the lens flare, the
    /// vignette's tint and the player's sharpening), and the screen effects over them (M1 P3:
    /// [`PostParams::effects`], the world's effect stack resolved for this frame). Defaults to
    /// Sapphire's default look, without the layers or any effect.
    pub post: PostParams,
    /// Point and spot lights. When more are visible than the renderer's
    /// [`RenderSettings::max_lights`](crate::RenderSettings::max_lights) allows, it keeps the
    /// nearest; a cluster that more reach than it can list keeps the earliest in this order.
    pub lights: Vec<LocalLight>,
    /// This frame's characters: the instances the character light rig lights (M1 C2), each with
    /// its rig resolved for this frame. Instances not listed draw with the world's key light
    /// and unstyled GI ([`CharacterLighting::unstyled`]); an instance listed twice takes its
    /// last entry. The renderer ignores, and counts, an entry for an instance it doesn't draw or
    /// whose lighting or face isn't valid ([`CharacterInstance::is_valid`]).
    pub characters: Vec<CharacterInstance>,
    /// Mesh instances added, changed or removed since the previous packet.
    pub instances: InstanceDeltas,
    /// New poses for skinned or morphed instances, applied after [`Self::instances`], so an
    /// instance can be added and posed in one packet. Add them with [`Self::push_skinned`].
    pub skinned: Vec<SkinnedInstance>,
    /// This frame's skinning palettes, back to back: each [`SkinnedInstance::palette`] is a
    /// range of it. The per-frame bone-matrix buffer of spec §4.6.
    pub palettes: Vec<Affine3A>,
    /// This frame's morph target weights, back to back: each
    /// [`SkinnedInstance::morph_weights`] is a range of it.
    pub morph_weights: Vec<f32>,
}

impl FramePacket {
    /// The main camera's view, if this frame draws one.
    pub fn main_view(&self) -> Option<&ViewData> {
        self.views.first()
    }

    /// Poses instance `id`: appends `palette` to [`Self::palettes`] and `morph_weights` to
    /// [`Self::morph_weights`], then a [`SkinnedInstance`] naming both ranges. Allocates nothing
    /// once the packet's vectors have grown to hold a frame's poses.
    pub fn push_skinned(
        &mut self,
        id: InstanceId,
        palette: impl IntoIterator<Item = Affine3A>,
        morph_weights: impl IntoIterator<Item = f32>,
    ) {
        let palette = append(&mut self.palettes, palette);
        let morph_weights = append(&mut self.morph_weights, morph_weights);
        self.skinned.push(SkinnedInstance { id, palette, morph_weights });
    }

    /// The skinning palette `skinned` names, or `None` if its range is outside this packet's.
    pub fn palette(&self, skinned: &SkinnedInstance) -> Option<&[Affine3A]> {
        self.palettes.get(skinned.palette.range())
    }

    /// The morph weights `skinned` names, or `None` if its range is outside this packet's.
    pub fn morph_weights(&self, skinned: &SkinnedInstance) -> Option<&[f32]> {
        self.morph_weights.get(skinned.morph_weights.range())
    }

    /// Empties the packet for `frame`, keeping its vectors' capacity.
    fn reset(&mut self, frame: u64) {
        let Self {
            frame: number,
            views,
            sun,
            sky,
            celestial,
            sky_layers,
            sky_material,
            ambient,
            probe_keys,
            fog,
            shadow_tint,
            weather,
            wind,
            character_light,
            day_night,
            emission_scale,
            time_seconds,
            post,
            lights,
            characters,
            instances,
            skinned,
            palettes,
            morph_weights,
        } = self;
        *number = frame;
        views.clear();
        *sun = DirectionalLight::default();
        *sky = SkyGradient::default();
        *celestial = Celestial::default();
        *sky_layers = SkyLayers::default();
        *sky_material = None;
        *ambient = AmbientLight::default();
        *probe_keys = ProbeKeys::default();
        *fog = Fog::default();
        *shadow_tint = Vec3::ZERO;
        *weather = Weather::default();
        *wind = Wind::default();
        *character_light = CharacterLight::default();
        *day_night = 0.0;
        *emission_scale = 1.0;
        *time_seconds = 0.0;
        *post = PostParams::default();
        lights.clear();
        characters.clear();
        instances.changed.clear();
        instances.removed.clear();
        skinned.clear();
        palettes.clear();
        morph_weights.clear();
    }
}

/// An empty frame 0 in the renderer's default look: no views, lights, characters or instances,
/// and every scene constant at its type's default, but emission, which shows as authored.
impl Default for FramePacket {
    fn default() -> Self {
        Self {
            frame: 0,
            views: Vec::new(),
            sun: DirectionalLight::default(),
            sky: SkyGradient::default(),
            celestial: Celestial::default(),
            sky_layers: SkyLayers::default(),
            sky_material: None,
            ambient: AmbientLight::default(),
            probe_keys: ProbeKeys::default(),
            fog: Fog::default(),
            shadow_tint: Vec3::ZERO,
            weather: Weather::default(),
            wind: Wind::default(),
            character_light: CharacterLight::default(),
            day_night: 0.0,
            emission_scale: 1.0,
            time_seconds: 0.0,
            post: PostParams::default(),
            lights: Vec::new(),
            characters: Vec::new(),
            instances: InstanceDeltas::default(),
            skinned: Vec::new(),
            palettes: Vec::new(),
            morph_weights: Vec::new(),
        }
    }
}

/// Appends `values` to `vec` and returns where they went.
fn append<T>(vec: &mut Vec<T>, values: impl IntoIterator<Item = T>) -> PacketRange {
    let start = vec.len();
    vec.extend(values);
    let index = |at: usize| u32::try_from(at).expect("a packet's palettes and weights fit u32 ranges");
    PacketRange { start: index(start), len: index(vec.len() - start) }
}

/// The ring of packets extraction writes into, reused frame after frame.
///
/// [`Self::begin`] starts the next frame's packet in the oldest slot; extract systems then fill
/// [`Self::current_mut`], and the renderer reads [`Self::current`]. Before the first `begin`,
/// the current packet is an empty frame 0, so begun frames count from 1.
#[derive(Clone, Debug, Default)]
pub struct FramePacketRing {
    packets: [FramePacket; RING_SIZE],
    current: usize,
}

impl FramePacketRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts the next frame: empties the oldest packet, numbers it one past the current one
    /// and makes it current.
    pub fn begin(&mut self) -> &mut FramePacket {
        let frame = self.packets[self.current].frame + 1;
        self.current = (self.current + 1) % RING_SIZE;
        let packet = &mut self.packets[self.current];
        packet.reset(frame);
        packet
    }

    /// The packet begun last.
    pub fn current(&self) -> &FramePacket {
        &self.packets[self.current]
    }

    pub fn current_mut(&mut self) -> &mut FramePacket {
        &mut self.packets[self.current]
    }
}

/// One view: a camera the renderer draws from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewData {
    pub camera: CameraData,
    /// The camera cut to a new shot this frame, or jumped (a teleport, a respawn): nothing it
    /// drew before carries over. Temporal AA drops its history, and velocity counts the camera
    /// as unmoved. Set it for the first frame of the new shot only.
    pub cut: bool,
}

impl ViewData {
    /// A view from `camera` that continues the last frame's.
    pub fn new(camera: CameraData) -> Self {
        Self { camera, cut: false }
    }
}

/// Names one mesh instance across packets. The app picks it (the sandbox uses the entity's
/// bits); it must be unique among live instances and stay the same while the instance lives.
/// The renderer maps it to a slot in its instance buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId(pub u64);

/// Changes to the set of mesh instances since the previous packet. The renderer applies
/// [`Self::removed`] first, then [`Self::changed`] in order, so an instance removed and added
/// again within one frame survives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstanceDeltas {
    /// Instances added, or whose mesh or transform changed, with their current state.
    pub changed: Vec<MeshInstance>,
    /// Instances that stopped drawing. IDs the renderer doesn't know are ignored, so reporting
    /// one that was never drawn is harmless.
    pub removed: Vec<InstanceId>,
}

impl InstanceDeltas {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removed.is_empty()
    }
}

/// A skinned or morphed instance's pose this frame (spec §4.6, §5.12 "Skinning and morph
/// targets"). The instance itself, its mesh and transform, comes through
/// [`FramePacket::instances`]; this says only how its mesh deforms.
///
/// - **Palette:** one matrix per entry of the mesh's skinning palette
///   ([`Skin::binds`](sapphire_assets::mesh::Skin)), in its order:
///   `model_from_joint(binds[i].joint) × binds[i].inverse_bind_matrix`, with the joint's
///   transform taken from the animated pose, in the mesh's object space (the instance's
///   transform then places it in the world). A mesh with morph targets but no skin takes an
///   empty palette.
/// - **Morph weights:** one per morph target, in the mesh's order. Missing weights count as 0,
///   and weights past the mesh's targets are ignored. The renderer never reads the targets'
///   default weights: whatever animates them starts from those.
///
/// An instance that has never been posed draws in its bind pose with every weight at 0. The
/// renderer ignores, and counts, a pose for an instance it doesn't draw with a skinned or
/// morphed mesh, one whose palette length differs from the mesh's, and one with a non-finite
/// matrix or weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkinnedInstance {
    pub id: InstanceId,
    /// The range of [`FramePacket::palettes`] holding its palette.
    pub palette: PacketRange,
    /// The range of [`FramePacket::morph_weights`] holding its weights.
    pub morph_weights: PacketRange,
}

/// A character's face this frame (spec §5.12 "Faces"; M1 C3): what the face permutation of the
/// anime character shader reads beside the character's lighting, through the same entry.
///
/// Instances of a character that draw no face material (its body, hair) may carry any valid
/// face, the default included; the renderer packs it all the same. An invalid one drops the
/// whole entry, lighting included ([`CharacterInstance::is_valid`]). A face drawn by an instance the frame
/// doesn't list faces +Z with +Y up, as models do at rest, with every weight 0.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CharacterFace {
    pub head: HeadBasis,
    pub expression: FaceExpression,
}

impl CharacterFace {
    /// Whether the shader can read it: a head basis that orthonormalizes
    /// ([`HeadBasis::orthonormalized`]) and finite weights.
    pub fn is_valid(&self) -> bool {
        let FaceExpression { blush, gloom } = self.expression;
        self.head.orthonormalized().is_some() && blush.is_finite() && gloom.is_finite()
    }
}

/// Which way a head faces this frame, in world space: the face shader takes the key light in
/// this basis (`sapphire_assets::character_maps`, "Head space"), its right being
/// `forward × up`.
///
/// For a skinned character it is the model's forward and up (+Z and +Y, as glTF and VRM models
/// face) turned by the head joint's world rotation relative to its rest pose ([`Self::turned`]):
/// the head bone of the rig's humanoid mapping. Neither vector need be unit length, nor `up`
/// exactly perpendicular to `forward`: the renderer orthonormalizes them, keeping `forward`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadBasis {
    pub forward: Vec3,
    pub up: Vec3,
}

impl HeadBasis {
    /// A head at rest in a model at the world's origin: facing +Z with +Y up.
    pub const REST: Self = Self { forward: Vec3::Z, up: Vec3::Y };

    /// The rest basis turned by `rotation`: for a head joint, its world-space rotation in the
    /// current pose times the inverse of its model-space rotation at rest.
    pub fn turned(rotation: Quat) -> Self {
        Self { forward: rotation * Vec3::Z, up: rotation * Vec3::Y }
    }

    /// Forward and up as a unit basis, up made perpendicular to forward; `None` if the vectors
    /// aren't finite or don't span a plane.
    pub fn orthonormalized(self) -> Option<(Vec3, Vec3)> {
        let forward = self.forward.try_normalize()?;
        let up = (self.up - forward * self.up.dot(forward)).try_normalize()?;
        Some((forward, up))
    }
}

impl Default for HeadBasis {
    fn default() -> Self {
        Self::REST
    }
}

/// A face's expression weights, each scaling a channel of its material's expression map
/// (`sapphire_assets::character_maps::face_expression`): 0 is none, 1 the full painted effect.
/// Driven by facial animation curves, never by material swaps, so every character wearing a
/// material animates its own. The shader clamps them to `[0, 1]`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FaceExpression {
    /// Tints the painted cheeks toward the material's blush color.
    pub blush: f32,
    /// Pushes the painted region (the brow, the upper face) into its shadow tones.
    pub gloom: f32,
}

/// A range of one of a [`FramePacket`]'s flat arrays. `Copy`, unlike `Range<u32>`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PacketRange {
    pub start: u32,
    pub len: u32,
}

impl PacketRange {
    pub fn range(self) -> Range<usize> {
        let start = self.start as usize;
        start..start + self.len as usize
    }
}

/// The main camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraData {
    /// World to view space: right-handed, the camera looks down −Z.
    pub view: Mat4,
    /// View to clip space. Must be reverse-Z with an infinite far plane and Y-up NDC, as built
    /// by `glam::camera::rh::proj::directx::perspective_infinite_reverse` (WebGPU shares
    /// DirectX's clip space).
    pub proj: Mat4,
    /// World-space eye position.
    pub position: Vec3,
}

impl CameraData {
    /// A perspective camera at `eye` looking at `target`.
    pub fn look_at(eye: Vec3, target: Vec3, up: Vec3, fov_y_radians: f32, aspect: f32, z_near: f32) -> Self {
        Self {
            view: view::look_at_mat4(eye, target, up),
            proj: directx::perspective_infinite_reverse(fov_y_radians, aspect, z_near),
            position: eye,
        }
    }
}

/// A light from infinitely far away: the key light ([`FramePacket::sun`]), or the sun or moon
/// themselves ([`Celestial`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DirectionalLight {
    /// Direction the light travels, from the sun into the scene. Need not be normalized.
    pub direction: Vec3,
    /// Linear RGB.
    pub color: Vec3,
    pub intensity: f32,
}

impl Default for DirectionalLight {
    /// White light of intensity 1, straight down.
    fn default() -> Self {
        Self { direction: Vec3::NEG_Y, color: Vec3::ONE, intensity: 1.0 }
    }
}

/// The sun and the moon, each with the light it gives: where it is (opposite the direction its
/// light travels), its color and intensity; and the orientation of the celestial sphere the
/// stars are fixed to. The sky draws the discs and glows where the bodies are, and the stars
/// and the Milky Way turned by [`Self::sphere`] (W2); how they look is
/// [`FramePacket::sky_layers`]'.
///
/// The key light ([`FramePacket::sun`]) is resolved from the bodies by time of day: the sun's
/// while only the sun shines, the moon's while only the moon does, and a blend of the two
/// through twilight. A body's intensity can be 0 while its disc is still drawn (the moon by
/// day, a low sun at dusk): the disc's look is its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Celestial {
    pub sun: DirectionalLight,
    pub moon: DirectionalLight,
    /// Turns a direction in the celestial sphere's own frame, where the stars and the Milky Way
    /// stay put, into the world: time of day turns it about the sky's pole as the hours pass, so
    /// the stars rise and set with the hour.
    pub sphere: Quat,
}

impl Default for Celestial {
    /// The default sun straight overhead, a dark moon below the horizon, and the celestial
    /// sphere's pole straight up.
    fn default() -> Self {
        Self {
            sun: DirectionalLight::default(),
            moon: DirectionalLight { direction: Vec3::Y, color: Vec3::new(0.6, 0.7, 1.0), intensity: 0.0 },
            sphere: Quat::IDENTITY,
        }
    }
}

/// What the sky draws over its gradient (W2; look pillars 1 and 3), resolved by time of day like
/// the rest of the frame's look: the sun's and the moon's discs and glows, how bright the stars
/// are, the nebula's colors and the clouds' light and cover. Where the discs and stars are comes
/// from [`Celestial`]; their sizes, the star, nebula and cloud maps and the clouds' motion from
/// the sky material ([`FramePacket::sky_material`]).
///
/// The default draws nothing over the gradient (no discs, glows, stars, nebula or clouds): the
/// sky as it was drawn before them (M1 W1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkyLayers {
    pub sun: SkyDisc,
    pub moon: SkyDisc,
    /// Star brightness: 0 by day, 1 for a full night sky. It also lights the Milky Way.
    pub stars: f32,
    /// The nebula's colors (M1 X1 part 2e): black by day.
    pub nebula: Nebula,
    pub clouds: CloudLight,
}

/// The night sky's nebula (M1 X1 part 2e): the light of each of the sky material's nebula map's
/// three channels, linear RGB (HDR), added under the stars. A channel's light is its color
/// times its density; all black draws nothing, as by day.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Nebula {
    /// The map's R: its broad clouds.
    pub clouds: Vec3,
    /// G: the bright wisps along them.
    pub wisps: Vec3,
    /// B: sparse accents.
    pub accents: Vec3,
}

/// How the sky draws the sun or the moon: a disc and the glow around it, both in HDR so they
/// bloom. The disc's angular size comes from the sky material.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkyDisc {
    /// The disc's color, linear RGB; its radiance is `color × intensity`.
    pub color: Vec3,
    /// 0 hides the disc. The sun's is far above 1, so it blooms into a white-hot core.
    pub intensity: f32,
    /// The glow around the disc: linear RGB added at its center, fading smoothly to nothing at
    /// `glow_radius`. Black for none.
    pub glow: Vec3,
    /// The glow's angular radius, in radians.
    pub glow_radius: f32,
}

/// The clouds' light and cover (W2), for both cloud layers: the card clouds around the horizon
/// and the high cirrus. Clouds are painted in the anime way: a lit and a shaded tone split along
/// the painted terminator, with a rim along their edges toward the key light, never a darkened
/// albedo.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CloudLight {
    /// Linear RGB of the clouds' lit sides, facing the key light (the sun by day, the moon at
    /// night).
    pub lit: Vec3,
    /// Linear RGB of their shaded sides: an authored color, usually a sky-ward hue.
    pub shade: Vec3,
    /// Linear RGB added along cloud edges, strongest toward the key light: the silver lining.
    pub rim: Vec3,
    /// How much of the card clouds shows, 0 (a clear sky) to 1 (the clouds as painted): their
    /// SDF dissolve threshold, so clouds shrink to their cores and vanish rather than fade.
    pub coverage: f32,
    /// The same for the cirrus layer.
    pub cirrus: f32,
}

/// Low-frequency ambient light (look pillar 4): a hemisphere of sky light from above and bounce
/// light from below, `color × intensity` in linear RGB.
///
/// A surface facing `n` receives `mix(ground, sky, 0.5 + 0.5 n.y) × intensity`, the irradiance
/// of a sky-and-ground hemisphere (`ambient_light` in `frame.wgsl`). The anime character shader
/// takes it through the character rig's stylized GI ([`CharacterGi`], C2); the environment
/// shader (W4) and the probe grid, as the tint of its per-key bake (W5), will read it too.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmbientLight {
    /// From above.
    pub sky: Vec3,
    /// From below.
    pub ground: Vec3,
    pub intensity: f32,
}

impl Default for AmbientLight {
    /// The default sky's horizon from above and its ground from below, at 12%: the default
    /// day's 10:00 ambient (`sapphire_world::tod`).
    fn default() -> Self {
        let sky = SkyGradient::default();
        Self { sky: sky.horizon, ground: sky.ground, intensity: 0.12 }
    }
}

/// Distance and height fog (look pillar 3: layered atmosphere; M1 W3, the [`fog`](crate::fog)
/// module): two distance layers and a height layer, each fading what lies behind it toward its
/// color, plus light scattered toward the camera around the key light. Every world pass applies
/// it at its final color, the sky included, so distant geometry melts into the sky without a
/// seam; fog cards take its light.
///
/// Densities are extinction per meter: a layer's optical depth is its density integrated over
/// the meters of view ray inside it, and what lies behind keeps `exp(−depth)` of its light. The
/// layers mix in proportion to their depths along the ray. The default has none, and a frame
/// without any density draws exactly as it would without fog.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fog {
    /// Haze near the camera.
    pub near: FogLayer,
    /// Aerial perspective: what melts distant geometry into the sky. Its color is a tint on the
    /// sky's horizon ([`SkyGradient::horizon`]): white is the horizon itself, so geometry far
    /// enough away takes the sky's own color.
    pub far: FogLayer,
    pub height: HeightFog,
    /// Linear RGB light the fog scatters toward the camera around the key light's direction
    /// (the sun by day, the moon at night), color × strength, in a forward lobe; black for none.
    /// It adds where the fog is, so the sun side of a hazy view glows.
    pub sun_inscatter: Vec3,
    /// How much of the fog characters take (the instances the frame lists in
    /// [`FramePacket::characters`], and their outlines): their optical depth is scaled by it,
    /// from 0 (characters keep their colors at any distance) to 1 (as the world). Anime games
    /// keep characters readable in fog, so time of day sets it below 1.
    pub characters: f32,
}

impl Fog {
    /// Whether any layer has a density: a frame whose fog doesn't draws exactly as without it.
    pub fn is_visible(&self) -> bool {
        [self.near.density, self.far.density, self.height.density].iter().any(|density| *density > 0.0)
    }
}

/// One distance layer of [`Fog`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FogLayer {
    /// Linear RGB (the far layer's: a tint on the sky's horizon).
    pub color: Vec3,
    /// Extinction per meter: 0 is clear.
    pub density: f32,
    /// Meters from the camera where the layer begins.
    pub start: f32,
}

/// Fog that thickens toward the ground: its density is `density × exp(−falloff × (y − base))`
/// at height `y` (meters, world space), so it lies in valleys and thins over hilltops.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeightFog {
    /// Linear RGB.
    pub color: Vec3,
    /// Extinction per meter at `base`.
    pub density: f32,
    /// How fast it thins with height, per meter: 0 keeps `density` at every height.
    pub falloff: f32,
    /// The height, in meters, where `density` holds.
    pub base: f32,
}

/// The weather's wetness inputs (world_lighting_post.md §7), which every uber-shader will read
/// as frame uniforms: what the wet surface model (Lagarde's: porous albedo darkened, roughness
/// lowered, puddles filled to a flat, smooth normal, rain ripples) needs from the weather.
///
/// They reach the GPU (the frame uniforms' `weather`), and the environment shader's header says
/// where they will apply, but no shader reads them until weather and that model arrive (M2).
/// Nothing fills them yet either: the default is a dry world.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Weather {
    /// How wet surfaces are, 0 (dry) to 1 (soaked).
    pub wetness: f32,
    /// How far puddles have filled, 0 (none) to 1 (every puddle a material's mask allows).
    pub puddles: f32,
}

/// The wind foliage sways in (M1 W6, the [`foliage`](crate::foliage) module): resolved by the
/// world each frame (a zone's wind, its gusts through the day), and plain data here (rule R4).
/// Foliage materials say how far each level of a plant moves at unit strength; the time is the
/// packet's [`FramePacket::time_seconds`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wind {
    /// The direction the wind blows toward. Only its horizontal part counts; with none the air
    /// is calm.
    pub direction: Vec3,
    /// 0 is calm and 1 the strength materials' responses are authored at; a gale is a few.
    pub strength: f32,
    /// How much gusts swell and ease the strength as they travel downwind, 0 (steady) to 1
    /// (from calm to double).
    pub gustiness: f32,
}

impl Default for Wind {
    /// Calm: foliage stands still.
    fn default() -> Self {
        Self { direction: Vec3::X, strength: 0.0, gustiness: 0.0 }
    }
}

/// Time of day's curve for the character rig's key light (spec §5.12 "Character light rig").
/// Characters are lit by a virtual key light of their own, whose color and brightness time of
/// day curves separately from the world's (as WuWa fits its character brightness across the
/// day), so they read well at every hour.
///
/// The world resolves each character's key from it and the key light's direction, through the
/// character's rig preset (`sapphire_world::light_rig`, M1 C2), into [`CharacterLighting::key`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterLight {
    /// Linear RGB.
    pub color: Vec3,
    pub intensity: f32,
}

impl Default for CharacterLight {
    /// White light of intensity 1, as the default key light.
    fn default() -> Self {
        Self { color: Vec3::ONE, intensity: 1.0 }
    }
}

/// One character's data for a frame: the character light rig that lights the instance `id`
/// (M1 C2), the character's face (M1 C3), its body, which its shadow is cast from (M1 R4), and
/// how much of it draws (M1 C7). A character drawn as several instances (body, face, hair, a
/// weapon) lists each, with the same lighting, face, body and opacity.
///
/// Being listed is what makes an instance a character for shadows too: it never casts into the
/// world's shadow cascades and receives only them (environment casters), so a character never
/// shadows itself; its own shadow on the ground and the environment comes from its body (see
/// [`CharacterBody`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterInstance {
    pub id: InstanceId,
    pub lighting: CharacterLighting,
    pub face: CharacterFace,
    pub body: CharacterBody,
    /// How much of the character draws (M1 C7, spec §5.12 "Proximity fade"): 1 whole, 0 none,
    /// clamped to that range. Below 1 a screen-space dither keeps that share of its pixels in
    /// every pass that draws it (surfaces, outlines, eyes through hair, the prepasses), which
    /// temporal AA resolves into a blend and MSAA into a share of each pixel's samples, and the
    /// character takes no rim. The world resolves it from the camera's distance to the body
    /// (`sapphire_world::ProximityFade`), so the camera never clips into a character. Its shadow
    /// stays whole.
    pub opacity: f32,
}

impl CharacterInstance {
    /// Whether the renderer can draw by it: its lighting, its face and its body are all valid,
    /// and its opacity is a number.
    pub fn is_valid(&self) -> bool {
        self.lighting.is_valid() && self.face.is_valid() && self.body.is_valid() && !self.opacity.is_nan()
    }

    /// How far it has faded: 1 − its opacity, within 0 to 1.
    pub fn fade(&self) -> f32 {
        1.0 - self.opacity.clamp(0.0, 1.0)
    }
}

/// A character's body this frame, as a capsule in world space: every point within `radius` of
/// the segment from `base` (its feet) to `top` (its head). Its shadow on the ground and the
/// environment is cast from it (M1 R4, spec §5.12 "Lighting"):
///
/// - **On desktop** each character near the camera draws a shadow map of its own, a
///   high-resolution frustum from the key light around its body and its instances' bounds, which
///   environment surfaces receive: crisp contact shadows under its feet. Its own instances never
///   receive it.
/// - **On phones,** and for characters past the desktop's maps, the capsule itself casts a soft
///   shadow onto environment surfaces.
///
/// The instances listed with equal bodies are one character, which casts one shadow; a character
/// drawn as several instances lists the same body with each. Capsule AO (W5) will read the body
/// too. A body of radius 0 (the default) casts no shadow of a character's own: the character
/// still receives the world's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CharacterBody {
    pub base: Vec3,
    pub top: Vec3,
    pub radius: f32,
}

impl CharacterBody {
    /// A capsule from `base` to `top`, `radius` thick.
    pub fn capsule(base: Vec3, top: Vec3, radius: f32) -> Self {
        Self { base, top, radius }
    }

    /// Whether the renderer can draw by it: finite, with a radius of 0 or more.
    pub fn is_valid(&self) -> bool {
        self.base.is_finite() && self.top.is_finite() && self.radius.is_finite() && self.radius >= 0.0
    }

    /// Whether it casts a shadow: a radius above 0.
    pub fn casts(&self) -> bool {
        self.radius > 0.0
    }

    /// The body moved by `transform`: its ends placed, its radius grown by the transform's
    /// largest scale.
    pub fn transformed(&self, transform: Mat4) -> Self {
        let scale = transform
            .x_axis
            .truncate()
            .length()
            .max(transform.y_axis.truncate().length())
            .max(transform.z_axis.truncate().length());
        Self {
            base: transform.transform_point3(self.base),
            top: transform.transform_point3(self.top),
            radius: self.radius * scale,
        }
    }
}

/// A character's light rig resolved for one frame (spec §5.12 "Character light rig" and
/// "Stylized character GI", M1 C2): what the anime character shader lights the character by,
/// in place of the world's key light and plain ambient. The world resolves it from the
/// character's preset, time of day and the camera (`sapphire_world::light_rig`); the renderer
/// only uploads it.
///
/// Local lights are not part of it: they keep their own directions, and the rig only caps
/// their sum ([`Self::local_light_ceiling`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterLighting {
    /// The character's key light: the direction its light travels (art-directed from the sun or
    /// moon, or fixed to the camera) and its color and intensity (time of day's
    /// [`CharacterLight`], scaled by the preset). It replaces [`FramePacket::sun`] for the
    /// character's light split and rim.
    pub key: DirectionalLight,
    /// Scales the materials' rim light, which takes the key light's color.
    pub rim: f32,
    /// The most luminance the local lights add together, in units of a unit key light, so
    /// stacked lights tint a character rather than blow it out (the reference games'
    /// `_FilterLight`).
    pub local_light_ceiling: f32,
    pub gi: CharacterGi,
}

impl CharacterLighting {
    /// Lighting by `key` with unstyled GI: the frame's ambient as the surface's normal receives
    /// it, unclamped and untinted. The renderer lights instances that aren't listed as characters
    /// with the world's key light this way.
    pub fn unstyled(key: DirectionalLight) -> Self {
        Self { key, rim: 1.0, local_light_ceiling: 2.0, gi: CharacterGi::default() }
    }

    /// Whether every value is one the shader can light with: finite numbers, a key direction
    /// that isn't zero, and no negative intensity, color, scale or ceiling. The value ceiling may
    /// be infinite (no clamp).
    pub fn is_valid(&self) -> bool {
        let Self { key, rim, local_light_ceiling, gi } = *self;
        let colors = [key.color, gi.skin_tint];
        let scalars =
            [key.intensity, rim, local_light_ceiling, gi.intensity, gi.camera_bias, gi.max_saturation];
        key.direction.is_finite()
            && key.direction != Vec3::ZERO
            && colors.iter().all(|color| color.is_finite() && color.min_element() >= 0.0)
            && scalars.iter().all(|value| value.is_finite() && *value >= 0.0)
            && gi.head.is_none_or(Vec3::is_finite)
            && gi.max_value >= 0.0
    }
}

/// A character's stylized GI (spec §5.12 "Stylized character GI", M1 C2): the frame's ambient
/// light ([`FramePacket::ambient`]) seen through normals flattened toward low frequency, then
/// clamped and tinted, so characters take the scene's light without its noise and never go
/// muddy (WuWa's character GI). Per pixel:
///
/// 1. The normal turns toward a sphere's around [`Self::head`], if there is one, by the
///    material region's sphere weight (faces and hair, `RegionGi::sphere` in
///    `sapphire_assets::material`), then bends toward the camera by [`Self::camera_bias`].
/// 2. The ambient hemisphere at that normal, times [`Self::intensity`].
/// 3. An HSV clamp: saturation held to [`Self::max_saturation`] and value (the brightest
///    channel) to [`Self::max_value`], hue kept.
/// 4. The skin tint, by the region's skin weight (`RegionGi::skin`), and ambient occlusion
///    (the vertex color's).
///
/// The default changes nothing: the ambient as the normal receives it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterGi {
    /// The center of the character's head, world space; without one, no region spherizes.
    pub head: Option<Vec3>,
    /// Scales the frame's ambient light.
    pub intensity: f32,
    /// How far the normal bends toward the camera: the unit view vector times this is added to
    /// it before renormalizing, so 0 keeps the normal and 1 turns a silhouette's halfway.
    pub camera_bias: f32,
    /// Saturation's ceiling, 0 to 1 (1: no clamp).
    pub max_saturation: f32,
    /// Value's ceiling, linear light (infinite: no clamp).
    pub max_value: f32,
    /// Linear RGB the GI of skin is multiplied by.
    pub skin_tint: Vec3,
}

impl Default for CharacterGi {
    fn default() -> Self {
        Self {
            head: None,
            intensity: 1.0,
            camera_bias: 0.0,
            max_saturation: 1.0,
            max_value: f32::INFINITY,
            skin_tint: Vec3::ONE,
        }
    }
}

/// A point or spot light (spec §5.12 "Lighting"). The anime character shader adds it to the
/// sun's light within the same two-tone model: the same light/shadow split and authored shadow
/// color, in the light's color (see `anime_character.wgsl`).
///
/// Its brightness is `color × intensity` wherever it is near, fading smoothly to nothing at
/// `range`, with no inverse-square hot spot: an intensity of 1 lights a surface next to it as
/// brightly as a unit sun.
///
/// The renderer ignores a light that can't light anything: a range under a millimeter or not
/// finite, an intensity that isn't positive, a black or non-finite color, or a spot without a
/// direction. Negative color channels count as 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalLight {
    /// World-space position.
    pub position: Vec3,
    /// Linear RGB.
    pub color: Vec3,
    pub intensity: f32,
    /// Distance in meters at which the light has faded to nothing.
    pub range: f32,
    pub kind: LightKind,
}

/// The shape of a [`LocalLight`]'s light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightKind {
    /// Light in every direction.
    Point,
    /// A cone along `direction` (need not be normalized): full brightness within
    /// `inner_angle` of it, fading smoothly to nothing at `outer_angle`. Both are half-angles in
    /// radians, at most π; an inner angle beyond the outer one is taken as the outer one.
    Spot { direction: Vec3, inner_angle: f32, outer_angle: f32 },
}

/// One drawn mesh, as an instance delta carries it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshInstance {
    pub id: InstanceId,
    pub mesh: MeshHandle,
    /// Object to world. Mirroring (negative determinant) is not supported yet: it flips
    /// winding, so the anime character and outline passes would cull the wrong faces.
    pub transform: Mat4,
}

/// A three-stop sky gradient in linear RGB: the sky pass's colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyGradient {
    pub zenith: Vec3,
    pub horizon: Vec3,
    pub ground: Vec3,
}

impl Default for SkyGradient {
    /// A clear midday sky.
    fn default() -> Self {
        Self {
            zenith: Vec3::new(0.10, 0.30, 0.80),
            horizon: Vec3::new(0.62, 0.78, 0.95),
            ground: Vec3::new(0.36, 0.34, 0.42),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_is_reverse_z_with_infinite_far() {
        let camera = CameraData::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y, 1.0, 1.5, 0.1);
        let depth = |z: f32| {
            let clip = camera.proj * camera.view * Vec3::new(0.0, 0.0, z).extend(1.0);
            clip.z / clip.w
        };
        assert!((depth(4.9) - 1.0).abs() < 1e-5, "near plane maps to 1");
        assert!(depth(0.0) > depth(-100.0), "nearer is greater");
        assert!(depth(-1.0e7) > 0.0 && depth(-1.0e7) < 1.0e-6, "far away approaches 0");
    }

    fn fill(packet: &mut FramePacket, instances: u64) {
        let camera = CameraData::look_at(Vec3::Z, Vec3::ZERO, Vec3::Y, 1.0, 1.0, 0.1);
        packet.views.push(ViewData::new(camera));
        packet.sun.intensity = 2.0;
        packet.celestial.sphere = Quat::from_rotation_x(0.5);
        packet.sky_layers.stars = 1.0;
        packet.sky_layers.clouds.coverage = 0.5;
        packet.sky_material = Some(MaterialHandle(3));
        packet.fog.far.density = 0.01;
        packet.shadow_tint = Vec3::splat(0.4);
        packet.weather = Weather { wetness: 0.5, puddles: 0.25 };
        packet.wind = Wind { direction: Vec3::Z, strength: 1.5, gustiness: 0.5 };
        packet.day_night = 1.0;
        packet.emission_scale = 0.25;
        packet.time_seconds = 1.5;
        packet.lights.push(LocalLight {
            position: Vec3::Y,
            color: Vec3::ONE,
            intensity: 1.0,
            range: 2.0,
            kind: LightKind::Point,
        });
        let lighting = CharacterLighting::unstyled(DirectionalLight::default());
        let face = CharacterFace {
            expression: FaceExpression { blush: 0.5, gloom: 0.0 },
            ..CharacterFace::default()
        };
        let body = CharacterBody::capsule(Vec3::ZERO, Vec3::Y, 0.25);
        packet.characters.push(CharacterInstance { id: InstanceId(0), lighting, face, body, opacity: 1.0 });
        for id in 0..instances {
            packet.instances.changed.push(MeshInstance {
                id: InstanceId(id),
                mesh: MeshHandle::from_raw(0),
                transform: Mat4::IDENTITY,
            });
        }
        packet.instances.removed.push(InstanceId(99));
        let palette = (0..3).map(|joint| Affine3A::from_translation(Vec3::splat(joint as f32)));
        packet.push_skinned(InstanceId(0), palette, [0.5, 1.0]);
        packet.push_skinned(InstanceId(1), [Affine3A::IDENTITY], []);
    }

    #[test]
    fn ring_numbers_frames_and_starts_each_packet_empty() {
        let mut ring = FramePacketRing::new();
        assert_eq!(ring.current(), &FramePacket::default(), "an empty frame 0 before the first begin");
        for frame in 1..=7 {
            let packet = ring.begin();
            assert_eq!(packet.frame, frame);
            assert_eq!(packet, &FramePacket { frame, ..FramePacket::default() }, "reset completely");
            fill(packet, 4);
            assert_eq!(ring.current().instances.changed.len(), 4);
        }
    }

    #[test]
    fn warm_packets_reuse_their_storage() {
        let mut ring = FramePacketRing::new();
        // Warm every slot once.
        for _ in 0..RING_SIZE {
            fill(ring.begin(), 8);
        }
        let storage = |packet: &FramePacket| {
            (
                packet.views.as_ptr(),
                packet.views.capacity(),
                packet.lights.as_ptr(),
                packet.characters.as_ptr(),
                packet.instances.changed.as_ptr(),
                packet.instances.changed.capacity(),
                packet.instances.removed.as_ptr(),
                packet.skinned.as_ptr(),
                packet.palettes.as_ptr(),
                packet.morph_weights.as_ptr(),
            )
        };
        let warm: Vec<_> = ring.packets.iter().map(storage).collect();
        for _ in 0..3 * RING_SIZE {
            fill(ring.begin(), 8);
        }
        let reused: Vec<_> = ring.packets.iter().map(storage).collect();
        assert_eq!(warm, reused, "no packet vector was reallocated");
    }

    #[test]
    fn head_bases_turn_with_the_head_and_orthonormalize() {
        let turned = HeadBasis::turned(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        assert!(turned.forward.abs_diff_eq(Vec3::X, 1e-6) && turned.up.abs_diff_eq(Vec3::Y, 1e-6));
        let (forward, up) = HeadBasis { forward: Vec3::new(0.0, 0.0, 3.0), up: Vec3::new(0.0, 2.0, 1.0) }
            .orthonormalized()
            .expect("a basis");
        assert_eq!((forward, up), (Vec3::Z, Vec3::Y), "up loses its forward part");
        assert_eq!(HeadBasis { forward: Vec3::ZERO, up: Vec3::Y }.orthonormalized(), None);
        assert_eq!(HeadBasis { forward: Vec3::Y, up: Vec3::NEG_Y }.orthonormalized(), None, "parallel");
        assert_eq!(HeadBasis { forward: Vec3::Z, up: Vec3::splat(f32::NAN) }.orthonormalized(), None);
    }

    #[test]
    fn faces_are_valid_with_a_basis_and_finite_weights() {
        assert!(CharacterFace::default().is_valid(), "at rest");
        let tilted = HeadBasis { forward: Vec3::new(0.0, 0.0, 3.0), up: Vec3::new(0.0, 2.0, 1.0) };
        assert!(CharacterFace { head: tilted, ..CharacterFace::default() }.is_valid());
        let broken = [
            CharacterFace { head: HeadBasis { forward: Vec3::Y, up: Vec3::Y }, ..CharacterFace::default() },
            CharacterFace {
                expression: FaceExpression { blush: f32::NAN, gloom: 0.0 },
                ..CharacterFace::default()
            },
            CharacterFace {
                expression: FaceExpression { blush: 0.0, gloom: f32::INFINITY },
                ..CharacterFace::default()
            },
        ];
        for face in broken {
            assert!(!face.is_valid(), "{face:?}");
        }
        // Weights outside [0, 1] are valid: the shader clamps them.
        let strong = CharacterFace {
            expression: FaceExpression { blush: 2.0, gloom: -1.0 },
            ..CharacterFace::default()
        };
        assert!(strong.is_valid());
        let lighting = CharacterLighting::unstyled(DirectionalLight::default());
        let character = CharacterInstance {
            id: InstanceId(0),
            lighting,
            face: broken[0],
            body: CharacterBody::default(),
            opacity: 1.0,
        };
        assert!(!character.is_valid(), "a broken face makes the entry invalid");
        assert!(CharacterInstance { face: strong, ..character }.is_valid());
    }

    #[test]
    fn bodies_are_valid_when_finite_and_cast_with_a_radius() {
        let body = CharacterBody::capsule(Vec3::ZERO, Vec3::Y, 0.3);
        assert!(body.is_valid() && body.casts());
        assert!(CharacterBody::default().is_valid(), "no shadow of its own, but valid");
        assert!(!CharacterBody::default().casts());
        for broken in [
            CharacterBody { radius: -0.1, ..body },
            CharacterBody { radius: f32::NAN, ..body },
            CharacterBody { top: Vec3::splat(f32::INFINITY), ..body },
        ] {
            assert!(!broken.is_valid(), "{broken:?}");
        }
        let lighting = CharacterLighting::unstyled(DirectionalLight::default());
        let face = CharacterFace::default();
        let broken = CharacterBody { radius: -1.0, ..body };
        assert!(
            !CharacterInstance { id: InstanceId(0), lighting, face, body: broken, opacity: 1.0 }.is_valid()
        );
        let moved = body.transformed(Mat4::from_scale_rotation_translation(
            Vec3::splat(2.0),
            Quat::IDENTITY,
            Vec3::X,
        ));
        assert_eq!(moved, CharacterBody::capsule(Vec3::X, Vec3::new(1.0, 2.0, 0.0), 0.6));
    }

    #[test]
    fn skinned_instances_name_their_palette_and_weights() {
        let mut packet = FramePacket::default();
        fill(&mut packet, 0);
        let [first, second] = packet.skinned[..] else { panic!("two poses") };
        assert_eq!(first.palette, PacketRange { start: 0, len: 3 });
        assert_eq!(second.palette, PacketRange { start: 3, len: 1 });
        assert_eq!(second.morph_weights, PacketRange { start: 2, len: 0 });
        let palette = packet.palette(&first).expect("in range");
        assert_eq!(palette[2].translation, Vec3::splat(2.0).into());
        assert_eq!(packet.morph_weights(&first), Some(&[0.5, 1.0][..]));
        assert_eq!(packet.palette(&second), Some(&[Affine3A::IDENTITY][..]));
        assert_eq!(packet.morph_weights(&second), Some(&[][..]));
        let outside = SkinnedInstance { palette: PacketRange { start: 3, len: 2 }, ..second };
        assert_eq!(packet.palette(&outside), None);
    }
}
