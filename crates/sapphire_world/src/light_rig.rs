//! The character light rig (M1 C2; look pillar 1 on characters, M1 plan §2; spec §5.12
//! "Character light rig" and "Stylized character GI"): characters are lit by a virtual key
//! light and a stylized GI of their own rather than by the world's sun and ambient, so they read
//! well wherever they stand and at every hour, as Wuthering Waves runs its character lighting as
//! a separate pipeline with presets.
//!
//! # Data
//!
//! - **[`LightRig`]** (reflected, serializable): a preset. Its key light follows the world's
//!   (the sun, the moon, or both through twilight) or a direction fixed to the camera, or a
//!   blend, under art-directed limits: an elevation band, so faces are never lit from below or
//!   straight above, and an azimuth limit around the camera's side, past which a key from
//!   behind folds back toward the camera, so they always keep a lit side.
//!   Its color and brightness scale time of day's character key, which curves through the day
//!   apart from the world's light but fitted to it (a little brighter at night). Its GI settings
//!   flatten and clamp the ambient light characters take; it also scales the rim and caps the
//!   local lights.
//! - **[`LightRigLibrary`]** (a `LightRigLibrary` document, schema 1): named presets, the first
//!   the default. [`LightRigLibrary::default_rigs`] is the shipped one, compiled in until packs
//!   carry it as content: `gameplay` and `portrait`.
//! - **[`CharacterLightRig`]** (an ECS component): which preset a character uses, and where its
//!   head is in its own space (the center its face and hair GI is spherized around).
//! - **Per-shot overrides:** a [`TodCutscene`](crate::tod::TodCutscene) may carry a rig for its
//!   shot, which every character takes over its own by the track's eased weight, the same blend
//!   its time-of-day values follow.
//!
//! # Resolving
//!
//! Once per frame and character, [`TodFrame::character_lighting`](crate::tod::TodFrame) turns a
//! preset, time of day's look, a playing shot's rig and the camera into the plain
//! [`CharacterLighting`] the frame packet carries ([`FramePacket::characters`]; rule R4: the
//! renderer never sees presets). [`LightRig::resolve`] does it without time of day. Resolving
//! allocates nothing and costs about 45 ns a character on an Apple M5 (release), 85 ns for a key
//! fixed to the camera, which turns along a great circle.
//!
//! The key's direction is resolved in the character's frame as the camera sees it: its azimuth
//! around the vertical axis through the head, from the camera's side (0° lights the character
//! from where the camera is, positive from the camera's right), and its elevation above the
//! horizon. Judging from the head rather than the camera's axis keeps the limits true for a
//! character at the edge of the screen. The elevation is clamped to its band; an azimuth past
//! its limit folds back linearly, from the limit to 0° directly behind, so a key that passes
//! behind the character turns smoothly through the camera's side instead of jumping from one side
//! of the face to the other. A key within the limits is the world's key light exactly, so a
//! preset whose limits a scene never reaches lights it as the world's sun would.
//!
//! # Local lights
//!
//! Local lights keep their own directions: the limits are for the key light alone, and a
//! lantern held below a face is meant to light it from below. The rig only caps their sum
//! ([`LightRig::local_light_ceiling`]).
//!
//! # Later milestones
//!
//! - **Per-zone presets:** a zone's time-of-day volume carrying a rig, blended over the
//!   characters' own by the volume's weight as a shot's is (the research notes' per-volume
//!   character presets).
//! - **Keyed shot rigs and per-character overrides:** rigs keyed over a shot's seconds, and a
//!   sequencer track (M3) giving one character of a shot a rig of its own.
//! - **Cast shadows (R4):** how much of the environment's cast shadow a preset lets reach a
//!   character, if cutscenes need close-ups free of dappled shade.
//! - **The head from the skeleton:** characters' heads from their head bones once animation
//!   poses them (M2); [`CharacterLightRig::head`] is fixed in the entity's space until then.
//! - **Presets as content** loaded from packs (A4), and pushed by live link (A9) through
//!   [`LightRigLibrary::from_ron`] as desktop hot reload already reloads this crate's document in
//!   development builds (A8); characters keep their [`LightRigId`]s, and an ID past a shorter
//!   library's end falls back to its default.
//!
//! [`FramePacket::characters`]: sapphire_render::FramePacket::characters

mod library;
#[cfg(test)]
mod tests;

use bevy_ecs::prelude::Component;
use bevy_reflect::Reflect;
use sapphire_math::det::{atan2, sin_cos, slerp_direction};
use sapphire_math::{Vec3, Vec4};
use sapphire_render::{CameraData, CharacterGi, CharacterLight, CharacterLighting, DirectionalLight};
use serde::{Deserialize, Serialize};

pub use library::{LightRigError, LightRigId, LightRigLibrary, NamedLightRig};

use crate::tod::Rgb;

/// A character light rig preset. Every field left out of a document takes its neutral value: the
/// world's key light with no limits, time of day's character key as it is, and unstyled GI.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(default, deny_unknown_fields)]
pub struct LightRig {
    pub key: LightRigKey,
    pub gi: LightRigGi,
    /// Scales the materials' rim light, which takes the key light's color: 1 as authored.
    pub rim: f32,
    /// The most luminance the local lights add together, in units of a unit key light
    /// (the reference games' `_FilterLight`).
    pub local_light_ceiling: f32,
}

impl Default for LightRig {
    fn default() -> Self {
        let neutral = CharacterLighting::unstyled(DirectionalLight::default());
        Self {
            key: LightRigKey::default(),
            gi: LightRigGi::default(),
            rim: neutral.rim,
            local_light_ceiling: neutral.local_light_ceiling,
        }
    }
}

/// Where a rig's key light comes from, and its color and brightness.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(default, deny_unknown_fields)]
pub struct LightRigKey {
    /// 0 follows the world's key light (the sun, the moon, or both through twilight); 1 holds the
    /// key at [`Self::camera_direction`] around the camera, wherever the sun is (portraits,
    /// dialogue); between, the key turns that far along the great circle from one to the other.
    pub camera_follow: f32,
    /// The key's place around the camera, for [`Self::camera_follow`].
    pub camera_direction: CameraAngles,
    /// The lowest the key may come from, in degrees above the horizon, -90 to 90.
    pub min_elevation: f32,
    /// The highest, in degrees, at least [`Self::min_elevation`].
    pub max_elevation: f32,
    /// How far round from the camera's side the key may come, in degrees, 0 to 180. A key from
    /// further round, behind the character, folds back toward the camera's side, reaching it
    /// when directly behind: faces keep a lit side, and a key passing behind the character turns
    /// smoothly instead of jumping from one side to the other.
    pub max_azimuth: f32,
    /// Linear RGB multiplying time of day's character key color.
    pub tint: Rgb,
    /// Scales time of day's character key intensity.
    pub intensity: f32,
}

impl Default for LightRigKey {
    fn default() -> Self {
        Self {
            camera_follow: 0.0,
            camera_direction: CameraAngles::default(),
            min_elevation: -90.0,
            max_elevation: 90.0,
            max_azimuth: 180.0,
            tint: [1.0; 3],
            intensity: 1.0,
        }
    }
}

/// A direction around a character as the camera sees it, in degrees: the azimuth around the
/// vertical axis from the camera's side (0° from where the camera is, positive toward its right,
/// ±180° from behind the character), and the elevation above the horizon.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(default, deny_unknown_fields)]
pub struct CameraAngles {
    pub azimuth: f32,
    pub elevation: f32,
}

impl Default for CameraAngles {
    /// A classic portrait key: from the camera's left, raised.
    fn default() -> Self {
        Self { azimuth: -35.0, elevation: 30.0 }
    }
}

/// A rig's stylized GI ([`CharacterGi`] says what each step does). Faces and hair take the
/// sphere around the head, and skin the skin tint, by their material regions' weights.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(default, deny_unknown_fields)]
pub struct LightRigGi {
    /// Scales the frame's ambient light.
    pub intensity: f32,
    /// How far GI normals bend toward the camera: 0 keeps them, 1 turns a silhouette's halfway.
    pub camera_bias: f32,
    /// Saturation's ceiling, 0 to 1 (1: no clamp).
    pub max_saturation: f32,
    /// Value's ceiling (the brightest channel), linear light; none by default.
    pub max_value: Option<f32>,
    /// Linear RGB the GI of skin is multiplied by.
    pub skin_tint: Rgb,
}

impl Default for LightRigGi {
    fn default() -> Self {
        Self { intensity: 1.0, camera_bias: 0.0, max_saturation: 1.0, max_value: None, skin_tint: [1.0; 3] }
    }
}

/// Which preset a character's instances are lit by, and where its head is: an ECS component
/// for the app's extraction, which resolves it into the frame packet each frame.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct CharacterLightRig {
    pub rig: LightRigId,
    /// The center of the head in the entity's own space: the sphere face and hair GI normals
    /// turn toward, and the point the key's limits are judged around.
    pub head: Vec3,
}

impl LightRig {
    /// Checks every value is in its range. Returns the field and what is wrong.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        const UNIT: &str = "must be within 0..=1";
        const ELEVATION: &str = "must be within -90..=90 degrees";
        let key = &self.key;
        within(key.camera_follow, 0.0, 1.0, "key.camera_follow", UNIT)?;
        let azimuth = "must be within -180..=180 degrees";
        within(key.camera_direction.azimuth, -180.0, 180.0, "key.camera_direction.azimuth", azimuth)?;
        within(key.camera_direction.elevation, -90.0, 90.0, "key.camera_direction.elevation", ELEVATION)?;
        within(key.min_elevation, -90.0, 90.0, "key.min_elevation", ELEVATION)?;
        let above_min = "must be within `min_elevation`..=90 degrees";
        within(key.max_elevation, key.min_elevation, 90.0, "key.max_elevation", above_min)?;
        within(key.max_azimuth, 0.0, 180.0, "key.max_azimuth", "must be within 0..=180 degrees")?;
        color(key.tint, "key.tint")?;
        non_negative(key.intensity, "key.intensity")?;
        let gi = &self.gi;
        non_negative(gi.intensity, "gi.intensity")?;
        non_negative(gi.camera_bias, "gi.camera_bias")?;
        within(gi.max_saturation, 0.0, 1.0, "gi.max_saturation", UNIT)?;
        if let Some(value) = gi.max_value {
            non_negative(value, "gi.max_value")?;
        }
        color(gi.skin_tint, "gi.skin_tint")?;
        non_negative(self.rim, "rim")?;
        non_negative(self.local_light_ceiling, "local_light_ceiling")
    }

    /// This preset's lighting for a character whose head is at `head` (world space), seen by
    /// `camera`, under the world's `key_light` and time of day's `character_light`.
    pub fn resolve(
        &self,
        key_light: &DirectionalLight,
        character_light: &CharacterLight,
        camera: &CameraData,
        head: Vec3,
    ) -> CharacterLighting {
        let key = &self.key;
        let gi = &self.gi;
        CharacterLighting {
            key: DirectionalLight {
                direction: key.direction(key_light.direction, camera, head),
                color: character_light.color * Vec3::from(key.tint),
                intensity: character_light.intensity * key.intensity,
            },
            rim: self.rim,
            local_light_ceiling: self.local_light_ceiling,
            gi: CharacterGi {
                head: Some(head),
                intensity: gi.intensity,
                camera_bias: gi.camera_bias,
                max_saturation: gi.max_saturation,
                max_value: gi.max_value.unwrap_or(f32::INFINITY),
                skin_tint: gi.skin_tint.into(),
            },
        }
    }
}

impl LightRigKey {
    /// The direction the rig's key light travels for a character whose head is at `head`, seen
    /// by `camera`, given the direction the world's key light travels. Exactly `world` when the
    /// key follows it and stays within the limits.
    pub fn direction(&self, world: Vec3, camera: &CameraData, head: Vec3) -> Vec3 {
        let frame = CameraYaw::of(camera, head);
        let toward_world = (-world).try_normalize().unwrap_or(Vec3::Y);
        let follow = self.camera_follow;
        let toward = if follow > 0.0 {
            let fixed = frame.toward(self.camera_direction.azimuth, self.camera_direction.elevation);
            slerp_direction(toward_world, fixed, follow.min(1.0))
        } else {
            toward_world
        };
        let (azimuth, elevation) = frame.angles(toward);
        let limited = (
            fold_azimuth(azimuth, self.max_azimuth),
            elevation.clamp(self.min_elevation, self.max_elevation),
        );
        if follow <= 0.0 && limited == (azimuth, elevation) {
            return world;
        }
        -frame.toward(limited.0, limited.1)
    }
}

/// `azimuth` within `max` of the camera's side, in degrees: unchanged up to `max`, and folded
/// back beyond it, linearly from `max` to 0 directly behind (±180°), so the result is
/// continuous all the way round.
fn fold_azimuth(azimuth: f32, max: f32) -> f32 {
    let beyond = azimuth.abs() - max;
    if beyond <= 0.0 {
        return azimuth;
    }
    // `beyond` is positive, so `max` is under 180 and the room behind it is too.
    azimuth.signum() * max * (1.0 - beyond / (180.0 - max))
}

/// A character's horizontal frame as the camera sees it: unit vectors along the ground from the
/// character toward the camera's side, and toward the camera's right.
#[derive(Clone, Copy, Debug)]
struct CameraYaw {
    back: Vec3,
    right: Vec3,
}

impl CameraYaw {
    /// The frame for a character whose head is at `head`: from the head toward the camera, so a
    /// character at the edge of the screen is judged by how the camera sees it.
    fn of(camera: &CameraData, head: Vec3) -> Self {
        let to_camera = camera.position - head;
        // The view matrix's rows are the camera's axes in world space; its +Z points back.
        let horizontal = |axis: Vec4| Vec3::new(axis.x, 0.0, axis.z);
        let back = Vec3::new(to_camera.x, 0.0, to_camera.z)
            .try_normalize()
            // A camera straight above or below the head: its own back axis, or looking straight
            // down or up, the edge of the screen toward the camera's side (the bottom edge
            // looking down, the top edge looking up).
            .or_else(|| horizontal(camera.view.row(2)).try_normalize())
            .or_else(|| {
                let up = horizontal(camera.view.row(1));
                (if camera.view.row(2).y >= 0.0 { -up } else { up }).try_normalize()
            })
            .unwrap_or(Vec3::Z);
        Self { back, right: Vec3::Y.cross(back) }
    }

    /// The azimuth and elevation of the unit vector `toward`, in degrees.
    fn angles(&self, toward: Vec3) -> (f32, f32) {
        let (side, front) = (toward.dot(self.right), toward.dot(self.back));
        let azimuth = atan2(side, front).to_degrees();
        let elevation = atan2(toward.y, (side * side + front * front).sqrt()).to_degrees();
        (azimuth, elevation)
    }

    /// The unit vector at `azimuth` and `elevation`, in degrees.
    fn toward(&self, azimuth: f32, elevation: f32) -> Vec3 {
        let (sin_azimuth, cos_azimuth) = sin_cos(azimuth.to_radians());
        let (sin_elevation, cos_elevation) = sin_cos(elevation.to_radians());
        (self.right * sin_azimuth + self.back * cos_azimuth) * cos_elevation + Vec3::Y * sin_elevation
    }
}

type Check = Result<(), (&'static str, &'static str)>;

fn within(value: f32, min: f32, max: f32, field: &'static str, problem: &'static str) -> Check {
    if (min..=max).contains(&value) { Ok(()) } else { Err((field, problem)) }
}

fn non_negative(value: f32, field: &'static str) -> Check {
    if value.is_finite() && value >= 0.0 { Ok(()) } else { Err((field, "must be finite and not negative")) }
}

fn color(rgb: Rgb, field: &'static str) -> Check {
    if rgb.iter().all(|&v| v.is_finite() && v >= 0.0) {
        Ok(())
    } else {
        Err((field, "must be finite and not negative"))
    }
}
