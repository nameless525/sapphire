//! `.meta` sidecars: each source asset's identity and import settings (spec §5.27).
//!
//! Every source asset has a sidecar beside it, named after the whole source file name
//! (`content/samples/mannequin.glb.meta`). It is a RON document
//! ([`sapphire_serialize::document`]) holding an [`AssetMeta`]:
//!
//! ```ron
//! (
//!     type: "sapphire::assets::AssetMeta",
//!     version: 1,
//!     data: (
//!         id: "4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c",
//!         importer: Gltf((
//!             outline: (
//!                 weld_tolerance: 0.0001,
//!             ),
//!             lods: (
//!                 levels: [
//!                     (
//!                         triangle_ratio: 0.5,
//!                         max_error: 0.002,
//!                     ),
//!                     (
//!                         triangle_ratio: 0.25,
//!                         max_error: 0.01,
//!                     ),
//!                     (
//!                         triangle_ratio: 0.125,
//!                         max_error: 0.03,
//!                     ),
//!                 ],
//!                 mobile_skip: 1,
//!             ),
//!             position_format: Unorm16,
//!             materials: {
//!                 "Skin": "0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b",
//!             },
//!         )),
//!     ),
//! )
//! ```
//!
//! Foliage meshes (M1 W6) add `foliage: (normals: …, slots: […], blend: …)`
//! ([`FoliageSettings`]), written out only when set.
//!
//! Material documents (`skin.material.ron`) have sidecars too, with `importer: Material(())`.
//! VRM characters (`alicia.vrm`) have `importer: Vrm((mesh: (…), version: Some(V1)))`: the mesh
//! settings above, and a note of the file's VRM version, read from the file when the sidecar is
//! created ([`ImportSettings::for_new_sidecar`]; M1 A6).
//!
//! [`resolve`] keeps these rules:
//!
//! - **IDs never change.** A missing sidecar is created on first import, with a fresh random
//!   ID and the default settings of the importer for the file's extension. An existing
//!   sidecar is only ever read; one that doesn't parse is an error, never replaced.
//! - **Sidecars are committed with their sources.** A locked run (CI) fails on a missing
//!   sidecar instead of creating one, because an ID made there would be thrown away.
//! - **Identity moves with the sidecar.** Moving or renaming a source keeps its ID when its
//!   sidecar moves too. A run that finds both a source without a sidecar and a sidecar
//!   without a source stops, rather than give a moved source a new ID.
//! - **IDs are unique.** Two sidecars holding one ID (a source copied with its sidecar) are
//!   an error; deleting the copy's sidecar gives it a new ID.
//! - **Sub-assets derive theirs.** A source that cooks to more than one asset (a VRM's rig,
//!   M1 A6) gives each further asset an ID derived from its own and the sub-asset's name
//!   ([`sub_asset_id`]), so those IDs are as stable as the sidecar's and need no storage.
//!
//! A sidecar may leave settings out, and they take their defaults (`#[serde(default)]`), so a
//! new setting needs no schema version. The importer and its settings are part of the
//! asset's [cache key](crate::cache); the ID is not, because no cooked format holds it yet.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use sapphire_assets::AssetId;
use sapphire_assets::mesh::PositionFormat;
use sapphire_reflect::Versioned;
use sapphire_serialize::{DocumentError, from_ron, to_ron};
use serde::{Deserialize, Serialize};

use crate::bake::OutlineSettings;
use crate::bake::foliage::FoliageSettings;
use crate::character_maps::{MapKind, MapSettings};
use crate::import::vrm::{self, VrmSettings};
use crate::material::{self, MaterialSettings};
use crate::optimize::LodSettings;
use crate::probes::{self, ProbeGridSettings};
use crate::texture::{HdrSettings, TextureClass, TextureSettings};

/// Extension of sidecar files, appended to the whole source file name.
pub const SIDECAR_EXTENSION: &str = "meta";

/// A source asset's sidecar document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::assets"]
pub struct AssetMeta {
    /// The asset's identity, set when the sidecar is created.
    pub id: AssetId,
    /// The importer that reads the source, with its settings.
    pub importer: ImportSettings,
}

impl Versioned for AssetMeta {
    const VERSION: u32 = 1;
}

/// Which importer reads a source, with that importer's settings. A new kind of source joins
/// the pipeline as a variant here and a [`Recipe`](crate::cook::Recipe) that cooks it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ImportSettings {
    /// glTF 2.0 (`.gltf`, `.glb`), flattened into one mesh, skinned if the scene is.
    Gltf(MeshSettings),
    /// PNG, TGA or EXR, cooked as its texture class says (M1 A2).
    Texture(TextureSettings),
    /// An anime character map document (`.facesdf.ron`, `.ramp.ron`, `.ilm.ron`): the map it
    /// describes is generated, then cooked as a texture (M1 A7).
    CharacterMap(MapSettings),
    /// A material document (`.material.ron`) of any family (M1 A3).
    Material(MaterialSettings),
    /// A VRM character (`.vrm`), 1.0 or 0.x: a mesh as glTF's, and its rig (M1 A6).
    Vrm(VrmSettings),
    /// An ambient probe grid document (`.probes.ron`), baked from the meshes it names (M1 W5).
    ProbeGrid(ProbeGridSettings),
}

impl ImportSettings {
    /// The default settings for `source`, picked by its extension, or `None` if no importer
    /// reads it.
    pub fn for_source(source: &Path) -> Option<Self> {
        // Map and material documents are known by compound extensions, before the last one is
        // looked at.
        if let Some(kind) = MapKind::for_path(source) {
            return Some(Self::CharacterMap(kind.default_settings()));
        }
        if material::is_source(source) {
            return Some(Self::Material(MaterialSettings::default()));
        }
        if probes::is_source(source) {
            return Some(Self::ProbeGrid(ProbeGridSettings::default()));
        }
        let extension = source.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "gltf" | "glb" => Some(Self::Gltf(MeshSettings::default())),
            "vrm" => Some(Self::Vrm(VrmSettings::default())),
            // Integer images are colors until their sidecar says otherwise; float images are HDR.
            "png" | "tga" => Some(Self::Texture(TextureSettings::default())),
            "exr" => {
                Some(Self::Texture(TextureSettings { class: TextureClass::Hdr(HdrSettings::default()) }))
            }
            _ => None,
        }
    }

    /// The settings a new sidecar for `source` gets: [`Self::for_source`]'s, plus what only the
    /// file can tell: a VRM's version, its sidecar's note. A file that can't be read or isn't a
    /// VRM notes nothing, and its cook reports why.
    pub fn for_new_sidecar(source: &Path) -> Option<Self> {
        let mut settings = Self::for_source(source)?;
        if let Self::Vrm(vrm) = &mut settings {
            vrm.version = fs::read(source).ok().and_then(|bytes| vrm::detect_version(&bytes));
        }
        Some(settings)
    }

    /// The settings of a mesh importer, or `None` for other kinds of source.
    pub fn mesh(&self) -> Option<&MeshSettings> {
        match self {
            Self::Gltf(settings) => Some(settings),
            Self::Vrm(settings) => Some(&settings.mesh),
            Self::Texture(_) | Self::CharacterMap(_) | Self::Material(_) | Self::ProbeGrid(_) => None,
        }
    }

    /// [`Self::mesh`], to change.
    pub fn mesh_mut(&mut self) -> Option<&mut MeshSettings> {
        match self {
            Self::Gltf(settings) => Some(settings),
            Self::Vrm(settings) => Some(&mut settings.mesh),
            Self::Texture(_) | Self::CharacterMap(_) | Self::Material(_) | Self::ProbeGrid(_) => None,
        }
    }
}

/// Import settings for meshes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MeshSettings {
    /// How outline normals are baked where the source doesn't author them.
    pub outline: OutlineSettings,
    /// The LOD chain, and how much of it the mobile cook drops.
    pub lods: LodSettings,
    /// How positions are stored: quantized to 16 bits across the bounds (the default), or as
    /// f32 for meshes too large for that or whose edges must meet a neighbor's exactly.
    pub position_format: PositionFormat,
    /// Material assets bound to the source's material slots, by slot name (the glTF material's
    /// name): `materials: {"Skin": "<id>"}`. Unbound slots embed the material the importer
    /// generates from the source.
    pub materials: BTreeMap<String, AssetId>,
    /// Foliage (M1 W6): volume normals transferred onto leaf cards and the foliage stream's wind
    /// and gradient data (`foliage: (normals: Hull(count: 6), slots: ["Leaves"])`). Meshes
    /// without it cook no foliage stream; written out only when set, so other sidecars stay as
    /// they were.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foliage: Option<FoliageSettings>,
}

/// The sidecar of `source`: its whole file name with `.meta` appended.
pub fn sidecar_path(source: &Path) -> PathBuf {
    let mut path = source.as_os_str().to_owned();
    path.push(".");
    path.push(SIDECAR_EXTENSION);
    PathBuf::from(path)
}

/// The source a sidecar belongs to.
fn source_path(sidecar: &Path) -> PathBuf {
    sidecar.with_extension("")
}

fn is_sidecar(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == SIDECAR_EXTENSION)
}

/// A fresh random asset ID, laid out as a version 4 UUID (RFC 9562 §5.4).
pub fn new_id() -> Result<AssetId, getrandom::Error> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes)?;
    bytes[6] = 0x40 | (bytes[6] & 0x0f);
    bytes[8] = 0x80 | (bytes[8] & 0x3f);
    Ok(AssetId::from_bytes(bytes))
}

/// The ID of sub-asset `name` of the source whose ID is `parent`: blake3 of both, laid out as a
/// version 8 (custom) UUID (RFC 9562 §5.8), so it never collides with a random version 4 one.
/// The same parent and name always give the same ID.
pub fn sub_asset_id(parent: AssetId, name: &str) -> AssetId {
    let hash = blake3::Hasher::new_derive_key("sapphire_pipeline 2026-10-25 sub-asset id")
        .update(&parent.to_bytes())
        .update(name.as_bytes())
        .finalize();
    let mut bytes: [u8; 16] = hash.as_bytes()[..16].try_into().expect("blake3 hashes are 32 bytes");
    bytes[6] = 0x80 | (bytes[6] & 0x0f);
    bytes[8] = 0x80 | (bytes[8] & 0x3f);
    AssetId::from_bytes(bytes)
}

/// Sidecars that can't be read, written or trusted.
#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("failed to read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} is not a valid sidecar", path.display())]
    Parse {
        path: PathBuf,
        /// Boxed: RON's errors are large, and every cook result carries this type.
        #[source]
        source: Box<DocumentError>,
    },
    #[error("failed to write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "{} and {} hold the same ID {id}; delete the sidecar of the copy to give it a new ID",
        first.display(),
        second.display()
    )]
    DuplicateId { id: AssetId, first: PathBuf, second: PathBuf },
    #[error(
        "no sidecar for {}: create it with an unlocked cook and commit it with its source",
        list(sources)
    )]
    Missing { sources: Vec<PathBuf> },
    #[error(
        "{} {} no sidecar, and {} {} no source. If a source was moved or renamed, move its \
         sidecar with it to keep its ID; otherwise delete the orphaned sidecar",
        list(sources),
        has(sources),
        list(orphaned),
        has(orphaned)
    )]
    Moved { sources: Vec<PathBuf>, orphaned: Vec<PathBuf> },
    #[error("failed to get randomness for a new asset ID")]
    Entropy(#[source] getrandom::Error),
}

fn list(paths: &[PathBuf]) -> String {
    paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
}

fn has(paths: &[PathBuf]) -> &'static str {
    if paths.len() == 1 { "has" } else { "have" }
}

/// Reads a sidecar.
pub fn read(path: &Path) -> Result<AssetMeta, MetaError> {
    let text =
        fs::read_to_string(path).map_err(|source| MetaError::Read { path: path.to_owned(), source })?;
    from_ron(&text).map_err(|source| MetaError::Parse { path: path.to_owned(), source: Box::new(source) })
}

/// Writes `meta` as a new sidecar at `path`. Never replaces a file: returns `false` if one
/// is already there (another cook created it first).
pub(crate) fn create(path: &Path, meta: &AssetMeta) -> Result<bool, MetaError> {
    let text = to_ron(meta).expect("an AssetMeta always serializes");
    let write_error = |source| MetaError::Write { path: path.to_owned(), source };
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(write_error(error)),
    };
    file.write_all(text.as_bytes()).map_err(write_error)?;
    Ok(true)
}

/// Replaces the sidecar at `path` with `meta`, through a temporary file and a rename. Cooks
/// never do this; explicit commands that change a source's settings at the user's request do
/// (`extract-materials` binding the documents it writes).
pub(crate) fn rewrite(path: &Path, meta: &AssetMeta) -> Result<(), MetaError> {
    let text = to_ron(meta).expect("an AssetMeta always serializes");
    let write_error = |source| MetaError::Write { path: path.to_owned(), source };
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    fs::write(&temp, text).map_err(write_error)?;
    fs::rename(&temp, path).map_err(write_error)
}

/// The source assets and sidecars under a content root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceTree {
    /// Files an importer reads, in path order.
    pub sources: Vec<PathBuf>,
    /// `.meta` files, in path order.
    pub sidecars: Vec<PathBuf>,
}

impl SourceTree {
    /// Walks `root`, skipping hidden files and directories. Files that are neither sources
    /// nor sidecars are ignored.
    pub fn scan(root: &Path) -> Result<Self, MetaError> {
        let mut tree = Self::default();
        tree.walk(root)?;
        Ok(tree)
    }

    fn walk(&mut self, dir: &Path) -> Result<(), MetaError> {
        let read_error = |source| MetaError::Read { path: dir.to_owned(), source };
        let mut entries = fs::read_dir(dir)
            .map_err(read_error)?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(read_error)?;
        entries.sort();
        for path in entries {
            let hidden = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
            if hidden {
                continue;
            }
            if path.is_dir() {
                self.walk(&path)?;
            } else if is_sidecar(&path) {
                self.sidecars.push(path);
            } else if ImportSettings::for_source(&path).is_some() {
                self.sources.push(path);
            }
        }
        Ok(())
    }
}

/// A source asset and its sidecar.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceAsset {
    pub path: PathBuf,
    pub meta: AssetMeta,
}

/// A source tree's sidecars, as [`resolve`] found and made them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resolved {
    /// Every source with its sidecar, in the tree's order.
    pub assets: Vec<SourceAsset>,
    /// Sidecars this run created.
    pub created: Vec<PathBuf>,
    /// Sidecars whose source doesn't exist.
    pub orphaned: Vec<PathBuf>,
}

/// Reads the sidecar of every source in `tree`, creating missing ones unless `locked`, and
/// checks the rules in the [module docs](self).
pub fn resolve(tree: &SourceTree, locked: bool) -> Result<Resolved, MetaError> {
    let present: HashSet<&Path> = tree.sidecars.iter().map(PathBuf::as_path).collect();
    // A sidecar whose source exists but no importer reads is left alone: it may belong to an
    // importer this build doesn't have.
    let orphaned: Vec<PathBuf> =
        tree.sidecars.iter().filter(|sidecar| !source_path(sidecar).exists()).cloned().collect();

    let mut metas = Vec::with_capacity(tree.sources.len());
    let mut missing = Vec::new();
    for source in &tree.sources {
        let sidecar = sidecar_path(source);
        if present.contains(sidecar.as_path()) {
            metas.push(Some(read(&sidecar)?));
        } else {
            metas.push(None);
            missing.push(source.clone());
        }
    }
    let mut owners: HashMap<AssetId, PathBuf> = HashMap::new();
    for (source, meta) in tree.sources.iter().zip(&metas) {
        if let Some(meta) = meta {
            claim(&mut owners, meta.id, sidecar_path(source))?;
        }
    }

    if !missing.is_empty() {
        if locked {
            return Err(MetaError::Missing { sources: missing });
        }
        if !orphaned.is_empty() {
            return Err(MetaError::Moved { sources: missing, orphaned });
        }
    }
    let mut created = Vec::new();
    let mut assets = Vec::with_capacity(tree.sources.len());
    for (source, meta) in tree.sources.iter().zip(metas) {
        let meta = match meta {
            Some(meta) => meta,
            None => {
                let sidecar = sidecar_path(source);
                let importer =
                    ImportSettings::for_new_sidecar(source).expect("scanned sources have an importer");
                let fresh = AssetMeta { id: new_id().map_err(MetaError::Entropy)?, importer };
                let meta = if create(&sidecar, &fresh)? {
                    created.push(sidecar.clone());
                    fresh
                } else {
                    read(&sidecar)?
                };
                claim(&mut owners, meta.id, sidecar)?;
                meta
            }
        };
        assets.push(SourceAsset { path: source.clone(), meta });
    }
    Ok(Resolved { assets, created, orphaned })
}

/// Records that `sidecar` holds `id`, failing if another sidecar already does.
fn claim(owners: &mut HashMap<AssetId, PathBuf>, id: AssetId, sidecar: PathBuf) -> Result<(), MetaError> {
    match owners.get(&id) {
        Some(first) => Err(MetaError::DuplicateId { id, first: first.clone(), second: sidecar }),
        None => {
            owners.insert(id, sidecar);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::samples;
    use crate::test_util::TempDir;

    const ID: &str = "4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c";

    fn meta(weld_tolerance: f32) -> AssetMeta {
        let outline = OutlineSettings::with_weld_tolerance(weld_tolerance).unwrap();
        let settings = MeshSettings { outline, ..MeshSettings::default() };
        AssetMeta { id: ID.parse().unwrap(), importer: ImportSettings::Gltf(settings) }
    }

    /// Two mesh sources, `a.glb` and `nested/b.gltf`; neither has a sidecar.
    fn tree(dir: &Path) -> [PathBuf; 2] {
        let a = dir.join("a.glb");
        let b = dir.join("nested/b.gltf");
        fs::create_dir_all(b.parent().unwrap()).unwrap();
        fs::write(&a, samples::mannequin_glb()).unwrap();
        fs::write(&b, "{}").unwrap();
        [a, b]
    }

    fn resolve_dir(dir: &Path, locked: bool) -> Result<Resolved, MetaError> {
        resolve(&SourceTree::scan(dir).unwrap(), locked)
    }

    #[test]
    fn sidecars_are_versioned_documents() {
        let text = to_ron(&meta(0.002)).unwrap();
        let level = |ratio: &str, error: &str| {
            format!(
                "                    (\n                        triangle_ratio: {ratio},\n                        \
                 max_error: {error},\n                    ),\n"
            )
        };
        let expected = format!(
            "(\n    type: \"sapphire::assets::AssetMeta\",\n    version: 1,\n    data: (\n        id: \
             \"{ID}\",\n        importer: Gltf((\n            outline: (\n                \
             weld_tolerance: 0.002,\n            ),\n            lods: (\n                levels: [\n{}{}{}                \
             ],\n                mobile_skip: 1,\n            ),\n            position_format: Unorm16,\n            materials: {{}},\n        )),\n    \
             ),\n)\n",
            level("0.5", "0.002"),
            level("0.25", "0.01"),
            level("0.125", "0.03"),
        );
        assert_eq!(text, expected);
        assert_eq!(from_ron::<AssetMeta>(&text).unwrap(), meta(0.002));

        // Settings may be left out, and must be valid.
        let short = format!(
            r#"(type: "sapphire::assets::AssetMeta", version: 1, data: (id: "{ID}", importer: Gltf(())))"#
        );
        assert_eq!(
            from_ron::<AssetMeta>(&short).unwrap().importer,
            ImportSettings::Gltf(MeshSettings::default())
        );
        let invalid = short.replace("Gltf(())", "Gltf((outline: (weld_tolerance: -1.0)))");
        let error = from_ron::<AssetMeta>(&invalid).unwrap_err();
        assert!(error.to_string().contains("positive, finite"), "{error}");

        // Mesh settings beyond the bake: LODs and the position format.
        let custom = short.replace(
            "Gltf(())",
            "Gltf((lods: (levels: [(triangle_ratio: 0.3, max_error: 0.05)], mobile_skip: 0), \
             position_format: Float32))",
        );
        let settings =
            from_ron::<AssetMeta>(&custom).unwrap().importer.mesh().expect("mesh settings").clone();
        assert_eq!(settings.position_format, PositionFormat::Float32);
        assert_eq!(settings.lods.levels().len(), 1);
        assert_eq!(settings.lods.mobile_skip(), 0);
        let no_lods = short.replace("Gltf(())", "Gltf((lods: (levels: [])))");
        let settings =
            from_ron::<AssetMeta>(&no_lods).unwrap().importer.mesh().expect("mesh settings").clone();
        assert_eq!(settings.lods, LodSettings::none(), "no levels: nothing for mobile to skip");
        let too_many = short.replace("Gltf(())", "Gltf((lods: (levels: [], mobile_skip: 1)))");
        let error = from_ron::<AssetMeta>(&too_many).unwrap_err();
        assert!(error.to_string().contains("mobile_skip"), "{error}");
        let rising = short.replace(
            "Gltf(())",
            "Gltf((lods: (levels: [(triangle_ratio: 0.5, max_error: 0.1), (triangle_ratio: 0.6, max_error: 0.2)])))",
        );
        assert!(from_ron::<AssetMeta>(&rising).is_err(), "ratios must fall level by level");
    }

    #[test]
    fn sidecars_sit_next_to_their_source() {
        let source = Path::new("content/samples/mannequin.glb");
        assert_eq!(sidecar_path(source), Path::new("content/samples/mannequin.glb.meta"));
        assert_eq!(source_path(&sidecar_path(source)), source);
        assert!(ImportSettings::for_source(Path::new("model.GLB")).is_some());
        assert_eq!(
            ImportSettings::for_source(Path::new("albedo.png")),
            Some(ImportSettings::Texture(TextureSettings::default()))
        );
        assert!(matches!(
            ImportSettings::for_source(Path::new("sky.EXR")),
            Some(ImportSettings::Texture(TextureSettings { class: TextureClass::Hdr(_) }))
        ));
        assert_eq!(
            ImportSettings::for_source(Path::new("characters/skin.material.ron")),
            Some(ImportSettings::Material(MaterialSettings::default()))
        );
        assert!(ImportSettings::for_source(Path::new("tiers.ron")).is_none(), "other RON is not a material");
        assert!(ImportSettings::for_source(Path::new("notes.txt")).is_none());
        assert!(ImportSettings::for_source(Path::new("model.glb.meta")).is_none());
    }

    #[test]
    fn new_ids_are_random_version_4_uuids() {
        let (a, b) = (new_id().unwrap(), new_id().unwrap());
        assert_ne!(a, b);
        for id in [a, b] {
            let text = id.to_string();
            assert_eq!(&text[14..15], "4", "version: {text}");
            assert!("89ab".contains(&text[19..20]), "variant: {text}");
        }
    }

    #[test]
    fn missing_sidecars_are_created_and_existing_ids_never_change() {
        let dir = TempDir::new("meta_create");
        let [a, b] = tree(dir.path());
        let kept = to_ron(&meta(0.002)).unwrap();
        fs::write(sidecar_path(&b), &kept).unwrap();
        fs::write(dir.path().join("notes.txt"), "not an asset").unwrap();
        fs::write(dir.path().join(".hidden.glb"), "skipped").unwrap();

        let first = resolve_dir(dir.path(), false).unwrap();
        assert_eq!(first.created, [sidecar_path(&a)]);
        assert!(first.orphaned.is_empty());
        let [asset_a, asset_b] = &first.assets[..] else { panic!("two assets") };
        assert_eq!((&asset_a.path, &asset_b.path), (&a, &b));
        assert_eq!(asset_b.meta, meta(0.002));
        assert_eq!(asset_a.meta.importer, ImportSettings::Gltf(MeshSettings::default()));
        assert_eq!(read(&sidecar_path(&a)).unwrap(), asset_a.meta, "written as returned");
        assert_eq!(fs::read_to_string(sidecar_path(&b)).unwrap(), kept, "existing sidecars aren't rewritten");

        let second = resolve_dir(dir.path(), true).unwrap();
        assert!(second.created.is_empty());
        assert_eq!(second.assets, first.assets, "IDs are stable across runs");
    }

    #[test]
    fn locked_runs_never_create_sidecars() {
        let dir = TempDir::new("meta_locked");
        let [a, b] = tree(dir.path());
        let Err(MetaError::Missing { sources }) = resolve_dir(dir.path(), true) else {
            panic!("a locked run needs every sidecar");
        };
        assert_eq!(sources, [a.clone(), b]);
        assert!(!sidecar_path(&a).exists());
    }

    #[test]
    fn broken_and_duplicate_sidecars_are_errors() {
        let dir = TempDir::new("meta_errors");
        let [a, b] = tree(dir.path());
        fs::write(sidecar_path(&a), "(type: \"sapphire::assets::AssetMeta\", version: 1, data: (id: \"x\"))")
            .unwrap();
        assert!(
            matches!(resolve_dir(dir.path(), false), Err(MetaError::Parse { path, .. }) if path == sidecar_path(&a))
        );
        assert!(!sidecar_path(&b).exists(), "nothing is created before every sidecar reads");

        let copy = to_ron(&meta(0.002)).unwrap();
        fs::write(sidecar_path(&a), &copy).unwrap();
        fs::write(sidecar_path(&b), &copy).unwrap();
        let Err(MetaError::DuplicateId { id, first, second }) = resolve_dir(dir.path(), false) else {
            panic!("one ID in two sidecars");
        };
        assert_eq!((id.to_string(), first, second), (ID.to_owned(), sidecar_path(&a), sidecar_path(&b)));
        assert_eq!(fs::read_to_string(sidecar_path(&b)).unwrap(), copy, "never rewritten");
    }

    #[test]
    fn moving_a_source_without_its_sidecar_stops_the_cook() {
        let dir = TempDir::new("meta_moved");
        let [a, b] = tree(dir.path());
        resolve_dir(dir.path(), false).unwrap();
        let moved = dir.path().join("renamed.glb");
        fs::rename(&a, &moved).unwrap();

        let Err(MetaError::Moved { sources, orphaned }) = resolve_dir(dir.path(), false) else {
            panic!("a source without a sidecar next to a sidecar without a source");
        };
        assert_eq!((sources, orphaned), (vec![moved.clone()], vec![sidecar_path(&a)]));
        assert!(!sidecar_path(&moved).exists());

        // Moving the sidecar too keeps the ID.
        let id = read(&sidecar_path(&a)).unwrap().id;
        fs::rename(sidecar_path(&a), sidecar_path(&moved)).unwrap();
        let resolved = resolve_dir(dir.path(), true).unwrap();
        assert_eq!(resolved.assets.iter().find(|asset| asset.path == moved).unwrap().meta.id, id);

        // An orphan alone is reported, not an error.
        fs::remove_file(&b).unwrap();
        let resolved = resolve_dir(dir.path(), true).unwrap();
        assert_eq!(resolved.orphaned, [sidecar_path(&b)]);
        assert_eq!(resolved.assets.len(), 1);
    }
}
