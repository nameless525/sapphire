//! The character light rig's presets as plain data (M1 C2; M6 T4 moved them here): [`LightRig`] and
//! its parts, and [`LightRigLibrary`], a document of named presets. They live beside the day's keys,
//! which they scale, so an editor reflects and checks them without the renderer.
//! `sapphire_world::light_rig` resolves a preset against a camera into a frame's character lighting
//! and re-exports all of this; its docs say what each field does.
//!
//! The reflected types keep the stable names they had in `sapphire_world` (`sapphire::world::…`),
//! which documents' envelopes name.

use std::collections::BTreeSet;

use bevy_reflect::Reflect;
use sapphire_look::{CharacterLighting, DirectionalLight};
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::Rgb;

/// A character light rig preset. Every field left out of a document takes its neutral value: the
/// world's key light with no limits, time of day's character key as it is, unstyled GI, and no
/// camera rim.
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
    /// A rim light held around the camera (M1 C8): none by default, the key light's rim.
    pub camera_rim: LightRigRim,
}

impl Default for LightRig {
    fn default() -> Self {
        let neutral = CharacterLighting::unstyled(DirectionalLight::default());
        Self {
            key: LightRigKey::default(),
            gi: LightRigGi::default(),
            rim: neutral.rim,
            local_light_ceiling: neutral.local_light_ceiling,
            camera_rim: LightRigRim::default(),
        }
    }
}

/// A rig's rim light held around the camera (M1 C8; [`sapphire_look::CameraRim`] says what it does):
/// where it comes from as the camera sees the character, and its share of the rim's gating.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(default, deny_unknown_fields)]
pub struct LightRigRim {
    /// 0 to 1: how much of the rim's light gate and back-light weighting take this light rather
    /// than the key light. 0 keeps the key light's rim exactly.
    pub share: f32,
    /// Where it comes from around the character as the camera sees it.
    pub direction: CameraAngles,
}

impl Default for LightRigRim {
    /// None, from behind the character on the camera's right and above, where a rim light
    /// stands.
    fn default() -> Self {
        Self { share: 0.0, direction: CameraAngles { azimuth: 150.0, elevation: 35.0 } }
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

/// A rig's stylized GI ([`sapphire_look::CharacterGi`] says what each step does). Faces and hair take
/// the sphere around the head, and skin the skin tint, by their material regions' weights.
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
        non_negative(self.local_light_ceiling, "local_light_ceiling")?;
        let rim = &self.camera_rim;
        within(rim.share, 0.0, 1.0, "camera_rim.share", UNIT)?;
        within(rim.direction.azimuth, -180.0, 180.0, "camera_rim.direction.azimuth", azimuth)?;
        within(rim.direction.elevation, -90.0, 90.0, "camera_rim.direction.elevation", ELEVATION)
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

/// Named character light rig presets (a `LightRigLibrary` document, schema 1); the first is the
/// default. Characters name presets in it by [`LightRigId`]; the world holds it as a resource
/// (`sapphire_world::light_rig::LightRigs`).
///
/// A library has at least one preset, its names are distinct and not empty, and every preset is
/// in range: it is valid however it was made, deserialization included. It reflects (M6 T4) so an
/// editor can open it as a document, whose every edit is to the text and read back through
/// [`Self::from_ron`]'s checks; reflection's own `FromReflect` would build one unchecked, so nothing
/// else should build one by it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(try_from = "UncheckedLibrary")]
pub struct LightRigLibrary {
    rigs: Vec<NamedLightRig>,
}

impl Versioned for LightRigLibrary {
    const VERSION: u32 = 1;
}

/// A preset and the name a library knows it by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct NamedLightRig {
    pub name: String,
    pub rig: LightRig,
}

/// A preset of a [`LightRigLibrary`], by its place in the library.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LightRigId(pub u16);

impl LightRigId {
    /// The library's first preset.
    pub const DEFAULT: Self = Self(0);
}

/// A library that isn't valid.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum LightRigError {
    #[error("there are no presets")]
    Empty,
    #[error("preset {0}: a name can't be empty")]
    EmptyName(usize),
    #[error("preset `{0}` appears twice")]
    DuplicateName(String),
    #[error("more presets than a light rig ID can name")]
    TooMany,
    #[error("preset `{name}`: `{field}` {problem}")]
    Invalid { name: String, field: &'static str, problem: &'static str },
}

/// A library as written, before [`LightRigLibrary::new`] checks it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedLibrary {
    rigs: Vec<NamedLightRig>,
}

impl TryFrom<UncheckedLibrary> for LightRigLibrary {
    type Error = LightRigError;

    fn try_from(library: UncheckedLibrary) -> Result<Self, LightRigError> {
        Self::new(library.rigs)
    }
}

impl LightRigLibrary {
    /// A library of `rigs`, checked: at least one, distinct non-empty names, every preset in
    /// range.
    pub fn new(rigs: Vec<NamedLightRig>) -> Result<Self, LightRigError> {
        if rigs.is_empty() {
            return Err(LightRigError::Empty);
        }
        if rigs.len() > usize::from(u16::MAX) + 1 {
            return Err(LightRigError::TooMany);
        }
        let mut names = BTreeSet::new();
        for (index, NamedLightRig { name, rig }) in rigs.iter().enumerate() {
            if name.is_empty() {
                return Err(LightRigError::EmptyName(index));
            }
            if !names.insert(name.as_str()) {
                return Err(LightRigError::DuplicateName(name.clone()));
            }
            rig.validate().map_err(|(field, problem)| LightRigError::Invalid {
                name: name.clone(),
                field,
                problem,
            })?;
        }
        Ok(Self { rigs })
    }

    /// Reads a library document, upgrading older schema versions and checking it.
    pub fn from_ron(text: &str) -> Result<Self, DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    pub fn to_ron(&self) -> Result<String, DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// The preset called `name`.
    pub fn find(&self, name: &str) -> Option<LightRigId> {
        let index = self.rigs.iter().position(|rig| rig.name == name)?;
        Some(LightRigId(index as u16))
    }

    /// Preset `id`, or the default for an ID past the library's end (a library replaced by a
    /// shorter one, in hot reload).
    pub fn rig(&self, id: LightRigId) -> &LightRig {
        &self.rigs.get(usize::from(id.0)).unwrap_or(&self.rigs[0]).rig
    }

    /// The name of preset `id`, if the library has it.
    pub fn name(&self, id: LightRigId) -> Option<&str> {
        self.rigs.get(usize::from(id.0)).map(|rig| rig.name.as_str())
    }

    /// The presets' names, in order: [`LightRigId`] `i` names the `i`th.
    pub fn names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.rigs.iter().map(|rig| rig.name.as_str())
    }
}
