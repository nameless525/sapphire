//! The extract boundary (spec §4.6, rule R4): the plain data the renderer draws from.
//!
//! Each frame the app's extract systems fill one [`FramePacket`] from the simulation, and the
//! renderer reads only that. A packet holds `Copy` data and owned vectors, never references
//! into the simulation or gameplay types, so it can be handed to render workers while the next
//! frame simulates (§4.2).
//!
//! - **Views and scene constants** are rewritten whole every frame; they are small. The scene
//!   constants are the frame's look: the key light, the sun and moon, the sky gradient, ambient
//!   light, fog, the shadow tint, the character rig's key light, the day/night factor and
//!   post-processing. Time of day resolves all of them from one set of keys
//!   (`sapphire_world::tod`, M1 W1; look pillar 1); the renderer sees only the values.
//! - **Local lights** are rewritten whole every frame too: a frame has a few dozen at most
//!   (spec §3.4), and the renderer culls and lists them per frame anyway.
//! - **Characters** are rewritten whole every frame as well: each [`CharacterInstance`] gives an
//!   instance its character light rig ([`CharacterLighting`], M1 C2), resolved by the world for
//!   this frame's camera and time of day, which changes whenever either moves.
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
use glam::{Affine3A, Mat4, Vec3};

use crate::{MeshHandle, PostParams};

/// Packets in a [`FramePacketRing`]: one extracting, one rendering, one spare.
const RING_SIZE: usize = 3;

/// Everything the renderer needs to draw one frame.
///
/// # Later milestones
///
/// The packet grows with the systems that feed it (spec §4.6): VFX spawns and emitter
/// parameters, gameplay-triggered post effects in `post` (P3), the UI draw list, and debug
/// draws (dev builds). Shadow cascades (R4) and UI 3D insets add views after the main
/// camera.
///
/// Some scene constants are carried for the packages that will read them and drawn from
/// nothing yet: [`Self::celestial`] (the sky's sun and moon, W2), [`Self::fog`] (W3) and
/// [`Self::shadow_tint`] (R4, W4). Each type's docs name its readers.
#[derive(Clone, Debug, Default, PartialEq)]
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
    /// The sun and the moon themselves, for the sky to draw (W2).
    pub celestial: Celestial,
    pub ambient: AmbientLight,
    pub fog: Fog,
    /// Linear RGB the key light keeps inside cast shadows, so shadows are colored rather than
    /// black (look pillar 4): black is a physical shadow, a dim blue-violet the anime one. Read
    /// by shadows (R4) and the environment shader (W4). Characters in cast shadow take their
    /// materials' authored shade colors instead (C1).
    pub shadow_tint: Vec3,
    /// Time of day's curve for the character rig's key light: the world resolves each
    /// character's [`CharacterLighting`] from it, so the renderer reads it only through
    /// [`Self::characters`].
    pub character_light: CharacterLight,
    /// 0 by day, 1 at night: the anime character shader's day/night ramp blend (C1; spec §5.12
    /// "Ramps"), and anything else that has a day and a night look. The frame uniforms carry it
    /// as `day_night`.
    pub day_night: f32,
    /// Presentation time in seconds, read from the owning clock (rule R2), for shader
    /// animation. The app wraps it to a fixed period (the sandbox uses an hour) so it keeps
    /// f32 precision in long sessions; shaders may only use it for effects that are periodic
    /// or tolerate the jump at the wrap.
    pub time_seconds: f32,
    /// Exposure, bloom, tone curve, grade and vignette. Defaults to Sapphire's default look.
    pub post: PostParams,
    /// Point and spot lights. When more are visible than the renderer's
    /// [`RenderSettings::max_lights`](crate::RenderSettings::max_lights) allows, it keeps the
    /// nearest; a cluster that more reach than it can list keeps the earliest in this order.
    pub lights: Vec<LocalLight>,
    /// This frame's characters: the instances the character light rig lights (M1 C2), each with
    /// its rig resolved for this frame. Instances not listed draw with the world's key light
    /// and unstyled GI ([`CharacterLighting::unstyled`]); an instance listed twice takes its
    /// last entry. The renderer ignores, and counts, an entry for an instance it doesn't draw or
    /// whose lighting isn't valid ([`CharacterLighting::is_valid`]).
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
            ambient,
            fog,
            shadow_tint,
            character_light,
            day_night,
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
        *ambient = AmbientLight::default();
        *fog = Fog::default();
        *shadow_tint = Vec3::ZERO;
        *character_light = CharacterLight::default();
        *day_night = 0.0;
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

/// The sun and the moon as the sky draws them (W2: their discs and halos), each with the light
/// it gives: where it is (opposite the direction its light travels), its color and intensity.
///
/// The key light ([`FramePacket::sun`]) is resolved from these by time of day: the sun's while
/// only the sun shines, the moon's while only the moon does, and a blend of the two through
/// twilight. A body's intensity can be 0 while it is still drawn (the moon by day).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Celestial {
    pub sun: DirectionalLight,
    pub moon: DirectionalLight,
}

impl Default for Celestial {
    /// The default sun straight overhead, and a dark moon below the horizon.
    fn default() -> Self {
        Self {
            sun: DirectionalLight::default(),
            moon: DirectionalLight { direction: Vec3::Y, color: Vec3::new(0.6, 0.7, 1.0), intensity: 0.0 },
        }
    }
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

/// Distance and height fog (look pillar 3: layered atmosphere), read by W3: two distance layers
/// and a height layer, each fading what lies behind it toward its color, plus light scattered
/// toward the camera around the sun.
///
/// Densities are extinction per meter: a layer's fog is `1 − exp(−density × meters)` over the
/// meters of view ray inside it. The default has none.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fog {
    /// Haze near the camera.
    pub near: FogLayer,
    /// Aerial perspective: what melts distant geometry into the sky. W3 takes its color from
    /// the sky's horizon in the view direction, tinted by this layer's.
    pub far: FogLayer,
    pub height: HeightFog,
    /// Linear RGB light the fog scatters toward the camera around the sun's direction
    /// (color × strength); black for none.
    pub sun_inscatter: Vec3,
}

/// One distance layer of [`Fog`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FogLayer {
    /// Linear RGB.
    pub color: Vec3,
    /// Extinction per meter: 0 is clear.
    pub density: f32,
    /// Meters from the camera where the layer begins.
    pub start: f32,
}

/// Fog that thickens toward the ground: its density is `density × exp(−falloff × (y − base))`
/// at height `y` (meters, world space).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeightFog {
    /// Linear RGB.
    pub color: Vec3,
    /// Extinction per meter at `base`.
    pub density: f32,
    /// How fast it thins with height, per meter.
    pub falloff: f32,
    /// The height, in meters, where `density` holds.
    pub base: f32,
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

/// One character's data for a frame (M1 C2): the character light rig that lights the instance
/// `id`. A character drawn as several instances (body, hair, a weapon) lists each.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterInstance {
    pub id: InstanceId,
    pub lighting: CharacterLighting,
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
        packet.views.push(ViewData { camera });
        packet.sun.intensity = 2.0;
        packet.fog.far.density = 0.01;
        packet.shadow_tint = Vec3::splat(0.4);
        packet.day_night = 1.0;
        packet.time_seconds = 1.5;
        packet.lights.push(LocalLight {
            position: Vec3::Y,
            color: Vec3::ONE,
            intensity: 1.0,
            range: 2.0,
            kind: LightKind::Point,
        });
        let lighting = CharacterLighting::unstyled(DirectionalLight::default());
        packet.characters.push(CharacterInstance { id: InstanceId(0), lighting });
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
