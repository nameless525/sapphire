//! Import presets (M6 C5): a sidecar's starting settings for a kind of source, copied into the
//! sidecar when it is made.
//!
//! An `ImportPreset` document (`<name>.preset.ron`, version 1) holds a sidecar's body without its ID:
//! an importer with its settings, and optionally the `budget` `check` holds the source to
//! ([`crate::check`]). Its importer names the kind of source it is for: a `Vrm((…))` preset starts a
//! VRM's sidecar and nothing else's.
//!
//! ```ron
//! (
//!     type: "sapphire::pipeline::ImportPreset",
//!     version: 1,
//!     data: (
//!         importer: Gltf((lods: (levels: [(triangle_ratio: 0.5, max_error: 0.002)], mobile_skip: 0))),
//!         budget: (role: Enemy),
//!     ),
//! )
//! ```
//!
//! **Copied, never inherited** (m6-plan §3, "The pipeline's shape"): a preset is read once, when a
//! sidecar is made, and the sidecar keeps what it was given. No cache key reads a preset, and editing
//! one changes no sidecar made before it, so a key still reads its source and its sidecar alone.
//!
//! **The built-in presets** are this crate's `presets/` documents, compiled in ([`BUILT_IN`]): a
//! character of each kind of model, `vrm-character`, `gltf-character` and `fbx-character`. Their mesh
//! settings are the cook's own defaults, which are a character's, and they cast it as a hero.
//! `new-character` starts a model's sidecar from its kind's ([`for_model`]), or from a document
//! `--preset` names. A cook making a sidecar for a new source ([`crate::meta::resolve`]) starts a VRM's
//! from `vrm-character`, since a VRM is always a character; a new glTF or FBX may be anything (a prop,
//! a zone's piece, a weapon), so it starts from the importer's defaults, as every other source does.
//!
//! What only the file can tell is the file's, not the preset's: a VRM sidecar's `version` is read from
//! the VRM ([`ImportSettings::for_new_sidecar`]) whatever the preset says.

use std::fs;
use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use sapphire_assets::AssetId;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::check::Budget;
use crate::meta::{AssetMeta, ImportSettings};

/// The end of a preset document's file name.
pub const SUFFIX: &str = ".preset.ron";

/// The built-in presets, by name: this crate's `presets/` documents.
pub const BUILT_IN: [(&str, &str); 3] = [
    ("vrm-character", include_str!("../presets/vrm-character.preset.ron")),
    ("gltf-character", include_str!("../presets/gltf-character.preset.ron")),
    ("fbx-character", include_str!("../presets/fbx-character.preset.ron")),
];

/// An `ImportPreset` document: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct ImportPreset {
    /// The importer, with the settings a new sidecar starts with: which kind of source it is for.
    pub importer: ImportSettings,
    /// What `check` holds the source to, if anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
}

impl Versioned for ImportPreset {
    const VERSION: u32 = 1;
}

/// Errors from reading or applying a preset.
#[derive(Debug, thiserror::Error)]
pub enum PresetError {
    #[error("failed to read the preset {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} is not a valid import preset", path.display())]
    Document {
        path: PathBuf,
        #[source]
        source: Box<DocumentError>,
    },
    #[error("no built-in preset is called {name:?}: there are {}", names())]
    Unknown { name: String },
    #[error("{} isn't a source any importer reads", .0.display())]
    NotASource(PathBuf),
    #[error("the preset {preset} is for {wanted} sources, but {} is read as {found}", file.display())]
    WrongKind { preset: String, wanted: &'static str, file: PathBuf, found: &'static str },
}

fn names() -> String {
    BUILT_IN.map(|(name, _)| name).join(", ")
}

impl ImportPreset {
    /// The built-in preset `name` ([`BUILT_IN`]).
    pub fn built_in(name: &str) -> Result<Self, PresetError> {
        let (_, text) = BUILT_IN
            .iter()
            .find(|(found, _)| *found == name)
            .ok_or_else(|| PresetError::Unknown { name: name.to_owned() })?;
        sapphire_serialize::from_ron(text).map_err(|source| PresetError::Document {
            path: PathBuf::from(format!("presets/{name}{SUFFIX}")),
            source: Box::new(source),
        })
    }

    /// Reads the preset document at `path`.
    pub fn read(path: &Path) -> Result<Self, PresetError> {
        let text =
            fs::read_to_string(path).map_err(|source| PresetError::Read { path: path.to_owned(), source })?;
        sapphire_serialize::from_ron(&text)
            .map_err(|source| PresetError::Document { path: path.to_owned(), source: Box::new(source) })
    }

    /// A new sidecar for the source at `source`, with `id`: this preset's settings and budget, and what
    /// only the file can tell (a VRM's version). `name` names the preset in the error for a source of
    /// another kind.
    pub fn sidecar(&self, name: &str, source: &Path, id: AssetId) -> Result<AssetMeta, PresetError> {
        let found = ImportSettings::for_new_sidecar(source)
            .ok_or_else(|| PresetError::NotASource(source.to_owned()))?;
        if !found.same_importer(&self.importer) {
            return Err(PresetError::WrongKind {
                preset: name.to_owned(),
                wanted: self.importer.importer_name(),
                file: source.to_owned(),
                found: found.importer_name(),
            });
        }
        let mut importer = self.importer.clone();
        if let (ImportSettings::Vrm(settings), ImportSettings::Vrm(file)) = (&mut importer, &found) {
            settings.version = file.version;
        }
        Ok(AssetMeta { budget: self.budget.clone(), ..AssetMeta::new(id, importer) })
    }
}

/// The built-in preset `new-character` starts `model`'s sidecar from: its kind's character, or `None`
/// if it isn't a model.
pub fn for_model(model: &Path) -> Option<&'static str> {
    match ImportSettings::for_source(model)? {
        ImportSettings::Vrm(_) => Some("vrm-character"),
        ImportSettings::Gltf(_) => Some("gltf-character"),
        ImportSettings::Fbx(_) => Some("fbx-character"),
        _ => None,
    }
}

/// The sidecar a cook makes for a new source at `source`, with `id` (see the [module docs](self)): a
/// VRM's from `vrm-character`, any other's with its importer's defaults. `None` if no importer reads it.
pub fn for_cook(source: &Path, id: AssetId) -> Option<AssetMeta> {
    let settings = ImportSettings::for_new_sidecar(source)?;
    if matches!(settings, ImportSettings::Vrm(_)) {
        let preset =
            ImportPreset::built_in("vrm-character").expect("the built-in presets read (a test holds them)");
        return Some(preset.sidecar("vrm-character", source, id).expect("a VRM's preset fits a VRM"));
    }
    Some(AssetMeta::new(id, settings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Role;
    use crate::import::vrm::VrmVersion;
    use crate::import::vrm::samples;
    use crate::meta::MeshSettings;
    use crate::test_util::TempDir;

    const ID: &str = "4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c";

    #[test]
    fn the_built_in_presets_are_characters_with_the_cooks_mesh_settings() {
        for (name, _) in BUILT_IN {
            let preset = ImportPreset::built_in(name).unwrap();
            let mesh = preset.importer.mesh().expect("a model's preset");
            assert_eq!(*mesh, MeshSettings::default(), "{name} starts from the cook's own mesh settings");
            assert_eq!(preset.budget, Some(Budget { role: Some(Role::Hero), waive: Vec::new() }), "{name}");
            let written = sapphire_serialize::to_ron(&preset).unwrap();
            assert_eq!(sapphire_serialize::from_ron::<ImportPreset>(&written).unwrap(), preset, "{name}");
        }
        assert!(matches!(ImportPreset::built_in("prop"), Err(PresetError::Unknown { .. })));
    }

    #[test]
    fn each_kind_of_model_has_its_preset() {
        assert_eq!(for_model(Path::new("a/hero.VRM")), Some("vrm-character"));
        assert_eq!(for_model(Path::new("hero.glb")), Some("gltf-character"));
        assert_eq!(for_model(Path::new("hero.gltf")), Some("gltf-character"));
        assert_eq!(for_model(Path::new("hero.fbx")), Some("fbx-character"));
        assert_eq!(for_model(Path::new("hero.png")), None);
        for model in ["hero.vrm", "hero.glb", "hero.fbx"] {
            let name = for_model(Path::new(model)).unwrap();
            let preset = ImportPreset::built_in(name).unwrap();
            assert!(preset.importer.same_importer(&ImportSettings::for_source(Path::new(model)).unwrap()));
        }
    }

    #[test]
    fn a_sidecar_takes_the_presets_settings_and_the_files_version() {
        let dir = TempDir::new("preset_sidecar");
        let vrm = dir.path().join("hero.vrm");
        fs::write(&vrm, samples::vrm1().to_glb()).unwrap();
        let mut preset = ImportPreset::built_in("vrm-character").unwrap();
        preset.importer.mesh_mut().unwrap().position_format = sapphire_assets::mesh::PositionFormat::Float32;
        let meta = preset.sidecar("mine", &vrm, ID.parse().unwrap()).unwrap();
        assert_eq!(meta.id.to_string(), ID);
        let ImportSettings::Vrm(settings) = &meta.importer else { panic!("a VRM's") };
        assert_eq!(settings.version, Some(VrmVersion::V1), "the file's version, not the preset's None");
        assert_eq!(settings.mesh.position_format, sapphire_assets::mesh::PositionFormat::Float32);
        assert_eq!(meta.budget, preset.budget);

        let glb = dir.path().join("prop.glb");
        let error = preset.sidecar("mine", &glb, ID.parse().unwrap()).unwrap_err().to_string();
        assert!(error.contains("the preset mine is for VRM sources"), "{error}");
        assert!(error.contains("prop.glb is read as glTF"), "{error}");
    }

    #[test]
    fn a_cook_starts_a_vrm_from_its_preset_and_anything_else_from_defaults() {
        let dir = TempDir::new("preset_cook");
        let vrm = dir.path().join("hero.vrm");
        fs::write(&vrm, samples::vrm1().to_glb()).unwrap();
        let meta = for_cook(&vrm, ID.parse().unwrap()).unwrap();
        assert_eq!(meta.budget.and_then(|budget| budget.role), Some(Role::Hero));
        let glb = dir.path().join("prop.glb");
        let meta = for_cook(&glb, ID.parse().unwrap()).unwrap();
        assert_eq!(meta, AssetMeta::new(ID.parse().unwrap(), ImportSettings::Gltf(MeshSettings::default())));
        assert!(for_cook(Path::new("notes.txt"), ID.parse().unwrap()).is_none());
    }

    #[test]
    fn a_preset_document_refuses_unknown_fields() {
        let dir = TempDir::new("preset_read");
        let path = dir.path().join("enemy.preset.ron");
        let text = "(type: \"sapphire::pipeline::ImportPreset\", version: 1, data: (importer: Gltf(()), \
                    budget: (role: Enemy)))";
        fs::write(&path, text).unwrap();
        let preset = ImportPreset::read(&path).unwrap();
        assert_eq!(preset.importer, ImportSettings::Gltf(MeshSettings::default()));
        fs::write(&path, text.replace("budget", "budgets")).unwrap();
        assert!(matches!(ImportPreset::read(&path), Err(PresetError::Document { .. })));
    }
}
