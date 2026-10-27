//! Image importer for PNG, TGA and EXR (M1 A2): source file → [`SourceImage`], the
//! engine-neutral image the texture cook reads.
//!
//! Decoding is the `image` crate's, built with exactly these three formats. Every source
//! becomes linear `f32` RGBA, so mip generation and the encoders see one representation:
//!
//! - **Integer samples** (8- and 16-bit PNG, TGA) are scaled to `[0, 1]`. What they encode is
//!   the texture class's business ([`Transfer`]): colors are sRGB-encoded in these files, so
//!   color classes decode RGB to linear light; data maps (normals, masks, ILM, SDF) are
//!   stored linearly and are taken as they are. Alpha is always linear.
//! - **Float samples** (EXR, half or full float) are linear light by definition and are never
//!   decoded. Non-finite samples are rejected: no cooked format can hold them.
//!
//! Gray sources keep their [`ChannelLayout`], so a class can tell a one-channel mask from an
//! RGB one; their pixels are still expanded to RGBA (`(l, l, l, a)`), as an image viewer shows
//! them. Rows are top to bottom (TGA's bottom-up origin is flipped by the decoder), which is
//! the order textures are uploaded in.
//!
//! A source's size is checked against [`SOURCE_BUDGET`] from its header, before any pixel is
//! decoded, so an oversized or hostile file fails at once, naming its size.
//!
//! A source file is self-contained, so this importer lists no dependencies for the cache key.

use std::fmt;
use std::io::Cursor;
use std::path::Path;

use image::ImageDecoder;
use sapphire_assets::texture::MAX_TEXTURE_DIMENSION;

use crate::cache::Stage;
use crate::color::srgb_to_linear;

/// This importer, as cache keys know it. Bump the version whenever import output changes
/// for the same file.
pub const IMPORTER: Stage = Stage { name: "image", version: 1 };

/// The largest image the pipeline decodes: a size on each side and a count of texels in all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexelBudget {
    /// The most texels on a side.
    pub side: u32,
    /// The most texels in all.
    pub texels: u64,
}

/// What every source image the pipeline decodes must fit: 16384 texels on a side, the most a
/// cooked texture may have ([`MAX_TEXTURE_DIMENSION`]), and 64 Mi texels in all (8192 × 8192, or
/// 16384 × 4096). Every source becomes linear `f32` RGBA, 16 bytes a texel, and a texture's cook
/// holds more than one such copy (its prepared level 0 and the mip chain), so the texel count is
/// what bounds a cook's memory: 1 GiB for one expanded copy at the limit. The largest committed
/// sources (2048 × 2048, 4096 × 576) use at most a sixteenth of it.
pub const SOURCE_BUDGET: TexelBudget = TexelBudget { side: MAX_TEXTURE_DIMENSION, texels: 64 << 20 };

impl TexelBudget {
    /// Whether a `width × height` image fits.
    pub fn fits(self, width: u32, height: u32) -> bool {
        width <= self.side && height <= self.side && u64::from(width) * u64::from(height) <= self.texels
    }
}

impl fmt::Display for TexelBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let texels = if self.texels.is_multiple_of(1 << 20) {
            format!("{} Mi", self.texels >> 20)
        } else {
            self.texels.to_string()
        };
        write!(f, "{} texels on a side and {texels} texels in all", self.side)
    }
}

/// The source formats this importer reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceFormat {
    Png,
    Tga,
    Exr,
}

impl SourceFormat {
    /// The format of `path`, by its extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "png" => Some(Self::Png),
            "tga" => Some(Self::Tga),
            "exr" => Some(Self::Exr),
            _ => None,
        }
    }

    fn image_format(self) -> image::ImageFormat {
        match self {
            Self::Png => image::ImageFormat::Png,
            Self::Tga => image::ImageFormat::Tga,
            Self::Exr => image::ImageFormat::OpenExr,
        }
    }
}

/// How a source's integer samples encode its RGB channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    /// sRGB-encoded colors, decoded to linear light on import.
    Srgb,
    /// Linear values (data, or colors that already are linear), taken as they are.
    Linear,
}

/// The channels a source stores. A class that packs data takes its *logical* channels in
/// this order: gray `[l]`, gray + alpha `[l, a]`, RGB `[r, g, b]`, RGBA `[r, g, b, a]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelLayout {
    Gray,
    GrayAlpha,
    Rgb,
    Rgba,
}

impl ChannelLayout {
    /// How many logical channels the source has.
    pub fn channel_count(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::GrayAlpha => 2,
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }

    /// The logical channels of an expanded RGBA pixel, first [`Self::channel_count`] used.
    pub fn logical(self, [r, g, b, a]: [f32; 4]) -> [f32; 4] {
        match self {
            Self::Gray => [r, 0.0, 0.0, 0.0],
            Self::GrayAlpha => [r, a, 0.0, 0.0],
            Self::Rgb => [r, g, b, 0.0],
            Self::Rgba => [r, g, b, a],
        }
    }
}

/// How a source stored its samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleDepth {
    U8,
    U16,
    /// Half or full float (EXR); both decode to `f32`.
    Float,
}

/// A decoded source image: linear `f32` RGBA, row-major, top row first.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceImage {
    pub width: u32,
    pub height: u32,
    pub layout: ChannelLayout,
    pub depth: SampleDepth,
    /// `width × height` pixels, expanded to RGBA as the [module docs](self) describe.
    pub pixels: Vec<[f32; 4]>,
}

impl SourceImage {
    /// An image from linear RGBA pixels, as a generator makes it. `None` if the pixel count
    /// doesn't match the size or the size is empty.
    pub fn new(width: u32, height: u32, layout: ChannelLayout, pixels: Vec<[f32; 4]>) -> Option<Self> {
        let expected = u64::from(width) * u64::from(height);
        (expected > 0 && pixels.len() as u64 == expected).then_some(Self {
            width,
            height,
            layout,
            depth: SampleDepth::Float,
            pixels,
        })
    }
}

/// Errors from importing an image.
#[derive(Debug, thiserror::Error)]
pub enum ImageImportError {
    #[error("failed to decode the image")]
    Decode(#[source] image::ImageError),
    #[error("{0:?} images are not supported; save as 8- or 16-bit gray, RGB or RGBA, or float RGB(A)")]
    UnsupportedLayout(image::ColorType),
    #[error("the image is empty")]
    Empty,
    #[error(
        "the image is {width} × {height} texels, over the pipeline's budget of {budget}; shrink or split it"
    )]
    TooLarge { width: u32, height: u32, budget: TexelBudget },
    #[error("pixel ({x}, {y}) has a non-finite value, which no texture format can hold")]
    NonFinite { x: u32, y: u32 },
}

/// Decodes a source image read as `format`, decoding integer RGB through `transfer`.
pub fn import_slice(
    bytes: &[u8],
    format: SourceFormat,
    transfer: Transfer,
) -> Result<SourceImage, ImageImportError> {
    import_within(bytes, format, transfer, SOURCE_BUDGET)
}

/// Decodes `bytes` as `format`, as [`image::load_from_memory_with_format`] does, once the
/// header shows the image within `budget`: an image over it fails before any pixel is decoded.
pub fn decode(
    bytes: &[u8],
    format: image::ImageFormat,
    budget: TexelBudget,
) -> Result<image::DynamicImage, ImageImportError> {
    let mut decoder = image::ImageReader::with_format(Cursor::new(bytes), format)
        .into_decoder()
        .map_err(ImageImportError::Decode)?;
    let (width, height) = decoder.dimensions();
    if !budget.fits(width, height) {
        return Err(ImageImportError::TooLarge { width, height, budget });
    }
    // What `ImageReader::decode` does after reading the header: the decoded image counts
    // against the decoder's allocation limit.
    let mut limits = image::Limits::default();
    limits.reserve(decoder.total_bytes()).map_err(ImageImportError::Decode)?;
    decoder.set_limits(limits).map_err(ImageImportError::Decode)?;
    image::DynamicImage::from_decoder(decoder).map_err(ImageImportError::Decode)
}

/// [`import_slice`] within `budget`.
fn import_within(
    bytes: &[u8],
    format: SourceFormat,
    transfer: Transfer,
    budget: TexelBudget,
) -> Result<SourceImage, ImageImportError> {
    let decoded = decode(bytes, format.image_format(), budget)?;
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 {
        return Err(ImageImportError::Empty);
    }
    let (layout, depth) = match decoded.color() {
        image::ColorType::L8 => (ChannelLayout::Gray, SampleDepth::U8),
        image::ColorType::La8 => (ChannelLayout::GrayAlpha, SampleDepth::U8),
        image::ColorType::Rgb8 => (ChannelLayout::Rgb, SampleDepth::U8),
        image::ColorType::Rgba8 => (ChannelLayout::Rgba, SampleDepth::U8),
        image::ColorType::L16 => (ChannelLayout::Gray, SampleDepth::U16),
        image::ColorType::La16 => (ChannelLayout::GrayAlpha, SampleDepth::U16),
        image::ColorType::Rgb16 => (ChannelLayout::Rgb, SampleDepth::U16),
        image::ColorType::Rgba16 => (ChannelLayout::Rgba, SampleDepth::U16),
        image::ColorType::Rgb32F => (ChannelLayout::Rgb, SampleDepth::Float),
        image::ColorType::Rgba32F => (ChannelLayout::Rgba, SampleDepth::Float),
        other => return Err(ImageImportError::UnsupportedLayout(other)),
    };
    let pixels = match depth {
        SampleDepth::U8 => {
            let decode = channel_decoder(transfer, 255);
            expand(decoded.into_rgba8().as_raw(), |v: u8| decode[usize::from(v)], |v| f32::from(v) / 255.0)
        }
        SampleDepth::U16 => {
            let decode = channel_decoder(transfer, 65535);
            expand(
                decoded.into_rgba16().as_raw(),
                |v: u16| decode[usize::from(v)],
                |v| f32::from(v) / 65535.0,
            )
        }
        SampleDepth::Float => {
            let image = decoded.into_rgba32f();
            if let Some(i) =
                image.as_raw().as_chunks::<4>().0.iter().position(|p| !p.iter().all(|v| v.is_finite()))
            {
                let i = i as u32;
                return Err(ImageImportError::NonFinite { x: i % width, y: i / width });
            }
            image.as_raw().as_chunks::<4>().0.to_vec()
        }
    };
    Ok(SourceImage { width, height, layout, depth, pixels })
}

/// The linear value of every integer sample `0..=max` for an RGB channel under `transfer`.
fn channel_decoder(transfer: Transfer, max: u16) -> Vec<f32> {
    let scale = f32::from(max);
    (0..=max)
        .map(|v| {
            let v = f32::from(v) / scale;
            match transfer {
                Transfer::Srgb => srgb_to_linear(v),
                Transfer::Linear => v,
            }
        })
        .collect()
}

/// Expands RGBA integer samples, decoding RGB with `color` and alpha with `alpha`.
fn expand<T: Copy>(samples: &[T], color: impl Fn(T) -> f32, alpha: impl Fn(T) -> f32) -> Vec<[f32; 4]> {
    samples.as_chunks::<4>().0.iter().map(|&[r, g, b, a]| [color(r), color(g), color(b), alpha(a)]).collect()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::DynamicImage;

    use super::*;
    use crate::color::linear_to_srgb;

    fn encode(image: DynamicImage, format: image::ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn picks_formats_by_extension() {
        assert_eq!(SourceFormat::from_path(Path::new("a/b.PNG")), Some(SourceFormat::Png));
        assert_eq!(SourceFormat::from_path(Path::new("b.tga")), Some(SourceFormat::Tga));
        assert_eq!(SourceFormat::from_path(Path::new("sky.exr")), Some(SourceFormat::Exr));
        assert_eq!(SourceFormat::from_path(Path::new("b.jpg")), None);
    }

    #[test]
    fn imports_8_bit_sources_as_srgb_or_linear() {
        let pixels = [[0u8, 64, 128, 255], [255, 188, 1, 0], [10, 20, 30, 40], [99, 99, 99, 128]];
        let rgba = image::RgbaImage::from_fn(2, 2, |x, y| image::Rgba(pixels[(y * 2 + x) as usize]));
        for format in [SourceFormat::Png, SourceFormat::Tga] {
            let bytes = encode(DynamicImage::ImageRgba8(rgba.clone()), format.image_format());
            let srgb = import_slice(&bytes, format, Transfer::Srgb).unwrap();
            let linear = import_slice(&bytes, format, Transfer::Linear).unwrap();
            assert_eq!(
                (srgb.width, srgb.height, srgb.layout, srgb.depth),
                (2, 2, ChannelLayout::Rgba, SampleDepth::U8)
            );
            for ((s, l), p) in srgb.pixels.iter().zip(&linear.pixels).zip(pixels) {
                for c in 0..3 {
                    assert_eq!(l[c], f32::from(p[c]) / 255.0, "{format:?}");
                    // sRGB decodes, and re-encodes to the stored value.
                    assert_eq!((linear_to_srgb(s[c]) * 255.0).round() as u8, p[c], "{format:?}");
                }
                // Alpha is linear either way.
                assert_eq!((s[3], l[3]), (f32::from(p[3]) / 255.0, f32::from(p[3]) / 255.0));
            }
            assert!((srgb.pixels[0][2] - 0.2158605).abs() < 1e-6, "sRGB 128 is linear 0.216");
        }
    }

    #[test]
    fn imports_16_bit_and_gray_sources() {
        let gray =
            image::ImageBuffer::<image::Luma<u16>, _>::from_fn(3, 1, |x, _| image::Luma([x as u16 * 30000]));
        let bytes = encode(DynamicImage::ImageLuma16(gray), image::ImageFormat::Png);
        let image = import_slice(&bytes, SourceFormat::Png, Transfer::Linear).unwrap();
        assert_eq!((image.layout, image.depth), (ChannelLayout::Gray, SampleDepth::U16));
        assert_eq!(image.pixels[1], [30000.0 / 65535.0, 30000.0 / 65535.0, 30000.0 / 65535.0, 1.0]);
        assert_eq!(image.layout.logical(image.pixels[2]), [60000.0 / 65535.0, 0.0, 0.0, 0.0]);

        let gray_alpha = image::ImageBuffer::<image::LumaA<u8>, _>::from_pixel(1, 1, image::LumaA([51, 204]));
        let bytes = encode(DynamicImage::ImageLumaA8(gray_alpha), image::ImageFormat::Png);
        let image = import_slice(&bytes, SourceFormat::Png, Transfer::Linear).unwrap();
        assert_eq!(image.layout, ChannelLayout::GrayAlpha);
        assert_eq!(image.layout.logical(image.pixels[0]), [0.2, 0.8, 0.0, 0.0]);

        let rgb16 = image::ImageBuffer::<image::Rgb<u16>, _>::from_pixel(1, 1, image::Rgb([65535, 0, 32768]));
        let bytes = encode(DynamicImage::ImageRgb16(rgb16), image::ImageFormat::Png);
        let image = import_slice(&bytes, SourceFormat::Png, Transfer::Srgb).unwrap();
        assert_eq!((image.layout, image.depth), (ChannelLayout::Rgb, SampleDepth::U16));
        let [r, g, b, a] = image.pixels[0];
        assert_eq!((r, g, a), (1.0, 0.0, 1.0));
        assert!((b - srgb_to_linear(32768.0 / 65535.0)).abs() < 1e-7);
    }

    #[test]
    fn imports_float_sources_as_linear() {
        let values = [[0.0f32, 0.5, 2.0], [16.0, 0.001, 1.0]];
        let hdr = image::Rgb32FImage::from_fn(2, 1, |x, _| image::Rgb(values[x as usize]));
        let bytes = encode(DynamicImage::ImageRgb32F(hdr), image::ImageFormat::OpenExr);
        // Float samples are linear whatever the class says.
        let image = import_slice(&bytes, SourceFormat::Exr, Transfer::Srgb).unwrap();
        assert_eq!(image.depth, SampleDepth::Float);
        assert!(matches!(image.layout, ChannelLayout::Rgb | ChannelLayout::Rgba));
        assert_eq!(image.pixels, [[0.0, 0.5, 2.0, 1.0], [16.0, 0.001, 1.0, 1.0]]);

        let bad = image::Rgba32FImage::from_fn(2, 2, |x, y| {
            image::Rgba([if (x, y) == (1, 1) { f32::NAN } else { 0.0 }; 4])
        });
        let bytes = encode(DynamicImage::ImageRgba32F(bad), image::ImageFormat::OpenExr);
        assert!(matches!(
            import_slice(&bytes, SourceFormat::Exr, Transfer::Linear),
            Err(ImageImportError::NonFinite { x: 1, y: 1 })
        ));
    }

    #[test]
    fn reports_broken_files() {
        let bytes = encode(DynamicImage::ImageRgba8(image::RgbaImage::new(2, 2)), image::ImageFormat::Png);
        assert!(matches!(
            import_slice(&bytes[..bytes.len() / 2], SourceFormat::Png, Transfer::Srgb),
            Err(ImageImportError::Decode(_))
        ));
        assert!(matches!(
            import_slice(&bytes, SourceFormat::Exr, Transfer::Srgb),
            Err(ImageImportError::Decode(_))
        ));
        assert_eq!(SourceImage::new(2, 2, ChannelLayout::Rgba, vec![[0.0; 4]; 3]), None);
        assert_eq!(SourceImage::new(0, 2, ChannelLayout::Rgba, Vec::new()), None);
    }

    /// A PNG declaring `width × height` 8-bit gray, with an empty IDAT: a few dozen bytes whose
    /// header passes for an image of any size, as a hostile or broken file's can.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        fn crc(bytes: &[u8]) -> u32 {
            let mut crc = !0u32;
            for &byte in bytes {
                crc ^= u32::from(byte);
                for _ in 0..8 {
                    crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
                }
            }
            !crc
        }
        let chunk = |png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]| {
            png.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let start = png.len();
            png.extend_from_slice(kind);
            png.extend_from_slice(data);
            let crc = crc(&png[start..]);
            png.extend_from_slice(&crc.to_be_bytes());
        };
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = Vec::new();
        header.extend_from_slice(&width.to_be_bytes());
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[8, 0, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &header);
        chunk(&mut png, b"IDAT", &[]);
        chunk(&mut png, b"IEND", &[]);
        png
    }

    #[test]
    fn oversized_images_fail_from_their_header() {
        // Too wide, too tall, or within both sides but over the texel count: refused before a
        // pixel buffer exists (an import that decoded would fail on the empty IDAT instead).
        for (width, height) in [(16385, 1), (1, 16385), (16384, 4097), (8193, 8192), (30000, 30000)] {
            let error =
                import_slice(&png_header(width, height), SourceFormat::Png, Transfer::Linear).unwrap_err();
            assert!(
                matches!(error, ImageImportError::TooLarge { width: w, height: h, budget } if (w, h) == (width, height) && budget == SOURCE_BUDGET),
                "{width}x{height}: {error:?}"
            );
            let message = error.to_string();
            assert!(message.contains(&format!("{width} × {height} texels")), "{message}");
            assert!(message.contains("16384 texels on a side and 64 Mi texels in all"), "{message}");
        }
        // A header whose image wouldn't fit the decoder's own allocation limit (512 MiB) fails
        // there, as it reads the header: also before any pixel buffer exists.
        let huge = import_slice(&png_header(0x7FFF_FFFF, 0x7FFF_FFFF), SourceFormat::Png, Transfer::Linear);
        assert!(matches!(huge, Err(ImageImportError::Decode(image::ImageError::Limits(_)))), "{huge:?}");
        // Within the budget, the header passes and the missing pixels fail the decode.
        assert!(matches!(
            import_slice(&png_header(16, 16), SourceFormat::Png, Transfer::Linear),
            Err(ImageImportError::Decode(_))
        ));
    }

    #[test]
    fn the_budget_holds_at_its_edges() {
        assert!(SOURCE_BUDGET.fits(16384, 4096) && SOURCE_BUDGET.fits(4096, 16384));
        assert!(SOURCE_BUDGET.fits(8192, 8192) && SOURCE_BUDGET.fits(1, 1));
        assert!(!SOURCE_BUDGET.fits(16384, 4097) && !SOURCE_BUDGET.fits(8192, 8193));
        assert!(!SOURCE_BUDGET.fits(16385, 1) && !SOURCE_BUDGET.fits(1, 16385));
        assert!(!SOURCE_BUDGET.fits(u32::MAX, u32::MAX), "no overflow");
        assert_eq!(SOURCE_BUDGET.texels, 8192 * 8192);
    }

    #[test]
    fn images_within_the_budget_decode_as_before() {
        // A valid image over a smaller budget fails by its size, on either limit.
        let rgba = image::RgbaImage::from_fn(8, 4, |x, y| image::Rgba([x as u8 * 30, y as u8 * 60, 7, 200]));
        let png = encode(DynamicImage::ImageRgba8(rgba), image::ImageFormat::Png);
        for budget in [TexelBudget { side: 7, texels: 1000 }, TexelBudget { side: 8, texels: 31 }] {
            assert!(matches!(
                import_within(&png, SourceFormat::Png, Transfer::Srgb, budget),
                Err(ImageImportError::TooLarge { width: 8, height: 4, .. })
            ));
        }
        assert!(
            import_within(&png, SourceFormat::Png, Transfer::Srgb, TexelBudget { side: 8, texels: 32 })
                .is_ok()
        );

        // Within it, every format decodes to exactly what decoding without the check gives.
        let gray16 = image::ImageBuffer::<image::Luma<u16>, _>::from_fn(5, 3, |x, y| {
            image::Luma([(x * 9000 + y) as u16])
        });
        let hdr =
            image::Rgba32FImage::from_fn(3, 2, |x, y| image::Rgba([x as f32 * 2.5, y as f32, 0.25, 1.0]));
        let samples = [
            (png, image::ImageFormat::Png),
            (encode(DynamicImage::ImageLuma16(gray16), image::ImageFormat::Png), image::ImageFormat::Png),
            (
                encode(DynamicImage::ImageRgba8(image::RgbaImage::new(3, 5)), image::ImageFormat::Tga),
                image::ImageFormat::Tga,
            ),
            (
                encode(DynamicImage::ImageRgba32F(hdr), image::ImageFormat::OpenExr),
                image::ImageFormat::OpenExr,
            ),
        ];
        for (bytes, format) in samples {
            let checked = decode(&bytes, format, SOURCE_BUDGET).unwrap();
            let plain = image::load_from_memory_with_format(&bytes, format).unwrap();
            assert_eq!(checked, plain, "{format:?}");
        }
    }
}
