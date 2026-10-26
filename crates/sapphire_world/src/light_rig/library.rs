//! [`LightRigLibrary`]: named presets, as a document.

use bevy_ecs::prelude::Resource;
use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use super::LightRig;

/// The shipped presets (see [`LightRigLibrary::default_rigs`]).
const DEFAULT_RIGS: &str = include_str!("../../default_light_rigs.ron");

/// Named character light rig presets (a `LightRigLibrary` document, schema 1); the first is the
/// default. An ECS resource, which characters name presets in by [`LightRigId`].
///
/// A library has at least one preset, its names are distinct and not empty, and every preset is
/// in range: it is valid however it was made, deserialization included.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath, Resource)]
#[type_path = "sapphire::world"]
#[serde(try_from = "UncheckedLibrary")]
pub struct LightRigLibrary {
    rigs: Vec<NamedLightRig>,
}

impl Versioned for LightRigLibrary {
    const VERSION: u32 = 1;
}

/// A preset and the name a library knows it by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
        for (index, NamedLightRig { name, rig }) in rigs.iter().enumerate() {
            if name.is_empty() {
                return Err(LightRigError::EmptyName(index));
            }
            if rigs[..index].iter().any(|earlier| earlier.name == *name) {
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

    /// Sapphire's presets (`default_light_rigs.ron` beside this crate's manifest), compiled in:
    /// `gameplay`, the default, which follows the sun and moon within limits that keep faces lit
    /// from the camera's side of the head and from above the horizon, and `portrait`, a key
    /// fixed to the camera for dialogue, menus and photo shots.
    pub fn default_rigs() -> Self {
        Self::from_ron(DEFAULT_RIGS).expect("the shipped presets are checked by this module's tests")
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
