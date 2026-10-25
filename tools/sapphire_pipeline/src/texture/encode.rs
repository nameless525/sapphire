//! One mip level through ctt: linear `f32` texels in, the cooked format's bytes out.
//!
//! Each level is its own single-surface `ctt::convert` call, so ctt never builds or touches a
//! mip chain (its BC7 chains are broken, issue #114) and converts only what the cook asks:
//! quantizing, sRGB encoding and float packing for uncompressed formats, and the encoder's
//! input conversion before block compression. Encoders are named explicitly instead of
//! ctt's automatic pick, so a ctt release that reorders its encoders can't change our output
//! behind the cooker's version.
//!
//! **Alpha is passed through untouched.** ctt's float path premultiplies straight alpha on
//! load and divides it back out on store, which loses color where alpha is small and wrecks
//! data packed into alpha (ILM maps). The levels arrive already filtered, so the cook tells
//! ctt they are premultiplied on both sides, which makes it do neither.

use ctt::encoders::Encoder;
use ctt::encoders::astcenc::{AstcencSettings, AstcencUsage, NormalSwizzle};
use ctt::encoders::bc7enc::Bc7encSettings;
use ctt::{
    AlphaMode, ColorSpace, Container, ConvertSettings, FormatDesc, ImageRef, SurfaceRef, TargetFormat,
    TextureKind,
};
use sapphire_assets::texture::TextureFormat;

use super::mips::Level;

/// ctt's quality preset for every encoder: its balanced default. Changing it changes every
/// compressed texture, so it is part of the cooker's version.
const QUALITY: ctt::Quality = ctt::Quality::Basic;

/// How a level becomes its format's bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Encoding {
    /// Converted, not compressed.
    Uncompressed,
    /// bc7enc-rdo. Perceptual weighting suits colors and is wrong for data.
    Bc7 { perceptual: bool },
    /// Intel's ISPC kernels: BC4, BC5 and BC6H.
    Ispc,
    /// astcenc, with what the data represents.
    Astc(AstcUsage),
}

/// What an ASTC texture holds, which picks astcenc's profile, error metric and channel layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AstcUsage {
    /// Colors; perceptual error.
    Color,
    /// RGBA data, compared channel by channel.
    Data,
    /// One channel, in R.
    OneChannel,
    /// Two channels in R and G, stored as luminance plus alpha.
    TwoChannels,
    /// A normal's X and Y in R and G, stored as luminance plus alpha with angular error.
    Normal,
}

/// Encodes one level. `level`'s texels are linear and carry the format's channels first.
pub(super) fn encode_level(
    level: &Level,
    format: TextureFormat,
    encoding: Encoding,
) -> Result<Vec<u8>, ctt::Error> {
    let target = ctt::Format::new(format.vk_format()).expect("TextureFormat VkFormats are ktx2 formats");
    let target = match encoding {
        Encoding::Uncompressed => TargetFormat::Uncompressed(target),
        Encoding::Bc7 { perceptual } => TargetFormat::Compressed {
            format: target,
            encoder: Encoder::Bc7enc(Bc7encSettings { perceptual, ..Bc7encSettings::default() }),
        },
        Encoding::Ispc => {
            TargetFormat::Compressed { format: target, encoder: Encoder::Intel(Default::default()) }
        }
        Encoding::Astc(usage) => {
            let (usage, perceptual) = match usage {
                AstcUsage::Color => (AstcencUsage::Color, true),
                AstcUsage::Data => (AstcencUsage::Color, false),
                AstcUsage::OneChannel => (AstcencUsage::SingleChannel, false),
                AstcUsage::TwoChannels => (AstcencUsage::TwoChannel, false),
                AstcUsage::Normal => (AstcencUsage::NormalMap { swizzle: NormalSwizzle::AstcDefault }, false),
            };
            let settings = AstcencSettings { usage, perceptual, ..AstcencSettings::default() };
            TargetFormat::Compressed { format: target, encoder: Encoder::Astcenc(settings) }
        }
    };
    let texels: &[u8] = bytemuck::cast_slice(&level.pixels);
    let surface = SurfaceRef {
        data: texels,
        width: level.width,
        height: level.height,
        depth: 1,
        stride: level.width * 16,
        slice_stride: 0,
    };
    let image = ImageRef {
        surfaces: vec![vec![surface]],
        kind: TextureKind::Texture2D,
        desc: FormatDesc {
            format: ctt::Format::R32G32B32A32_SFLOAT,
            color_space: ColorSpace::Linear,
            alpha: AlphaMode::Premultiplied,
        },
    };
    let settings = ConvertSettings {
        format: Some(target),
        container: Container::Raw,
        quality: QUALITY,
        output_color_space: Some(if format.is_srgb() { ColorSpace::Srgb } else { ColorSpace::Linear }),
        output_alpha: Some(AlphaMode::Premultiplied),
        ..ConvertSettings::default()
    };
    let ctt::PipelineOutput::Raw(mut image) = ctt::convert(image, settings)? else {
        unreachable!("Container::Raw returns the image");
    };
    let surface = image.surfaces.swap_remove(0).swap_remove(0);
    let expected = format.image_byte_len(level.width, level.height);
    if surface.data.len() as u64 != expected {
        return Err(ctt::Error::DataLengthMismatch {
            expected: expected as usize,
            actual: surface.data.len(),
        });
    }
    Ok(surface.data)
}
