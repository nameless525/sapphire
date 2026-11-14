//! A texture's levels above its tail (`.smip`, M5 M3): one mip level each, an asset of its own.
//!
//! The cook splits a texture whose chain reaches above [`TAIL_SIZE`](crate::texture::TAIL_SIZE) into its
//! tail and its levels ([`crate::texture`]'s "Tails"). The tail is the texture's own asset, a whole KTX2
//! holding the levels from its top down, and the only thing a material depends on. Each level above it is a
//! [`TextureLevel`], whose ID derives from the texture's ([`TextureLevel::id`]), so loading, hashing,
//! residency, unmounting and patching take it as they take any asset, and packs hold it as they hold any.
//!
//! A level depends on its tail: loading one loads the tail first and keeps it loaded, so a level always
//! arrives for a texture the renderer already holds. Nothing in a pack depends on a level, so what loads a
//! texture loads its tail alone, and an app loads the levels it wants and raises the texture with them
//! (`AssetServer`'s "Texture levels", M5 M4).
//!
//! Every level above a tail can be its texture's top: its size is whole blocks of its format, as WebGPU
//! asks of a texture's level 0 (M5 M0). Levels are counted from the base, so level 0 is the texture at full
//! size, and a texture's levels above its tail are 0 to the tail's [`above`](crate::texture::TextureTail)
//! less one, each half the size of the one before it.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{AlignedVec, CookedError};

use crate::AssetId;
use crate::texture::{MAX_TEXTURE_DIMENSION, TextureFormat, full_mip_count};

/// Conventional file extension for cooked texture levels.
pub const TEXTURE_LEVEL_EXTENSION: &str = "smip";

/// The most levels a texture has, and so one more than the deepest level there can be.
pub const MAX_LEVELS: u32 = full_mip_count(MAX_TEXTURE_DIMENSION, MAX_TEXTURE_DIMENSION);

/// One level of a texture, above its tail. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::assets"]
pub struct TextureLevel {
    /// The texture it's a level of: its tail's ID, which is the texture's.
    pub texture: AssetId,
    /// Which level, from the base: 0 is the texture at full size.
    pub level: u32,
    /// The texture's format, as its `VkFormat` value (KTX2's numbering, [`TextureFormat::vk_format`]).
    pub format: u32,
    /// The level's size in texels.
    pub width: u32,
    pub height: u32,
    /// The level's blocks, tightly packed, as a KTX2 level is.
    pub data: Vec<u8>,
}

impl Versioned for TextureLevel {
    /// 1: the first, when textures split into a tail and levels (M5 M3).
    const VERSION: u32 = 1;
}

/// A cooked texture level that can't be read, or breaks an invariant of the format.
#[derive(Debug, thiserror::Error)]
pub enum TextureLevelError {
    /// The envelope, the schema version or the archive itself.
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("VkFormat {0} is not a format Sapphire cooks")]
    UnsupportedFormat(u32),
    #[error("level {0} is deeper than a texture's levels go (under {MAX_LEVELS})")]
    Level(u32),
    #[error(
        "level {level} at {width}x{height} would be a texture larger than {MAX_TEXTURE_DIMENSION} a side"
    )]
    InvalidSize { level: u32, width: u32, height: u32 },
    #[error("{width}x{height} isn't a whole number of {format:?} blocks, so it can't be a texture's top")]
    NotWholeBlocks { width: u32, height: u32, format: TextureFormat },
    #[error("{found} bytes of level data, expected {expected}")]
    DataSize { expected: u64, found: u64 },
}

impl TextureLevel {
    /// The ID of level `level` of the texture `texture`, as the cook gives it.
    pub fn id(texture: AssetId, level: u32) -> AssetId {
        texture.sub_asset(&format!("level/{level}"))
    }

    /// The level as cooked bytes: the envelope header, then the archive.
    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked texture level bytes in place and checks them ([`ArchivedTextureLevel::validate`]).
    /// The bytes must start 16-byte aligned.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedTextureLevel, TextureLevelError> {
        let level = sapphire_serialize::access::<Self>(bytes)?;
        level.validate()?;
        Ok(level)
    }
}

impl ArchivedTextureLevel {
    /// Checks every invariant in [`TextureLevelError`]. [`TextureLevel::read`] calls it.
    pub fn validate(&self) -> Result<(), TextureLevelError> {
        let format = self.format.to_native();
        let format =
            TextureFormat::from_vk_format(format).ok_or(TextureLevelError::UnsupportedFormat(format))?;
        let (level, width, height) = (self.level(), self.width.to_native(), self.height.to_native());
        if level >= MAX_LEVELS {
            return Err(TextureLevelError::Level(level));
        }
        let base = |side: u32| u64::from(side) << level;
        let fits = |side: u32| side > 0 && base(side) <= u64::from(MAX_TEXTURE_DIMENSION);
        if !fits(width) || !fits(height) {
            return Err(TextureLevelError::InvalidSize { level, width, height });
        }
        let (block_width, block_height) = format.block_dimensions();
        if !width.is_multiple_of(block_width) || !height.is_multiple_of(block_height) {
            return Err(TextureLevelError::NotWholeBlocks { width, height, format });
        }
        let (expected, found) = (format.image_byte_len(width, height), self.data.len() as u64);
        if expected != found {
            return Err(TextureLevelError::DataSize { expected, found });
        }
        Ok(())
    }

    /// The texture it's a level of.
    pub fn texture(&self) -> AssetId {
        self.texture
    }

    /// Which level, from the base.
    pub fn level(&self) -> u32 {
        self.level.to_native()
    }

    /// The texture's format.
    pub fn format(&self) -> TextureFormat {
        TextureFormat::from_vk_format(self.format.to_native()).expect("validated when read")
    }

    /// The level's size in texels.
    pub fn extent(&self) -> (u32, u32) {
        (self.width.to_native(), self.height.to_native())
    }

    /// The level's blocks.
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::AstcBlock;

    fn level() -> TextureLevel {
        let format = TextureFormat::Bc7RgbaUnormSrgb;
        TextureLevel {
            texture: AssetId::from_bytes([7; 16]),
            level: 1,
            format: format.vk_format(),
            width: 8,
            height: 4,
            data: (0..format.image_byte_len(8, 4)).map(|i| i as u8).collect(),
        }
    }

    #[test]
    fn a_level_reads_in_place() {
        let bytes = level().to_cooked().unwrap();
        let read = TextureLevel::read(&bytes).unwrap();
        assert_eq!(read.texture(), AssetId::from_bytes([7; 16]));
        assert_eq!(
            (read.level(), read.extent(), read.format()),
            (1, (8, 4), TextureFormat::Bc7RgbaUnormSrgb)
        );
        assert_eq!(read.data(), level().data.as_slice());
    }

    /// A level's ID is its texture's and its place, and nothing else: the same on every cook and machine.
    #[test]
    fn a_levels_id_derives_from_its_textures() {
        let texture = AssetId::from_bytes([7; 16]);
        assert_eq!(TextureLevel::id(texture, 0).to_string(), "c0b5ee8b-06eb-8e15-8007-c944f7d4a635");
        assert_ne!(TextureLevel::id(texture, 0), TextureLevel::id(texture, 1));
        assert_ne!(TextureLevel::id(texture, 0), TextureLevel::id(AssetId::from_bytes([8; 16]), 0));
    }

    /// Each row breaks one invariant and must come back as its error.
    #[test]
    fn a_bad_level_is_refused() {
        type Edit = fn(&mut TextureLevel);
        type Expected = fn(&TextureLevelError) -> bool;
        let rows: [(&str, Edit, Expected); 6] = [
            ("an unknown format", |l| l.format = 0, |e| matches!(e, TextureLevelError::UnsupportedFormat(0))),
            ("too deep", |l| l.level = MAX_LEVELS, |e| matches!(e, TextureLevelError::Level(_))),
            ("empty", |l| l.width = 0, |e| matches!(e, TextureLevelError::InvalidSize { .. })),
            (
                "a base too large",
                |l| (l.level, l.width) = (12, 8),
                |e| matches!(e, TextureLevelError::InvalidSize { level: 12, width: 8, .. }),
            ),
            (
                "part of a block",
                |l| l.format = TextureFormat::Astc { block: AstcBlock::B6x6, srgb: false }.vk_format(),
                |e| matches!(e, TextureLevelError::NotWholeBlocks { width: 8, height: 4, .. }),
            ),
            (
                "short",
                |l| l.data.truncate(l.data.len() - 1),
                |e| matches!(e, TextureLevelError::DataSize { .. }),
            ),
        ];
        let mut failures = Vec::new();
        for (name, edit, expected) in rows {
            let mut level = level();
            edit(&mut level);
            let bytes = level.to_cooked().unwrap();
            match TextureLevel::read(&bytes) {
                Err(error) if expected(&error) => {}
                other => failures.push(format!("{name}: {:?}", other.map(ArchivedTextureLevel::level))),
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
