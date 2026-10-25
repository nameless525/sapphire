//! Materials as assets (M1 A3): RON documents, one type per material family, cooked into
//! [`Material`]s, rkyv archives in `sapphire_serialize`'s cooked envelope (`.smat`).
//!
//! # Families
//!
//! | Family | Document | GPU block | Texture slots | Shaders |
//! |---|---|---|---|---|
//! | Anime character | [`AnimeCharacterMaterial`] | [`AnimeCharacterParams`], 896 B | base color, control, normal, ramp, face shadow, surface mask, matcap | the anime character shader and its outlines (C1 grows it) |
//! | Environment (stylized GGX) | [`EnvironmentMaterial`] | [`EnvironmentParams`], 144 B | base color, normal, mask, terminator band, emission | W4 |
//! | Foliage | [`FoliageMaterial`] | [`FoliageParams`], 160 B | base color, mask | W6 |
//! | Sky | [`SkyMaterial`] | [`SkyParams`], 96 B | moon, stars, Milky Way, clouds, cirrus | W2 |
//! | Fog card | [`FogCardMaterial`] | [`FogCardParams`], 80 B | map | W3 |
//!
//! Each document is a `Versioned` type with a stable name under `sapphire::render`
//! (`(type: "sapphire::render::AnimeCharacterMaterial", version: 1, data: (...))`), registered
//! for loading by name ([`register_types`]: hot reload, live link, the editor). Fields left out
//! take their defaults, so a family gains fields without a schema version. Colors are linear,
//! as glTF factors are. Textures are referenced by [`AssetId`], never by path (rule R5); each
//! family's `TEXTURE_SLOTS` says what each slot must hold ([`TextureUsage`]), which the cook
//! checks against the texture's class.
//!
//! # Cooked materials
//!
//! A [`Material`] is what the renderer uploads: the family, the permutation-relevant
//! [`MaterialFlags`], the family's GPU parameter block exactly as its shaders read it, and per
//! texture slot the texture bound there with the swizzle that texture was cooked with. The
//! renderer copies the block into its slot of the family's material buffer byte for byte and
//! binds the textures (its group 3); [`Material::from_document`] packs it. The block layouts are
//! `#[repr(C)]` structs here, mirrored in WGSL (`sapphire_render/shaders/materials.wgsl`) and
//! checked against naga's layout of that source.
//!
//! - **Swizzles reach the shader in the block.** ASTC stores two-channel data as luminance plus
//!   alpha (swizzle `ra01`, A2); every block's `swizzles` field holds one 12-bit code per slot
//!   ([`swizzle_code`]), which shaders apply after sampling, so both cooks read the same logical
//!   channels. The cook takes each code from the cooked texture itself, which is why a
//!   material's cache key depends on its textures' keys; the renderer checks the textures it
//!   binds against the recorded swizzles.
//! - **Bound slots** set bit `i` of the block's `modes.x`, so shaders skip unbound ones on a
//!   branch. [`Material::read`] checks both against the bindings, and that every float in the
//!   block is finite.
//! - A block whose layout or packing changes bumps [`Material::VERSION`], so every material
//!   recooks, and with it `Mesh::VERSION`, since meshes embed generated materials (a
//!   compile-time check pairs the two).
//!
//! # Meshes and their materials
//!
//! **Decision (M1 A3):** a mesh's material slots are named after the source's materials (glTF
//! material names), and each slot either embeds a material the importer generated from the
//! source or names a material asset by ID ([`SlotMaterial`](crate::mesh::SlotMaterial)). The
//! glTF importer generates anime character materials from base color factors and the material
//! extras the Blender workflow exports, so every glTF draws as authored with no setup (the
//! sample mannequin's five materials carry over exactly). A mesh's sidecar binds slots to
//! material documents by name (`materials: {"Skin": "<id>"}`), for materials an artist authors
//! with ramps, control and face maps. Why:
//!
//! - **It fits VRM (A6):** the importer maps MToon onto generated anime character materials, and
//!   the look-dev character's slots bind to documents that use A7's authored ramps, control maps
//!   and SDF face maps.
//! - **One material path at runtime:** embedded and standalone materials are the same
//!   [`Material`], uploaded the same way. A mesh's referenced materials load as dependencies
//!   before it, once packs resolve IDs (A4).
//! - **Edits stay cheap:** a material asset is its own file, so editing it recooks only it and
//!   updates one slot of the material buffer (hot reload, A8; live link, A9). Importers never
//!   write sources, and a cook never rewrites a sidecar.
//! - **Generated materials reference no textures,** so a mesh never depends on a texture's
//!   cook. Textured materials are documents: VRM import (A6) extracts a file's images as texture
//!   sources and its textured MToon materials as documents, bound in the mesh's sidecar, while
//!   generated materials, from MToon's factors, cover its untextured slots. Packing a generated
//!   material that names a texture is an error.
//!
//! # Later milestones
//!
//! - Shaders for the environment (W4), foliage (W6), sky (W2) and fog card (W3) families: their
//!   blocks are defined and uploaded, and their group 3 layouts exist, but nothing draws them
//!   yet.
//! - Ramp arrays and per-region reads of the control map (C1): until then the anime character
//!   shader shades with region 0 and two bands.

mod character;
mod common;
mod environment;
mod fog_card;
mod foliage;
mod sky;

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

use bevy_reflect::TypePath;
use sapphire_reflect::{TypeRegistry, Versioned};
use sapphire_serialize::DocumentError;
pub use sapphire_serialize::cooked::AlignedVec;
use sapphire_serialize::cooked::CookedError;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use self::character::{
    AnimeCharacterMaterial, AnimeCharacterParams, AnimeCharacterRegion, BandDiffuse, CharacterRegion,
    CharacterSurface, DiffuseMode, EyeShading, FaceShading, HairHighlight, HairShading, HighlightStyle,
    LightSplit, Matcap, Outline, RampDiffuse, Rim, Specular, SpecularModel,
};
pub use self::common::{AlphaMode, DistanceStylization, SWIZZLE_BITS, swizzle_code};
use self::common::{GpuBlock, bound_bits, pack_swizzles};
pub use self::environment::{EnvironmentMaterial, EnvironmentParams, Terminator, TerminatorBand};
pub use self::fog_card::{FogCardMaterial, FogCardParams};
pub use self::foliage::{FoliageGradient, FoliageMaterial, FoliageParams, Translucency, Wind};
pub use self::sky::{CloudLayer, Disc, Moon, SkyMaterial, SkyParams, Stars};
use crate::AssetId;
use crate::texture::Swizzle;

/// Conventional file extension for cooked materials.
pub const MATERIAL_EXTENSION: &str = "smat";

/// Registers every material document, so `sapphire_serialize::from_ron_dynamic` loads them by
/// type name.
pub fn register_types(registry: &mut TypeRegistry) {
    registry.register::<AnimeCharacterMaterial>();
    registry.register::<EnvironmentMaterial>();
    registry.register::<FoliageMaterial>();
    registry.register::<SkyMaterial>();
    registry.register::<FogCardMaterial>();
}

/// The material families. Each has a document type, a GPU block and texture slots of its own.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    rkyv::Portable,
    rkyv::bytecheck::CheckBytes,
)]
#[rkyv(as = MaterialFamily)]
#[bytecheck(crate = rkyv::bytecheck)]
#[repr(u8)]
pub enum MaterialFamily {
    AnimeCharacter,
    Environment,
    Foliage,
    Sky,
    FogCard,
}

impl MaterialFamily {
    /// Every family, in a stable order.
    pub const ALL: [Self; 5] =
        [Self::AnimeCharacter, Self::Environment, Self::Foliage, Self::Sky, Self::FogCard];

    /// The family's position in [`Self::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// The name used in messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::AnimeCharacter => "anime character",
            Self::Environment => "environment",
            Self::Foliage => "foliage",
            Self::Sky => "sky",
            Self::FogCard => "fog card",
        }
    }

    /// The family's texture slots, in binding order.
    pub fn texture_slots(self) -> &'static [TextureSlot] {
        match self {
            Self::AnimeCharacter => AnimeCharacterMaterial::TEXTURE_SLOTS,
            Self::Environment => EnvironmentMaterial::TEXTURE_SLOTS,
            Self::Foliage => FoliageMaterial::TEXTURE_SLOTS,
            Self::Sky => SkyMaterial::TEXTURE_SLOTS,
            Self::FogCard => FogCardMaterial::TEXTURE_SLOTS,
        }
    }

    /// Bytes in the family's GPU block: its stride in the family's material buffer.
    pub fn params_size(self) -> usize {
        match self {
            Self::AnimeCharacter => size_of::<AnimeCharacterParams>(),
            Self::Environment => size_of::<EnvironmentParams>(),
            Self::Foliage => size_of::<FoliageParams>(),
            Self::Sky => size_of::<SkyParams>(),
            Self::FogCard => size_of::<FogCardParams>(),
        }
    }
}

impl fmt::Display for MaterialFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One texture slot of a family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureSlot {
    /// The slot's name in messages: `base_color`, `ramp`.
    pub name: &'static str,
    pub usage: TextureUsage,
}

/// What a texture slot must hold. The pipeline checks a bound texture's class against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureUsage {
    /// sRGB color: albedo (the color class).
    Color,
    /// Light: color or HDR (matcaps, emission, star fields).
    Light,
    /// Tangent-space normals, X and Y.
    Normal,
    /// Linear data in one to four channels.
    Mask,
    /// The anime character control map: an ILM map (linear RGBA; its alpha is the region).
    Control,
    /// Shadow-color ramps: uncompressed, a row per region.
    Ramp,
    /// SDF face-shadow threshold maps.
    FaceShadow,
}

impl TextureUsage {
    /// The name used in messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::Color => "a color texture",
            Self::Light => "a color or HDR texture",
            Self::Normal => "a normal map",
            Self::Mask => "a mask",
            Self::Control => "a control (ILM) map",
            Self::Ramp => "a ramp",
            Self::FaceShadow => "an SDF face-shadow map",
        }
    }
}

/// Material properties the renderer chooses pipelines by: the anime character shader's static
/// permutation bits that come from the material (spec §5.12; skinning and morphs come from the
/// mesh).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MaterialFlags(u32);

impl MaterialFlags {
    /// Discards pixels below an alpha cutoff.
    pub const ALPHA_TEST: Self = Self(1 << 0);
    /// Blends over what is behind it.
    pub const TRANSPARENT: Self = Self(1 << 1);
    /// Draws an inverted-hull outline.
    pub const OUTLINE: Self = Self(1 << 2);
    /// An anime character face.
    pub const FACE: Self = Self(1 << 3);
    /// Anime character hair.
    pub const HAIR: Self = Self(1 << 4);
    /// An anime character eye.
    pub const EYE: Self = Self(1 << 5);

    const ALL: u32 = (1 << 6) - 1;

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    /// The flags with these bits, or `None` if any bit is unknown.
    pub const fn from_bits(bits: u32) -> Option<Self> {
        if bits & !Self::ALL == 0 { Some(Self(bits)) } else { None }
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for MaterialFlags {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl BitOrAssign for MaterialFlags {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

/// A family's document type.
pub trait MaterialDocument: Versioned + Serialize + DeserializeOwned + TypePath {
    const FAMILY: MaterialFamily;

    /// Checks every field's range.
    fn validate(&self) -> Result<(), MaterialError>;

    /// The texture each of the family's slots binds, in slot order.
    fn textures(&self) -> Vec<Option<AssetId>>;

    /// The permutation-relevant flags.
    fn flags(&self) -> MaterialFlags;

    /// The family's GPU block as bytes, given each slot's swizzle (identity where unbound).
    fn params(&self, swizzles: &[Swizzle]) -> Vec<u8>;
}

/// A cooked material. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::render"]
pub struct Material {
    pub family: MaterialFamily,
    /// [`MaterialFlags`] bits.
    pub flags: u32,
    /// The family's GPU block, [`MaterialFamily::params_size`] bytes, as its shaders read it.
    pub params: Vec<u8>,
    /// One per texture slot of the family, in slot order.
    pub textures: Vec<TextureBinding>,
}

impl Versioned for Material {
    /// 1: M1 A3's blocks. Bump it whenever a family's block layout or its packing
    /// (`gpu_params`) changes: the cache key holds it, so every material recooks, and meshes,
    /// whose generated materials embed blocks, must bump `Mesh::VERSION` with it (a compile-time
    /// check in `mesh` pairs the two).
    const VERSION: u32 = 1;
}

/// What one texture slot binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct TextureBinding {
    /// The texture asset, or `None` for an unbound slot.
    pub texture: Option<AssetId>,
    /// The texture's swizzle as `KTXswizzle` text (`rgba` when unbound), as the cook read it from
    /// the cooked texture. The GPU block encodes the same.
    pub swizzle: [u8; 4],
}

impl TextureBinding {
    pub const UNBOUND: Self = Self { texture: None, swizzle: *b"rgba" };
}

impl ArchivedTextureBinding {
    pub fn texture(&self) -> Option<AssetId> {
        self.texture.as_ref().copied()
    }

    /// The swizzle the slot's texture must have. Valid once its material was read.
    pub fn swizzle(&self) -> Swizzle {
        Swizzle::from_ktx(&self.swizzle).unwrap_or_default()
    }
}

/// A material that can't be cooked or read.
#[derive(Debug, thiserror::Error)]
pub enum MaterialError {
    /// The envelope, the schema version or the archive itself.
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("{field} {reason}")]
    Invalid { field: String, reason: &'static str },
    #[error("the {family} family has {expected} texture slots, not {found}")]
    SlotCount { family: MaterialFamily, expected: usize, found: usize },
    #[error("the {family} family's GPU block is {expected} bytes, not {found}")]
    ParamsSize { family: MaterialFamily, expected: usize, found: usize },
    #[error("texture slot {slot} has the swizzle {swizzle:?}, which isn't four of `rgba01`")]
    InvalidSwizzle { slot: usize, swizzle: String },
    #[error("texture slot {slot} binds no texture, so its swizzle must be `rgba`, not {swizzle:?}")]
    UnboundSwizzle { slot: usize, swizzle: String },
    #[error("unknown material flag bits {0:#x}")]
    UnknownFlags(u32),
    #[error("the GPU block holds a value that isn't finite")]
    NonFinite,
    #[error("the GPU block's {0} don't match the texture bindings")]
    BlockMismatch(&'static str),
}

impl Material {
    /// Validates `document` and packs it, given the swizzle of each slot's texture (one per
    /// slot; unbound slots take the identity whatever is passed).
    pub fn from_document<D: MaterialDocument>(
        document: &D,
        swizzles: &[Swizzle],
    ) -> Result<Self, MaterialError> {
        document.validate()?;
        let family = D::FAMILY;
        let textures = document.textures();
        let expected = family.texture_slots().len();
        if textures.len() != expected || swizzles.len() != expected {
            let found = if textures.len() != expected { textures.len() } else { swizzles.len() };
            return Err(MaterialError::SlotCount { family, expected, found });
        }
        let swizzles: Vec<Swizzle> = textures
            .iter()
            .zip(swizzles)
            .map(|(texture, &swizzle)| if texture.is_some() { swizzle } else { Swizzle::IDENTITY })
            .collect();
        let bindings = textures
            .iter()
            .zip(&swizzles)
            .map(|(&texture, swizzle)| TextureBinding { texture, swizzle: swizzle.to_ktx() })
            .collect();
        Ok(Self {
            family,
            flags: document.flags().bits(),
            params: document.params(&swizzles),
            textures: bindings,
        })
    }

    /// The material as cooked bytes: the envelope header, then the archive.
    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked material bytes in place and checks them ([`ArchivedMaterial::validate`]).
    /// The bytes must start 16-byte aligned.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedMaterial, MaterialError> {
        let material = sapphire_serialize::access::<Self>(bytes)?;
        material.validate()?;
        Ok(material)
    }

    /// [`Self::read`], then a copy out of the archive, for tools and tests.
    pub fn from_cooked(bytes: &[u8]) -> Result<Self, MaterialError> {
        let archived = Self::read(bytes)?;
        rkyv::deserialize::<Self, rkyv::rancor::Error>(archived)
            .map_err(|error| MaterialError::Cooked(error.into()))
    }

    /// The flags, if every bit is known.
    pub fn material_flags(&self) -> MaterialFlags {
        MaterialFlags::from_bits(self.flags).unwrap_or_default()
    }
}

impl ArchivedMaterial {
    /// Checks what the renderer relies on beyond bytecheck: the block's size and the slot count
    /// match the family, swizzles are valid and unbound slots have none, and flags are known.
    pub fn validate(&self) -> Result<(), MaterialError> {
        let family = self.family;
        let (expected, found) = (family.params_size(), self.params.len());
        if found != expected {
            return Err(MaterialError::ParamsSize { family, expected, found });
        }
        let (expected, found) = (family.texture_slots().len(), self.textures.len());
        if found != expected {
            return Err(MaterialError::SlotCount { family, expected, found });
        }
        for (slot, binding) in self.textures.iter().enumerate() {
            let text = || String::from_utf8_lossy(&binding.swizzle).into_owned();
            let Some(swizzle) = Swizzle::from_ktx(&binding.swizzle) else {
                return Err(MaterialError::InvalidSwizzle { slot, swizzle: text() });
            };
            if binding.texture.is_none() && swizzle != Swizzle::IDENTITY {
                return Err(MaterialError::UnboundSwizzle { slot, swizzle: text() });
            }
        }
        let flags = self.flags.to_native();
        MaterialFlags::from_bits(flags).ok_or(MaterialError::UnknownFlags(flags))?;
        let params = self.params.as_slice();
        match family {
            MaterialFamily::AnimeCharacter => self.check_block::<AnimeCharacterParams>(params),
            MaterialFamily::Environment => self.check_block::<EnvironmentParams>(params),
            MaterialFamily::Foliage => self.check_block::<FoliageParams>(params),
            MaterialFamily::Sky => self.check_block::<SkyParams>(params),
            MaterialFamily::FogCard => self.check_block::<FogCardParams>(params),
        }
    }

    /// Checks the block itself: every float is finite (one NaN pixel spreads through bloom),
    /// and its bound-slot bits and swizzle codes are the bindings', which the renderer checks
    /// against the textures it binds.
    fn check_block<B: GpuBlock>(&self, params: &[u8]) -> Result<(), MaterialError> {
        let block: B = bytemuck::pod_read_unaligned(params);
        if block.vectors().iter().flatten().any(|c| !c.is_finite()) {
            return Err(MaterialError::NonFinite);
        }
        let textures: Vec<Option<AssetId>> =
            self.textures.iter().map(ArchivedTextureBinding::texture).collect();
        if block.modes()[0] & 0xff != bound_bits(&textures) {
            return Err(MaterialError::BlockMismatch("bound slots"));
        }
        let swizzles: Vec<Swizzle> = self.textures.iter().map(ArchivedTextureBinding::swizzle).collect();
        if block.swizzles() != pack_swizzles(&swizzles) {
            return Err(MaterialError::BlockMismatch("swizzles"));
        }
        Ok(())
    }

    pub fn flags(&self) -> MaterialFlags {
        MaterialFlags::from_bits(self.flags.to_native()).unwrap_or_default()
    }

    /// The GPU block, as the shaders read it.
    pub fn params(&self) -> &[u8] {
        self.params.as_slice()
    }
}

/// A document of any family, as read from a `.material.ron` source.
#[derive(Clone, Debug, PartialEq)]
pub enum AnyMaterialDocument {
    AnimeCharacter(AnimeCharacterMaterial),
    Environment(EnvironmentMaterial),
    Foliage(FoliageMaterial),
    Sky(SkyMaterial),
    FogCard(FogCardMaterial),
}

/// Calls `$body` with the document inside `$any`, bound to `$document`.
macro_rules! with_document {
    ($any:expr, $document:ident => $body:expr) => {
        match $any {
            AnyMaterialDocument::AnimeCharacter($document) => $body,
            AnyMaterialDocument::Environment($document) => $body,
            AnyMaterialDocument::Foliage($document) => $body,
            AnyMaterialDocument::Sky($document) => $body,
            AnyMaterialDocument::FogCard($document) => $body,
        }
    };
}

impl AnyMaterialDocument {
    /// Reads a material document of whichever family its envelope names.
    pub fn from_ron(text: &str) -> Result<Self, DocumentError> {
        use sapphire_reflect::stable_name;
        use sapphire_serialize::from_ron;
        let type_name = sapphire_serialize::document::type_name(text)?;
        Ok(match type_name.as_str() {
            name if name == stable_name::<AnimeCharacterMaterial>() => Self::AnimeCharacter(from_ron(text)?),
            name if name == stable_name::<EnvironmentMaterial>() => Self::Environment(from_ron(text)?),
            name if name == stable_name::<FoliageMaterial>() => Self::Foliage(from_ron(text)?),
            name if name == stable_name::<SkyMaterial>() => Self::Sky(from_ron(text)?),
            name if name == stable_name::<FogCardMaterial>() => Self::FogCard(from_ron(text)?),
            _ => {
                return Err(DocumentError::WrongType {
                    expected: "a material family's document",
                    found: type_name,
                });
            }
        })
    }

    /// Writes the document.
    pub fn to_ron(&self) -> Result<String, DocumentError> {
        with_document!(self, document => sapphire_serialize::to_ron(document))
    }

    pub fn family(&self) -> MaterialFamily {
        with_document!(self, document => family_of(document))
    }

    pub fn validate(&self) -> Result<(), MaterialError> {
        with_document!(self, document => document.validate())
    }

    /// The texture each of the family's slots binds, in slot order.
    pub fn textures(&self) -> Vec<Option<AssetId>> {
        with_document!(self, document => document.textures())
    }

    /// [`Material::from_document`] for whichever family this is.
    pub fn cook(&self, swizzles: &[Swizzle]) -> Result<Material, MaterialError> {
        with_document!(self, document => Material::from_document(document, swizzles))
    }
}

fn family_of<D: MaterialDocument>(_: &D) -> MaterialFamily {
    D::FAMILY
}

#[cfg(test)]
mod tests;
