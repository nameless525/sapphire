//! Anime character maps (M1 A7): SDF face maps, ramps and ILM maps, generated from RON
//! documents and cooked as textures.
//!
//! The anime character shader (C1, C3) reads three authored maps besides its color and
//! normal maps (spec §5.12): an SDF face-shadow map, ramp rows by material ID, and an ILM
//! map of painted masks. Each is a [`sapphire_serialize::document`] with a `.meta` sidecar
//! like any source asset, named by a compound extension:
//!
//! | Document | Type | Inputs | Generates | Texture class |
//! |---|---|---|---|---|
//! | `<name>.facesdf.ron` | [`FaceSdfMap`] | Threshold masks per light angle | The face's threshold map ([`face_sdf`]) | SDF: R16 or RG16 on desktop, R8 or RG8 on mobile |
//! | `<name>.ramp.ron` | [`RampMap`] | None: color stops | Day and night rows per material ID ([`ramp`]) | Ramp: RGBA8 sRGB |
//! | `<name>.ilm.ron` | [`IlmMap`] | Grayscale masks per channel | The four channels packed ([`ilm`]) | ILM: BC7, ASTC 4×4 |
//!
//! What the generated texels mean (the ILM channel assignment, the ramp rows, the SDF
//! angle encoding) is `sapphire_assets::character_maps`, shared with the shader.
//!
//! # Inputs
//!
//! Documents name their input images (PNG, TGA or EXR) by paths relative to their own
//! directory, `/`-separated. An input is **part of the document's asset, not an asset of
//! its own**: a cook leaves the images that documents in the content tree read out of its
//! sources, so they get no sidecar and no cooked texture of their own, as a glTF's external
//! buffers don't. Inputs are decoded as linear data whatever their encoding, and must all
//! have the document's size: maps are generated at their inputs' resolution.
//!
//! # Recipes and the cache key
//!
//! Each kind of document is a [`MapSettings`] variant of the sidecar's importer settings
//! (`importer: CharacterMap(FaceSdf((precision: Unorm16)))`) and a recipe: its own importer
//! stage, which reads the document and its inputs and generates the map as a
//! [`SourceImage`], then the texture cooker ([`texture::COOKER`]), which cooks that image as
//! its class like any imported one. The inputs are file dependencies of the key, by the
//! path the document names them with, so editing a mask recooks exactly the maps that read
//! it, and moving a document together with its inputs keeps its key. Importers that decode
//! images embed the image importer's version in theirs ([`with_image_importer`]).
//!
//! `sapphire_pipeline bake-map` writes a document's generated map as a PNG before
//! compression ([`to_png`]), for checking a map without cooking the tree.
//!
//! # Later milestones
//!
//! - Distances that use anti-aliased mask edges (Gustavson and Strand's edge distance
//!   transform) for sub-pixel accurate shadow edges from low-resolution masks; masks are
//!   binarized today, so their resolution sets the edge's precision.
//! - Ramp texture arrays once texture arrays exist (A2's later milestones), and color-coded
//!   material-ID maps (a palette of flat colors) besides gray levels.
//! - Material documents (A3) that name these maps by asset ID, and a per-instance debug view
//!   of the face map's threshold (F5).

pub mod face_sdf;
pub mod ilm;
pub mod ramp;

mod distance;
#[cfg(test)]
mod tests;

use std::io;
use std::path::{Component, Path, PathBuf};

use sapphire_assets::texture::MAX_TEXTURE_DIMENSION;
use sapphire_reflect::{TypePath, Versioned};
use sapphire_serialize::DocumentError;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub use self::face_sdf::{FaceSdfError, FaceSdfMap, FaceSdfSettings};
pub use self::ilm::{IlmError, IlmMap};
pub use self::ramp::{RampBakeSettings, RampError, RampMap};
use crate::cache::Stage;
use crate::cook::Cook;
use crate::import::image::{
    self as image_import, ChannelLayout, ImageImportError, SourceFormat, SourceImage, Transfer,
};
use crate::texture::{self, IlmSettings, TextureClass, TextureError, TextureSettings};

/// The kinds of map document, each known by its compound extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapKind {
    FaceSdf,
    Ramp,
    Ilm,
}

impl MapKind {
    /// Every kind, in a stable order.
    pub const ALL: [Self; 3] = [Self::FaceSdf, Self::Ramp, Self::Ilm];

    /// The end of a document's file name, lowercase.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::FaceSdf => ".facesdf.ron",
            Self::Ramp => ".ramp.ron",
            Self::Ilm => ".ilm.ron",
        }
    }

    /// The name used in messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::FaceSdf => "face SDF",
            Self::Ramp => "ramp",
            Self::Ilm => "ILM",
        }
    }

    /// The kind of map document at `path`, by the end of its file name (in any case).
    pub fn for_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        // A bare `.ramp.ron` names no map.
        Self::ALL.into_iter().find(|kind| name.len() > kind.suffix().len() && name.ends_with(kind.suffix()))
    }

    /// The default import settings of this kind's documents.
    pub fn default_settings(self) -> MapSettings {
        match self {
            Self::FaceSdf => MapSettings::FaceSdf(FaceSdfSettings::default()),
            Self::Ramp => MapSettings::Ramp(RampBakeSettings::default()),
            Self::Ilm => MapSettings::Ilm(IlmSettings::default()),
        }
    }

    /// This kind's importer, as cache keys know it.
    pub fn importer(self) -> Stage {
        match self {
            Self::FaceSdf => face_sdf::IMPORTER,
            Self::Ramp => ramp::IMPORTER,
            Self::Ilm => ilm::IMPORTER,
        }
    }
}

/// Import settings of a map document: how its generated map cooks. What the map holds is the
/// document's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MapSettings {
    FaceSdf(FaceSdfSettings),
    Ramp(RampBakeSettings),
    Ilm(IlmSettings),
}

impl MapSettings {
    /// The kind of document these settings are for.
    pub fn kind(&self) -> MapKind {
        match self {
            Self::FaceSdf(_) => MapKind::FaceSdf,
            Self::Ramp(_) => MapKind::Ramp,
            Self::Ilm(_) => MapKind::Ilm,
        }
    }
}

/// `version` of a map importer that decodes images, with the image importer's version in its
/// low 16 bits: masks are decoded by that importer, so a bump there must recook these maps
/// too.
pub const fn with_image_importer(version: u32) -> u32 {
    (version << 16) | image_import::IMPORTER.version
}

/// Errors from reading a map document and its inputs, generating its map, or cooking it.
#[derive(Debug, thiserror::Error)]
pub enum MapError {
    #[error("the document isn't valid UTF-8")]
    NotUtf8,
    #[error("not a valid {kind} document")]
    Document {
        kind: &'static str,
        /// Boxed: RON's errors are large, and every cook result carries this type.
        #[source]
        source: Box<DocumentError>,
    },
    #[error("input {0:?} must be a path relative to the document's directory")]
    InputPath(String),
    #[error("input {} is not an image (expected .png, .tga or .exr)", .0.display())]
    UnsupportedInput(PathBuf),
    #[error("failed to read input {}", path.display())]
    ReadInput {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to import input {}", path.display())]
    ImportInput {
        path: PathBuf,
        #[source]
        source: Box<ImageImportError>,
    },
    #[error(
        "input {} is {}x{}, but the map is {}x{}: every input must have the same size",
        path.display(), found[0], found[1], expected[0], expected[1]
    )]
    InputSize { path: PathBuf, found: [u32; 2], expected: [u32; 2] },
    #[error(
        "input {} is {}x{}, larger than a texture may be ({MAX_TEXTURE_DIMENSION} on a side)",
        path.display(), size[0], size[1]
    )]
    InputTooLarge { path: PathBuf, size: [u32; 2] },
    #[error(
        "input {} isn't a file in the content tree (input names must match the file's case exactly)",
        .0.display()
    )]
    MissingInput(PathBuf),
    #[error(
        "inputs {} have sidecars of their own, so they are assets of their own: delete those \
         sidecars to make the images part of this map, or stop naming them",
        list(.0)
    )]
    InputsWithSidecars(Vec<PathBuf>),
    #[error(transparent)]
    FaceSdf(#[from] FaceSdfError),
    #[error(transparent)]
    Ramp(#[from] RampError),
    #[error(transparent)]
    Ilm(#[from] IlmError),
    #[error("failed to cook the generated map")]
    Texture(#[source] Box<TextureError>),
}

/// A map generated from its document, before cooking.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedMap {
    /// Linear values in the class's logical channels (sRGB colors for ramps decoded to
    /// linear), as the texture cook takes an imported image.
    pub image: SourceImage,
    pub class: TextureClass,
}

/// Reads the map document at `path` (`bytes`) and its inputs, and generates its map.
pub fn generate(path: &Path, bytes: &[u8], settings: &MapSettings) -> Result<GeneratedMap, MapError> {
    let inputs = Inputs { document: path };
    let kind = settings.kind();
    match settings {
        MapSettings::FaceSdf(settings) => face_sdf::generate(&read_document(kind, bytes)?, settings, &inputs),
        MapSettings::Ramp(settings) => ramp::generate(&read_document(kind, bytes)?, settings),
        MapSettings::Ilm(settings) => ilm::generate(&read_document(kind, bytes)?, settings, &inputs),
    }
}

/// Generates and cooks one map document: the bytes of its cooked KTX2 file.
pub fn cook_source(
    path: &Path,
    bytes: &[u8],
    settings: &MapSettings,
    cook: Cook,
) -> Result<Vec<u8>, MapError> {
    let map = generate(path, bytes, settings)?;
    let cooked = texture::cook(&map.image, &TextureSettings { class: map.class }, cook)
        .map_err(|error| MapError::Texture(Box::new(error)))?;
    Ok(cooked.to_ktx2())
}

/// The inputs a `kind` document names, as it names them (relative to its directory), in
/// document order. Repeats are kept.
pub fn inputs(kind: MapKind, bytes: &[u8]) -> Result<Vec<String>, MapError> {
    Ok(match kind {
        MapKind::FaceSdf => read_document::<FaceSdfMap>(kind, bytes)?.inputs(),
        MapKind::Ramp => Vec::new(),
        MapKind::Ilm => read_document::<IlmMap>(kind, bytes)?.inputs(),
    })
}

/// Where the input a document at `document` names `input` lives: joined to the document's
/// directory, with `.` and `..` resolved without touching the file system, so the path
/// compares equal to the one a directory walk finds.
pub fn resolve_input(document: &Path, input: &str) -> Result<PathBuf, MapError> {
    let relative = Path::new(input);
    let invalid = || MapError::InputPath(input.to_owned());
    if input.is_empty() || relative.has_root() {
        return Err(invalid());
    }
    let mut resolved = document.parent().unwrap_or(Path::new("")).to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                // Up from a named directory; past the start of a relative path, keep `..`.
                if matches!(resolved.components().next_back(), Some(Component::Normal(_))) {
                    resolved.pop();
                } else {
                    resolved.push("..");
                }
            }
            Component::RootDir | Component::Prefix(_) => return Err(invalid()),
        }
    }
    Ok(resolved)
}

/// A generated map as a PNG file: the ramp class as 8-bit sRGB RGBA, the others as 16-bit
/// linear values (gray for one-channel SDF maps, RGB with blue 0 for two-channel ones, RGBA
/// for ILM maps).
pub fn to_png(map: &GeneratedMap) -> image::ImageResult<Vec<u8>> {
    let SourceImage { width, height, layout, ref pixels, .. } = map.image;
    let unorm16 = |v: f32| (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
    let image: image::DynamicImage = match (map.class, layout) {
        (TextureClass::Ramp(_), _) => {
            let srgb = |v: f32| crate::color::unorm8(crate::color::linear_to_srgb(v));
            let texels =
                pixels.iter().flat_map(|&[r, g, b, a]| [srgb(r), srgb(g), srgb(b), crate::color::unorm8(a)]);
            image::RgbaImage::from_raw(width, height, texels.collect()).expect("one texel per pixel").into()
        }
        (_, ChannelLayout::Gray) => {
            let texels = pixels.iter().map(|p| unorm16(p[0]));
            image::ImageBuffer::<image::Luma<u16>, _>::from_raw(width, height, texels.collect())
                .expect("one texel per pixel")
                .into()
        }
        (_, ChannelLayout::GrayAlpha) => {
            let texels = pixels.iter().flat_map(|p| [unorm16(p[0]), unorm16(p[3]), 0]);
            image::ImageBuffer::<image::Rgb<u16>, _>::from_raw(width, height, texels.collect())
                .expect("one texel per pixel")
                .into()
        }
        (_, ChannelLayout::Rgb | ChannelLayout::Rgba) => {
            let texels = pixels.iter().flat_map(|p| p.map(unorm16));
            image::ImageBuffer::<image::Rgba<u16>, _>::from_raw(width, height, texels.collect())
                .expect("one texel per pixel")
                .into()
        }
    };
    let mut buffer = io::Cursor::new(Vec::new());
    image.write_to(&mut buffer, image::ImageFormat::Png)?;
    Ok(buffer.into_inner())
}

/// Reads a `kind` document, of type `T`.
fn read_document<T: Versioned + DeserializeOwned + TypePath>(
    kind: MapKind,
    bytes: &[u8],
) -> Result<T, MapError> {
    let text = std::str::from_utf8(bytes).map_err(|_| MapError::NotUtf8)?;
    sapphire_serialize::from_ron(text)
        .map_err(|source| MapError::Document { kind: kind.name(), source: Box::new(source) })
}

/// Reads a document's input images.
struct Inputs<'a> {
    document: &'a Path,
}

impl Inputs<'_> {
    /// The input image the document names `input`, decoded as linear data.
    fn image(&self, input: &str) -> Result<(PathBuf, SourceImage), MapError> {
        let path = resolve_input(self.document, input)?;
        let format =
            SourceFormat::from_path(&path).ok_or_else(|| MapError::UnsupportedInput(path.clone()))?;
        let bytes =
            std::fs::read(&path).map_err(|source| MapError::ReadInput { path: path.clone(), source })?;
        let image = image_import::import_slice(&bytes, format, Transfer::Linear)
            .map_err(|source| MapError::ImportInput { path: path.clone(), source: Box::new(source) })?;
        let size = [image.width, image.height];
        if size.iter().any(|&side| side > MAX_TEXTURE_DIMENSION) {
            return Err(MapError::InputTooLarge { path, size });
        }
        Ok((path, image))
    }
}

fn list(paths: &[PathBuf]) -> String {
    paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
}

/// Fails unless `image`, read from `path`, is `expected` in size.
fn check_size(path: &Path, image: &SourceImage, expected: [u32; 2]) -> Result<(), MapError> {
    let found = [image.width, image.height];
    if found == expected {
        Ok(())
    } else {
        Err(MapError::InputSize { path: path.to_owned(), found, expected })
    }
}
