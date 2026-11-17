//! The benchmark's trees (M6 G6a): the species in `bench/trees/`, grown at each of their ages by
//! [`crate::tree`], for the nature zone's forest to place (M6 G2b).
//!
//! `trees.ron` (`sapphire::pipeline::BenchTrees`) lists the species, each a spec in `species/`, and each
//! preset's atlas size. Every species writes its own folder, the same at every preset but its atlas's size
//! and on every cook:
//!
//! | File (in `<species>/`) | What |
//! |---|---|
//! | `<species>_<age>.glb` | An age's model ([`crate::tree::mesh`]), cast as a tree (`budget: (role: Tree)`) so `sapphire_pipeline check` holds it to the tree row; its sidecar binds the two materials and bakes its leaves' normals from its clumps |
//! | `leaves.png` | The leaf atlas ([`crate::tree::atlas`]), capped to the tree row's 512 a side on the phones |
//! | `bark.material.ron`, `leaves.material.ron` | The foliage materials every age draws with |
//!
//! A tree is found by its path's ID, [`super::asset_id`]`("trees", "<species>/<species>_<age>.glb")`, as
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
use crate::tree::{self, BARK_SLOT, LEAVES_SLOT, TreeSpecies};

/// This part's folder, under `bench/` and in the output.
pub const PART: &str = "trees";

/// The trees' document, `bench/trees/trees.ron` (`sapphire::pipeline::BenchTrees`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct BenchTrees {
    /// Each species' spec, `species/<name>.tree.ron`, by name.
    pub species: Vec<String>,
    pub presets: Presets<TreeScale>,
}

impl Versioned for BenchTrees {
    const VERSION: u32 = 1;
}

/// What a preset makes of the trees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeScale {
    /// The leaf atlases' side: a power of two from 64 to 2048.
    pub atlas: u32,
}

/// The longest side the tree row lets a phone's map store: an atlas over it is capped there.
const PHONE_ATLAS: u32 = 512;

/// A species' folder's path to its file `name`.
fn path(species: &TreeSpecies, name: &str) -> String {
    format!("{}/{name}", species.name)
}

/// An age's model's path.
pub fn model_path(species: &TreeSpecies, age: usize) -> String {
    path(species, &format!("{}_{}.glb", species.name, species.ages[age].name))
}

/// Every species `dir` (`bench/trees/`) lists, read and checked, in its order.
pub fn species(dir: &Path) -> Result<Vec<TreeSpecies>, BenchError> {
    let document = dir.join("trees.ron");
    let trees: BenchTrees = read_document(&document)?;
    let mut species: Vec<TreeSpecies> = Vec::new();
    for name in &trees.species {
        let read = tree::read(&dir.join("species").join(format!("{name}{}", tree::SPEC_EXTENSION)))?;
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

/// Generates the trees from the documents in `dir` (`bench/trees/`) at `preset`'s atlas size: the same for
/// every cook.
pub fn generate(dir: &Path, preset: Preset, _cook: Cook) -> Result<Vec<Source>, BenchError> {
    let document = dir.join("trees.ron");
    let trees: BenchTrees = read_document(&document)?;
    let atlas = trees.presets.get(preset).atlas;
    if !atlas.is_power_of_two() || !(64..=2048).contains(&atlas) {
        return Err(BenchError::Invalid {
            path: document,
            why: format!("{preset}'s atlas is {atlas}: a power of two from 64 to 2048"),
        });
    }
    let mut sources = Vec::new();
    for species in species(dir)? {
        species_sources(&species, atlas, &mut sources)?;
    }
    Ok(sources)
}

/// `species`' sources, its atlas `atlas` a side.
pub fn species_sources(
    species: &TreeSpecies,
    atlas: u32,
    sources: &mut Vec<Source>,
) -> Result<(), BenchError> {
    let id = |name: &str| asset_id(PART, &path(species, name));
    let atlas_id = id("leaves.png");
    let materials = [(BARK_SLOT, id("bark.material.ron")), (LEAVES_SLOT, id("leaves.material.ron"))];
    for age in 0..species.ages.len() {
        let skeleton = tree::grow::grow(species, age);
        let mesh = MeshSettings {
            lods: LodSettings::none(),
            materials: materials.iter().map(|&(slot, id)| (slot.to_owned(), id)).collect(),
            foliage: Some(species.foliage(age)),
            ..MeshSettings::default()
        };
        let model = Source::asset(
            model_path(species, age),
            tree::mesh::tree_glb(species, age, &skeleton),
            ImportSettings::Gltf(mesh),
        );
        sources.push(model.with_budget(Budget { role: Some(Role::Tree), waive: Vec::new() }));
    }
    let max_size =
        MaxSize { desktop: None, mobile: NonZeroU32::new(PHONE_ATLAS).filter(|cap| atlas > cap.get()) };
    sources.push(Source::asset(
        path(species, "leaves.png"),
        tree::atlas::atlas_png(species, atlas)?,
        ImportSettings::Texture(TextureSettings {
            class: TextureClass::Color(ColorSettings::default()),
            max_size,
        }),
    ));
    for (name, material) in [
        ("bark.material.ron", species.bark_material()),
        ("leaves.material.ron", species.leaves_material(atlas_id)),
    ] {
        material.validate().map_err(|error| BenchError::Invalid {
            path: path(species, name).into(),
            why: format!("{error:#}"),
        })?;
        sources.push(Source::asset(
            path(species, name),
            document_bytes(&material),
            ImportSettings::Material(MaterialSettings::default()),
        ));
    }
    Ok(())
}
