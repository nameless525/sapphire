//! `extract-materials` (M1 A6): a mesh source's materials written out as material documents
//! bound in its sidecar, with a VRM's images as texture sources.
//!
//! # An explicit command
//!
//! Extraction runs only when asked (`sapphire_pipeline extract-materials <source>`), never as
//! part of a cook, as A3 decided:
//!
//! - A cook is a function of the source tree: it writes no sources and rewrites no sidecars, so
//!   cache keys hold and `--locked` cooks (CI) change nothing. A cook that extracted would add
//!   sources mid-run, and give them IDs a CI run throws away.
//! - Extracted files belong to the artist from then on. Running the command again adds only
//!   what is missing and overwrites nothing, so edits survive.
//! - A VRM draws without it: its import generates untextured materials from MToon's factors.
//!   Extraction brings its textures in, and starts the authoring (ramps, ILM maps, SDF face
//!   maps) that the look-dev character needs on top.
//!
//! # What it writes
//!
//! For `characters/alicia.vrm`, under `characters/alicia/` (the source's file stem; two mesh
//! sources with one stem in one directory would share it, so that is an error):
//!
//! | Path | Sidecar | What |
//! |---|---|---|
//! | `textures/<image>.png` | `Texture((class: …))` | An image a material uses, classed by its use ([`TextureUse::class`]): color for base, shade, matcap and emission; normal for normal maps; a mask for rim, outline-width and UV-animation masks. An image used in two classes is written once per class, `<image>-<class>.png` |
//! | `maps/<image>.png` | none | A shading shift texture, as the input of the ILM documents beside it: part of their assets (A7) |
//! | `maps/<image>.ilm.ron` | `CharacterMap(Ilm(…))` | A control map: the input's channel as the shadow bias over the range its material's scale gives ([`ShadingShift::shadow_bias_range`]), the other channels neutral; one per image and range |
//! | `materials/<slot>.material.ron` | `Material(())` | The slot's anime character material ([`Mtoon::to_material`]) binding those textures, or for a slot that isn't MToon the material its importer generated |
//!
//! and the source's sidecar binds each slot to its document (`materials: {"Skin": "<id>"}`).
//!
//! **Slots:** by default those whose documents bind a texture (base color, normal, matcap or a
//! control map): the ones a generated material can't draw as authored. `--all` extracts every
//! slot, as starting points for authoring. A slot the sidecar already binds is left alone.
//!
//! **Images:** every image an extracted MToon material uses is written, including those the
//! anime character family can't bind yet (a shade texture other than the base color's, rim,
//! outline-width and UV-animation masks, emission), so nothing authored is lost; they cook,
//! unreferenced, until C5 and C7 give them slots. PNG images are copied byte for byte; JPEG
//! images are decoded and written as PNG, which the texture importer reads. Block compression
//! needs sizes that are multiples of 4; an image that isn't fails its texture's cook with A2's
//! message, and is the artist's to resize (VRM exporters write powers of two).
//!
//! **Names** come from the image (or its texture's) and slot names, reduced to letters, digits,
//! `-` and `_`, and made unique ignoring case, so they are the same files on every file system.
//! They are decided over every slot of the source and every image its MToon materials use, not
//! only what one run extracts, so a file name always stands for the same slot, texture or map
//! while the source stays the same, and a later run reuses exactly the files an earlier one
//! wrote.
//!
//! # Running it again
//!
//! Nothing is overwritten. A file that exists keeps its contents, and with its sidecar its ID; a
//! file without a sidecar gets one. A slot whose document exists but isn't bound (its binding
//! was deleted) is bound to that document again. IDs are therefore stable across runs, and
//! deleting a document and its binding extracts it afresh.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};

use sapphire_assets::AssetId;
use sapphire_assets::character_maps::ilm::{NEUTRAL, SPECULAR_MASK, SPECULAR_THRESHOLD};
use sapphire_assets::material::AnimeCharacterMaterial;

use crate::character_maps::ilm::{ChannelInput, IlmMap, ImageChannel, MaterialIdInput};
use crate::character_maps::{MapKind, MapSettings};
use crate::import::gltf::{self, GltfImportError};
use crate::import::vrm::mtoon::{Channel, Mtoon};
use crate::import::vrm::{self, ImageFormat, VrmImport, VrmImportError};
use crate::material::MaterialSettings;
use crate::meta::{self, AssetMeta, ImportSettings, MetaError};
use crate::texture::{IlmSettings, TextureClass, TextureSettings};

/// Options for [`extract_materials`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtractOptions {
    /// Every slot the sidecar doesn't bind, not only those that bind textures.
    pub all: bool,
}

/// What [`extract_materials`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extraction {
    /// Files written, sources and sidecars, in order.
    pub written: Vec<PathBuf>,
    /// Slots this run bound, with their documents' IDs.
    pub bound: Vec<(String, AssetId)>,
    /// Slots left alone because the sidecar already binds them.
    pub kept: Vec<String>,
}

/// Errors from [`extract_materials`].
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("{} has no sidecar: cook the content tree once to give it its ID, then extract", .0.display())]
    NoSidecar(PathBuf),
    #[error("{} isn't a mesh source (.gltf, .glb or .vrm)", .0.display())]
    NotAMesh(PathBuf),
    #[error(transparent)]
    Meta(#[from] MetaError),
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
    #[error("failed to import {}", path.display())]
    Gltf {
        path: PathBuf,
        #[source]
        source: GltfImportError,
    },
    #[error("failed to import {}", path.display())]
    Vrm {
        path: PathBuf,
        /// Boxed, as in cook errors.
        #[source]
        source: Box<VrmImportError>,
    },
    #[error(
        "{} and {} would both extract into {}; rename one",
        path.display(),
        other.display(),
        folder.display()
    )]
    SharedFolder { path: PathBuf, other: PathBuf, folder: PathBuf },
    #[error("failed to extract an image")]
    Image(#[source] Box<VrmImportError>),
    #[error("failed to write image {index} as a PNG")]
    Encode {
        index: usize,
        #[source]
        source: image::ImageError,
    },
}

/// A slot to extract.
struct Slot {
    name: String,
    /// What the importer generated: the document of a slot that isn't MToon.
    generated: AnimeCharacterMaterial,
    mtoon: Option<Mtoon>,
}

/// Extracts the materials of the mesh source at `source` (see the [module docs](self)). The
/// source must have a sidecar.
pub fn extract_materials(source: &Path, options: &ExtractOptions) -> Result<Extraction, ExtractError> {
    let sidecar = meta::sidecar_path(source);
    if !sidecar.exists() {
        return Err(ExtractError::NoSidecar(source.to_owned()));
    }
    let mut asset = meta::read(&sidecar)?;
    let bytes =
        fs::read(source).map_err(|error| ExtractError::Read { path: source.to_owned(), source: error })?;
    let (slots, import) = match &asset.importer {
        ImportSettings::Gltf(_) => {
            let mesh = gltf::import_slice(&bytes, source.parent())
                .map_err(|error| ExtractError::Gltf { path: source.to_owned(), source: error })?;
            let slots = mesh
                .materials
                .into_iter()
                .map(|m| Slot { name: m.name, generated: m.generated, mtoon: None })
                .collect();
            (slots, None)
        }
        ImportSettings::Vrm(settings) => {
            let import = vrm::import(&bytes, settings.version)
                .map_err(|error| ExtractError::Vrm { path: source.to_owned(), source: Box::new(error) })?;
            let slots: Vec<Slot> = import
                .mesh
                .materials
                .iter()
                .zip(&import.slots)
                .map(|(m, mtoon)| Slot {
                    name: m.name.clone(),
                    generated: m.generated.clone(),
                    mtoon: mtoon.clone(),
                })
                .collect();
            (slots, Some(import))
        }
        ImportSettings::Texture(_)
        | ImportSettings::CharacterMap(_)
        | ImportSettings::Material(_)
        | ImportSettings::ProbeGrid(_)
        | ImportSettings::Assembly(_) => {
            return Err(ExtractError::NotAMesh(source.to_owned()));
        }
    };

    let mut extraction = Extraction::default();
    let bindings = &asset.importer.mesh().expect("mesh importers have mesh settings").materials;
    let mut chosen = Vec::new();
    for (index, slot) in slots.iter().enumerate() {
        if bindings.contains_key(&slot.name) {
            extraction.kept.push(slot.name.clone());
        } else if options.all || slot.mtoon.as_ref().is_some_and(Mtoon::is_textured) {
            chosen.push(index);
        }
    }
    if chosen.is_empty() {
        return Ok(extraction);
    }

    let root = folder(source)?;
    let names = Names::new(&slots, import.as_ref());
    let mut writer = Writer { written: Vec::new() };
    let mut resolved = Resolved::default();
    let mut bound = Vec::new();
    for index in chosen {
        let slot = &slots[index];
        let document = match (&slot.mtoon, &import) {
            (Some(mtoon), Some(import)) => {
                resolved.write(import, mtoon, &names, &root, &mut writer)?;
                let texture_of =
                    |usage| mtoon.textures().into_iter().find(|&(found, _)| found == usage).map(|(_, t)| t);
                mtoon.to_material(|usage| {
                    let texture = texture_of(usage)?;
                    let image = import.image_of(texture.texture);
                    match usage.class() {
                        Some(class) => resolved.textures.get(&(image, class.name())).copied(),
                        None => resolved.maps.get(&map_key(mtoon, image, texture.channel)?).copied(),
                    }
                })
            }
            _ => slot.generated.clone(),
        };
        let path = root.join("materials").join(format!(
            "{}{}",
            names.materials[index],
            crate::material::SOURCE_SUFFIX
        ));
        let text = sapphire_serialize::to_ron(&document).expect("a material document always serializes");
        writer.file(&path, || Ok(text.into_bytes()))?;
        let id = writer.sidecar(&path, ImportSettings::Material(MaterialSettings::default()))?;
        bound.push((slot.name.clone(), id));
    }

    let settings = asset.importer.mesh_mut().expect("mesh importers have mesh settings");
    for (slot, id) in &bound {
        settings.materials.insert(slot.clone(), *id);
    }
    meta::rewrite(&sidecar, &asset)?;
    writer.written.push(sidecar);
    extraction.written = writer.written;
    extraction.bound = bound;
    Ok(extraction)
}

/// The folder a source extracts into: beside it, named after its file stem. Another mesh source
/// with that stem in the same directory would share it, so that is an error.
fn folder(source: &Path) -> Result<PathBuf, ExtractError> {
    let parent = source.parent().unwrap_or(Path::new(""));
    let stem = source.file_stem().unwrap_or_default();
    let read_error = |error| ExtractError::Read { path: parent.to_owned(), source: error };
    let same_stem = |path: &Path| {
        path.file_stem().is_some_and(|other| {
            other.to_string_lossy().to_lowercase() == stem.to_string_lossy().to_lowercase()
        })
    };
    for entry in fs::read_dir(if parent.as_os_str().is_empty() { Path::new(".") } else { parent })
        .map_err(read_error)?
    {
        let other = entry.map_err(read_error)?.path();
        let is_mesh = ImportSettings::for_source(&other).is_some_and(|settings| settings.mesh().is_some());
        if is_mesh && same_stem(&other) && other.file_name() != source.file_name() {
            return Err(ExtractError::SharedFolder {
                path: source.to_owned(),
                other,
                folder: parent.join(stem),
            });
        }
    }
    Ok(parent.join(stem))
}

/// An ILM document's key: its input image, the channel it reads and its range (as bits). One
/// document stands for each.
type MapKey = (usize, Channel, [u32; 2]);

/// The ILM document key of a material's shading shift texture.
fn map_key(mtoon: &Mtoon, image: usize, channel: Channel) -> Option<MapKey> {
    Some((image, channel, mtoon.shading_shift_texture?.shadow_bias_range().map(f32::to_bits)))
}

/// The file names of everything a source can extract, decided over all its slots and every
/// image its MToon materials use, whatever one run extracts: a file name then always stands
/// for the same slot, texture or map while the source stays the same.
struct Names {
    /// Per slot, its document's file stem.
    materials: Vec<String>,
    /// Per image and texture class name, the texture's file name.
    textures: BTreeMap<(usize, &'static str), String>,
    /// Per image, the file name of its copy as an ILM input.
    inputs: BTreeMap<usize, String>,
    /// Per ILM document key ([`map_key`]), the document's file stem and its range.
    maps: BTreeMap<MapKey, (String, [f32; 2])>,
    /// Per image and texture class name, the class.
    classes: BTreeMap<(usize, &'static str), TextureClass>,
}

impl Names {
    fn new(slots: &[Slot], import: Option<&VrmImport>) -> Self {
        let mut materials = UniqueNames::default();
        let mut names = Self {
            materials: slots.iter().map(|slot| materials.claim(&file_name(&slot.name), "material")).collect(),
            textures: BTreeMap::new(),
            inputs: BTreeMap::new(),
            maps: BTreeMap::new(),
            classes: BTreeMap::new(),
        };
        let Some(import) = import else { return names };
        let mut maps = BTreeMap::new();
        for mtoon in slots.iter().filter_map(|slot| slot.mtoon.as_ref()) {
            for (usage, texture) in mtoon.textures() {
                let image = import.image_of(texture.texture);
                match usage.class() {
                    Some(class) => {
                        names.classes.insert((image, class.name()), class);
                    }
                    None => {
                        let key = map_key(mtoon, image, texture.channel).expect("a shading shift use");
                        maps.insert(key, mtoon.shading_shift_texture.expect("checked").shadow_bias_range());
                    }
                }
            }
        }
        let images: BTreeSet<usize> = names
            .classes
            .keys()
            .map(|&(image, _)| image)
            .chain(maps.keys().map(|&(image, ..)| image))
            .collect();
        let mut image_names = UniqueNames::default();
        let base: BTreeMap<usize, String> = images
            .iter()
            .map(|&image| {
                let name =
                    import.image(image).ok().and_then(|embedded| embedded.name).map(|name| file_name(&name));
                (image, image_names.claim(name.as_deref().unwrap_or_default(), &format!("image{image}")))
            })
            .collect();
        // Color first: the class an image's plain name goes to when it has several.
        let mut files = UniqueNames::default();
        for &image in &images {
            let mut classes: Vec<&'static str> =
                names.classes.keys().filter(|&&(i, _)| i == image).map(|&(_, class)| class).collect();
            classes.sort_by_key(|&class| (class != "color", class));
            for &class in &classes {
                let stem = if classes.len() == 1 {
                    base[&image].clone()
                } else {
                    format!("{}-{class}", base[&image])
                };
                names.textures.insert((image, class), format!("{}.png", files.claim(&stem, &stem)));
            }
        }
        let mut documents = UniqueNames::default();
        for (key, range) in maps {
            let image = key.0;
            names.inputs.insert(image, format!("{}.png", base[&image]));
            names.maps.insert(key, (documents.claim(&base[&image], &base[&image]), range));
        }
        names
    }
}

/// The assets extraction wrote or found, by what the documents ask for.
#[derive(Default)]
struct Resolved {
    /// By image and texture class name.
    textures: BTreeMap<(usize, &'static str), AssetId>,
    /// ILM documents by [`map_key`].
    maps: BTreeMap<MapKey, AssetId>,
}

impl Resolved {
    /// Writes (or finds) the textures, ILM inputs and ILM documents `mtoon` uses.
    fn write(
        &mut self,
        import: &VrmImport,
        mtoon: &Mtoon,
        names: &Names,
        root: &Path,
        writer: &mut Writer,
    ) -> Result<(), ExtractError> {
        for (usage, texture) in mtoon.textures() {
            let image = import.image_of(texture.texture);
            if let Some(class) = usage.class() {
                let key = (image, class.name());
                if let Entry::Vacant(entry) = self.textures.entry(key) {
                    let path = root.join("textures").join(&names.textures[&key]);
                    writer.file(&path, || png(import, image))?;
                    let class = names.classes[&key];
                    entry.insert(writer.sidecar(&path, ImportSettings::Texture(TextureSettings { class }))?);
                }
                continue;
            }
            let key = map_key(mtoon, image, texture.channel).expect("a shading shift use");
            if self.maps.contains_key(&key) {
                continue;
            }
            let input = &names.inputs[&image];
            writer.file(&root.join("maps").join(input), || png(import, image))?;
            let (stem, range) = &names.maps[&key];
            let channel = match texture.channel {
                Channel::R | Channel::Rgba => ImageChannel::R,
                Channel::G => ImageChannel::G,
                Channel::B => ImageChannel::B,
            };
            let document = IlmMap {
                size: None,
                specular_mask: ChannelInput::Constant(NEUTRAL[SPECULAR_MASK]),
                shadow_bias: ChannelInput::Image { path: input.clone(), channel, range: *range },
                specular_threshold: ChannelInput::Constant(NEUTRAL[SPECULAR_THRESHOLD]),
                material_id: MaterialIdInput::Constant(0),
            };
            let path = root.join("maps").join(format!("{stem}{}", MapKind::Ilm.suffix()));
            let text = sapphire_serialize::to_ron(&document).expect("an ILM document always serializes");
            writer.file(&path, || Ok(text.into_bytes()))?;
            let settings = ImportSettings::CharacterMap(MapSettings::Ilm(IlmSettings::default()));
            self.maps.insert(key, writer.sidecar(&path, settings)?);
        }
        Ok(())
    }
}

/// Image `index` as PNG bytes: PNG images as they are, others decoded and encoded.
fn png(import: &VrmImport, index: usize) -> Result<Vec<u8>, ExtractError> {
    let image_error = |error| ExtractError::Image(Box::new(error));
    let embedded = import.image(index).map_err(image_error)?;
    if embedded.format == ImageFormat::Png {
        return Ok(embedded.bytes.to_vec());
    }
    let decoded = import.decode(index).map_err(image_error)?;
    let mut bytes = Vec::new();
    decoded
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .map_err(|source| ExtractError::Encode { index, source })?;
    Ok(bytes)
}

/// Writes files that don't exist yet, and sidecars, recording what it wrote.
struct Writer {
    written: Vec<PathBuf>,
}

impl Writer {
    /// Writes `path` with `contents()` unless it exists.
    fn file(
        &mut self,
        path: &Path,
        contents: impl FnOnce() -> Result<Vec<u8>, ExtractError>,
    ) -> Result<(), ExtractError> {
        if path.exists() {
            return Ok(());
        }
        let bytes = contents()?;
        let write_error = |source| ExtractError::Write { path: path.to_owned(), source };
        fs::create_dir_all(path.parent().expect("extracted files are in a directory"))
            .map_err(write_error)?;
        fs::write(path, bytes).map_err(write_error)?;
        self.written.push(path.to_owned());
        Ok(())
    }

    /// The ID of `source`'s sidecar, created with `importer` and a new ID if there is none.
    fn sidecar(&mut self, source: &Path, importer: ImportSettings) -> Result<AssetId, ExtractError> {
        let path = meta::sidecar_path(source);
        if path.exists() {
            return Ok(meta::read(&path)?.id);
        }
        let meta = AssetMeta { id: meta::new_id().map_err(MetaError::Entropy)?, importer };
        if meta::create(&path, &meta)? {
            self.written.push(path);
            Ok(meta.id)
        } else {
            Ok(meta::read(&path)?.id)
        }
    }
}

/// File names, unique ignoring case.
#[derive(Default)]
struct UniqueNames {
    taken: HashSet<String>,
}

impl UniqueNames {
    /// `base` (or `fallback` when empty), with `-2`, `-3`, … appended until no name taken so far
    /// matches it ignoring case.
    fn claim(&mut self, base: &str, fallback: &str) -> String {
        let base = if base.is_empty() { fallback } else { base };
        let mut name = base.to_owned();
        let mut suffix = 1;
        while !self.taken.insert(name.to_lowercase()) {
            suffix += 1;
            name = format!("{base}-{suffix}");
        }
        name
    }
}

/// `name` reduced to a file name: letters, digits, `-` and `_`, others as `_`, without leading
/// or trailing `_`. Empty when nothing is left.
fn file_name(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    cleaned.trim_matches('_').to_owned()
}

#[cfg(test)]
mod tests;
