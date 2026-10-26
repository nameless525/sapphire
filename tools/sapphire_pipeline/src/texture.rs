//! Textures (M1 A2): texture classes as typed import settings, their cook rules per target,
//! and the cook from a [`SourceImage`] to a KTX2 [`Texture`].
//!
//! A texture's sidecar names its class (`importer: Texture((class: Normal(())))`); new PNG and
//! TGA sources default to [`TextureClass::Color`], EXR sources to [`TextureClass::Hdr`]. The
//! class decides how the source's integer samples are read ([`Transfer`]), how mips combine
//! ([`MipFilter`]) and the format per cook:
//!
//! | Class | Integer source | `desktop` | `mobile` | Mips | Logical channels |
//! |---|---|---|---|---|---|
//! | Color | sRGB | BC7 sRGB | ASTC sRGB, 4×4 by default | alpha-weighted | RGBA |
//! | Normal | linear | BC5 | ASTC, 4×4 by default (swizzle `ra01`) | renormalized | X, Y |
//! | Mask, 1 / 2 / 3–4 channels | linear | BC4 / BC5 / BC7 | ASTC (`r001` / `ra01` / —) | data | the first 1–4 |
//! | Ramp | sRGB | RGBA8 sRGB | RGBA8 sRGB | none unless asked | RGBA |
//! | ILM | linear | BC7 | ASTC 4×4 | data | RGBA |
//! | SDF, 1 / 2 channels | linear | R16 or R8 / RG16 or RG8 | R8 / RG8 | data | R and G as 1 (swizzle `r101`) / R, G |
//! | HDR | sRGB | BC6H | RGB9E5 or RGBA16F | data | RGB(A), clamped to `[0, 65504]` |
//!
//! - **Color alpha is coverage:** mips weight color by it, so color under transparent texels
//!   doesn't bleed into visible ones. Textures whose alpha holds data are masks or ILM maps.
//! - **Ramps are never block-compressed:** compression bands their gradients, and a 256 × N
//!   ramp is small (spec §3.4). They get no mips unless asked, because rows are separate
//!   materials and mips would blend them.
//! - **ILM maps** are data, so BC7 runs without perceptual weighting, and ASTC is fixed at 4×4
//!   (spec §5.27).
//! - **SDF face maps** are 16-bit on desktop by default, since the face shadow sweeps through
//!   their whole range, and 8-bit on mobile (spec §5.27), where 16-bit normalized formats
//!   aren't guaranteed. A two-channel map holds the horizontal and vertical thresholds (A7);
//!   a one-channel map's swizzle reads G as 1, the threshold of a texel no rising light
//!   shadows, so the face shader treats both alike (C3).
//! - **HDR** has no ASTC on mobile: ctt has no HDR ASTC encoder (its issue #74), and ASTC
//!   HDR isn't guaranteed on phones either. RGB9E5 (the default) is 4 bytes per texel and has
//!   no alpha; RGBA16F keeps alpha at twice the size.
//! - **Normal maps** store X and Y in `[0, 1]` and the shader rebuilds Z. The cook reads X and
//!   Y the same way, rebuilds Z, and averages and renormalizes the full vectors, so mips are
//!   unit normals. `flip_y` converts DirectX-convention (`-Y`) maps to glTF's `+Y`.
//! - **Logical channels** are what a material samples after applying the texture's swizzle:
//!   ASTC stores two channels as luminance plus alpha, and the swizzle undoes that, so both
//!   cooks read the same. A class that packs channels takes the source's logical channels in
//!   order ([`ChannelLayout::logical`]): an RGB mask's three, a gray + alpha mask's two.
//!
//! Block-compressed textures need sizes that are whole blocks (multiples of 4, or of the ASTC
//! block), which WebGPU requires of level 0; other sizes fail the cook with that reason.
//!
//! # Determinism
//!
//! A cook's output depends only on its inputs: mips are computed in a fixed order, the
//! encoders run serially (ctt without `rayon`) and the KTX2 writer writes no timestamps, so
//! two cooks of one source are byte-identical. Across machines, the encoders' SIMD kernels
//! (prebuilt ISPC, astcenc) and the platform's `powf` (sRGB decoding) may round differently;
//! the shared cache (later) takes whichever machine cooked first, which is still a valid
//! texture.
//!
//! # Later milestones
//!
//! - Texture arrays (the ramp arrays C1 indexes by material ID) and cubemaps (the sky, W2),
//!   as further shapes of the same container.
//! - Mips that preserve alpha-tested coverage, for hair and lashes (ctt's issue #107 is the
//!   same problem in its own mip generator).
//! - Per-cook size caps from the content budgets (spec §3.4: 2K desktop, 1K mobile), by
//!   dropping the top levels on mobile.
//! - Parallel encoding (ctt's `rayon` feature, whose output doesn't depend on the thread count)
//!   once content is large enough; a 2048² color texture takes 1.2 s (BC7) and 0.5 s (ASTC)
//!   serially on an M5.

pub mod mips;

mod encode;

use std::path::Path;

use sapphire_assets::character_maps::face_sdf;
use sapphire_assets::texture::{
    AstcBlock, Component, Swizzle, Texture, TextureDesc, TextureFormat, TextureFormatError, full_mip_count,
};
use serde::{Deserialize, Serialize};

use self::encode::{AstcUsage, Encoding};
use self::mips::{Level, MipFilter};
use crate::cache::Stage;
use crate::cook::Cook;
use crate::import::image::{
    self as image_import, ChannelLayout, ImageImportError, SourceFormat, SourceImage, Transfer,
};

/// Mips, encoding and the KTX2 container, as cache keys know them. Bump the version whenever
/// they change output for the same image. The name carries the versions of the crates whose
/// output the cook embeds, and a test keeps it in step with `Cargo.lock`: every `ctt*` crate
/// (the encoders are separate crates on caret ranges, so `cargo update` can move them alone)
/// and `ktx2`, whose generated descriptors the reader compares byte for byte.
pub const COOKER: Stage = Stage { name: "ktx2 (ktx2 0.5.0, ctt 0.6.0)", version: 2 };

/// The largest finite half float, the top of BC6H's and RGBA16F's range.
const HALF_MAX: f32 = 65504.0;

/// Import settings for textures.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextureSettings {
    /// What the texture holds, which decides how it cooks.
    pub class: TextureClass,
}

/// What a texture holds. See the [module docs](self) for how each cooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureClass {
    /// Albedo and other colors, sRGB-encoded.
    Color(ColorSettings),
    /// Tangent-space normal maps.
    Normal(NormalSettings),
    /// Linear data in one to four channels: AO, specular, matcap and outline masks.
    Mask(MaskSettings),
    /// Shadow-color ramps, one row per material: uncompressed sRGB.
    Ramp(RampSettings),
    /// Packed character control maps (shadow bias, specular, material ID): linear RGBA.
    Ilm(IlmSettings),
    /// SDF face-shadow threshold maps.
    Sdf(SdfSettings),
    /// High dynamic range color: skies, emissive, environment light.
    Hdr(HdrSettings),
}

impl Default for TextureClass {
    fn default() -> Self {
        Self::Color(ColorSettings::default())
    }
}

impl TextureClass {
    /// The name used in messages.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Color(_) => "color",
            Self::Normal(_) => "normal",
            Self::Mask(_) => "mask",
            Self::Ramp(_) => "ramp",
            Self::Ilm(_) => "ILM",
            Self::Sdf(_) => "SDF",
            Self::Hdr(_) => "HDR",
        }
    }

    /// How integer samples encode RGB: colors are sRGB in PNG and TGA files, data is linear.
    pub fn transfer(&self) -> Transfer {
        match self {
            Self::Color(_) | Self::Ramp(_) | Self::Hdr(_) => Transfer::Srgb,
            Self::Normal(_) | Self::Mask(_) | Self::Ilm(_) | Self::Sdf(_) => Transfer::Linear,
        }
    }

    /// Whether the cook writes a full mip chain or only level 0.
    pub fn mips(&self) -> bool {
        match self {
            Self::Color(s) => s.mips,
            Self::Normal(s) => s.mips,
            Self::Mask(s) => s.mips,
            Self::Ramp(s) => s.mips,
            Self::Ilm(s) => s.mips,
            Self::Sdf(s) => s.mips,
            Self::Hdr(s) => s.mips,
        }
    }
}

/// Settings for [`TextureClass::Color`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorSettings {
    pub mips: bool,
    /// The mobile cook's ASTC block.
    pub astc_block: AstcBlock,
}

impl Default for ColorSettings {
    fn default() -> Self {
        Self { mips: true, astc_block: AstcBlock::B4x4 }
    }
}

/// Settings for [`TextureClass::Normal`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NormalSettings {
    pub mips: bool,
    /// Negate Y: the source uses DirectX's `-Y` convention rather than glTF's `+Y`.
    pub flip_y: bool,
    /// The mobile cook's ASTC block.
    pub astc_block: AstcBlock,
}

impl Default for NormalSettings {
    fn default() -> Self {
        Self { mips: true, flip_y: false, astc_block: AstcBlock::B4x4 }
    }
}

/// Settings for [`TextureClass::Mask`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaskSettings {
    pub mips: bool,
    /// How many of the source's logical channels to keep, which picks the format.
    pub channels: MaskChannels,
    /// The mobile cook's ASTC block.
    pub astc_block: AstcBlock,
}

impl Default for MaskSettings {
    fn default() -> Self {
        Self { mips: true, channels: MaskChannels::Auto, astc_block: AstcBlock::B4x4 }
    }
}

/// How many channels a mask keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskChannels {
    /// As many as the source has: gray 1, gray + alpha 2, RGB 3, RGBA 4.
    #[default]
    Auto,
    One,
    Two,
    Three,
    Four,
}

/// Settings for [`TextureClass::Ramp`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RampSettings {
    /// Off by default: mips would blend the rows of different materials.
    pub mips: bool,
}

/// Settings for [`TextureClass::Ilm`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IlmSettings {
    /// A material-ID channel blends across material borders in mips; maps that rely on exact
    /// IDs at a distance can turn mips off.
    pub mips: bool,
}

impl Default for IlmSettings {
    fn default() -> Self {
        Self { mips: true }
    }
}

/// Settings for [`TextureClass::Sdf`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SdfSettings {
    pub mips: bool,
    /// One threshold channel, or horizontal and vertical ones.
    pub channels: SdfChannels,
    /// The desktop cook's precision; the mobile cook is always 8-bit.
    pub precision: SdfPrecision,
}

impl Default for SdfSettings {
    fn default() -> Self {
        Self { mips: true, channels: SdfChannels::One, precision: SdfPrecision::Unorm16 }
    }
}

/// How many threshold channels an SDF map has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SdfChannels {
    #[default]
    One,
    Two,
}

/// Bits per channel of a desktop SDF map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SdfPrecision {
    Unorm8,
    #[default]
    Unorm16,
}

/// Settings for [`TextureClass::Hdr`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HdrSettings {
    pub mips: bool,
    /// The mobile cook's format.
    pub mobile_format: HdrMobileFormat,
}

impl Default for HdrSettings {
    fn default() -> Self {
        Self { mips: true, mobile_format: HdrMobileFormat::Rgb9e5 }
    }
}

/// The uncompressed HDR format of the mobile cook.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HdrMobileFormat {
    /// Shared-exponent RGB, 4 bytes per texel, no alpha.
    #[default]
    Rgb9e5,
    /// Half-float RGBA, 8 bytes per texel.
    Rgba16Float,
}

/// Errors from importing or cooking a texture.
#[derive(Debug, thiserror::Error)]
pub enum TextureError {
    #[error("{} is not a texture source (expected .png, .tga or .exr)", path.display())]
    UnsupportedSource { path: std::path::PathBuf },
    #[error("failed to import the image")]
    Import(#[from] ImageImportError),
    #[error("{class} textures with this setting need {needed} channels; the source has {found}")]
    Channels { class: &'static str, needed: usize, found: usize },
    #[error("can't cook as {:?}", .0.format)]
    Shape(TextureDesc, #[source] TextureFormatError),
    #[error("failed to encode mip level {level} as {format:?}")]
    Encode {
        level: u32,
        format: TextureFormat,
        #[source]
        source: ctt::Error,
    },
}

/// How a cook stores a class: chosen from the class, the cook and the source's channels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub format: TextureFormat,
    pub swizzle: Swizzle,
    pub filter: MipFilter,
    encoding: Encoding,
    texel: Texel,
}

/// How a source pixel becomes the value mips are computed on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Texel {
    /// Expanded RGBA as imported.
    Rgba,
    /// The first `n` logical channels, the rest as a GPU samples a format without them
    /// (RGB 0, alpha 1).
    Channels(usize),
    /// A one-channel SDF face map: R, then G 1 as its swizzle reads it
    /// (`character_maps::face_sdf::ONE_CHANNEL_SWIZZLE`), so nothing shadows by the vertical
    /// channel.
    SdfHorizontal,
    /// X and Y in `[0, 1]` → a unit vector, Z rebuilt.
    Normal { flip_y: bool },
    /// RGB clamped to what half floats hold; alpha kept unless the format has none.
    Hdr { alpha: bool },
}

const R001: Swizzle = Swizzle([Component::R, Component::Zero, Component::Zero, Component::One]);
const RA01: Swizzle = Swizzle([Component::R, Component::A, Component::Zero, Component::One]);

/// The format and handling `class` gets in `cook`, for a source with `layout`.
pub fn plan(class: &TextureClass, cook: Cook, layout: ChannelLayout) -> Result<Plan, TextureError> {
    let available = layout.channel_count();
    let need = |needed: usize| {
        if needed <= available {
            Ok(needed)
        } else {
            Err(TextureError::Channels { class: class.name(), needed, found: available })
        }
    };
    let astc = |block, srgb| TextureFormat::Astc { block, srgb };
    let make = |format, encoding, swizzle, filter, texel| Plan { format, swizzle, filter, encoding, texel };
    let identity = Swizzle::IDENTITY;
    Ok(match (*class, cook) {
        (TextureClass::Color(_), Cook::Desktop) => make(
            TextureFormat::Bc7RgbaUnormSrgb,
            Encoding::Bc7 { perceptual: true },
            identity,
            MipFilter::Color,
            Texel::Rgba,
        ),
        (TextureClass::Color(s), Cook::Mobile) => make(
            astc(s.astc_block, true),
            Encoding::Astc(AstcUsage::Color),
            identity,
            MipFilter::Color,
            Texel::Rgba,
        ),
        (TextureClass::Normal(s), _) => {
            need(2)?;
            let texel = Texel::Normal { flip_y: s.flip_y };
            match cook {
                Cook::Desktop => {
                    make(TextureFormat::Bc5RgUnorm, Encoding::Ispc, identity, MipFilter::Normal, texel)
                }
                Cook::Mobile => make(
                    astc(s.astc_block, false),
                    Encoding::Astc(AstcUsage::Normal),
                    RA01,
                    MipFilter::Normal,
                    texel,
                ),
            }
        }
        (TextureClass::Mask(s), _) => {
            let channels = need(match s.channels {
                MaskChannels::Auto => available,
                MaskChannels::One => 1,
                MaskChannels::Two => 2,
                MaskChannels::Three => 3,
                MaskChannels::Four => 4,
            })?;
            let texel = Texel::Channels(channels);
            let block = astc(s.astc_block, false);
            let (format, encoding, swizzle) = match (cook, channels) {
                (Cook::Desktop, 1) => (TextureFormat::Bc4RUnorm, Encoding::Ispc, identity),
                (Cook::Desktop, 2) => (TextureFormat::Bc5RgUnorm, Encoding::Ispc, identity),
                (Cook::Desktop, _) => {
                    (TextureFormat::Bc7RgbaUnorm, Encoding::Bc7 { perceptual: false }, identity)
                }
                (Cook::Mobile, 1) => (block, Encoding::Astc(AstcUsage::OneChannel), R001),
                (Cook::Mobile, 2) => (block, Encoding::Astc(AstcUsage::TwoChannels), RA01),
                (Cook::Mobile, _) => (block, Encoding::Astc(AstcUsage::Data), identity),
            };
            make(format, encoding, swizzle, MipFilter::Data, texel)
        }
        (TextureClass::Ramp(_), _) => make(
            TextureFormat::Rgba8UnormSrgb,
            Encoding::Uncompressed,
            identity,
            MipFilter::Data,
            Texel::Rgba,
        ),
        (TextureClass::Ilm(_), Cook::Desktop) => make(
            TextureFormat::Bc7RgbaUnorm,
            Encoding::Bc7 { perceptual: false },
            identity,
            MipFilter::Data,
            Texel::Rgba,
        ),
        (TextureClass::Ilm(_), Cook::Mobile) => make(
            astc(AstcBlock::B4x4, false),
            Encoding::Astc(AstcUsage::Data),
            identity,
            MipFilter::Data,
            Texel::Rgba,
        ),
        (TextureClass::Sdf(s), _) => {
            let channels = need(match s.channels {
                SdfChannels::One => 1,
                SdfChannels::Two => 2,
            })?;
            let wide = cook == Cook::Desktop && s.precision == SdfPrecision::Unorm16;
            let format = match (channels, wide) {
                (1, false) => TextureFormat::R8Unorm,
                (1, true) => TextureFormat::R16Unorm,
                (_, false) => TextureFormat::Rg8Unorm,
                (_, true) => TextureFormat::Rg16Unorm,
            };
            let (swizzle, texel) = match channels {
                1 => (face_sdf::ONE_CHANNEL_SWIZZLE, Texel::SdfHorizontal),
                _ => (identity, Texel::Channels(channels)),
            };
            make(format, Encoding::Uncompressed, swizzle, MipFilter::Data, texel)
        }
        (TextureClass::Hdr(_), Cook::Desktop) => make(
            TextureFormat::Bc6hRgbUfloat,
            Encoding::Ispc,
            identity,
            MipFilter::Data,
            Texel::Hdr { alpha: false },
        ),
        (TextureClass::Hdr(s), Cook::Mobile) => {
            let (format, alpha) = match s.mobile_format {
                HdrMobileFormat::Rgb9e5 => (TextureFormat::Rgb9e5Ufloat, false),
                HdrMobileFormat::Rgba16Float => (TextureFormat::Rgba16Float, true),
            };
            make(format, Encoding::Uncompressed, identity, MipFilter::Data, Texel::Hdr { alpha })
        }
    })
}

/// A texture before encoding: its shape and every level's texels, linear, in the texture's
/// logical channels. Sampling the cooked texture and applying its swizzle gives these values,
/// up to the encoder's error.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedTexture {
    pub desc: TextureDesc,
    pub levels: Vec<Level>,
    encoding: Encoding,
}

/// A cooked texture: its shape and every level's bytes in the cooked format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookedTexture {
    pub desc: TextureDesc,
    pub levels: Vec<Vec<u8>>,
}

impl CookedTexture {
    /// The texture, borrowing these levels.
    pub fn texture(&self) -> Texture<'_> {
        Texture { desc: self.desc, levels: self.levels.iter().map(Vec::as_slice).collect() }
    }

    /// The texture as a cooked KTX2 file.
    pub fn to_ktx2(&self) -> Vec<u8> {
        self.texture().to_ktx2().expect("cooked textures are validated before encoding")
    }
}

/// Plans `image` for `cook` and builds its mip chain.
pub fn prepare(
    image: &SourceImage,
    settings: &TextureSettings,
    cook: Cook,
) -> Result<PreparedTexture, TextureError> {
    let plan = plan(&settings.class, cook, image.layout)?;
    let mip_level_count = if settings.class.mips() { full_mip_count(image.width, image.height) } else { 1 };
    let desc = TextureDesc {
        format: plan.format,
        width: image.width,
        height: image.height,
        mip_level_count,
        swizzle: plan.swizzle,
    };
    // Before any work: a size the format can't hold fails here, not after encoding.
    desc.validate().map_err(|error| TextureError::Shape(desc, error))?;

    let pixels = image.pixels.iter().map(|&pixel| prepare_texel(plan.texel, image.layout, pixel)).collect();
    let level0 = Level { width: image.width, height: image.height, pixels };
    let mut levels = mips::chain(level0, plan.filter, mip_level_count);
    if let Texel::Normal { .. } = plan.texel {
        // Back from unit vectors to the stored X and Y.
        for level in &mut levels {
            for pixel in &mut level.pixels {
                *pixel = [pixel[0] * 0.5 + 0.5, pixel[1] * 0.5 + 0.5, 0.0, 1.0];
            }
        }
    }
    Ok(PreparedTexture { desc, levels, encoding: plan.encoding })
}

impl PreparedTexture {
    /// Encodes every level.
    pub fn encode(&self) -> Result<CookedTexture, TextureError> {
        let format = self.desc.format;
        let levels =
            (0..)
                .zip(&self.levels)
                .map(|(level, texels)| {
                    encode::encode_level(texels, format, self.encoding)
                        .map_err(|source| TextureError::Encode { level, format, source })
                })
                .collect::<Result<_, _>>()?;
        Ok(CookedTexture { desc: self.desc, levels })
    }
}

/// Cooks `image` for `cook`.
pub fn cook(
    image: &SourceImage,
    settings: &TextureSettings,
    cook: Cook,
) -> Result<CookedTexture, TextureError> {
    prepare(image, settings, cook)?.encode()
}

/// Imports a texture source read from `path`, as its class reads it.
pub fn import(path: &Path, bytes: &[u8], settings: &TextureSettings) -> Result<SourceImage, TextureError> {
    let format = SourceFormat::from_path(path)
        .ok_or_else(|| TextureError::UnsupportedSource { path: path.to_owned() })?;
    Ok(image_import::import_slice(bytes, format, settings.class.transfer())?)
}

/// Imports and cooks one texture source: the bytes of its cooked KTX2 file.
pub fn cook_source(
    path: &Path,
    bytes: &[u8],
    settings: &TextureSettings,
    cook: Cook,
) -> Result<Vec<u8>, TextureError> {
    let image = import(path, bytes, settings)?;
    Ok(self::cook(&image, settings, cook)?.to_ktx2())
}

fn prepare_texel(texel: Texel, layout: ChannelLayout, pixel: [f32; 4]) -> [f32; 4] {
    match texel {
        Texel::Rgba => pixel,
        Texel::Channels(n) => {
            let logical = layout.logical(pixel);
            [0, 1, 2, 3].map(|c| {
                if c < n {
                    logical[c]
                } else if c == 3 {
                    1.0
                } else {
                    0.0
                }
            })
        }
        Texel::SdfHorizontal => {
            let [r, ..] = layout.logical(pixel);
            face_sdf::ONE_CHANNEL_SWIZZLE.apply([r, 0.0, 0.0, 1.0])
        }
        Texel::Normal { flip_y } => {
            let [x, y, ..] = layout.logical(pixel);
            let (x, y) = (x * 2.0 - 1.0, y * 2.0 - 1.0);
            let y = if flip_y { -y } else { y };
            let z = (1.0 - x * x - y * y).max(0.0).sqrt();
            let [x, y, z] = mips::normalize([x, y, z]);
            [x, y, z, 0.0]
        }
        Texel::Hdr { alpha } => {
            let [r, g, b, a] = pixel.map(|v| v.clamp(0.0, HALF_MAX));
            [r, g, b, if alpha { a.min(1.0) } else { 1.0 }]
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;

    use super::*;
    use crate::color::srgb_to_linear;
    use crate::cook::{CookError, CookOptions, CookReport, CookStatus, cook_dir};
    use crate::meta::{self, AssetMeta, ImportSettings, sidecar_path};
    use crate::test_util::TempDir;

    fn image(
        width: u32,
        height: u32,
        layout: ChannelLayout,
        pixel: impl Fn(u32, u32) -> [f32; 4],
    ) -> SourceImage {
        let pixels =
            (0..height).flat_map(|y| (0..width).map(move |x| (x, y))).map(|(x, y)| pixel(x, y)).collect();
        SourceImage::new(width, height, layout, pixels).unwrap()
    }

    /// A smooth pattern with every channel varying.
    fn gradient(width: u32, height: u32) -> SourceImage {
        image(width, height, ChannelLayout::Rgba, |x, y| {
            let (u, v) = (x as f32 / width as f32, y as f32 / height as f32);
            [srgb_to_linear(u), srgb_to_linear(v), srgb_to_linear(0.5 + 0.4 * (u * 6.0).sin()), 1.0 - 0.5 * v]
        })
    }

    fn settings(class: TextureClass) -> TextureSettings {
        TextureSettings { class }
    }

    fn format(class: TextureClass, cook: Cook, layout: ChannelLayout) -> (TextureFormat, Swizzle) {
        let plan = plan(&class, cook, layout).unwrap();
        (plan.format, plan.swizzle)
    }

    #[test]
    fn each_class_picks_its_format_per_cook() {
        use ChannelLayout::*;
        use Cook::*;
        use TextureFormat as F;
        let id = Swizzle::IDENTITY;
        let astc = |block, srgb| F::Astc { block, srgb };
        let color = TextureClass::Color(ColorSettings::default());
        assert_eq!(format(color, Desktop, Rgba), (F::Bc7RgbaUnormSrgb, id));
        assert_eq!(format(color, Mobile, Rgb), (astc(AstcBlock::B4x4, true), id));
        let coarse =
            TextureClass::Color(ColorSettings { astc_block: AstcBlock::B8x8, ..ColorSettings::default() });
        assert_eq!(format(coarse, Mobile, Rgba), (astc(AstcBlock::B8x8, true), id));

        let normal = TextureClass::Normal(NormalSettings::default());
        assert_eq!(format(normal, Desktop, Rgb), (F::Bc5RgUnorm, id));
        assert_eq!(format(normal, Mobile, Rgb), (astc(AstcBlock::B4x4, false), RA01));

        let mask = |channels| TextureClass::Mask(MaskSettings { channels, ..MaskSettings::default() });
        assert_eq!(format(mask(MaskChannels::Auto), Desktop, Gray), (F::Bc4RUnorm, id));
        assert_eq!(format(mask(MaskChannels::Auto), Desktop, GrayAlpha), (F::Bc5RgUnorm, id));
        assert_eq!(format(mask(MaskChannels::Auto), Desktop, Rgb), (F::Bc7RgbaUnorm, id));
        assert_eq!(format(mask(MaskChannels::Two), Desktop, Rgba), (F::Bc5RgUnorm, id));
        assert_eq!(format(mask(MaskChannels::One), Mobile, Rgb), (astc(AstcBlock::B4x4, false), R001));
        assert_eq!(format(mask(MaskChannels::Two), Mobile, Rgb), (astc(AstcBlock::B4x4, false), RA01));
        assert_eq!(format(mask(MaskChannels::Auto), Mobile, Rgba), (astc(AstcBlock::B4x4, false), id));

        let ramp = TextureClass::Ramp(RampSettings::default());
        assert_eq!(format(ramp, Desktop, Rgba), (F::Rgba8UnormSrgb, id));
        assert_eq!(format(ramp, Mobile, Rgb), (F::Rgba8UnormSrgb, id));

        let ilm = TextureClass::Ilm(IlmSettings::default());
        assert_eq!(format(ilm, Desktop, Rgba), (F::Bc7RgbaUnorm, id));
        assert_eq!(format(ilm, Mobile, Rgba), (astc(AstcBlock::B4x4, false), id));

        let sdf = |channels, precision| TextureClass::Sdf(SdfSettings { channels, precision, mips: true });
        let r101 = face_sdf::ONE_CHANNEL_SWIZZLE;
        assert_eq!(format(sdf(SdfChannels::One, SdfPrecision::Unorm16), Desktop, Gray), (F::R16Unorm, r101));
        assert_eq!(format(sdf(SdfChannels::One, SdfPrecision::Unorm8), Desktop, Gray), (F::R8Unorm, r101));
        assert_eq!(format(sdf(SdfChannels::Two, SdfPrecision::Unorm16), Desktop, Rgb), (F::Rg16Unorm, id));
        assert_eq!(format(sdf(SdfChannels::One, SdfPrecision::Unorm16), Mobile, Gray), (F::R8Unorm, r101));
        assert_eq!(
            format(sdf(SdfChannels::Two, SdfPrecision::Unorm16), Mobile, GrayAlpha),
            (F::Rg8Unorm, id)
        );

        let hdr = |mobile_format| TextureClass::Hdr(HdrSettings { mobile_format, mips: true });
        assert_eq!(format(hdr(HdrMobileFormat::Rgb9e5), Desktop, Rgb), (F::Bc6hRgbUfloat, id));
        assert_eq!(format(hdr(HdrMobileFormat::Rgb9e5), Mobile, Rgb), (F::Rgb9e5Ufloat, id));
        assert_eq!(format(hdr(HdrMobileFormat::Rgba16Float), Mobile, Rgba), (F::Rgba16Float, id));

        // Classes that need more channels than the source has say so.
        assert!(matches!(
            plan(&normal, Desktop, Gray),
            Err(TextureError::Channels { class: "normal", needed: 2, found: 1 })
        ));
        assert!(matches!(
            plan(&mask(MaskChannels::Three), Mobile, GrayAlpha),
            Err(TextureError::Channels { needed: 3, found: 2, .. })
        ));
        assert!(matches!(
            plan(&sdf(SdfChannels::Two, SdfPrecision::Unorm8), Desktop, Gray),
            Err(TextureError::Channels { class: "SDF", .. })
        ));
    }

    #[test]
    fn classes_read_sources_and_mip_as_described() {
        use TextureClass as C;
        let defaults = [
            C::Color(ColorSettings::default()),
            C::Normal(NormalSettings::default()),
            C::Mask(MaskSettings::default()),
            C::Ramp(RampSettings::default()),
            C::Ilm(IlmSettings::default()),
            C::Sdf(SdfSettings::default()),
            C::Hdr(HdrSettings::default()),
        ];
        let srgb: Vec<bool> = defaults.iter().map(|class| class.transfer() == Transfer::Srgb).collect();
        assert_eq!(srgb, [true, false, false, true, false, false, true]);
        let mips: Vec<bool> = defaults.iter().map(TextureClass::mips).collect();
        assert_eq!(mips, [true, true, true, false, true, true, true], "ramps have no mips unless asked");
        assert!(C::Ramp(RampSettings { mips: true }).mips());
    }

    #[test]
    fn prepared_levels_hold_logical_channels() {
        // A gray + alpha mask keeps (l, a) in R and G, with B 0 and A 1 as BC5 samples.
        let mask = image(8, 8, ChannelLayout::GrayAlpha, |_, _| [0.25, 0.25, 0.25, 0.75]);
        let prepared =
            prepare(&mask, &settings(TextureClass::Mask(MaskSettings::default())), Cook::Desktop).unwrap();
        assert_eq!(prepared.desc.mip_level_count, 4);
        assert!(
            prepared.levels.iter().all(|level| level.pixels.iter().all(|&p| p == [0.25, 0.75, 0.0, 1.0]))
        );

        // Normals: X and Y back in [0, 1] at every level, unit length with Z rebuilt.
        let tilted = image(4, 4, ChannelLayout::Rgb, |x, _| {
            if x % 2 == 0 { [0.8, 0.5, 1.0, 1.0] } else { [0.2, 0.5, 1.0, 1.0] }
        });
        let class = TextureClass::Normal(NormalSettings::default());
        let prepared = prepare(&tilted, &settings(class), Cook::Mobile).unwrap();
        let close = |a: [f32; 4], b: [f32; 4]| a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 1e-6);
        assert!(
            close(prepared.levels[0].pixels[0], [0.8, 0.5, 0.0, 1.0]),
            "{:?}",
            prepared.levels[0].pixels[0]
        );
        // Opposite tilts average to straight up.
        assert!(
            prepared.levels[1].pixels.iter().all(|p| (p[0] - 0.5).abs() < 1e-6 && (p[1] - 0.5).abs() < 1e-6)
        );
        let flipped = TextureClass::Normal(NormalSettings { flip_y: true, ..NormalSettings::default() });
        let up = image(4, 4, ChannelLayout::Rgb, |_, _| [0.5, 0.9, 1.0, 1.0]);
        let prepared = prepare(&up, &settings(flipped), Cook::Desktop).unwrap();
        assert!((prepared.levels[0].pixels[0][1] - 0.1).abs() < 1e-6);
        // X² + Y² > 1 (a badly quantized texel) renormalizes onto the unit circle.
        let over = image(4, 4, ChannelLayout::Rgb, |_, _| [1.0, 1.0, 0.5, 1.0]);
        let p = prepare(&over, &settings(class), Cook::Desktop).unwrap().levels[0].pixels[0];
        let (x, y) = (p[0] * 2.0 - 1.0, p[1] * 2.0 - 1.0);
        assert!(((x * x + y * y).sqrt() - 1.0).abs() < 1e-6);

        // HDR clamps to what half floats hold, and drops alpha where the format has none.
        let hdr = image(4, 4, ChannelLayout::Rgba, |_, _| [-1.0, 70000.0, 2.5, 0.5]);
        let class = TextureClass::Hdr(HdrSettings::default());
        assert_eq!(
            prepare(&hdr, &settings(class), Cook::Mobile).unwrap().levels[0].pixels[0],
            [0.0, HALF_MAX, 2.5, 1.0]
        );
        let rgba =
            TextureClass::Hdr(HdrSettings { mobile_format: HdrMobileFormat::Rgba16Float, mips: false });
        let prepared = prepare(&hdr, &settings(rgba), Cook::Mobile).unwrap();
        assert_eq!((prepared.desc.mip_level_count, prepared.levels[0].pixels[0][3]), (1, 0.5));
    }

    #[test]
    fn block_formats_need_whole_blocks() {
        let odd = gradient(30, 32);
        let color = settings(TextureClass::Color(ColorSettings::default()));
        let error = prepare(&odd, &color, Cook::Desktop).unwrap_err();
        assert!(
            matches!(
                error,
                TextureError::Shape(_, TextureFormatError::NotWholeBlocks { width: 30, height: 32, .. })
            ),
            "{error:?}"
        );
        // Uncompressed classes take any size, down to 1×1 levels.
        let ramp = settings(TextureClass::Ramp(RampSettings { mips: true }));
        let cooked = cook(&gradient(7, 3), &ramp, Cook::Mobile).unwrap();
        assert_eq!(cooked.desc.mip_level_count, 3);
        assert_eq!(cooked.levels.iter().map(Vec::len).collect::<Vec<_>>(), [7 * 3 * 4, 3 * 4, 4]);
        // A 6×6 ASTC block needs sizes in multiples of 6.
        let six = settings(TextureClass::Color(ColorSettings { astc_block: AstcBlock::B6x6, mips: true }));
        assert!(prepare(&gradient(32, 32), &six, Cook::Mobile).is_err());
        assert!(prepare(&gradient(36, 24), &six, Cook::Mobile).is_ok());
    }

    #[test]
    fn every_class_cooks_to_a_valid_ktx2_on_both_cooks() {
        use TextureClass as C;
        let classes = [
            C::Color(ColorSettings::default()),
            C::Normal(NormalSettings::default()),
            C::Mask(MaskSettings::default()),
            C::Mask(MaskSettings { channels: MaskChannels::One, ..MaskSettings::default() }),
            C::Mask(MaskSettings { channels: MaskChannels::Two, ..MaskSettings::default() }),
            C::Ramp(RampSettings::default()),
            C::Ilm(IlmSettings::default()),
            C::Sdf(SdfSettings::default()),
            C::Sdf(SdfSettings { channels: SdfChannels::Two, ..SdfSettings::default() }),
            C::Hdr(HdrSettings::default()),
            C::Hdr(HdrSettings { mobile_format: HdrMobileFormat::Rgba16Float, mips: true }),
        ];
        let source = gradient(24, 16);
        for class in classes {
            for cook in Cook::ALL {
                let cooked = self::cook(&source, &settings(class), cook).unwrap();
                let bytes = cooked.to_ktx2();
                let texture = Texture::from_ktx2(&bytes).unwrap();
                assert_eq!(texture, cooked.texture(), "{class:?} {cook}: the KTX2 round-trips");
                assert_eq!(texture.desc.mip_level_count, if class.mips() { 5 } else { 1 });
                // Deterministic: a second cook writes the same bytes.
                assert_eq!(
                    self::cook(&source, &settings(class), cook).unwrap().to_ktx2(),
                    bytes,
                    "{class:?} {cook}"
                );
            }
        }
    }

    #[test]
    fn uncompressed_levels_store_their_prepared_values() {
        // RGBA8 sRGB of an 8-bit sRGB source is the source's bytes, exactly.
        let bytes: Vec<[u8; 4]> = (0..12).map(|i| [i * 20, 255 - i * 20, i * 7, 255 - i]).collect();
        let source = image(4, 3, ChannelLayout::Rgba, |x, y| {
            let [r, g, b, a] = bytes[(y * 4 + x) as usize];
            let c = |v: u8| srgb_to_linear(f32::from(v) / 255.0);
            [c(r), c(g), c(b), f32::from(a) / 255.0]
        });
        let cooked =
            cook(&source, &settings(TextureClass::Ramp(RampSettings::default())), Cook::Desktop).unwrap();
        assert_eq!(cooked.levels[0], bytes.concat());

        // 16-bit SDF levels hold the values to 16 bits.
        let sdf = image(4, 4, ChannelLayout::Gray, |x, y| [(x * 4 + y) as f32 / 15.0, 0.0, 0.0, 1.0]);
        let cooked = cook(&sdf, &settings(TextureClass::Sdf(SdfSettings::default())), Cook::Desktop).unwrap();
        let level0: Vec<u16> =
            cooked.levels[0].as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
        assert_eq!(level0[1], (4.0 / 15.0 * 65535.0f32).round() as u16);
        assert_eq!(level0[15], 65535);
    }

    #[test]
    fn the_cooker_names_the_locked_encoder_and_container_versions() {
        // (name, version) of every locked package.
        let packages: Vec<(&str, &str)> = include_str!("../../../Cargo.lock")
            .split("[[package]]")
            .filter_map(|package| {
                let field = |key: &str| {
                    let start = package.find(&format!("\n{key} = \""))? + key.len() + 5;
                    package[start..].split('"').next()
                };
                Some((field("name")?, field("version")?))
            })
            .collect();
        let versions = |matches: fn(&str) -> bool| -> Vec<&str> {
            packages.iter().filter(|(name, _)| matches(name)).map(|&(_, version)| version).collect()
        };
        let (ctt, ktx2) = (versions(|name| name.starts_with("ctt")), versions(|name| name == "ktx2"));
        assert!(ctt.len() >= 4 && ktx2.len() == 1, "ctt {ctt:?}, ktx2 {ktx2:?}");
        assert!(ctt.iter().all(|&version| version == ctt[0]), "ctt crates at different versions: {ctt:?}");
        assert_eq!(
            COOKER.name,
            format!("ktx2 (ktx2 {}, ctt {})", ktx2[0], ctt[0]),
            "an encoder or ktx2 upgrade changes cooked bytes: rename the cooker with it"
        );
    }

    /// Writes `image` to `path` in `format`, creating directories.
    fn write_image(path: &Path, image: ::image::DynamicImage, format: ::image::ImageFormat) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        fs::write(path, bytes.into_inner()).unwrap();
    }

    /// A PNG, a TGA and an EXR source under `content/`, cooked through the content cache.
    struct Tree {
        dir: TempDir,
    }

    impl Tree {
        fn new(name: &str) -> Self {
            let tree = Self { dir: TempDir::new(name) };
            let rgba = ::image::RgbaImage::from_fn(32, 16, |x, y| {
                ::image::Rgba([(x * 8) as u8, (y * 16) as u8, 128, 255])
            });
            let png = tree.source().join("characters/albedo.png");
            write_image(&png, rgba.clone().into(), ::image::ImageFormat::Png);
            write_image(&tree.source().join("props/crate.tga"), rgba.into(), ::image::ImageFormat::Tga);
            let sky =
                ::image::Rgb32FImage::from_fn(16, 8, |x, y| ::image::Rgb([x as f32 * 0.5, y as f32, 4.0]));
            write_image(&tree.source().join("sky.exr"), sky.into(), ::image::ImageFormat::OpenExr);
            tree
        }

        fn source(&self) -> std::path::PathBuf {
            self.dir.path().join("content")
        }

        fn out(&self, cook: Cook) -> std::path::PathBuf {
            self.dir.path().join("out").join(cook.name())
        }

        fn cook(&self, cook: Cook, cache: bool) -> CookReport {
            self.try_cook(cook, cache).unwrap()
        }

        fn try_cook(&self, cook: Cook, cache: bool) -> Result<CookReport, CookError> {
            let cache_dir = self.dir.path().join("cache");
            let options = CookOptions { cook, cache: cache.then_some(cache_dir.as_path()), locked: false };
            cook_dir(&self.source(), &self.out(cook), &options)
        }

        fn set_class(&self, relative: &str, class: TextureClass) {
            let sidecar = sidecar_path(&self.source().join(relative));
            let mut meta = meta::read(&sidecar).unwrap();
            meta.importer = ImportSettings::Texture(TextureSettings { class });
            fs::write(&sidecar, sapphire_serialize::to_ron::<AssetMeta>(&meta).unwrap()).unwrap();
        }
    }

    fn statuses(report: &CookReport) -> Vec<CookStatus> {
        report.assets.iter().map(|asset| asset.status).collect()
    }

    #[test]
    fn content_trees_cook_textures_into_ktx2() {
        let tree = Tree::new("texture_tree");
        let srgb_astc = TextureFormat::Astc { block: AstcBlock::B4x4, srgb: true };
        for (cook, formats) in [
            (
                Cook::Desktop,
                [
                    TextureFormat::Bc7RgbaUnormSrgb,
                    TextureFormat::Bc7RgbaUnormSrgb,
                    TextureFormat::Bc6hRgbUfloat,
                ],
            ),
            (Cook::Mobile, [srgb_astc, srgb_astc, TextureFormat::Rgb9e5Ufloat]),
        ] {
            let report = tree.cook(cook, true);
            let outputs: Vec<_> = report.assets.iter().map(|asset| asset.output.clone()).collect();
            let out = tree.out(cook);
            assert_eq!(
                outputs,
                [out.join("characters/albedo.ktx2"), out.join("props/crate.ktx2"), out.join("sky.ktx2")]
            );
            for (output, format) in outputs.iter().zip(formats) {
                let bytes = fs::read(output).unwrap();
                let texture = Texture::from_ktx2(&bytes).unwrap();
                assert_eq!(texture.desc.format, format, "{}", output.display());
            }
        }
        // The first cook made a sidecar per source, with its extension's default class.
        let importer =
            |relative: &str| meta::read(&sidecar_path(&tree.source().join(relative))).unwrap().importer;
        assert_eq!(importer("characters/albedo.png"), ImportSettings::Texture(TextureSettings::default()));
        assert!(matches!(
            importer("sky.exr"),
            ImportSettings::Texture(TextureSettings { class: TextureClass::Hdr(_) })
        ));
        // The PNG and the TGA hold the same pixels, so they cook to the same bytes.
        let out = tree.out(Cook::Desktop);
        assert_eq!(
            fs::read(out.join("characters/albedo.ktx2")).unwrap(),
            fs::read(out.join("props/crate.ktx2")).unwrap()
        );
    }

    #[test]
    fn texture_cooks_are_deterministic_and_keyed_by_their_settings() {
        let tree = Tree::new("texture_keys");
        let first = tree.cook(Cook::Desktop, false);
        let bytes: Vec<Vec<u8>> = first.assets.iter().map(|asset| fs::read(&asset.output).unwrap()).collect();
        // Cooking again without a cache produces the same bytes.
        let second = tree.cook(Cook::Desktop, false);
        assert_eq!(statuses(&second), [CookStatus::Cooked; 3]);
        let again: Vec<Vec<u8>> =
            second.assets.iter().map(|asset| fs::read(&asset.output).unwrap()).collect();
        assert_eq!(again, bytes);

        let cached = tree.cook(Cook::Desktop, true);
        assert_eq!(statuses(&tree.cook(Cook::Desktop, true)), [CookStatus::UpToDate; 3]);
        // A new class is a new key: only that texture recooks, to its new format.
        tree.set_class("characters/albedo.png", TextureClass::Mask(MaskSettings::default()));
        let report = tree.cook(Cook::Desktop, true);
        assert_eq!(statuses(&report), [CookStatus::Cooked, CookStatus::UpToDate, CookStatus::UpToDate]);
        assert_ne!(report.assets[0].key, cached.assets[0].key);
        let mask = Texture::from_ktx2(&fs::read(&report.assets[0].output).unwrap()).unwrap().desc;
        assert_eq!(mask.format, TextureFormat::Bc7RgbaUnorm);
        // So is any setting within a class, even one only the other cook uses.
        let coarse =
            TextureClass::Mask(MaskSettings { astc_block: AstcBlock::B8x8, ..MaskSettings::default() });
        tree.set_class("characters/albedo.png", coarse);
        assert_eq!(statuses(&tree.cook(Cook::Desktop, true))[0], CookStatus::Cooked);
        // Going back to the old settings finds the old key's output in the cache.
        tree.set_class("characters/albedo.png", TextureClass::Color(ColorSettings::default()));
        let report = tree.cook(Cook::Desktop, true);
        assert_eq!(
            (report.assets[0].key, report.assets[0].status),
            (cached.assets[0].key, CookStatus::Cached)
        );
        assert_eq!(fs::read(&report.assets[0].output).unwrap(), bytes[0]);
    }

    #[test]
    fn bad_texture_sources_fail_with_their_path() {
        let tree = Tree::new("texture_errors");
        // A normal map needs two channels; a gray source has one.
        let path = tree.source().join("gray.png");
        write_image(&path, ::image::GrayImage::new(8, 8).into(), ::image::ImageFormat::Png);
        tree.cook(Cook::Desktop, false);
        tree.set_class("gray.png", TextureClass::Normal(NormalSettings::default()));
        let texture_error = |error: CookError| match error {
            CookError::Texture { path: p, source } if p == path => *source,
            other => panic!("expected a texture error for {}, got {other:?}", path.display()),
        };
        let error = texture_error(tree.try_cook(Cook::Desktop, false).unwrap_err());
        assert!(
            matches!(error, TextureError::Channels { class: "normal", needed: 2, found: 1 }),
            "{error:?}"
        );
        // A file that isn't the image its extension says.
        fs::write(&path, b"not a png").unwrap();
        let error = texture_error(tree.try_cook(Cook::Desktop, false).unwrap_err());
        assert!(matches!(error, TextureError::Import(ImageImportError::Decode(_))), "{error:?}");
    }
}
