//! The benchmark's cast (M5 B1's placeholder; M5 B3 builds the cast on it): figures built from specs, each in
//! the anime character family, as many copies of each as a preset says.
//!
//! `bench/cast/` holds:
//!
//! | File | What |
//! |---|---|
//! | `cast.ron` | [`BenchCast`]: each preset's scale |
//! | `figures/<name>.figure.ron` | Figure specs ([`crate::figure`]), whose `dir` the cast replaces with its own |
//!
//! It generates, for copy `c` of each spec, under `cast/<name>_NN/` in the output: the files
//! [`figure::files`] makes (the model, its palette, control map, layer mask and matcap), the ILM document
//! that packs the control map, and an anime character material with a region for each of the spec's,
//! which the model's sidecar binds to its slot. Copies are the same bytes under different IDs: as many
//! meshes and textures for a pool to hold as distinct characters would be.

use std::path::Path;

use bevy_reflect::TypePath;
use sapphire_assets::material::{AnimeCharacterMaterial, CharacterRegion, Matcap, MaterialDocument};
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use super::{BenchError, Preset, Presets, Source, asset_id, document_bytes, read_document};
use crate::character_maps::MapKind;
use crate::figure::{self, Figure};
use crate::material::MaterialSettings;
use crate::meta::{ImportSettings, MeshSettings};
use crate::texture::{ColorSettings, MaskChannels, MaskSettings, TextureClass, TextureSettings};

/// This part's folder, under `bench/` and in the output.
pub const PART: &str = "cast";

/// The cast's document, `bench/cast/cast.ron` (`sapphire::pipeline::BenchCast`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct BenchCast {
    pub presets: Presets<CastScale>,
}

impl Versioned for BenchCast {
    /// 1: M5 B1.
    const VERSION: u32 = 1;
}

/// What a preset makes of the cast.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CastScale {
    /// Copies of each figure.
    pub copies: u32,
}

/// The most copies of a figure.
const MAX_COPIES: u32 = 100;

/// Generates the cast from the documents in `dir` (`bench/cast/`) at `preset`'s scale.
pub fn generate(dir: &Path, preset: Preset) -> Result<Vec<Source>, BenchError> {
    let document_path = dir.join("cast.ron");
    let cast: BenchCast = read_document(&document_path)?;
    let scale = *cast.presets.get(preset);
    if !(1..=MAX_COPIES).contains(&scale.copies) {
        let why = format!("{preset}: 1 to {MAX_COPIES} copies");
        return Err(BenchError::Invalid { path: document_path, why });
    }
    let mut sources = Vec::new();
    for path in figure::specs(&dir.join("figures"))? {
        let spec = figure::read(&path)?;
        for copy in 0..scale.copies {
            let folder = format!("{}_{copy:02}", spec.name);
            figure_sources(&Figure { dir: folder, ..spec.clone() }, &path, &mut sources)?;
        }
    }
    Ok(sources)
}

/// A figure's files, the ILM document over its control map, and its material.
fn figure_sources(figure: &Figure, spec: &Path, sources: &mut Vec<Source>) -> Result<(), BenchError> {
    let path = |suffix: &str| format!("{}/{}{suffix}", figure.dir, figure.name);
    let id = |suffix: &str| asset_id(PART, &path(suffix));
    let color =
        || ImportSettings::Texture(TextureSettings { class: TextureClass::Color(ColorSettings::default()) });
    let mask = MaskSettings { channels: MaskChannels::Four, ..MaskSettings::default() };

    let material = AnimeCharacterMaterial {
        base_color: [1.0; 4],
        base_color_map: Some(id("_palette.png")),
        control_map: Some(id(".ilm.ron")),
        regions: vec![CharacterRegion::default(); figure.regions.len()],
        matcap: Some(Matcap { map: id("_matcap.png"), strength: 1.0, shadow_strength: 0.55 }),
        layer_mask: Some(id("_layers.png")),
        ..AnimeCharacterMaterial::default()
    };
    material.validate().map_err(|error| BenchError::Invalid {
        path: spec.to_owned(),
        why: format!("its material: {error:#}"),
    })?;

    for (file, bytes) in figure::files(figure)? {
        let importer = match file.strip_prefix(&path("")).unwrap_or(&file) {
            ".glb" => Some(ImportSettings::Gltf(MeshSettings {
                materials: [(figure.name.clone(), id(".material.ron"))].into(),
                ..MeshSettings::default()
            })),
            "_palette.png" | "_matcap.png" => Some(color()),
            "_layers.png" => {
                Some(ImportSettings::Texture(TextureSettings { class: TextureClass::Mask(mask) }))
            }
            // The control map is the ILM document's input, not a texture of its own.
            _ => None,
        };
        sources.push(match importer {
            Some(importer) => Source::asset(file, bytes, importer),
            None => Source::input(file, bytes),
        });
    }
    sources.push(Source::asset(
        path(".ilm.ron"),
        ilm_document(&format!("{}_control.png", figure.name)),
        ImportSettings::CharacterMap(MapKind::Ilm.default_settings()),
    ));
    sources.push(Source::asset(
        path(".material.ron"),
        document_bytes(&material),
        ImportSettings::Material(MaterialSettings::default()),
    ));
    Ok(())
}

/// The ILM document that packs a figure's control map, channel for channel, as a figure's own does.
fn ilm_document(control: &str) -> Vec<u8> {
    let channel = |name: &str, channel: &str| {
        format!("        {name}: Image(path: {control:?}, channel: {channel}),\n")
    };
    let mut text =
        String::from("(\n    type: \"sapphire::pipeline::IlmMap\",\n    version: 1,\n    data: (\n");
    text += &channel("specular_mask", "R");
    text += &channel("shadow_bias", "G");
    text += &channel("specular_threshold", "B");
    text += &channel("material_id", "A");
    text += "    ),\n)\n";
    text.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character_maps::ilm::IlmMap;

    #[test]
    fn the_ilm_document_reads() {
        let text = String::from_utf8(ilm_document("figure_control.png")).unwrap();
        sapphire_serialize::from_ron::<IlmMap>(&text).unwrap();
    }

    #[test]
    fn copies_have_ids_of_their_own() {
        assert_ne!(asset_id(PART, "a_00/a.glb"), asset_id(PART, "a_01/a.glb"));
    }
}
