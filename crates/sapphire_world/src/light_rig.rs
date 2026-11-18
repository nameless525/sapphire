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
//!   local lights. Its camera rim (M1 C8, none by default) holds a rim light around the camera,
//!   behind and above the character as the camera sees it, which takes a share of the rim's
//!   gating from the key light: the gameplay preset's limits fold a key from behind round to the
//!   camera's side, and the rim, which the key gates, would follow it off the silhouette.
//! - **[`LightRigLibrary`]** (a `LightRigLibrary` document, schema 1): named presets, the first
//!   the default. [`default_rigs`] is the shipped one, compiled in until packs carry it as
//!   content: `gameplay` and `portrait`.
//! - **[`LightRigs`]** (an ECS resource): the library the world's characters are lit by.
//! - **Where they live (M6 T4):** the presets and the library are `sapphire_tod::light_rig`'s plain
//!   data, re-exported here, so an editor reflects them without the renderer; resolving them
//!   against a camera is this module's ([`ResolveRig`], [`ResolveRim`], [`KeyDirection`]).
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
//! renderer never sees presets). [`ResolveRig::resolve`] does it without time of day. Resolving
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
//! - **Presets as content** loaded from packs (A4), and pushed by live link (A9) through
//!   [`LightRigLibrary::from_ron`] as desktop hot reload already reloads this crate's document in
//!   development builds (A8); characters keep their [`LightRigId`]s, and an ID past a shorter
//!   library's end falls back to its default.
//!
//! [`FramePacket::characters`]: sapphire_render::FramePacket::characters

mod library;
#[cfg(test)]
mod tests;

use std::ops::Deref;

use bevy_ecs::prelude::{Component, Resource};
use sapphire_math::det::{atan2, sin_cos, slerp_direction};
use sapphire_math::{Vec3, Vec4};
use sapphire_render::{
    CameraData, CameraRim, CharacterGi, CharacterLight, CharacterLighting, CharacterShade, DirectionalLight,
};
pub use sapphire_tod::light_rig::{
    CameraAngles, LightRig, LightRigError, LightRigGi, LightRigId, LightRigKey, LightRigLibrary, LightRigRim,
    NamedLightRig,
};

pub use library::default_rigs;

/// The library of presets the world's characters are lit by, as an ECS resource: the library is
/// `sapphire_tod`'s plain data, which keeps no ECS. It reads as the library it holds.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct LightRigs(pub LightRigLibrary);

impl Deref for LightRigs {
    type Target = LightRigLibrary;

    fn deref(&self) -> &LightRigLibrary {
        &self.0
    }
}

/// Which preset a character's instances are lit by, and where its head is: an ECS component
/// for the app's extraction, which resolves it into the frame packet each frame.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct CharacterLightRig {
    pub rig: LightRigId,
    /// The center of the head in the entity's own space: the sphere face and hair GI normals
    /// turn toward, and the point the key's limits are judged around. Whatever poses a character
    /// keeps it current from its head bone (the sandbox's clip player does, M2 N2).
    pub head: Vec3,
}

/// Resolves a [`LightRig`] (a trait since M6 T4 moved the preset to `sapphire_tod` as plain data).
pub trait ResolveRig {
    /// This preset's lighting for a character whose head is at `head` (world space), seen by
    /// `camera`, under the world's `key_light` and time of day's `character_light`, its camera
    /// rim (M1 C8) placed around the camera. Its shadow side takes the key light's own color:
    /// time of day's shadow hue (M1 C8) comes with
    /// [`TodFrame::character_lighting`](crate::tod::TodFrame::character_lighting).
    fn resolve(
        &self,
        key_light: &DirectionalLight,
        character_light: &CharacterLight,
        camera: &CameraData,
        head: Vec3,
    ) -> CharacterLighting;
}

impl ResolveRig for LightRig {
    fn resolve(
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
            shade: CharacterShade::default(),
            camera_rim: self.camera_rim.resolve(camera, head),
        }
    }
}

/// Resolves a [`LightRigRim`].
pub trait ResolveRim {
    /// The rim light for a character whose head is at `head`, seen by `camera`: its direction
    /// placed around the camera's view of the head. Without a share it is none
    /// ([`CameraRim::default`]), which the camera doesn't move.
    fn resolve(&self, camera: &CameraData, head: Vec3) -> CameraRim;
}

impl ResolveRim for LightRigRim {
    fn resolve(&self, camera: &CameraData, head: Vec3) -> CameraRim {
        if self.share <= 0.0 {
            return CameraRim::default();
        }
        let toward = CameraYaw::of(camera, head).toward(self.direction.azimuth, self.direction.elevation);
        CameraRim { direction: -toward, share: self.share }
    }
}

/// Resolves a [`LightRigKey`].
pub trait KeyDirection {
    /// The direction the rig's key light travels for a character whose head is at `head`, seen
    /// by `camera`, given the direction the world's key light travels. Exactly `world` when the
    /// key follows it and stays within the limits.
    fn direction(&self, world: Vec3, camera: &CameraData, head: Vec3) -> Vec3;
}

impl KeyDirection for LightRigKey {
    fn direction(&self, world: Vec3, camera: &CameraData, head: Vec3) -> Vec3 {
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
