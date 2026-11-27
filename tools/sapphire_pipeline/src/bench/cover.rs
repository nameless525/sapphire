//! The benchmark's ground cover (M6 G6b): the species in `bench/cover/`, each built at its variants by
//! [`crate::tree::cover`], for the nature zone's foliage to place (M6 G2b) beside the trees.
//!
//! `cover.ron` (`sapphire::pipeline::BenchCover`) lists the species, each a spec in `species/`, and each
//! preset's atlas size. Every species writes its own folder, the same at every preset but its atlas's size
//! and on every cook:
//!
//! | File (in `<species>/`) | What |
//! |---|---|
//! | `<species>_<variant>.glb` | A variant's model, cast as ground cover (`budget: (role: Cover)`) so `sapphire_pipeline check` holds it to the cover row; its sidecar binds its materials and, for a plant, bakes its foliage |
//! | `leaves.png`, `stone.png` | A plant's painted atlas, capped to the cover row's 256 a side on the phones, or a rock's swatches; a grass clump has none |
//! | `<slot>.material.ron` | The materials its slots draw with: foliage for a plant, the environment family for a rock |
//!
//! A model is found by its path's ID, [`super::asset_id`]`("cover", "<species>/<species>_<variant>.glb")`, as
//! any part names another's assets.

use std::num::NonZeroU32;
use std::path::Path;

use bevy_reflect::TypePath;
use sapphire_assets::material::MaterialDocument;
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use super::{BenchError, Preset, Presets, Source, asset_id, document_bytes, read_document};
use crate::check::{Budget, Role};
use crate::cook::Cook;
use crate::material::MaterialSettings;
use crate::meta::{ImportSettings, MeshSettings};
use crate::optimize::LodSettings;
use crate::texture::{ColorSettings, MaxSize, TextureClass, TextureSettings};
use crate::tree::cover::{self, CoverMaterial, CoverSpecies};

/// This part's folder, under `bench/` and in the output.
pub const PART: &str = "cover";

/// The ground cover's document, `bench/cover/cover.ron` (`sapphire::pipeline::BenchCover`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct BenchCover {
    /// Each species' spec, `species/<name>.cover.ron`, by name.
    pub species: Vec<String>,
    pub presets: Presets<CoverScale>,
}

impl Versioned for BenchCover {
    const VERSION: u32 = 1;
}

/// What a preset makes of the ground cover.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverScale {
    /// The plants' atlases' side: a power of two from 64 to 1024.
    pub atlas: u32,
}

/// The longest side the cover row lets a phone's map store: an atlas over it is capped there.
const PHONE_ATLAS: u32 = 256;

/// A species' folder's path to its file `name`.
fn path(species: &CoverSpecies, name: &str) -> String {
    format!("{}/{name}", species.name)
}

/// A variant's model's path.
pub fn model_path(species: &CoverSpecies, variant: usize) -> String {
    path(species, &format!("{}_{}.glb", species.name, species.variants[variant].name))
}

/// Every species `dir` (`bench/cover/`) lists, read and checked, in its order.
pub fn species(dir: &Path) -> Result<Vec<CoverSpecies>, BenchError> {
    let document = dir.join("cover.ron");
    let cover: BenchCover = read_document(&document)?;
    let mut species: Vec<CoverSpecies> = Vec::new();
    for name in &cover.species {
        let read = cover::read(&dir.join("species").join(format!("{name}{}", cover::SPEC_EXTENSION)))?;
        if read.name != *name || species.iter().any(|other| other.name == read.name) {
            return Err(BenchError::Invalid {
                path: document.clone(),
                why: format!("species {name:?} is listed twice, or its spec names another"),
            });
        }
        species.push(read);
    }
    Ok(species)
}

/// Generates the ground cover from the documents in `dir` (`bench/cover/`) at `preset`'s atlas size: the same
/// for every cook.
pub fn generate(dir: &Path, preset: Preset, _cook: Cook) -> Result<Vec<Source>, BenchError> {
    let document = dir.join("cover.ron");
    let cover: BenchCover = read_document(&document)?;
    let atlas = cover.presets.get(preset).atlas;
    if !atlas.is_power_of_two() || !(64..=1024).contains(&atlas) {
        return Err(BenchError::Invalid {
            path: document,
            why: format!("{preset}'s atlas is {atlas}: a power of two from 64 to 1024"),
        });
    }
    let mut sources = Vec::new();
    for species in species(dir)? {
        species_sources(&species, atlas, &mut sources)?;
    }
    Ok(sources)
}

/// `species`' sources, a plant's atlas `atlas` a side.
pub fn species_sources(
    species: &CoverSpecies,
    atlas: u32,
    sources: &mut Vec<Source>,
) -> Result<(), BenchError> {
    let id = |name: &str| asset_id(PART, &path(species, name));
    let painted = species.atlas(atlas)?;
    let atlas_id = painted.as_ref().map(|painted| id(painted.name));
    let materials = species.materials(atlas_id);
    for variant in 0..species.variants.len() {
        let mesh = MeshSettings {
            lods: LodSettings::none(),
            materials: materials.iter().map(|(slot, name, _)| ((*slot).to_owned(), id(name))).collect(),
            foliage: species.foliage(variant),
            ..MeshSettings::default()
        };
        let model = Source::asset(
            model_path(species, variant),
            species.model_glb(variant),
            ImportSettings::Gltf(mesh),
        );
        sources.push(model.with_budget(Budget { role: Some(Role::Cover), waive: Vec::new() }));
    }
    if let Some(painted) = painted {
        let max_size = MaxSize {
            desktop: None,
            mobile: NonZeroU32::new(PHONE_ATLAS).filter(|cap| painted.side > cap.get()),
        };
        sources.push(Source::asset(
            path(species, painted.name),
            painted.png,
            ImportSettings::Texture(TextureSettings {
                class: TextureClass::Color(ColorSettings::default()),
                max_size,
            }),
        ));
    }
    for (_, name, material) in materials {
        let invalid = |error: sapphire_assets::material::MaterialError| BenchError::Invalid {
            path: path(species, name).into(),
            why: format!("{error:#}"),
        };
        let bytes = match &material {
            CoverMaterial::Foliage(material) => {
                material.validate().map_err(invalid)?;
                document_bytes(material)
            }
            CoverMaterial::Environment(material) => {
                material.validate().map_err(invalid)?;
                document_bytes(material)
            }
        };
        sources.push(Source::asset(
            path(species, name),
            bytes,
            ImportSettings::Material(MaterialSettings::default()),
        ));
    }
    Ok(())
}
