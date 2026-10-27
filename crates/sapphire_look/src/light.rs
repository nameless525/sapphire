//! Light: the key light and the bodies that give it, ambient light, and a character's light rig
//! as time of day and the world resolve them.

use glam::{Quat, Vec3};

use crate::SkyGradient;

/// A light from infinitely far away: the key light (the frame packet's `sun`), or the sun or moon
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
/// and the Milky Way turned by [`Self::sphere`] (W2); how they look is the frame packet's
/// `sky_layers` ([`SkyLayers`](crate::SkyLayers)).
///
/// The key light (the frame packet's `sun`) is resolved from the bodies by time of day: the sun's
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
    /// [`CharacterLight`], scaled by the preset). It replaces the world's key light (the frame
    /// packet's `sun`) for the character's light split and rim.
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
/// light (the frame packet's `ambient`) seen through normals flattened toward low frequency, then
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
