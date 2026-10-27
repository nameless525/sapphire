//! [`EffectLibrary`]: named effect presets, as a document.

use bevy_ecs::prelude::Resource;
use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_render::{PostEffects, PostVariants};
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use super::layer::EffectLayer;

/// The shipped presets (see [`EffectLibrary::default_effects`]).
const DEFAULT_EFFECTS: &str = include_str!("../../default_screen_effects.ron");

/// The most layers a preset has: a push always fits a stack that has room for it, and an impact
/// frame needs one per kind at most, twice for a double flash.
pub const MAX_PRESET_LAYERS: usize = 8;

/// A preset: the layers a push starts together, by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectPreset {
    pub name: String,
    pub layers: Vec<EffectLayer>,
}

impl EffectPreset {
    /// The post variants its layers draw through, each at full weight, alone or together: what
    /// the renderer prewarms so a push shows on the frame it fires
    /// ([`Renderer::prewarm_post`](sapphire_render::Renderer::prewarm_post)).
    pub fn post_variants(&self) -> PostVariants {
        self.layers.iter().fold(PostVariants::NONE, |variants, layer| {
            let mut effects = PostEffects::NONE;
            layer.effect.blend_over(&mut effects, 1.0, 0);
            variants | PostVariants::of_effects(&effects)
        })
    }
}

/// A preset of an [`EffectLibrary`], by its place in the library.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct EffectPresetId(pub u16);

/// Named screen effect presets (an `EffectLibrary` document, schema 1). An ECS resource; the
/// overlay and the sandbox fire its presets by name.
///
/// A library has at least one preset, its names are distinct and not empty, and each preset has
/// one to [`MAX_PRESET_LAYERS`] valid layers: it is valid however it was made, deserialization
/// included.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath, Resource)]
#[type_path = "sapphire::world"]
#[serde(try_from = "UncheckedLibrary")]
pub struct EffectLibrary {
    presets: Vec<EffectPreset>,
}

impl Versioned for EffectLibrary {
    const VERSION: u32 = 1;
}

/// A library that isn't valid.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum EffectLibraryError {
    #[error("there are no presets")]
    Empty,
    #[error("preset {0}: a name can't be empty")]
    EmptyName(usize),
    #[error("preset `{0}` appears twice")]
    DuplicateName(String),
    #[error("more presets than a preset ID can name")]
    TooMany,
    #[error("preset `{0}`: a preset has between 1 and {MAX_PRESET_LAYERS} layers")]
    LayerCount(String),
    #[error("preset `{name}`, layer {layer}: `{field}` {problem}")]
    Invalid { name: String, layer: usize, field: &'static str, problem: &'static str },
}

/// A library as written, before [`EffectLibrary::new`] checks it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedLibrary {
    presets: Vec<EffectPreset>,
}

impl TryFrom<UncheckedLibrary> for EffectLibrary {
    type Error = EffectLibraryError;

    fn try_from(library: UncheckedLibrary) -> Result<Self, EffectLibraryError> {
        Self::new(library.presets)
    }
}

impl EffectLibrary {
    /// A library of `presets`, checked.
    pub fn new(presets: Vec<EffectPreset>) -> Result<Self, EffectLibraryError> {
        if presets.is_empty() {
            return Err(EffectLibraryError::Empty);
        }
        if presets.len() > usize::from(u16::MAX) + 1 {
            return Err(EffectLibraryError::TooMany);
        }
        for (index, EffectPreset { name, layers }) in presets.iter().enumerate() {
            if name.is_empty() {
                return Err(EffectLibraryError::EmptyName(index));
            }
            if presets[..index].iter().any(|earlier| earlier.name == *name) {
                return Err(EffectLibraryError::DuplicateName(name.clone()));
            }
            if layers.is_empty() || layers.len() > MAX_PRESET_LAYERS {
                return Err(EffectLibraryError::LayerCount(name.clone()));
            }
            for (layer, effect) in layers.iter().enumerate() {
                effect.validate().map_err(|(field, problem)| EffectLibraryError::Invalid {
                    name: name.clone(),
                    layer,
                    field,
                    problem,
                })?;
            }
        }
        Ok(Self { presets })
    }

    /// Sapphire's sample presets (`default_screen_effects.ron` beside this crate's manifest),
    /// compiled in: one per effect (`radial_blur`, `chromatic_aberration`, `inversion`,
    /// `duotone`, `speed_lines`) and `impact_frame`, all of them at once as a heavy hit lands.
    pub fn default_effects() -> Self {
        Self::from_ron(DEFAULT_EFFECTS).expect("the shipped presets are checked by this module's tests")
    }

    /// Reads a library document, upgrading older schema versions and checking it.
    pub fn from_ron(text: &str) -> Result<Self, DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    pub fn to_ron(&self) -> Result<String, DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// The preset called `name`.
    pub fn find(&self, name: &str) -> Option<EffectPresetId> {
        let index = self.presets.iter().position(|preset| preset.name == name)?;
        Some(EffectPresetId(index as u16))
    }

    /// Preset `id`, if the library has it.
    pub fn preset(&self, id: EffectPresetId) -> Option<&EffectPreset> {
        self.presets.get(usize::from(id.0))
    }

    /// The presets, in order: [`EffectPresetId`] `i` is the `i`th.
    pub fn presets(&self) -> &[EffectPreset] {
        &self.presets
    }

    /// The presets' names, in order.
    pub fn names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.presets.iter().map(|preset| preset.name.as_str())
    }

    /// The post variants every preset draws through, alone or with others playing: what a game
    /// that may push any of them prewarms at load
    /// ([`Renderer::prewarm_post`](sapphire_render::Renderer::prewarm_post)), so the first push
    /// of each shows on the frame it fires rather than after a compile stall.
    pub fn post_variants(&self) -> PostVariants {
        self.presets.iter().fold(PostVariants::NONE, |variants, preset| variants | preset.post_variants())
    }
}
