//! Materials (M1 A3): `.material.ron` documents cooked to `.smat` (spec §5.27).
//!
//! A material source is a RON document of one of the families in
//! [`sapphire_assets::material`] (`skin.material.ron`), with a sidecar like any asset
//! (`importer: Material(())`). It cooks to `skin.smat` beside where `skin.png` would cook to
//! `skin.ktx2`. The cook:
//!
//! 1. reads the document, of whichever family its envelope names, and validates it;
//! 2. checks every texture it references: an asset of the same content tree cooked as a
//!    texture (a texture source, or an anime character map document of A7), whose class the
//!    slot accepts ([`accepts`]: a ramp slot takes only ramps, a
//!    control slot only ILM maps, so a data map can't arrive sRGB-decoded);
//! 3. reads each texture's cooked KTX2 for the swizzle its shaders must apply (and checks
//!    that a ramp has A7's rows);
//! 4. packs the family's GPU block, and reads the cooked bytes back through the runtime's checks.
//!
//! Steps 1 and 2 run on every cook, before the cache is consulted: each referenced texture is a
//! [`Dependency::Asset`] of the material's cache key, by the texture's own key, so editing,
//! recooking or re-classing a texture recooks the materials that use it, and a reference to a
//! texture that is gone fails the cook even when the material itself is unchanged.

use std::path::{Path, PathBuf};

use sapphire_assets::AssetId;
use sapphire_assets::character_maps::ramp;
use sapphire_assets::material::{
    AnimeCharacterMaterial, AnyMaterialDocument, Material, MaterialError, TextureUsage,
};
use sapphire_assets::texture::{Swizzle, Texture, TextureFormatError};
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::cache::{CacheKey, Dependency, Stage};
use crate::character_maps::MapKind;
use crate::meta::ImportSettings;
use crate::texture::TextureClass;

/// Reading the document, as cache keys know it. Bump the version whenever its output changes
/// for the same file.
pub const IMPORTER: Stage = Stage { name: "material", version: 1 };

/// Validation, texture checks and packing. Bump the version whenever they change output for the
/// same document and textures.
pub const COOKER: Stage = Stage { name: "smat", version: 1 };

/// What a material source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".material.ron";

/// Import settings for materials: none yet. A struct, so settings (a mobile cook forcing band
/// diffuse, say) can join without changing existing sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaterialSettings {}

/// Whether `path` names a material source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file a material source at `relative` writes: `skin.material.ron` → `skin.smat`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// What a referenced asset cooks as, as far as texture slots care: its texture class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureKind {
    Color,
    Normal,
    Mask,
    Ramp,
    Ilm,
    Sdf,
    Hdr,
}

impl TextureKind {
    /// What an asset imported with `settings` cooks as, if a texture: a texture source by its
    /// class, an anime character map document (A7) by the class its generated map cooks as.
    pub fn of(settings: &ImportSettings) -> Option<Self> {
        Some(match settings {
            ImportSettings::Texture(settings) => match settings.class {
                TextureClass::Color(_) => Self::Color,
                TextureClass::Normal(_) => Self::Normal,
                TextureClass::Mask(_) => Self::Mask,
                TextureClass::Ramp(_) => Self::Ramp,
                TextureClass::Ilm(_) => Self::Ilm,
                TextureClass::Sdf(_) => Self::Sdf,
                TextureClass::Hdr(_) => Self::Hdr,
            },
            ImportSettings::CharacterMap(settings) => match settings.kind() {
                MapKind::FaceSdf => Self::Sdf,
                MapKind::Ramp => Self::Ramp,
                MapKind::Ilm => Self::Ilm,
                MapKind::LayerMask => Self::Mask,
            },
            ImportSettings::Gltf(_)
            | ImportSettings::Vrm(_)
            | ImportSettings::Assembly(_)
            | ImportSettings::Material(_)
            | ImportSettings::ProbeGrid(_) => return None,
        })
    }

    /// The name used in messages, as the texture class's.
    pub fn name(self) -> &'static str {
        match self {
            Self::Color => "color",
            Self::Normal => "normal",
            Self::Mask => "mask",
            Self::Ramp => "ramp",
            Self::Ilm => "ILM",
            Self::Sdf => "SDF",
            Self::Hdr => "HDR",
        }
    }
}

/// Whether a texture of `kind` can fill a slot that needs `usage`.
pub fn accepts(usage: TextureUsage, kind: TextureKind) -> bool {
    use TextureKind as K;
    match usage {
        TextureUsage::Color => kind == K::Color,
        TextureUsage::Light => matches!(kind, K::Color | K::Hdr),
        TextureUsage::Normal => kind == K::Normal,
        TextureUsage::Mask => matches!(kind, K::Mask | K::Ilm),
        TextureUsage::Control => kind == K::Ilm,
        TextureUsage::Ramp => kind == K::Ramp,
        TextureUsage::FaceShadow => kind == K::Sdf,
    }
}

/// What a material's cook knows about an asset it references.
#[derive(Clone, Copy, Debug)]
pub struct Referenced<'a> {
    /// The asset's source.
    pub source: &'a Path,
    /// What it cooks as, if a texture ([`TextureKind::of`]).
    pub class: Option<TextureKind>,
    /// Its cooked file and cache key, once cooked: textures cook before materials.
    pub cooked: Option<(&'a Path, CacheKey)>,
}

/// Errors from cooking a material.
#[derive(Debug, thiserror::Error)]
pub enum MaterialCookError {
    #[error("not UTF-8 text")]
    NotUtf8(#[source] std::str::Utf8Error),
    #[error("not a material document")]
    Document(#[source] Box<DocumentError>),
    #[error(transparent)]
    Invalid(#[from] MaterialError),
    #[error("texture slot `{slot}` references {id}, which no source in the content tree has")]
    MissingTexture { slot: &'static str, id: AssetId },
    #[error("texture slot `{slot}` references {}, which is not a texture", path.display())]
    NotATexture { slot: &'static str, path: PathBuf },
    #[error("texture slot `{slot}` needs {usage}, but {} is a {class} texture", path.display())]
    WrongClass { slot: &'static str, usage: &'static str, path: PathBuf, class: &'static str },
    #[error("texture slot `{slot}` references {}, which hasn't cooked", path.display())]
    NotCooked { slot: &'static str, path: PathBuf },
    #[error(
        "a ramp has {rows} rows of equal height (a day and a night row per material ID), but this \
         one is {height} texels tall",
        rows = ramp::ROWS
    )]
    RampRows { height: u32 },
    #[error("failed to read the cooked texture {}", path.display())]
    ReadTexture {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the cooked texture {} is invalid", path.display())]
    InvalidTexture {
        path: PathBuf,
        #[source]
        source: TextureFormatError,
    },
}

/// Reads and validates a material source.
pub fn read(bytes: &[u8]) -> Result<AnyMaterialDocument, MaterialCookError> {
    let text = std::str::from_utf8(bytes).map_err(MaterialCookError::NotUtf8)?;
    let document =
        AnyMaterialDocument::from_ron(text).map_err(|error| MaterialCookError::Document(Box::new(error)))?;
    document.validate()?;
    Ok(document)
}

/// The textures `document` references, checked against their slots, as cache key
/// dependencies. `lookup` describes the asset with an ID, if the content tree has one.
pub fn dependencies<'a>(
    document: &AnyMaterialDocument,
    lookup: impl Fn(AssetId) -> Option<Referenced<'a>>,
) -> Result<Vec<Dependency>, MaterialCookError> {
    let mut dependencies = Vec::new();
    for (slot, id) in bound_slots(document) {
        let (_, key) = texture(document, slot, id, &lookup)?;
        dependencies.push(Dependency::Asset { id, key });
    }
    Ok(dependencies)
}

/// Cooks `document` against the textures `lookup` describes: the cooked material's bytes.
pub fn cook<'a>(
    document: &AnyMaterialDocument,
    lookup: impl Fn(AssetId) -> Option<Referenced<'a>>,
) -> Result<Vec<u8>, MaterialCookError> {
    let slots = document.family().texture_slots();
    let mut swizzles = vec![Swizzle::IDENTITY; slots.len()];
    for (slot, id) in bound_slots(document) {
        let (path, _) = texture(document, slot, id, &lookup)?;
        let bytes = std::fs::read(path)
            .map_err(|source| MaterialCookError::ReadTexture { path: path.to_owned(), source })?;
        let texture = Texture::from_ktx2(&bytes)
            .map_err(|source| MaterialCookError::InvalidTexture { path: path.to_owned(), source })?;
        swizzles[slot] = texture.desc.swizzle;
        if matches!(document, AnyMaterialDocument::AnimeCharacter(_)) && slot == AnimeCharacterMaterial::RAMP
        {
            check_ramp(texture.desc.height)?;
        }
    }
    let material = document.cook(&swizzles)?;
    let bytes = material.to_cooked().map_err(MaterialError::from)?;
    // The runtime's checks, so a cook never writes a material the renderer would refuse.
    Material::read(&bytes)?;
    Ok(bytes.into_vec())
}

/// The slots that bind a texture, with its ID.
fn bound_slots(document: &AnyMaterialDocument) -> impl Iterator<Item = (usize, AssetId)> {
    document.textures().into_iter().enumerate().filter_map(|(slot, id)| Some((slot, id?)))
}

/// A ramp must have the rows of A7's layout (`sapphire_assets::character_maps::ramp`): equal
/// rows, a day and a night one per material ID. The regions' rows index those.
fn check_ramp(height: u32) -> Result<(), MaterialCookError> {
    if height.is_multiple_of(ramp::ROWS) { Ok(()) } else { Err(MaterialCookError::RampRows { height }) }
}

/// The cooked file and cache key of the texture `id` bound to `slot`, once it is checked
/// against what the slot accepts.
fn texture<'a>(
    document: &AnyMaterialDocument,
    slot: usize,
    id: AssetId,
    lookup: &impl Fn(AssetId) -> Option<Referenced<'a>>,
) -> Result<(&'a Path, CacheKey), MaterialCookError> {
    let desc = document.family().texture_slots()[slot];
    let referenced = lookup(id).ok_or(MaterialCookError::MissingTexture { slot: desc.name, id })?;
    let Some(class) = referenced.class else {
        return Err(MaterialCookError::NotATexture { slot: desc.name, path: referenced.source.to_owned() });
    };
    if !accepts(desc.usage, class) {
        return Err(MaterialCookError::WrongClass {
            slot: desc.name,
            usage: desc.usage.name(),
            path: referenced.source.to_owned(),
            class: class.name(),
        });
    }
    // Textures cook first; one that didn't (its cook failed) leaves its materials uncooked.
    referenced
        .cooked
        .ok_or_else(|| MaterialCookError::NotCooked { slot: desc.name, path: referenced.source.to_owned() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character_maps::MapKind;
    use crate::texture::{ColorSettings, TextureSettings};

    #[test]
    fn material_sources_cook_beside_their_textures() {
        assert!(is_source(Path::new("characters/skin.material.ron")));
        assert!(is_source(Path::new("SKIN.Material.RON")));
        assert!(!is_source(Path::new("tiers.ron")));
        assert!(!is_source(Path::new("skin.material")));
        assert_eq!(output_path(Path::new("a/skin.material.ron"), "smat"), Path::new("a/skin.smat"));
    }

    #[test]
    fn slots_accept_only_the_classes_that_fit() {
        use TextureKind as K;
        assert!(accepts(TextureUsage::Color, K::Color) && !accepts(TextureUsage::Color, K::Ramp));
        assert!(accepts(TextureUsage::Control, K::Ilm) && !accepts(TextureUsage::Control, K::Color));
        assert!(!accepts(TextureUsage::Control, K::Mask), "the control map's alpha is the material ID");
        assert!(accepts(TextureUsage::Ramp, K::Ramp) && !accepts(TextureUsage::Ramp, K::Color));
        assert!(accepts(TextureUsage::FaceShadow, K::Sdf) && !accepts(TextureUsage::FaceShadow, K::Mask));
        assert!(accepts(TextureUsage::Mask, K::Mask) && accepts(TextureUsage::Mask, K::Ilm));
        assert!(accepts(TextureUsage::Light, K::Hdr) && !accepts(TextureUsage::Light, K::Normal));
    }

    #[test]
    fn character_maps_count_as_the_textures_they_cook_to() {
        let kind = |kind: MapKind| TextureKind::of(&ImportSettings::CharacterMap(kind.default_settings()));
        assert_eq!(kind(MapKind::FaceSdf), Some(TextureKind::Sdf));
        assert_eq!(kind(MapKind::Ramp), Some(TextureKind::Ramp));
        assert_eq!(kind(MapKind::Ilm), Some(TextureKind::Ilm));
        assert_eq!(kind(MapKind::LayerMask), Some(TextureKind::Mask), "a body's layer mask (M1 C7)");
        let color =
            ImportSettings::Texture(TextureSettings { class: TextureClass::Color(ColorSettings::default()) });
        assert_eq!(TextureKind::of(&color), Some(TextureKind::Color));
        assert_eq!(TextureKind::of(&ImportSettings::Material(MaterialSettings::default())), None);
    }
}
