//! `new-character` (M6 C5): a model's sidecar, and every slot's anime character material and maps,
//! written with fresh IDs so the model cooks as a character with no hand edit.
//!
//! `sapphire_pipeline new-character --model <vrm|glb|fbx> [--out <dir>] [--role <role>]
//! [--preset <file>] [--part <slot>=<part>]…` does what a person did by hand before it, copying a
//! worked character's documents and making every ID afresh (m6-handoff's "A playable kind", step 1):
//!
//! 1. **The model** is copied into `--out` (the model's own folder when left out), unless it is there.
//! 2. **Its sidecar** starts from a preset ([`crate::preset`]): the document `--preset` names, or the
//!    built-in character preset of the model's kind. `--role` casts it, in place of the preset's role.
//! 3. **Every slot** is extracted as `extract-materials --all` extracts it ([`crate::extract`]): its
//!    material document under `<stem>/materials/`, with a VRM's or FBX's images as textures. Then, by
//!    the slot's part ([`Part`]), its document gains the maps the anime character shader reads, each a
//!    document with its sidecar under `<stem>/shading/`, named after the slot:
//!
//! | Document | Parts | What it starts as | Bound |
//! |---|---|---|---|
//! | `<slot>.ramp.ron` | all but eyes | A row per region holding its shade color end to end (its deep shade color below the deep threshold, where the material has one): the tint the band mode gave the shadow side | Always: `diffuse: Ramp` |
//! | `<slot>.ilm.ron` | all but eyes | The values the shader reads with no control map: highlights everywhere, no shadow bias, threshold ½, material ID 0 | Unless the material has a control map, or several regions (its vertices pick their regions, which a constant ID would override): written to paint, then bind |
//! | `<slot>.layers.ron` | skin and cloth | Every channel 1, what an unbound layer mask reads | Unless the material has one (a face's and hair's own masks take its place, so they have none) |
//! | `<slot>.facesdf.ron`, `<slot>_threshold_<angle>.png` | faces | Masks every 15° baked from the slot's own normals, as the figures' are (`figure::face`) | `surface: Face` |
//!
//! Hair takes `surface: Hair` with its highlight at the defaults; eyes keep what the importer made of
//! them. A layer mask is a body's alone, so one the importer bound on a face or hair (a VRM's rim and
//! glow) is unbound, its document left under `maps/`. So the scaffolded character draws nearly as its
//! imported materials did, but for its face, shaded by its map, and those masks. Nearly: the ramp's
//! colors and the ILM's halves are stored in 8 bits (the shadow bias 128/255 is an offset of +0.004,
//! the threshold 0.502), and the band mode's softened deep step is a hard edge in the ramp. No test
//! compares the two looks. The maps are there to paint.
//!
//! Every texture and map sidecar it makes starts capped at the role's row (`max_size`, its `map`
//! side on each cook, and at most 1K on mobile), so `check`'s `map-size` and `uncapped-map` hold as
//! made: a cap at or over a map's size changes nothing.
//!
//! # Parts
//!
//! A slot's part comes from `--part <slot>=<part>` if given, else from what the importer made of it
//! (VRoid Studio's eye materials are eyes, M1 C6), else from its name: VRoid Studio's
//! (`<Part>_<nn>_<GROUP>`: `HAIR` is hair, `SKIN` on the `Face` part the face, other `SKIN` and the
//! `FACE` group's mouth and lines skin, `EYE` eyes, `CLOTH` cloth), then whole words in any case, split
//! at anything but a letter and where a lower case letter meets a capital (`face`, `hair`, `eye` or
//! `eyes`, `skin` or `body`: `HairFront` is hair, `Hairpin` and `Surface` cloth), and cloth for anything
//! else.
//!
//! # Running it again
//!
//! It never overwrites a file. A second run over the same tree reads what is there and writes only
//! what is missing, so it changes nothing; a slot the sidecar binds is left alone, as extraction leaves
//! it. A file where the scaffold would write one of another kind (a texture's sidecar on a ramp's
//! path), another model under the model's name, a `--part` naming no slot, or a model copied into
//! another folder that names files beside it (a `.gltf`'s buffers, an FBX's images) is refused,
//! named, and nothing is written: the whole run is planned first (`extract::Plan`), every slot
//! through, before the first write.
//!
//! IDs come from one [`IdSource`]: the OS's randomness from the command, a seeded sequence in tests,
//! so a test's scaffold is the same bytes on every run.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use glam::Vec3;
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterSurface, DiffuseMode, FaceShading, HairShading, RampDiffuse,
};

use crate::character_maps::MapKind;
use crate::character_maps::face_sdf::{Edges, FaceSdfMap, ThresholdMask};
use crate::character_maps::ilm::{ChannelInput, IlmMap, MaterialIdInput};
use crate::character_maps::layer_mask::{self, LayerMaskMap};
use crate::character_maps::ramp::{ColorStop, MaterialRamps, Ramp, RampMap, SrgbColor, StopRamp};
use crate::check::{self, Budget, Role};
use crate::cook::Cook;
use crate::extract::{self, ExtractError, ExtractOptions, Extraction, Finishing, Plan, Writer};
use crate::figure::face;
use crate::meta::{self, AssetMeta, IdSource, ImportSettings, MetaError};
use crate::preset::{self, ImportPreset, PresetError};
use crate::texture::MaxSize;

/// Texels a side of a face's threshold masks: the face SDF's size.
pub const MASK_SIZE: u32 = 512;

/// The size of the maps that start as constants: the smallest whole block.
const CONSTANT_SIZE: [u32; 2] = [4, 4];

/// What part of a character a slot is, which picks its maps (see the [module docs](self)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Part {
    Skin,
    Face,
    Hair,
    Eye,
    /// Clothes, and anything no other part names.
    Cloth,
}

impl Part {
    pub const ALL: [Self; 5] = [Self::Skin, Self::Face, Self::Hair, Self::Eye, Self::Cloth];

    /// The part's name on the command line and in reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Skin => "skin",
            Self::Face => "face",
            Self::Hair => "hair",
            Self::Eye => "eye",
            Self::Cloth => "cloth",
        }
    }

    /// The part of the slot `name` whose importer made `generated` of it, when the command names none.
    pub fn of(name: &str, generated: &AnimeCharacterMaterial) -> Self {
        match generated.surface {
            CharacterSurface::Eye(_) => return Self::Eye,
            CharacterSurface::Face(_) => return Self::Face,
            CharacterSurface::Hair(_) => return Self::Hair,
            CharacterSurface::Body => {}
        }
        // VRoid Studio's names: a part, a number and its group in capitals, as `Face_00_SKIN`,
        // `HairBack_00_HAIR` or `N00_000_00_Body_00_SKIN (Instance)`.
        let tokens: Vec<&str> =
            name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        let vroid = tokens.windows(3).find_map(|window| {
            let [part, number, group] = window else { return None };
            let numbered = number.chars().all(|c| c.is_ascii_digit());
            let grouped = ["HAIR", "SKIN", "FACE", "EYE", "CLOTH"].contains(group);
            (numbered && grouped).then_some((*part, *group))
        });
        match vroid {
            Some((_, "HAIR")) => return Self::Hair,
            Some(("Face", "SKIN")) => return Self::Face,
            // The `FACE` group is the mouth and the face's lines, drawn on the face's skin.
            Some((_, "SKIN" | "FACE")) => return Self::Skin,
            Some((_, "EYE")) => return Self::Eye,
            Some(_) => return Self::Cloth,
            None => {}
        }
        // Otherwise whole words in any case, split at anything but a letter and where a lower case
        // letter meets a capital: `hair_front` and `HairFront` are hair, `Hairpin` and `Surface` aren't.
        let words = words(name);
        let has = |names: &[&str]| words.iter().any(|word| names.contains(&word.as_str()));
        if has(&["face"]) {
            Self::Face
        } else if has(&["hair"]) {
            Self::Hair
        } else if has(&["eye", "eyes"]) {
            Self::Eye
        } else if has(&["skin", "body"]) {
            Self::Skin
        } else {
            Self::Cloth
        }
    }
}

/// `name`'s words in lower case, split at anything but a letter and where a lower case letter meets a
/// capital.
fn words(name: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut last = None;
    for c in name.chars() {
        if !c.is_alphabetic() {
            last = None;
            continue;
        }
        let starts = match last {
            None => true,
            Some(previous) => char::is_lowercase(previous) && c.is_uppercase(),
        };
        if starts {
            words.push(String::new());
        }
        words.last_mut().expect("a word was started").extend(c.to_lowercase());
        last = Some(c);
    }
    words
}

impl fmt::Display for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Returned for a part no [`Part`] is called.
#[derive(Debug, thiserror::Error)]
#[error("{0:?} isn't a part: skin, face, hair, eye or cloth")]
pub struct UnknownPart(String);

impl FromStr for Part {
    type Err = UnknownPart;

    fn from_str(name: &str) -> Result<Self, UnknownPart> {
        Self::ALL.into_iter().find(|part| part.name() == name).ok_or_else(|| UnknownPart(name.to_owned()))
    }
}

/// What [`new_character`] is asked to do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewCharacter {
    /// The model: a `.vrm`, `.glb`, `.gltf` or `.fbx`.
    pub model: PathBuf,
    /// The folder it goes in; the model's own when `None`.
    pub out: Option<PathBuf>,
    /// The role its new sidecar casts it in, in place of the preset's.
    pub role: Option<Role>,
    /// A preset document for its new sidecar, in place of the built-in one of its kind.
    pub preset: Option<PathBuf>,
    /// Parts by slot name, in place of the ones the slots' names say.
    pub parts: BTreeMap<String, Part>,
}

/// What [`new_character`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scaffold {
    /// The model in its folder.
    pub model: PathBuf,
    /// Files written, in order: the model's copy, its sidecar, and every document, input and sidecar.
    pub written: Vec<PathBuf>,
    /// Each slot this run scaffolded, with its part.
    pub parts: Vec<(String, Part)>,
    /// Slots left alone, because the sidecar binds them already.
    pub kept: Vec<String>,
    /// Slots whose ILM document is written but not bound: their vertices pick their regions, so the
    /// map waits for its material IDs to be painted.
    pub unbound_ilm: Vec<String>,
}

/// Errors from [`new_character`].
#[derive(Debug, thiserror::Error)]
pub enum ScaffoldError {
    #[error("{} isn't a model (.vrm, .glb, .gltf or .fbx)", .0.display())]
    NotAModel(PathBuf),
    #[error("failed to read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "{} is another file than {}: move it, or scaffold into another folder",
        found.display(),
        model.display()
    )]
    ModelClash { model: PathBuf, found: PathBuf },
    #[error(
        "{} names files beside it that a copy into another folder would leave behind ({}): scaffold it in \
         its own folder (no --out), or move them with it",
        model.display(),
        files.join(", ")
    )]
    ExternalFiles { model: PathBuf, files: Vec<String> },
    #[error("--part names slot {slot:?}, but the model's slots are {}", slots.join(", "))]
    UnknownSlot { slot: String, slots: Vec<String> },
    #[error("failed to write slot {slot:?}'s face masks")]
    Masks {
        slot: String,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Preset(#[from] PresetError),
    #[error(transparent)]
    Meta(#[from] MetaError),
    #[error(transparent)]
    Extract(#[from] ExtractError),
}

/// Scaffolds the character `request` asks for (see the [module docs](self)), with new IDs from `ids`.
pub fn new_character(request: &NewCharacter, ids: &mut IdSource) -> Result<Scaffold, ScaffoldError> {
    let preset_name =
        preset::for_model(&request.model).ok_or_else(|| ScaffoldError::NotAModel(request.model.clone()))?;
    let read_error = |path: &Path| {
        let path = path.to_owned();
        move |source| ScaffoldError::Read { path, source }
    };
    let bytes = fs::read(&request.model).map_err(read_error(&request.model))?;
    let folder = match &request.out {
        Some(out) => out.clone(),
        None => request.model.parent().unwrap_or(Path::new("")).to_path_buf(),
    };
    let name = request.model.file_name().ok_or_else(|| ScaffoldError::NotAModel(request.model.clone()))?;
    let model = folder.join(name);
    let mut scaffold = Scaffold { model: model.clone(), ..Scaffold::default() };

    // The preset first, so a bad one writes nothing.
    let (name, preset) = match &request.preset {
        Some(path) => (path.display().to_string(), ImportPreset::read(path)?),
        None => (preset_name.to_owned(), ImportPreset::built_in(preset_name)?),
    };
    let write_error = |path: &Path| {
        let path = path.to_owned();
        move |source| ScaffoldError::Write { path, source }
    };

    // Everything is checked before the first write, so a refusal writes nothing: the model's place,
    // the files a model in another folder leaves behind, then the whole run as a plan, which meets
    // every slot name `--part` gives and every clash on the way.
    let copied = !model.exists();
    if !copied && fs::read(&model).map_err(read_error(&model))? != bytes {
        return Err(ScaffoldError::ModelClash { model: request.model.clone(), found: model });
    }
    if copied {
        let files = external_files(&request.model, &bytes);
        if !files.is_empty() {
            return Err(ScaffoldError::ExternalFiles { model: request.model.clone(), files });
        }
    }
    let sidecar = meta::sidecar_path(&model);
    let planned = if sidecar.exists() {
        meta::read(&sidecar)?
    } else {
        // Its ID is drawn when it is written, so the run's IDs come in the same order as ever.
        let mut meta = preset.sidecar(&name, &model, IdSource::seeded(0).next_id()?)?;
        if let Some(role) = request.role {
            meta.budget.get_or_insert_with(Budget::default).role = Some(role);
        }
        meta
    };
    // Every texture and map starts capped at its role's row, so the maps meet `check` as made.
    let role = planned.budget.as_ref().and_then(|budget| budget.role);
    // A character's maps stay within `uncapped-map`'s 1K on mobile whatever its row says.
    let max_size = role.map_or(MaxSize::NONE, |role| MaxSize {
        desktop: NonZeroU32::new(role.limits(Cook::Desktop).map),
        mobile: NonZeroU32::new(role.limits(Cook::Mobile).map.min(check::MOBILE_MAP)),
    });
    let options = ExtractOptions { all: true, max_size };
    let plan = Plan { sidecar: &planned, bytes: &bytes };
    slots(&model, &options, &mut IdSource::seeded(0), Some(plan), request)?;

    if copied {
        fs::create_dir_all(&folder).map_err(write_error(&folder))?;
        if meta::create_file(&model, &bytes).map_err(write_error(&model))? {
            scaffold.written.push(model.clone());
        }
    }
    if !sidecar.exists() {
        let meta = AssetMeta { id: ids.next_id()?, ..planned };
        if meta::create(&sidecar, &meta)? {
            scaffold.written.push(sidecar.clone());
        }
    }
    let (extraction, parts, unbound_ilm) = slots(&model, &options, ids, None, request)?;
    scaffold.written.extend(extraction.written);
    scaffold.parts = parts;
    scaffold.kept = extraction.kept;
    scaffold.unbound_ilm = unbound_ilm;
    Ok(scaffold)
}

/// Extracts and finishes every slot of `model` (or plans it, with `plan`): the extraction, each
/// slot's part, and the slots whose ILM is written unbound.
type Slots = (Extraction, Vec<(String, Part)>, Vec<String>);

fn slots(
    model: &Path,
    options: &ExtractOptions,
    ids: &mut IdSource,
    plan: Option<Plan<'_>>,
    request: &NewCharacter,
) -> Result<Slots, ScaffoldError> {
    let mut parts = Vec::new();
    let mut unbound_ilm = Vec::new();
    let known = |slots: &[&str]| match request.parts.keys().find(|slot| !slots.contains(&slot.as_str())) {
        Some(slot) => Err(ScaffoldError::UnknownSlot {
            slot: slot.clone(),
            slots: slots.iter().map(|slot| (*slot).to_owned()).collect(),
        }),
        None => Ok(()),
    };
    let extraction = extract::extract(model, options, ids, plan, known, |slot: Finishing<'_, '_>| {
        let part =
            request.parts.get(slot.slot).copied().unwrap_or_else(|| Part::of(slot.slot, slot.document));
        if !finish(slot.slot, part, slot.index, slot.stem, slot.root, slot.mesh, slot.document, slot.writer)?
        {
            unbound_ilm.push(slot.slot.to_owned());
        }
        parts.push((slot.slot.to_owned(), part));
        Ok::<_, ScaffoldError>(())
    })?;
    Ok((extraction, parts, unbound_ilm))
}

/// The files `model` names outside itself, which a copy into another folder would leave behind: a
/// glTF's (or GLB's) buffers and images by URI, and an FBX's images it doesn't embed.
fn external_files(model: &Path, bytes: &[u8]) -> Vec<String> {
    let extension = model.extension().map(|extension| extension.to_string_lossy().to_lowercase());
    match extension.as_deref() {
        Some("gltf" | "glb" | "vrm") => {
            // A GLB's JSON is its first chunk, after the 12-byte header and the chunk's 8.
            let json = match bytes.get(..4) {
                Some(b"glTF") => bytes
                    .get(12..16)
                    .map(|length| u32::from_le_bytes(length.try_into().expect("four bytes")) as usize)
                    .and_then(|length| bytes.get(20..20 + length))
                    .unwrap_or_default(),
                _ => bytes,
            };
            let Ok(document) = serde_json::from_slice::<serde_json::Value>(json) else { return Vec::new() };
            ["buffers", "images"]
                .into_iter()
                .filter_map(|key| document.get(key)?.as_array())
                .flatten()
                .filter_map(|entry| entry.get("uri")?.as_str())
                .filter(|uri| !uri.starts_with("data:"))
                .map(str::to_owned)
                .collect()
        }
        Some("fbx") => {
            let Ok(import) = crate::import::fbx::import_with_textures(bytes) else { return Vec::new() };
            let mut files: Vec<String> = import
                .slots
                .iter()
                .flat_map(|slot| [&slot.base_color, &slot.normal])
                .flatten()
                .filter(|texture| texture.embedded.is_none() && !texture.file.is_empty())
                .map(|texture| texture.file.clone())
                .collect();
            files.sort();
            files.dedup();
            files
        }
        _ => Vec::new(),
    }
}

/// Writes slot `slot`'s maps as `part`'s, under `root`'s `shading/`, and binds them in its `document`:
/// see the [module docs](self). Whether its ILM document is bound (or it has none to bind).
#[expect(clippy::too_many_arguments, reason = "a slot's whole description, as extraction hands it over")]
fn finish(
    slot: &str,
    part: Part,
    index: usize,
    stem: &str,
    root: &Path,
    mesh: &crate::mesh::ImportedMesh,
    document: &mut AnimeCharacterMaterial,
    writer: &mut Writer<'_>,
) -> Result<bool, ScaffoldError> {
    if part == Part::Eye {
        return Ok(true);
    }
    let dir = root.join("shading");
    let path = |kind: MapKind| dir.join(format!("{stem}{}", kind.suffix()));
    let mut ilm_bound = true;

    if let DiffuseMode::Bands(bands) = &document.diffuse {
        let ramp = ramp(slot, document, bands.deep_threshold);
        let id = map(writer, &path(MapKind::Ramp), &ramp)?;
        document.diffuse = DiffuseMode::Ramp(RampDiffuse { map: id, mix: 1.0 });
    }
    if document.control_map.is_none() {
        let ilm = IlmMap {
            size: Some(CONSTANT_SIZE),
            specular_mask: ChannelInput::Constant(1.0),
            shadow_bias: ChannelInput::Constant(0.5),
            specular_threshold: ChannelInput::Constant(0.5),
            material_id: MaterialIdInput::Constant(0),
        };
        let id = map(writer, &path(MapKind::Ilm), &ilm)?;
        if document.regions.len() == 1 {
            document.control_map = Some(id);
        } else {
            ilm_bound = false;
        }
    }
    match part {
        Part::Face if !matches!(document.surface, CharacterSurface::Face(_)) => {
            let sdf = face_sdf(slot, index, stem, &dir, mesh, writer)?;
            let id = map(writer, &path(MapKind::FaceSdf), &sdf)?;
            document.surface = CharacterSurface::Face(FaceShading::new(id));
            // A layer mask is a body's: one the importer bound is left in `maps/`, unbound, as the
            // look-dev character's face has none.
            document.layer_mask = None;
        }
        Part::Hair if document.surface == CharacterSurface::Body => {
            document.surface = CharacterSurface::Hair(HairShading::default());
            document.layer_mask = None;
        }
        Part::Skin | Part::Face | Part::Hair | Part::Eye | Part::Cloth => {}
    }
    // A layer mask is a body's: a face's or hair's own masks take its place.
    if document.layer_mask.is_none() && document.surface == CharacterSurface::Body {
        let layers = LayerMaskMap {
            size: Some(CONSTANT_SIZE),
            matcap: layer_mask::unpainted(),
            metallic: layer_mask::unpainted(),
            emission: layer_mask::unpainted(),
            stocking: layer_mask::unpainted(),
        };
        document.layer_mask = Some(map(writer, &path(MapKind::LayerMask), &layers)?);
    }
    Ok(ilm_bound)
}

/// Writes the map document `map` at `path` unless a file is there, with its sidecar at the defaults a
/// cook would give it: its ID.
fn map<T: sapphire_reflect::Versioned + serde::Serialize + bevy_reflect::TypePath>(
    writer: &mut Writer<'_>,
    path: &Path,
    map: &T,
) -> Result<sapphire_assets::AssetId, ScaffoldError> {
    let text = sapphire_serialize::to_ron(map).expect("a map document always serializes");
    writer.file(path, || Ok(text.into_bytes()))?;
    let settings = ImportSettings::for_source(path).expect("a map document's path names its kind");
    Ok(writer.sidecar(path, settings)?)
}

/// A slot's ramp: a row per region of `document`, its shade color from end to end, or with a deep
/// threshold its deep shade color below it, as the band mode tinted the shadow side.
fn ramp(slot: &str, document: &AnimeCharacterMaterial, deep_threshold: Option<f32>) -> RampMap {
    let color = |rgb: [f32; 3]| {
        let [r, g, b] = rgb.map(|c| crate::color::unorm8(crate::color::linear_to_srgb(c)));
        SrgbColor([r, g, b, 255])
    };
    let stop = |at: f32, rgb: [f32; 3]| ColorStop { at, color: color(rgb), interpolation: None };
    // Where the band mode's deep threshold falls along the ramp, which runs from the split's input at
    // -1 (0) to the light split's threshold (1).
    let deep = deep_threshold
        .map(|deep| ((deep + 1.0) / (document.light_split.threshold + 1.0).max(1e-4)).clamp(0.0, 1.0));
    let materials = document
        .regions
        .iter()
        .enumerate()
        .map(|(i, region)| {
            let stops = match deep {
                Some(at) => vec![
                    stop(0.0, region.deep_shade_color),
                    stop(at, region.deep_shade_color),
                    stop(at, region.shade_color),
                    stop(1.0, region.shade_color),
                ],
                None => vec![stop(0.0, region.shade_color), stop(1.0, region.shade_color)],
            };
            let name = if document.regions.len() == 1 { slot.to_owned() } else { format!("{slot} {i}") };
            MaterialRamps { name, day: Ramp::Stops(StopRamp { interpolation: None, stops }), night: None }
        })
        .collect();
    RampMap { interpolation: Default::default(), materials }
}

/// Bakes slot `index`'s threshold masks from its normals into `dir` as `<stem>_threshold_<angle>.png`,
/// unless they are there, and gives the face SDF document that reads them.
fn face_sdf(
    slot: &str,
    index: usize,
    stem: &str,
    dir: &Path,
    mesh: &crate::mesh::ImportedMesh,
    writer: &mut Writer<'_>,
) -> Result<FaceSdfMap, ScaffoldError> {
    let names: Vec<(f32, String)> =
        face::ANGLES.iter().map(|&angle| (angle, format!("{stem}{}", face::mask_suffix(angle)))).collect();
    if writer.planning() {
        for (_, name) in &names {
            writer.file(&dir.join(name), || unreachable!("a plan writes nothing"))?;
        }
    } else if names.iter().any(|(_, name)| !dir.join(name).exists()) {
        let corners =
            mesh.lods[0].submeshes.iter().filter(|submesh| submesh.material as usize == index).flat_map(
                |submesh| {
                    let range =
                        submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
                    mesh.indices[range].as_chunks::<3>().0.iter().map(|triangle| {
                        triangle.map(|i| {
                            let vertex = &mesh.vertices[i as usize];
                            (vertex.uv, Vec3::from(vertex.normal))
                        })
                    })
                },
            );
        let masks = face::bake(corners, MASK_SIZE)
            .map_err(|source| ScaffoldError::Masks { slot: slot.to_owned(), source })?;
        for ((_, name), (_, png)) in names.iter().zip(masks) {
            writer.file(&dir.join(name), || Ok(png))?;
        }
    }
    let horizontal = names.into_iter().map(|(angle, path)| ThresholdMask { angle, path }).collect();
    // The bake's masks are black and white, so their edges read as binary ones.
    Ok(FaceSdfMap { horizontal, vertical: Vec::new(), allowed_violations: 0, edges: Edges::Binary })
}

#[cfg(test)]
mod tests;
