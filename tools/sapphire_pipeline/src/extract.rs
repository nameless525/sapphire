//! `extract-materials` (M1 A6): a mesh source's materials written out as material documents
//! bound in its sidecar, with a VRM's images as texture sources, and the images an FBX's
//! materials use bound (M1 A10).
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
//!   maps) that the look-dev character needs on top. An FBX likewise draws in its generated
//!   materials' colors until extraction binds the images they use.
//!
//! # What it writes
//!
//! For `characters/alicia.vrm`, under `characters/alicia/` (the source's file stem; two mesh
//! sources with one stem in one directory would share it, so that is an error):
//!
//! | Path | Sidecar | What |
//! |---|---|---|
//! | `textures/<image>.png` | `Texture((class: …))` | An image a material uses, classed by its use ([`TextureUse::class`](crate::import::vrm::mtoon::TextureUse::class)): color for base, shade, matcap, rim multiply and emissive textures; normal for normal maps; a mask for outline-width and UV-animation masks. An image used in two classes is written once per class, `<image>-<class>.png` |
//! | `maps/<image>.png` | none | A shading shift texture, or a texture a layer mask holds, as the input of the documents beside it: part of their assets (A7) |
//! | `maps/<image>.ilm.ron` | `CharacterMap(Ilm(…))` | A control map: the input's channel as the shadow bias over the range its material's scale gives ([`ShadingShift::shadow_bias_range`](crate::import::vrm::mtoon::ShadingShift::shadow_bias_range)), the other channels neutral; one per image and range |
//! | `maps/<images>.layers.ron` | `CharacterMap(LayerMask(…))` | A body's layer mask (M1 C7; [`Mtoon::layer_mask`]): R the rim multiply texture's brightest channel, masking the matcap, and B the emissive texture's, painting the glow, both decoded from sRGB, the other channels 1; one per pair of inputs, named after their images |
//! | `materials/<slot>.material.ron` | `Material(())` | The slot's anime character material ([`Mtoon::to_material`]) binding those textures and maps, or for a slot that isn't MToon the material its importer generated |
//!
//! and the source's sidecar binds each slot to its document (`materials: {"Skin": "<id>"}`).
//!
//! **Slots:** by default those whose documents bind a texture (base color, normal or matcap, a
//! control map or a layer mask): the ones a generated material can't draw as authored. `--all`
//! extracts every slot, as starting points for authoring. A slot the sidecar already binds is
//! left alone.
//!
//! **FBX textures** (M1 A10): a slot's document is the material its import generated, with the
//! base color and normal map images its FBX material uses bound as `base_color_map` and
//! `normal_map`.
//!
//! - **Bound where they lie:** an image the FBX names (relative to itself, or absolute) that is a
//!   texture source (PNG, TGA or EXR) inside the FBX's folder tree is bound through its own
//!   sidecar. Paths are compared canonical, so `..`, `.` and links can't lead out of the folder,
//!   and two spellings of one file are one image.
//! - **One role an image** (review BR-002): an image keeps one class, its *first role*, decided
//!   over every slot before anything is written. With no sidecar that is its first use's class,
//!   color before normal, which its sidecar is created as. A sidecar a cook created with the
//!   default settings (color) is switched to the image's use, keeping its ID, when that use is
//!   its only one and no material document under the FBX's folder binds it; otherwise, and with
//!   any other sidecar, the sidecar's class stays as it is. A use of another class is left
//!   unbound, so the tree still cooks, and reported ([`Extraction::conflicts`]): giving that use
//!   an image of its own (a copy) is the artist's call. Material documents elsewhere in the
//!   content tree aren't looked at; the next cook catches one an earlier switch broke.
//! - **Never outside the folder** (review BR-003): extraction neither binds nor reads an image
//!   outside the FBX's folder tree, as a downloaded kit's `../Textures/` or an absolute path from
//!   the exporting machine names, which no cook of the folder would see. It fails, naming the
//!   slot and the image, and asks for a copy beside the FBX (listed in the folder's
//!   `MODIFIED.md` for third-party content): an image not found inside the folder is looked for
//!   there by its file name. Nothing is written before every image the extracted slots use is
//!   found.
//! - **Written out:** an image the FBX embeds, or names in another format (JPEG), is written as
//!   `textures/<image>.png` as a VRM's are (PNG bytes copied, others converted), once per class
//!   it is used as. An image that is neither embedded nor found fails the extraction, naming the
//!   slot and the file.
//!
//! **Images:** every image an extracted MToon material uses is written, including those the
//! anime character family can't bind (a shade texture other than the base color's, a rim
//! multiply or emissive texture no layer mask holds, outline-width and UV-animation masks), so
//! nothing authored is lost; they cook, unreferenced. PNG images are copied byte for byte; JPEG
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

use crate::character_maps::ilm::{ChannelInput, Encoding, IlmMap, ImageChannel, MaterialIdInput};
use crate::character_maps::layer_mask::{self, LayerMaskMap};
use crate::character_maps::{MapKind, MapSettings};
use crate::import::fbx::{self, FbxImportError, FbxTexture, SlotTextures};
use crate::import::gltf::{self, GltfImportError};
use crate::import::image::SourceFormat;
use crate::import::vrm::mtoon::{Binding, Channel, LayerTexture, Mtoon};
use crate::import::vrm::{self, ImageFormat, VrmImport, VrmImportError};
use crate::material::{MaterialSettings, TextureKind};
use crate::meta::{self, AssetMeta, ImportSettings, MetaError};
use crate::texture::{
    ColorSettings, IlmSettings, MaskSettings, NormalSettings, TextureClass, TextureSettings,
};

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
    /// Uses of FBX images left unbound, because the image keeps another role.
    pub conflicts: Vec<RoleConflict>,
}

/// A slot's use of an FBX image that extraction left unbound, because the image, bound where it
/// lies, keeps another role (see the [module docs](self)): its document draws without that map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleConflict {
    /// The slot whose material uses the image.
    pub slot: String,
    /// The image, as a canonical path.
    pub image: PathBuf,
    /// The class the slot's material uses it as (`color` or `normal`).
    pub used_as: &'static str,
    /// The class it keeps: its sidecar's (`non-texture` if that isn't a texture's), or its first
    /// use's.
    pub keeps: &'static str,
}

/// Errors from [`extract_materials`].
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("{} has no sidecar: cook the content tree once to give it its ID, then extract", .0.display())]
    NoSidecar(PathBuf),
    #[error("{} isn't a mesh source (.gltf, .glb, .fbx or .vrm)", .0.display())]
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
    Fbx {
        path: PathBuf,
        /// Boxed, as in cook errors.
        #[source]
        source: Box<FbxImportError>,
    },
    #[error("slot {slot:?} uses the image {texture}, which isn't embedded or beside the FBX")]
    MissingTexture { slot: String, texture: String },
    #[error(
        "slot {slot:?} uses the image {}, which lies outside {}, the FBX's folder, so it can't be \
         bound: copy it to {} (and list the copy in the folder's MODIFIED.md if it is third-party \
         content), then extract again",
        image.display(),
        folder.display(),
        copy.display()
    )]
    OutsideFolder { slot: String, image: PathBuf, folder: PathBuf, copy: PathBuf },
    #[error("failed to convert the image {texture} to a PNG")]
    TextureImage {
        texture: String,
        #[source]
        source: image::ImageError,
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
    /// The images an FBX slot's material uses; none for other sources.
    textures: SlotTextures,
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
                .map(|m| Slot {
                    name: m.name,
                    generated: m.generated,
                    mtoon: None,
                    textures: SlotTextures::default(),
                })
                .collect();
            (slots, None)
        }
        ImportSettings::Fbx(_) => {
            let import = fbx::import_with_textures(&bytes)
                .map_err(|error| ExtractError::Fbx { path: source.to_owned(), source: Box::new(error) })?;
            let slots = import
                .mesh
                .materials
                .into_iter()
                .zip(import.slots)
                .map(|(m, textures)| Slot { name: m.name, generated: m.generated, mtoon: None, textures })
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
                    textures: SlotTextures::default(),
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
        } else if options.all
            || slot.mtoon.as_ref().is_some_and(Mtoon::is_textured)
            || fbx_uses(&slot.textures).next().is_some()
        {
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
    let mut images = FbxImages::new(source, fbx_image_names(&slots))?;
    images.plan(&slots, &chosen)?;
    let mut bound = Vec::new();
    for index in chosen {
        let slot = &slots[index];
        let document = match (&slot.mtoon, &import) {
            (Some(mtoon), Some(import)) => {
                resolved.write(import, mtoon, &names, &root, &mut writer)?;
                let texture_of =
                    |usage| mtoon.textures().into_iter().find(|&(found, _)| found == usage).map(|(_, t)| t);
                mtoon.to_material(|binding| match binding {
                    Binding::Texture(usage) => {
                        let image = import.image_of(texture_of(usage)?.texture);
                        resolved.textures.get(&(image, usage.class()?.name())).copied()
                    }
                    Binding::ControlMap => {
                        let shift = mtoon.shading_shift_texture?.texture;
                        let key = map_key(mtoon, import.image_of(shift.texture), shift.channel)?;
                        resolved.maps.get(&key).copied()
                    }
                    Binding::LayerMask => resolved.layers.get(&layer_key(import, mtoon)?).copied(),
                })
            }
            _ => {
                let mut document = slot.generated.clone();
                for (class, texture) in fbx_uses(&slot.textures) {
                    let Some(id) = images.bind(&slot.name, texture, class, &root, &mut writer)? else {
                        continue;
                    };
                    match class {
                        TextureClass::Normal(_) => document.normal_map = Some(id),
                        _ => document.base_color_map = Some(id),
                    }
                }
                document
            }
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
    extraction.conflicts = images.conflicts;
    Ok(extraction)
}

/// The images an FBX slot uses, with the class each cooks as: the base color as a color, the
/// normal map as a normal map.
fn fbx_uses(textures: &SlotTextures) -> impl Iterator<Item = (TextureClass, &FbxTexture)> {
    let color = TextureClass::Color(ColorSettings::default());
    let normal = TextureClass::Normal(NormalSettings::default());
    [(color, &textures.base_color), (normal, &textures.normal)]
        .into_iter()
        .filter_map(|(class, texture)| Some((class, texture.as_ref()?)))
}

/// What identifies an FBX image: the file it names, or its name when it names none.
fn fbx_image_key(texture: &FbxTexture, class: TextureClass) -> (String, &'static str) {
    let identity = if texture.file.is_empty() { format!("#{}", texture.name) } else { texture.file.clone() };
    (identity, class.name())
}

/// The file name each FBX image would be written under (by [`fbx_image_key`]), decided over
/// every slot, whatever one run writes, as [`Names`] decides a VRM's: `<image>.png`, or
/// `<image>-<class>.png` for an image used as two classes.
fn fbx_image_names(slots: &[Slot]) -> BTreeMap<(String, &'static str), String> {
    let mut uses: BTreeMap<String, (String, Vec<&'static str>)> = BTreeMap::new();
    for slot in slots {
        for (class, texture) in fbx_uses(&slot.textures) {
            let (identity, class) = fbx_image_key(texture, class);
            let (_, classes) = uses.entry(identity).or_insert_with(|| (file_name(&texture.name), Vec::new()));
            if !classes.contains(&class) {
                classes.push(class);
            }
        }
    }
    let mut files = UniqueNames::default();
    let mut names = BTreeMap::new();
    for (identity, (stem, mut classes)) in uses {
        // Color first: the class an image's plain name goes to when it has several.
        classes.sort_by_key(|&class| (class != "color", class));
        for &class in &classes {
            let stem = if classes.len() == 1 { stem.clone() } else { format!("{stem}-{class}") };
            names.insert((identity.clone(), class), format!("{}.png", files.claim(&stem, "image")));
        }
    }
    names
}

/// The bytes of an FBX image extraction writes out.
enum ImageSource<'t> {
    Embedded(&'t [u8]),
    /// A file inside the FBX's folder.
    File(PathBuf),
}

/// Where an image an FBX names lies.
enum Located {
    /// Inside the FBX's folder tree: its canonical path.
    Inside(PathBuf),
    /// Outside it: the file named.
    Outside(PathBuf),
    /// Nowhere on disk.
    Missing,
}

/// An FBX's images as extraction binds them (see the [module docs](self)).
struct FbxImages<'a> {
    /// The FBX's directory, which the files it names are relative to.
    dir: &'a Path,
    /// That directory, canonical: the tree images are bound where they lie in.
    folder: PathBuf,
    /// By [`fbx_image_key`].
    names: BTreeMap<(String, &'static str), String>,
    /// The texture assets written under the extraction folder so far, by [`fbx_image_key`].
    bound: BTreeMap<(String, &'static str), AssetId>,
    /// The role each image bound where it lies keeps, by its canonical path ([`Self::plan`]).
    roles: BTreeMap<PathBuf, Role>,
    conflicts: Vec<RoleConflict>,
}

/// The class an image bound where it lies keeps, whatever else the FBX uses it as.
#[derive(Clone, Copy, Debug)]
struct Role {
    /// Its class name: the sidecar's, or the class it is created or switched to.
    keeps: &'static str,
    /// The class a sidecar is created as, or switched to from the cook's default: an image with
    /// a sidecar that stays as it is has none.
    writes: Option<TextureClass>,
}

impl<'a> FbxImages<'a> {
    /// The images of the FBX at `source`, written under the file names `names` gives.
    fn new(source: &'a Path, names: BTreeMap<(String, &'static str), String>) -> Result<Self, ExtractError> {
        let dir = source.parent().unwrap_or(Path::new(""));
        let folder = fs::canonicalize(if dir.as_os_str().is_empty() { Path::new(".") } else { dir })
            .map_err(|error| ExtractError::Read { path: dir.to_owned(), source: error })?;
        Ok(Self { dir, folder, names, bound: BTreeMap::new(), roles: BTreeMap::new(), conflicts: Vec::new() })
    }

    /// Where the file `texture` names lies. One not inside the FBX's folder tree (outside it, or
    /// missing) is looked for beside the FBX by its file name: where the error for an image
    /// outside the folder asks for a copy.
    fn locate(&self, texture: &FbxTexture) -> Located {
        if texture.file.is_empty() {
            return Located::Missing;
        }
        let named = self.dir.join(&texture.file);
        // Canonical, so `..`, `.` and links can't lead out of the folder unseen.
        let inside = |path: &Path| {
            fs::canonicalize(path).ok().filter(|found| found.starts_with(&self.folder) && found.is_file())
        };
        if let Some(found) = inside(&named) {
            return Located::Inside(found);
        }
        let beside = Path::new(&texture.file).file_name().map(|name| self.dir.join(name));
        if let Some(found) = beside.as_deref().and_then(inside) {
            return Located::Inside(found);
        }
        if named.is_file() { Located::Outside(named) } else { Located::Missing }
    }

    /// The texture source inside the FBX's folder that `texture` binds where it lies, if any.
    fn in_place(&self, texture: &FbxTexture) -> Option<PathBuf> {
        match self.locate(texture) {
            Located::Inside(path) if SourceFormat::from_path(&path).is_some() => Some(path),
            _ => None,
        }
    }

    /// Decides, before anything is written, the role each image bound where it lies keeps
    /// ([`Self::keeps`]), over every use of every slot, whatever this run extracts (as file
    /// names are decided), and fails if an image `chosen` slots use can't be found or lies
    /// outside the FBX's folder.
    fn plan(&mut self, slots: &[Slot], chosen: &[usize]) -> Result<(), ExtractError> {
        let mut uses: BTreeMap<PathBuf, Vec<TextureClass>> = BTreeMap::new();
        for slot in slots {
            for (class, texture) in fbx_uses(&slot.textures) {
                if let Some(path) = self.in_place(texture) {
                    let classes = uses.entry(path).or_default();
                    if !classes.iter().any(|known| known.name() == class.name()) {
                        classes.push(class);
                    }
                }
            }
        }
        for (path, classes) in uses {
            let role = self.keeps(&path, &classes)?;
            self.roles.insert(path, role);
        }
        for &index in chosen {
            let slot = &slots[index];
            for (_, texture) in fbx_uses(&slot.textures) {
                self.source(&slot.name, texture)?;
            }
        }
        Ok(())
    }

    /// Where `texture`'s bytes come from when it isn't bound where it lies: its embedded copy, or
    /// a file inside the FBX's folder in another format. Fails for one found nowhere or outside
    /// the folder, which extraction never binds or reads.
    fn source<'t>(&self, slot: &str, texture: &'t FbxTexture) -> Result<ImageSource<'t>, ExtractError> {
        if let Some(bytes) = &texture.embedded {
            return Ok(ImageSource::Embedded(bytes));
        }
        match self.locate(texture) {
            Located::Inside(path) => Ok(ImageSource::File(path)),
            Located::Outside(image) => {
                let copy = self.dir.join(image.file_name().unwrap_or_default());
                Err(ExtractError::OutsideFolder {
                    slot: slot.to_owned(),
                    image,
                    folder: self.dir.to_owned(),
                    copy,
                })
            }
            Located::Missing => {
                Err(ExtractError::MissingTexture { slot: slot.to_owned(), texture: texture.to_string() })
            }
        }
    }

    /// The role of the image at `path`, which the FBX uses as `classes`:
    ///
    /// - With no sidecar, its first class, color before normal (as file names go), which its
    ///   sidecar is created as.
    /// - With the sidecar a cook gives every image (color, default settings), the one class the
    ///   FBX uses it as, which the sidecar is switched to, keeping its ID; unless the FBX uses it
    ///   as several, or a material document under the FBX's folder binds it (it is shared, and
    ///   keeps color).
    /// - With any other sidecar, the class it gives, unchanged.
    fn keeps(&self, path: &Path, classes: &[TextureClass]) -> Result<Role, ExtractError> {
        let first =
            *classes.iter().min_by_key(|class| (class.name() != "color", class.name())).expect("one use");
        let sidecar = meta::sidecar_path(path);
        if !sidecar.exists() {
            return Ok(Role { keeps: first.name(), writes: Some(first) });
        }
        let meta = meta::read(&sidecar)?;
        let keeps = TextureKind::of(&meta.importer).map_or("non-texture", TextureKind::name);
        let default = meta.importer == ImportSettings::Texture(TextureSettings::default());
        Ok(match classes {
            [only] if default && only.name() != keeps && !self.bound_elsewhere(meta.id)? => {
                Role { keeps: only.name(), writes: Some(*only) }
            }
            _ => Role { keeps, writes: None },
        })
    }

    /// Whether a material document under the FBX's folder binds asset `id`.
    fn bound_elsewhere(&self, id: AssetId) -> Result<bool, ExtractError> {
        let text = id.to_string();
        let read_error = |path: &Path| {
            let path = path.to_owned();
            move |source| ExtractError::Read { path, source }
        };
        let mut dirs = vec![self.folder.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in fs::read_dir(&dir).map_err(read_error(&dir))? {
                let entry = entry.map_err(read_error(&dir))?;
                let path = entry.path();
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                // Not following links: the folder's own tree only.
                let kind = entry.file_type().map_err(read_error(&path))?;
                if kind.is_dir() {
                    dirs.push(path);
                } else if kind.is_file()
                    && crate::material::is_source(&path)
                    && fs::read_to_string(&path).map_err(read_error(&path))?.contains(&text)
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// The texture asset that `texture`, used as `class` by `slot`, binds: the file where it
    /// lies, or one written under `root`; `None` if the image keeps another role, reported in
    /// [`Self::conflicts`].
    fn bind(
        &mut self,
        slot: &str,
        texture: &FbxTexture,
        class: TextureClass,
        root: &Path,
        writer: &mut Writer,
    ) -> Result<Option<AssetId>, ExtractError> {
        if let Some(path) = self.in_place(texture) {
            return self.bind_in_place(slot, &path, class, writer);
        }
        let key = fbx_image_key(texture, class);
        if let Some(&id) = self.bound.get(&key) {
            return Ok(Some(id));
        }
        let bytes = match self.source(slot, texture)? {
            ImageSource::Embedded(bytes) => bytes.to_vec(),
            ImageSource::File(path) => {
                fs::read(&path).map_err(|error| ExtractError::Read { path, source: error })?
            }
        };
        let path = root.join("textures").join(&self.names[&key]);
        writer.file(&path, || {
            as_png(&bytes)
                .map_err(|source| ExtractError::TextureImage { texture: texture.to_string(), source })
        })?;
        let id = writer.sidecar(&path, ImportSettings::Texture(TextureSettings { class }))?;
        self.bound.insert(key, id);
        Ok(Some(id))
    }

    /// The ID of the texture source at `path` (canonical) if it keeps `class`'s role, its
    /// sidecar created or switched as planned; `None`, reported, if it keeps another.
    fn bind_in_place(
        &mut self,
        slot: &str,
        path: &Path,
        class: TextureClass,
        writer: &mut Writer,
    ) -> Result<Option<AssetId>, ExtractError> {
        // Planned with every use, unless the file appeared since (then this use is its only one).
        let role = match self.roles.get(path) {
            Some(&role) => role,
            None => {
                let role = self.keeps(path, &[class])?;
                self.roles.insert(path.to_owned(), role);
                role
            }
        };
        if role.keeps != class.name() {
            self.conflicts.push(RoleConflict {
                slot: slot.to_owned(),
                image: path.to_owned(),
                used_as: class.name(),
                keeps: role.keeps,
            });
            return Ok(None);
        }
        let sidecar = meta::sidecar_path(path);
        let Some(class) = role.writes else { return Ok(Some(meta::read(&sidecar)?.id)) };
        let settings = ImportSettings::Texture(TextureSettings { class });
        if !sidecar.exists() {
            return writer.sidecar(path, settings).map(Some);
        }
        let mut meta = meta::read(&sidecar)?;
        if meta.importer != settings {
            meta.importer = settings;
            meta::rewrite(&sidecar, &meta)?;
            writer.written.push(sidecar);
        }
        Ok(Some(meta.id))
    }
}

/// `bytes` as a PNG: a PNG as it is, other formats the `image` crate reads decoded and encoded.
fn as_png(bytes: &[u8]) -> Result<Vec<u8>, image::ImageError> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(bytes.to_vec());
    }
    let mut png = Vec::new();
    image::load_from_memory(bytes)?.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)?;
    Ok(png)
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

/// A layer mask document's key: the image and channel its R (the matcap's mask) and its B (the
/// glow) read, if any. One document stands for each.
type LayerKey = [Option<(usize, Channel)>; 2];

/// The layer mask document key of a material, if it has a layer mask ([`Mtoon::layer_mask`]).
fn layer_key(import: &VrmImport, mtoon: &Mtoon) -> Option<LayerKey> {
    let mask = mtoon.layer_mask()?;
    let input = |texture: Option<LayerTexture>| {
        texture.map(|texture| (import.image_of(texture.texture.texture), texture.channel))
    };
    Some([input(mask.matcap), input(mask.emission)])
}

/// The ILM and layer mask documents' name for a channel.
fn image_channel(channel: Channel) -> ImageChannel {
    match channel {
        Channel::R | Channel::Rgba => ImageChannel::R,
        Channel::G => ImageChannel::G,
        Channel::B => ImageChannel::B,
    }
}

/// The file names of everything a source can extract, decided over all its slots and every
/// image its MToon materials use, whatever one run extracts: a file name then always stands
/// for the same slot, texture or map while the source stays the same.
struct Names {
    /// Per slot, its document's file stem.
    materials: Vec<String>,
    /// Per image and texture class name, the texture's file name.
    textures: BTreeMap<(usize, &'static str), String>,
    /// Per image, the file name of its copy as an ILM or layer mask input.
    inputs: BTreeMap<usize, String>,
    /// Per ILM document key ([`map_key`]), the document's file stem and its range.
    maps: BTreeMap<MapKey, (String, [f32; 2])>,
    /// Per layer mask document key ([`layer_key`]), the document's file stem.
    layers: BTreeMap<LayerKey, String>,
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
            layers: BTreeMap::new(),
            classes: BTreeMap::new(),
        };
        let Some(import) = import else { return names };
        let mut maps = BTreeMap::new();
        let mut layers = BTreeSet::new();
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
            layers.extend(layer_key(import, mtoon));
        }
        let layer_inputs = |key: &LayerKey| key.iter().flatten().map(|&(image, _)| image).collect::<Vec<_>>();
        let images: BTreeSet<usize> = names
            .classes
            .keys()
            .map(|&(image, _)| image)
            .chain(maps.keys().map(|&(image, ..)| image))
            .chain(layers.iter().flat_map(layer_inputs))
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
        // Layer masks are named after their inputs' images, R's then B's.
        let mut documents = UniqueNames::default();
        for key in layers {
            let images = layer_inputs(&key);
            for &image in &images {
                names.inputs.insert(image, format!("{}.png", base[&image]));
            }
            let mut stems: Vec<&str> = images.iter().map(|image| base[image].as_str()).collect();
            stems.dedup();
            let stem = stems.join("-");
            names.layers.insert(key, documents.claim(&stem, &stem));
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
    /// Layer mask documents by [`layer_key`].
    layers: BTreeMap<LayerKey, AssetId>,
}

impl Resolved {
    /// Writes (or finds) the textures, map inputs, ILM documents and layer mask document
    /// `mtoon` uses.
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
            let input = Self::input(import, image, names, root, writer)?;
            let (stem, range) = &names.maps[&key];
            let document = IlmMap {
                size: None,
                specular_mask: ChannelInput::Constant(NEUTRAL[SPECULAR_MASK]),
                shadow_bias: ChannelInput::Image {
                    path: input,
                    channel: image_channel(texture.channel),
                    range: *range,
                    encoding: Encoding::Linear,
                },
                specular_threshold: ChannelInput::Constant(NEUTRAL[SPECULAR_THRESHOLD]),
                material_id: MaterialIdInput::Constant(0),
            };
            let path = root.join("maps").join(format!("{stem}{}", MapKind::Ilm.suffix()));
            let text = sapphire_serialize::to_ron(&document).expect("an ILM document always serializes");
            writer.file(&path, || Ok(text.into_bytes()))?;
            let settings = ImportSettings::CharacterMap(MapSettings::Ilm(IlmSettings::default()));
            self.maps.insert(key, writer.sidecar(&path, settings)?);
        }
        if let Some(key) = layer_key(import, mtoon)
            && !self.layers.contains_key(&key)
        {
            // Each painted channel is its texture's brightest, decoded from sRGB; the rest are 1.
            let mut channel = |held: Option<(usize, Channel)>| match held {
                Some((image, channel)) => Ok(ChannelInput::Image {
                    path: Self::input(import, image, names, root, writer)?,
                    channel: image_channel(channel),
                    range: [0.0, 1.0],
                    encoding: Encoding::Srgb,
                }),
                None => Ok::<_, ExtractError>(layer_mask::unpainted()),
            };
            let [matcap, emission] = key;
            let document = LayerMaskMap {
                size: None,
                matcap: channel(matcap)?,
                metallic: layer_mask::unpainted(),
                emission: channel(emission)?,
                stocking: layer_mask::unpainted(),
            };
            let stem = &names.layers[&key];
            let path = root.join("maps").join(format!("{stem}{}", MapKind::LayerMask.suffix()));
            let text =
                sapphire_serialize::to_ron(&document).expect("a layer mask document always serializes");
            writer.file(&path, || Ok(text.into_bytes()))?;
            let settings = ImportSettings::CharacterMap(MapSettings::LayerMask(MaskSettings::default()));
            self.layers.insert(key, writer.sidecar(&path, settings)?);
        }
        Ok(())
    }

    /// Writes (or finds) image `image`'s copy among the map inputs, and gives its file name.
    fn input(
        import: &VrmImport,
        image: usize,
        names: &Names,
        root: &Path,
        writer: &mut Writer,
    ) -> Result<String, ExtractError> {
        let input = &names.inputs[&image];
        writer.file(&root.join("maps").join(input), || png(import, image))?;
        Ok(input.clone())
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
        let meta = AssetMeta::new(meta::new_id().map_err(MetaError::Entropy)?, importer);
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
