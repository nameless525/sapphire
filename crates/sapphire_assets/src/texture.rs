//! Cooked texture format: KTX2 (`.ktx2`), as the pipeline's texture cook writes it (M1 A2).
//!
//! A cooked texture is a standard KTX 2.0 file, restricted to what the engine cooks:
//!
//! - One 2D image: no arrays, cubemaps or 3D. Level 0 is at most
//!   [`MAX_TEXTURE_DIMENSION`] on a side and, for block-compressed formats, a whole number of
//!   blocks on each side, which WebGPU (and so wgpu) requires.
//! - One to the full chain of mip levels, each tightly packed (rows of texels or blocks, no row
//!   padding), stored smallest first at the alignment the KTX2 spec asks for.
//! - One of the [`TextureFormat`]s, with the data format descriptor the `ktx2` crate generates
//!   for it, and no supercompression: packs compress their blocks (M1 A4).
//! - Key/value data, sorted by key: `KTXswizzle` when the stored channels aren't the
//!   texture's logical ones ([`Swizzle`]), `KTXwriter`, and `SapphireTextureVersion`, which
//!   must be [`TEXTURE_VERSION`].
//!
//! Standard tools read these files (`ktx info`). [`Texture`] borrows its levels from the file's
//! bytes, so reading copies nothing and the renderer uploads the levels as they are.
//!
//! **Decision (M1 A2):** KTX2 rather than an engine format. It already is a plain-data
//! container of GPU-ready levels with a self-describing format, tools exist for it, and A4's
//! packs hold it as one blob per texture. The writer lives here, next to the reader, as the
//! `.smesh` one does, so one module owns the format. It is written by hand on top of the
//! `ktx2` crate's header and DFD types because ctt's own KTX2 output can't carry the
//! key/value entries above.

use std::fmt;

use ktx2::dfd;
use ktx2::{Header, LevelIndex};
use serde::{Deserialize, Serialize};

/// Conventional file extension for cooked textures.
pub const TEXTURE_EXTENSION: &str = "ktx2";
/// The cooked texture profile described in the [module docs](self). Bump on any change to
/// it; readers reject files with another version.
pub const TEXTURE_VERSION: u32 = 1;
/// The largest width or height a cooked texture may have. Devices may support less: the
/// renderer checks its own limit when uploading.
pub const MAX_TEXTURE_DIMENSION: u32 = 16384;

/// Key/value entry holding [`TEXTURE_VERSION`] as decimal text.
const VERSION_KEY: &str = "SapphireTextureVersion";
/// The standard key naming the program that wrote the file. Constant, so a texture's bytes
/// don't depend on the pipeline's build.
const WRITER_KEY: &str = "KTXwriter";
const WRITER: &str = "sapphire_pipeline";
/// The standard key for a component mapping, as four characters from `rgba01`.
const SWIZZLE_KEY: &str = "KTXswizzle";

// Level data is written and read as little-endian bytes, which KTX2 specifies.
const _: () = assert!(cfg!(target_endian = "little"), "cooked textures assume a little-endian target");

/// ASTC block footprints the mobile cook writes. Larger blocks store fewer bits per texel.
/// Texture sidecars name them with serde (`astc_block: B6x6`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AstcBlock {
    /// 8 bits per texel: the highest quality.
    B4x4,
    /// 5.12 bits per texel.
    B5x5,
    /// 3.56 bits per texel.
    B6x6,
    /// 2 bits per texel.
    B8x8,
}

impl AstcBlock {
    /// Width and height of one block in texels.
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            Self::B4x4 => (4, 4),
            Self::B5x5 => (5, 5),
            Self::B6x6 => (6, 6),
            Self::B8x8 => (8, 8),
        }
    }
}

/// The formats cooked textures are stored in. Each maps to one `VkFormat` (in the KTX2
/// header) and one wgpu format (in `sapphire_render`); the names follow wgpu's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureFormat {
    /// 8-bit sRGB RGBA with linear alpha, uncompressed: ramps, which block compression bands.
    Rgba8UnormSrgb,
    /// 8-bit single channel: SDF face maps on mobile.
    R8Unorm,
    /// 8-bit two channels: two-channel SDF face maps on mobile.
    Rg8Unorm,
    /// 16-bit single channel: SDF face maps on desktop. Needs wgpu's
    /// `TEXTURE_FORMAT_16BIT_NORM`.
    R16Unorm,
    /// 16-bit two channels: two-channel SDF face maps on desktop. Needs
    /// `TEXTURE_FORMAT_16BIT_NORM`.
    Rg16Unorm,
    /// Half-float RGBA: HDR on mobile, where ASTC has no HDR profile wgpu can rely on.
    Rgba16Float,
    /// RGB with a shared 5-bit exponent and 9-bit mantissas, 32 bits per texel: the compact HDR
    /// format on mobile. Unsigned, no alpha.
    Rgb9e5Ufloat,
    /// BC4: one channel. Single-channel masks on desktop.
    Bc4RUnorm,
    /// BC5: two channels. Normal maps (X, Y) and two-channel masks on desktop.
    Bc5RgUnorm,
    /// BC6H, unsigned half-float RGB: HDR on desktop.
    Bc6hRgbUfloat,
    /// BC7, linear RGBA: ILM maps and three- or four-channel masks on desktop.
    Bc7RgbaUnorm,
    /// BC7, sRGB RGBA: color on desktop.
    Bc7RgbaUnormSrgb,
    /// ASTC LDR: everything block-compressed on mobile. `srgb` decodes RGB from sRGB.
    Astc { block: AstcBlock, srgb: bool },
}

impl TextureFormat {
    /// Every format, in a stable order.
    pub const ALL: [Self; 20] = [
        Self::Rgba8UnormSrgb,
        Self::R8Unorm,
        Self::Rg8Unorm,
        Self::R16Unorm,
        Self::Rg16Unorm,
        Self::Rgba16Float,
        Self::Rgb9e5Ufloat,
        Self::Bc4RUnorm,
        Self::Bc5RgUnorm,
        Self::Bc6hRgbUfloat,
        Self::Bc7RgbaUnorm,
        Self::Bc7RgbaUnormSrgb,
        Self::Astc { block: AstcBlock::B4x4, srgb: false },
        Self::Astc { block: AstcBlock::B4x4, srgb: true },
        Self::Astc { block: AstcBlock::B5x5, srgb: false },
        Self::Astc { block: AstcBlock::B5x5, srgb: true },
        Self::Astc { block: AstcBlock::B6x6, srgb: false },
        Self::Astc { block: AstcBlock::B6x6, srgb: true },
        Self::Astc { block: AstcBlock::B8x8, srgb: false },
        Self::Astc { block: AstcBlock::B8x8, srgb: true },
    ];

    /// Width and height of one block in texels: 1×1 for uncompressed formats.
    pub fn block_dimensions(self) -> (u32, u32) {
        match self {
            Self::Bc4RUnorm
            | Self::Bc5RgUnorm
            | Self::Bc6hRgbUfloat
            | Self::Bc7RgbaUnorm
            | Self::Bc7RgbaUnormSrgb => (4, 4),
            Self::Astc { block, .. } => block.dimensions(),
            _ => (1, 1),
        }
    }

    /// Bytes per block, or per texel for uncompressed formats.
    pub fn block_bytes(self) -> u32 {
        match self {
            Self::R8Unorm => 1,
            Self::Rg8Unorm | Self::R16Unorm => 2,
            Self::Rgba8UnormSrgb | Self::Rg16Unorm | Self::Rgb9e5Ufloat => 4,
            Self::Rgba16Float | Self::Bc4RUnorm => 8,
            Self::Bc5RgUnorm
            | Self::Bc6hRgbUfloat
            | Self::Bc7RgbaUnorm
            | Self::Bc7RgbaUnormSrgb
            | Self::Astc { .. } => 16,
        }
    }

    /// Whether texels are stored in blocks larger than one texel.
    pub fn is_compressed(self) -> bool {
        self.block_dimensions() != (1, 1)
    }

    /// Whether sampling decodes RGB from sRGB to linear.
    pub fn is_srgb(self) -> bool {
        matches!(self, Self::Rgba8UnormSrgb | Self::Bc7RgbaUnormSrgb | Self::Astc { srgb: true, .. })
    }

    /// Bytes of one tightly packed `width × height` image: whole blocks, no row padding.
    pub fn image_byte_len(self, width: u32, height: u32) -> u64 {
        let (block_width, block_height) = self.block_dimensions();
        u64::from(width.div_ceil(block_width))
            * u64::from(height.div_ceil(block_height))
            * u64::from(self.block_bytes())
    }

    /// The format's `VkFormat` value, as the KTX2 header stores it.
    pub fn vk_format(self) -> u32 {
        self.ktx2().value()
    }

    /// The format with this `VkFormat` value, if it is one Sapphire cooks.
    pub fn from_vk_format(value: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|format| format.vk_format() == value)
    }

    fn ktx2(self) -> ktx2::Format {
        use ktx2::Format as F;
        match self {
            Self::Rgba8UnormSrgb => F::R8G8B8A8_SRGB,
            Self::R8Unorm => F::R8_UNORM,
            Self::Rg8Unorm => F::R8G8_UNORM,
            Self::R16Unorm => F::R16_UNORM,
            Self::Rg16Unorm => F::R16G16_UNORM,
            Self::Rgba16Float => F::R16G16B16A16_SFLOAT,
            Self::Rgb9e5Ufloat => F::E5B9G9R9_UFLOAT_PACK32,
            Self::Bc4RUnorm => F::BC4_UNORM_BLOCK,
            Self::Bc5RgUnorm => F::BC5_UNORM_BLOCK,
            Self::Bc6hRgbUfloat => F::BC6H_UFLOAT_BLOCK,
            Self::Bc7RgbaUnorm => F::BC7_UNORM_BLOCK,
            Self::Bc7RgbaUnormSrgb => F::BC7_SRGB_BLOCK,
            Self::Astc { block, srgb } => match (block, srgb) {
                (AstcBlock::B4x4, false) => F::ASTC_4x4_UNORM_BLOCK,
                (AstcBlock::B4x4, true) => F::ASTC_4x4_SRGB_BLOCK,
                (AstcBlock::B5x5, false) => F::ASTC_5x5_UNORM_BLOCK,
                (AstcBlock::B5x5, true) => F::ASTC_5x5_SRGB_BLOCK,
                (AstcBlock::B6x6, false) => F::ASTC_6x6_UNORM_BLOCK,
                (AstcBlock::B6x6, true) => F::ASTC_6x6_SRGB_BLOCK,
                (AstcBlock::B8x8, false) => F::ASTC_8x8_UNORM_BLOCK,
                (AstcBlock::B8x8, true) => F::ASTC_8x8_SRGB_BLOCK,
            },
        }
    }

    /// The format's data format descriptor and KTX2 `typeSize`.
    fn dfd(self) -> (dfd::Basic, u32) {
        dfd::Basic::from_format(self.ktx2()).expect("the ktx2 crate describes every TextureFormat")
    }
}

/// Where a shader reads one logical channel from: a sampled channel or a constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Component {
    R,
    G,
    B,
    A,
    Zero,
    One,
}

impl Component {
    fn to_ktx(self) -> u8 {
        match self {
            Self::R => b'r',
            Self::G => b'g',
            Self::B => b'b',
            Self::A => b'a',
            Self::Zero => b'0',
            Self::One => b'1',
        }
    }

    fn from_ktx(byte: u8) -> Option<Self> {
        Some(match byte {
            b'r' => Self::R,
            b'g' => Self::G,
            b'b' => Self::B,
            b'a' => Self::A,
            b'0' => Self::Zero,
            b'1' => Self::One,
            _ => return None,
        })
    }
}

/// How a texture's logical RGBA channels are read from what sampling it returns.
///
/// Usually the identity. ASTC stores two-channel data (normal maps, two-channel masks) as
/// luminance plus alpha, which samples as `(x, x, x, y)`; its swizzle `ra01` turns that back
/// into the `(x, y, 0, 1)` a BC5 texture samples as, so materials (M1 A3) read both cooks the
/// same way once they apply it. wgpu views can't swizzle, so shaders do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Swizzle(pub [Component; 4]);

impl Swizzle {
    pub const IDENTITY: Self = Self([Component::R, Component::G, Component::B, Component::A]);

    /// The logical channels of a texel that sampled as `sampled`.
    pub fn apply(self, sampled: [f32; 4]) -> [f32; 4] {
        self.0.map(|component| match component {
            Component::R => sampled[0],
            Component::G => sampled[1],
            Component::B => sampled[2],
            Component::A => sampled[3],
            Component::Zero => 0.0,
            Component::One => 1.0,
        })
    }

    fn to_ktx(self) -> [u8; 4] {
        self.0.map(Component::to_ktx)
    }

    fn from_ktx(text: &[u8]) -> Option<Self> {
        let chars: [u8; 4] = text.try_into().ok()?;
        let mut components = [Component::R; 4];
        for (component, byte) in components.iter_mut().zip(chars) {
            *component = Component::from_ktx(byte)?;
        }
        Some(Self(components))
    }
}

impl Default for Swizzle {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl fmt::Display for Swizzle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.to_ktx().iter().try_for_each(|&byte| fmt::Write::write_char(f, char::from(byte)))
    }
}

/// The number of levels in a full mip chain for a `width × height` image, down to 1×1.
pub fn full_mip_count(width: u32, height: u32) -> u32 {
    u32::BITS - width.max(height).max(1).leading_zeros()
}

/// A texture's shape: everything except its texel data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureDesc {
    pub format: TextureFormat,
    /// Level 0's size in texels.
    pub width: u32,
    pub height: u32,
    /// Levels stored, from 1 to [`full_mip_count`]. Level `i` is `width >> i` by
    /// `height >> i`, each at least 1.
    pub mip_level_count: u32,
    pub swizzle: Swizzle,
}

impl TextureDesc {
    /// The size of `level` in texels.
    pub fn level_extent(&self, level: u32) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }

    /// The size of `level`'s data in bytes.
    pub fn level_byte_len(&self, level: u32) -> u64 {
        let (width, height) = self.level_extent(level);
        self.format.image_byte_len(width, height)
    }

    /// Checks the rules in the [module docs](self) that the shape alone decides.
    pub fn validate(&self) -> Result<(), TextureFormatError> {
        let (width, height) = (self.width, self.height);
        if !(1..=MAX_TEXTURE_DIMENSION).contains(&width) || !(1..=MAX_TEXTURE_DIMENSION).contains(&height) {
            return Err(TextureFormatError::InvalidSize { width, height });
        }
        let (block_width, block_height) = self.format.block_dimensions();
        if !width.is_multiple_of(block_width) || !height.is_multiple_of(block_height) {
            return Err(TextureFormatError::NotWholeBlocks { width, height, format: self.format });
        }
        let max = full_mip_count(width, height);
        if !(1..=max).contains(&self.mip_level_count) {
            return Err(TextureFormatError::InvalidLevelCount { count: self.mip_level_count, max });
        }
        Ok(())
    }
}

/// A cooked texture: its shape and the bytes of each mip level, largest first. Borrows the
/// level data, from a cooked file ([`Self::from_ktx2`]) or from the cooker's buffers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture<'a> {
    pub desc: TextureDesc,
    /// `desc.mip_level_count` levels, each [`TextureDesc::level_byte_len`] bytes.
    pub levels: Vec<&'a [u8]>,
}

/// Errors from reading, writing or validating a cooked texture.
#[derive(Debug, thiserror::Error)]
pub enum TextureFormatError {
    #[error("not a valid KTX2 file")]
    Container(#[source] ktx2::ParseError),
    #[error("VkFormat {0} is not a format Sapphire cooks")]
    UnsupportedFormat(u32),
    #[error("not a single 2D image (arrays, cubemaps and 3D textures aren't cooked)")]
    NotA2dImage,
    #[error("supercompressed level data is not supported; packs compress cooked data")]
    Supercompressed,
    #[error("no {VERSION_KEY} entry: not a texture the Sapphire pipeline cooked")]
    MissingVersion,
    #[error("unsupported texture version {found:?} (expected {TEXTURE_VERSION})")]
    UnsupportedVersion { found: String },
    #[error("invalid {SWIZZLE_KEY} value {0:?}")]
    InvalidSwizzle(String),
    #[error("the data format descriptor or type size doesn't match {0:?}")]
    DescriptorMismatch(TextureFormat),
    #[error("size {width}x{height} is empty or larger than {MAX_TEXTURE_DIMENSION}")]
    InvalidSize { width: u32, height: u32 },
    #[error("size {width}x{height} isn't a whole number of {format:?} blocks, which WebGPU requires")]
    NotWholeBlocks { width: u32, height: u32, format: TextureFormat },
    #[error("{count} mip levels; a texture this size has 1 to {max}")]
    InvalidLevelCount { count: u32, max: u32 },
    #[error("mip level {level} has {found} bytes, expected {expected}")]
    LevelSize { level: u32, expected: u64, found: u64 },
}

impl<'a> Texture<'a> {
    /// Checks every structural invariant the renderer relies on.
    pub fn validate(&self) -> Result<(), TextureFormatError> {
        self.desc.validate()?;
        let found = u32::try_from(self.levels.len()).unwrap_or(u32::MAX);
        if found != self.desc.mip_level_count {
            let max = full_mip_count(self.desc.width, self.desc.height);
            return Err(TextureFormatError::InvalidLevelCount { count: found, max });
        }
        for (level, data) in (0..).zip(&self.levels) {
            let expected = self.desc.level_byte_len(level);
            if data.len() as u64 != expected {
                return Err(TextureFormatError::LevelSize { level, expected, found: data.len() as u64 });
            }
        }
        Ok(())
    }

    /// Writes the texture as a KTX2 file. The bytes depend only on the texture, so equal
    /// textures write equal files.
    pub fn to_ktx2(&self) -> Result<Vec<u8>, TextureFormatError> {
        self.validate()?;
        let desc = &self.desc;
        let (dfd, type_size) = desc.format.dfd();
        let dfd = dfd_section(dfd);
        let kvd = key_value_data(desc.swizzle);

        let level_count = self.levels.len();
        let dfd_offset = Header::LENGTH + level_count * LevelIndex::LENGTH;
        let kvd_offset = dfd_offset + dfd.len();
        // KTX2 §3.9.7: levels start at multiples of lcm(texel block size, 4). Block sizes are
        // powers of two, so that is the larger of the two.
        let alignment = (desc.format.block_bytes() as usize).max(4);
        // Levels are stored smallest first, so a reader streaming the file gets usable low
        // mips early.
        let mut index =
            vec![LevelIndex { byte_offset: 0, byte_length: 0, uncompressed_byte_length: 0 }; level_count];
        let mut offset = kvd_offset + kvd.len();
        for (entry, data) in index.iter_mut().zip(&self.levels).rev() {
            offset = offset.next_multiple_of(alignment);
            let len = data.len() as u64;
            *entry =
                LevelIndex { byte_offset: offset as u64, byte_length: len, uncompressed_byte_length: len };
            offset += data.len();
        }

        let header = Header {
            format: Some(desc.format.ktx2()),
            type_size,
            pixel_width: desc.width,
            pixel_height: desc.height,
            pixel_depth: 0,
            layer_count: 0,
            face_count: 1,
            level_count: desc.mip_level_count,
            supercompression_scheme: None,
            index: ktx2::Index {
                dfd_byte_offset: to_u32(dfd_offset),
                dfd_byte_length: to_u32(dfd.len()),
                kvd_byte_offset: to_u32(kvd_offset),
                kvd_byte_length: to_u32(kvd.len()),
                sgd_byte_offset: 0,
                sgd_byte_length: 0,
            },
        };

        let mut out = Vec::with_capacity(offset);
        out.extend_from_slice(&header.as_bytes());
        for entry in &index {
            out.extend_from_slice(&entry.as_bytes());
        }
        out.extend_from_slice(&dfd);
        out.extend_from_slice(&kvd);
        for (entry, data) in index.iter().zip(&self.levels).rev() {
            out.resize(entry.byte_offset as usize, 0);
            out.extend_from_slice(data);
        }
        Ok(out)
    }

    /// Parses and validates a cooked KTX2 file. The levels borrow from `bytes`, which needs no
    /// particular alignment.
    pub fn from_ktx2(bytes: &'a [u8]) -> Result<Self, TextureFormatError> {
        let truncated = || TextureFormatError::Container(ktx2::ParseError::UnexpectedEnd);
        let header = bytes.first_chunk::<{ Header::LENGTH }>().ok_or_else(truncated)?;
        let header = Header::from_bytes(header).map_err(TextureFormatError::Container)?;
        if header.supercompression_scheme.is_some() {
            return Err(TextureFormatError::Supercompressed);
        }
        if header.pixel_height == 0
            || header.pixel_depth != 0
            || header.layer_count != 0
            || header.face_count != 1
        {
            return Err(TextureFormatError::NotA2dImage);
        }
        let vk_format = header.format.map_or(0, |format| format.value());
        let format = TextureFormat::from_vk_format(vk_format)
            .ok_or(TextureFormatError::UnsupportedFormat(vk_format))?;
        // The descriptor must be exactly the one the writer writes, compared as bytes before
        // `ktx2::Reader` parses it: ktx2 0.5.0 panics on some malformed descriptors (a texel
        // block dimension of 255), and runtime code must not panic on a bad file.
        let (dfd, type_size) = format.dfd();
        let dfd_start = header.index.dfd_byte_offset as usize;
        let dfd_end = dfd_start.saturating_add(header.index.dfd_byte_length as usize);
        let found = bytes.get(dfd_start..dfd_end).ok_or_else(truncated)?;
        if *found != dfd_section(dfd) || header.type_size != type_size {
            return Err(TextureFormatError::DescriptorMismatch(format));
        }
        // Checks that the key/value data and every level lie inside `bytes`.
        ktx2::Reader::new(bytes).map_err(TextureFormatError::Container)?;

        // These slices are taken from `bytes` itself, so they borrow it rather than the reader.
        let kvd_start = header.index.kvd_byte_offset as usize;
        let kvd_end = kvd_start.saturating_add(header.index.kvd_byte_length as usize);
        let kvd = bytes.get(kvd_start..kvd_end).unwrap_or_default();
        let mut version = None;
        let mut swizzle = Swizzle::IDENTITY;
        for (key, value) in ktx2::KeyValueDataIterator::new(kvd) {
            // String values end with a NUL.
            let value = value.strip_suffix(&[0]).unwrap_or(value);
            match key {
                VERSION_KEY => version = Some(value),
                SWIZZLE_KEY => {
                    swizzle = Swizzle::from_ktx(value).ok_or_else(|| {
                        TextureFormatError::InvalidSwizzle(String::from_utf8_lossy(value).into_owned())
                    })?;
                }
                _ => {}
            }
        }
        let version = version.ok_or(TextureFormatError::MissingVersion)?;
        if version != TEXTURE_VERSION.to_string().as_bytes() {
            return Err(TextureFormatError::UnsupportedVersion {
                found: String::from_utf8_lossy(version).into_owned(),
            });
        }

        let desc = TextureDesc {
            format,
            width: header.pixel_width,
            height: header.pixel_height,
            mip_level_count: header.level_count,
            swizzle,
        };
        desc.validate()?;
        let levels = (0..desc.mip_level_count as usize)
            .map(|level| {
                let at = Header::LENGTH + level * LevelIndex::LENGTH;
                let entry = bytes.get(at..at + LevelIndex::LENGTH).and_then(|entry| entry.try_into().ok());
                let entry = LevelIndex::from_bytes(
                    entry.ok_or(TextureFormatError::Container(ktx2::ParseError::UnexpectedEnd))?,
                );
                let start = usize::try_from(entry.byte_offset).unwrap_or(usize::MAX);
                let len = usize::try_from(entry.byte_length).unwrap_or(usize::MAX);
                bytes
                    .get(start..start.saturating_add(len))
                    .ok_or(TextureFormatError::Container(ktx2::ParseError::UnexpectedEnd))
            })
            .collect::<Result<_, _>>()?;
        let texture = Self { desc, levels };
        texture.validate()?;
        Ok(texture)
    }
}

/// The key/value data section: every entry, sorted by key as KTX2 requires, each padded to 4
/// bytes.
fn dfd_section(dfd: dfd::Basic) -> Vec<u8> {
    let block = dfd::Block::Basic(dfd).to_vec();
    // The section starts with its own total length.
    let mut section = to_u32(4 + block.len()).to_le_bytes().to_vec();
    section.extend_from_slice(&block);
    section
}

fn key_value_data(swizzle: Swizzle) -> Vec<u8> {
    let version = TEXTURE_VERSION.to_string();
    let swizzle = swizzle.to_ktx();
    let mut entries: Vec<(&str, &[u8])> =
        vec![(VERSION_KEY, version.as_bytes()), (WRITER_KEY, WRITER.as_bytes())];
    if swizzle != Swizzle::IDENTITY.to_ktx() {
        entries.push((SWIZZLE_KEY, &swizzle));
    }
    entries.sort_by_key(|&(key, _)| key.as_bytes());
    let mut out = Vec::new();
    for (key, value) in entries {
        // Key, NUL, value, NUL: every value here is a string.
        let len = key.len() + 1 + value.len() + 1;
        out.extend_from_slice(&to_u32(len).to_le_bytes());
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        out.extend_from_slice(value);
        out.push(0);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    out
}

fn to_u32(len: usize) -> u32 {
    u32::try_from(len).expect("KTX2 sections of a texture within MAX_TEXTURE_DIMENSION fit in u32")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Level bytes that differ between levels and positions, so a misplaced level shows.
    fn levels_for(desc: &TextureDesc) -> Vec<Vec<u8>> {
        (0..desc.mip_level_count)
            .map(|level| {
                (0..desc.level_byte_len(level)).map(|i| (i as u8).wrapping_mul(31) ^ level as u8).collect()
            })
            .collect()
    }

    fn desc(format: TextureFormat, width: u32, height: u32) -> TextureDesc {
        TextureDesc {
            format,
            width,
            height,
            mip_level_count: full_mip_count(width, height),
            swizzle: Swizzle::IDENTITY,
        }
    }

    fn write(desc: TextureDesc) -> Vec<u8> {
        let levels = levels_for(&desc);
        Texture { desc, levels: levels.iter().map(Vec::as_slice).collect() }.to_ktx2().unwrap()
    }

    #[test]
    fn mip_counts_and_extents() {
        assert_eq!(full_mip_count(1, 1), 1);
        assert_eq!(full_mip_count(2, 1), 2);
        assert_eq!(full_mip_count(256, 16), 9);
        assert_eq!(full_mip_count(255, 3), 8);
        assert_eq!(full_mip_count(1000, 1000), 10);
        let d = desc(TextureFormat::Bc7RgbaUnorm, 20, 12);
        let extents: Vec<_> = (0..d.mip_level_count).map(|level| d.level_extent(level)).collect();
        assert_eq!(extents, [(20, 12), (10, 6), (5, 3), (2, 1), (1, 1)]);
        // Small levels of block formats still take whole blocks.
        let bytes: Vec<_> = (0..d.mip_level_count).map(|level| d.level_byte_len(level)).collect();
        assert_eq!(bytes, [15 * 16, 6 * 16, 2 * 16, 16, 16]);
        assert_eq!(TextureFormat::Astc { block: AstcBlock::B6x6, srgb: true }.image_byte_len(13, 6), 3 * 16);
        assert_eq!(TextureFormat::Rgb9e5Ufloat.image_byte_len(3, 5), 60);
    }

    #[test]
    fn every_format_round_trips_through_ktx2() {
        for format in TextureFormat::ALL {
            assert_eq!(TextureFormat::from_vk_format(format.vk_format()), Some(format));
            let (block_width, block_height) = format.block_dimensions();
            let d =
                TextureDesc { swizzle: Swizzle::IDENTITY, ..desc(format, block_width * 5, block_height * 3) };
            let levels = levels_for(&d);
            let texture = Texture { desc: d, levels: levels.iter().map(Vec::as_slice).collect() };
            let bytes = texture.to_ktx2().unwrap();
            let read = Texture::from_ktx2(&bytes).unwrap();
            assert_eq!(read, texture, "{format:?}");
            assert_eq!(texture.to_ktx2().unwrap(), bytes, "{format:?}: writing is deterministic");

            // The ktx2 crate reads the same levels, smallest first in the file, each aligned.
            let reader = ktx2::Reader::new(&bytes[..]).unwrap();
            let levels: Vec<&[u8]> = reader.levels().map(|level| level.data).collect();
            assert_eq!(levels, texture.levels, "{format:?}");
            // String values carry their NUL, which the crate leaves in.
            assert_eq!(reader.writer(), Some("sapphire_pipeline\0"));
            let offsets: Vec<usize> =
                read.levels.iter().map(|level| level.as_ptr() as usize - bytes.as_ptr() as usize).collect();
            assert!(offsets.windows(2).all(|pair| pair[0] > pair[1]), "{format:?}: {offsets:?}");
            let alignment = format.block_bytes().max(4) as usize;
            assert!(offsets.iter().all(|offset| offset % alignment == 0), "{format:?}: {offsets:?}");
        }
    }

    #[test]
    fn swizzles_round_trip_and_apply() {
        let ra01 = Swizzle([Component::R, Component::A, Component::Zero, Component::One]);
        assert_eq!(ra01.to_string(), "ra01");
        assert_eq!(ra01.apply([0.25, 0.25, 0.25, 0.75]), [0.25, 0.75, 0.0, 1.0]);
        assert_eq!(Swizzle::IDENTITY.apply([1.0, 2.0, 3.0, 4.0]), [1.0, 2.0, 3.0, 4.0]);

        let d = TextureDesc {
            swizzle: ra01,
            ..desc(TextureFormat::Astc { block: AstcBlock::B4x4, srgb: false }, 8, 8)
        };
        let bytes = write(d);
        assert_eq!(Texture::from_ktx2(&bytes).unwrap().desc.swizzle, ra01);
        let reader = ktx2::Reader::new(&bytes[..]).unwrap();
        let keys: Vec<&str> = reader.key_value_data().map(|(key, _)| key).collect();
        assert_eq!(keys, [SWIZZLE_KEY, WRITER_KEY, VERSION_KEY], "sorted by key");
        assert!(reader.key_value_data().any(|(key, value)| key == SWIZZLE_KEY && value == b"ra01\0"));

        // The identity isn't written.
        let plain = write(desc(TextureFormat::Bc5RgUnorm, 8, 8));
        assert!(ktx2::Reader::new(&plain[..]).unwrap().key_value_data().all(|(key, _)| key != SWIZZLE_KEY));
    }

    #[test]
    fn reads_unaligned_input() {
        let d = desc(TextureFormat::Rgba16Float, 6, 5);
        let mut bytes = vec![0u8];
        bytes.extend(write(d));
        let texture = Texture::from_ktx2(&bytes[1..]).unwrap();
        assert_eq!(texture.desc, d);
        let levels = levels_for(&d);
        assert_eq!(texture.levels, levels.iter().map(Vec::as_slice).collect::<Vec<_>>());
    }

    #[test]
    fn rejects_textures_the_renderer_cannot_use() {
        let invalid = |d: TextureDesc| {
            let levels = levels_for(&TextureDesc { mip_level_count: d.mip_level_count.min(15), ..d });
            Texture { desc: d, levels: levels.iter().map(Vec::as_slice).collect() }.to_ktx2().unwrap_err()
        };
        let bc7 = desc(TextureFormat::Bc7RgbaUnormSrgb, 16, 8);
        assert!(matches!(
            invalid(TextureDesc { width: 18, ..bc7 }),
            TextureFormatError::NotWholeBlocks { width: 18, height: 8, .. }
        ));
        assert!(matches!(invalid(TextureDesc { width: 0, ..bc7 }), TextureFormatError::InvalidSize { .. }));
        assert!(matches!(
            invalid(TextureDesc { width: MAX_TEXTURE_DIMENSION + 4, ..bc7 }),
            TextureFormatError::InvalidSize { .. }
        ));
        assert!(matches!(
            invalid(TextureDesc { mip_level_count: 6, ..bc7 }),
            TextureFormatError::InvalidLevelCount { count: 6, max: 5 }
        ));
        assert!(matches!(
            invalid(TextureDesc { mip_level_count: 0, ..bc7 }),
            TextureFormatError::InvalidLevelCount { count: 0, max: 5 }
        ));

        let levels = levels_for(&bc7);
        let mut short: Vec<&[u8]> = levels.iter().map(Vec::as_slice).collect();
        short[1] = &short[1][1..];
        let error = Texture { desc: bc7, levels: short.clone() }.validate().unwrap_err();
        assert!(
            matches!(error, TextureFormatError::LevelSize { level: 1, expected: 32, found: 31 }),
            "{error}"
        );
        short.pop();
        let error = Texture { desc: bc7, levels: short }.validate().unwrap_err();
        assert!(matches!(error, TextureFormatError::InvalidLevelCount { count: 4, max: 5 }), "{error}");
    }

    #[test]
    fn rejects_files_it_did_not_write() {
        let bytes = write(desc(TextureFormat::Bc4RUnorm, 8, 4));
        let patched = |at: usize, value: u32| {
            let mut bytes = bytes.clone();
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            Texture::from_ktx2(&bytes).unwrap_err()
        };
        // Header fields, by offset: format 12, depth 28, layers 32, faces 36, levels 40,
        // supercompression 44.
        assert!(matches!(
            patched(12, ktx2::Format::BC1_RGB_UNORM_BLOCK.value()),
            TextureFormatError::UnsupportedFormat(_)
        ));
        assert!(matches!(
            patched(12, ktx2::Format::BC5_UNORM_BLOCK.value()),
            TextureFormatError::DescriptorMismatch(_)
        ));
        assert!(matches!(patched(28, 2), TextureFormatError::NotA2dImage));
        assert!(matches!(patched(32, 2), TextureFormatError::NotA2dImage));
        assert!(matches!(patched(36, 6), TextureFormatError::NotA2dImage));
        assert!(matches!(patched(44, 2), TextureFormatError::Supercompressed));
        assert!(matches!(patched(0, 0), TextureFormatError::Container(_)));
        assert!(matches!(Texture::from_ktx2(&bytes[..100]), Err(TextureFormatError::Container(_))));
        assert!(matches!(Texture::from_ktx2(&bytes[..40]), Err(TextureFormatError::Container(_))));

        // A texel block dimension of 255 makes ktx2 0.5.0's descriptor parser panic; the
        // reader turns it away first. The dimensions follow the DFD section's length (4 bytes),
        // the block header (8) and four one-byte fields.
        let dimensions = u32::from_le_bytes(bytes[48..52].try_into().unwrap()) as usize + 4 + 8 + 4;
        for i in 0..4 {
            let mut bad = bytes.clone();
            bad[dimensions + i] = 0xff;
            assert!(matches!(Texture::from_ktx2(&bad), Err(TextureFormatError::DescriptorMismatch(_))));
        }

        // A level index entry whose length doesn't match the format's.
        let first_entry = Header::LENGTH + 8;
        assert!(matches!(
            patched(first_entry, 15),
            TextureFormatError::LevelSize { level: 0, expected: 16, found: 15 }
        ));

        let mut versioned = bytes.clone();
        let at = versioned.windows(VERSION_KEY.len()).position(|w| w == VERSION_KEY.as_bytes()).unwrap();
        versioned[at + VERSION_KEY.len() + 1] = b'9';
        assert!(matches!(
            Texture::from_ktx2(&versioned),
            Err(TextureFormatError::UnsupportedVersion { found }) if found == "9"
        ));
        versioned[at] = b'X';
        assert!(matches!(Texture::from_ktx2(&versioned), Err(TextureFormatError::MissingVersion)));
    }
}
