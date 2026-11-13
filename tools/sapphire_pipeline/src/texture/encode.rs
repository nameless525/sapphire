//! One mip level to its cooked format's bytes: linear `f32` texels in, the format's bytes out.
//!
//! Every float operation between the levels and the stored texels is the cooker's own scalar Rust
//! here, so it rounds alike on every machine (M4 C3): quantizing, sRGB encoding, half floats and
//! RGB9E5, both for uncompressed formats and for the 8-bit or half-float images the block
//! encoders read. The block encoders themselves are [`super::kernels`], pinned to one instruction
//! set per architecture and to kernels whose results don't depend on the CPU's maker. ctt's
//! converter, which this replaced, picked SIMD paths by the CPU (its AVX-512 sRGB store estimates
//! reciprocals), as did the kernels' dispatchers.
//!
//! The arithmetic is ctt 0.6.1's own, so cooks kept their bytes: its sRGB encode is a minimax fit
//! within half a code value of the exact curve, evaluated with exact square roots and divides,
//! colour rounding to nearest even and linear 8- and 16-bit unorms rounding half away from zero.
//!
//! **Alpha is passed through untouched,** as the levels arrive: already filtered, never
//! premultiplied or divided here, so data packed into alpha (ILM maps) survives.
//!
//! Block formats take edge texels for the texels past a level's last whole block (a mip smaller
//! than a block), as ctt's encoders did.

use std::sync::{PoisonError, RwLock};
use std::thread;

use ctt_astcenc as astc;
use half::f16;
use sapphire_assets::texture::TextureFormat;

use super::kernels::{self, Blocks, EncodeError};
use super::mips::Level;

/// How a level becomes its format's bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Encoding {
    /// Converted, not compressed.
    Uncompressed,
    /// BC7, at the architecture's pinned encoder ([`kernels::bc7`]). Perceptual weighting suits
    /// colors and is wrong for data; only AArch64's encoder weighs.
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

/// The pinned astcenc's process-wide state. Config initialization writes a function-local static
/// preset pointer, whose value reaches the output, and every new context rewrites the angle tables
/// that compression reads, with the same values, which is still a race as C++ counts them. So
/// configs and contexts are made under the write lock, until the dependency owns that state per
/// invocation, and compression, which reads only its context and those tables, runs under the read
/// lock, any number at once.
static ASTC_TABLES: RwLock<()> = RwLock::new(());

/// Encodes one level, a block format's in strips on as many threads as the machine has
/// ([`kernels::strips`]): the same bytes as on one. `level`'s texels are linear and carry the
/// format's channels first.
pub(super) fn encode_level(
    level: &Level,
    format: TextureFormat,
    encoding: Encoding,
) -> Result<Vec<u8>, EncodeError> {
    encode_level_on(level, format, encoding, thread::available_parallelism().map_or(1, usize::from))
}

/// [`encode_level`] on up to `threads` threads.
fn encode_level_on(
    level: &Level,
    format: TextureFormat,
    encoding: Encoding,
    threads: usize,
) -> Result<Vec<u8>, EncodeError> {
    let (width, height) = (level.width, level.height);
    let bytes = match (encoding, format) {
        (Encoding::Uncompressed, _) => store(level, format),
        (Encoding::Bc7 { perceptual }, _) => {
            kernels::check_cpu()?;
            let texels = rgba8(level, format.is_srgb());
            let (texels, width, height) = pad_to_blocks(bytemuck::cast_slice(&texels), width, height, 4);
            kernels::bc7(&Blocks { texels: &texels, width, height, bytes_per_texel: 4 }, perceptual, threads)
        }
        (Encoding::Ispc, TextureFormat::Bc4RUnorm) => {
            kernels::check_cpu()?;
            let texels = channels8(level, 1);
            let (texels, width, height) = pad_to_blocks(&texels, width, height, 1);
            kernels::bc4(&Blocks { texels: &texels, width, height, bytes_per_texel: 1 }, threads)
        }
        (Encoding::Ispc, TextureFormat::Bc5RgUnorm) => {
            kernels::check_cpu()?;
            let texels = channels8(level, 2);
            let (texels, width, height) = pad_to_blocks(&texels, width, height, 2);
            kernels::bc5(&Blocks { texels: &texels, width, height, bytes_per_texel: 2 }, threads)
        }
        (Encoding::Ispc, TextureFormat::Bc6hRgbUfloat) => {
            kernels::check_cpu()?;
            // BC6H's unsigned kernel reads halves as integers: a negative one (a set sign bit, -0
            // included) would saturate, so it goes in as +0.
            let texels: Vec<u8> = level
                .pixels
                .iter()
                .flatten()
                .flat_map(|&c| {
                    let half = f16::from_f32(c);
                    if half.is_sign_negative() { [0, 0] } else { half.to_le_bytes() }
                })
                .collect();
            let (texels, width, height) = pad_to_blocks(&texels, width, height, 8);
            kernels::bc6h(&Blocks { texels: &texels, width, height, bytes_per_texel: 8 }, threads)
        }
        (Encoding::Ispc, _) => unreachable!("the plan gives Intel's kernels BC4, BC5 and BC6H only"),
        (Encoding::Astc(usage), TextureFormat::Astc { block, srgb }) => {
            kernels::check_cpu()?;
            astc(level, block.dimensions(), srgb, usage, threads)?
        }
        (Encoding::Astc(_), _) => unreachable!("the plan gives astcenc ASTC formats only"),
    };
    let expected = format.image_byte_len(width, height);
    assert_eq!(bytes.len() as u64, expected, "{format:?} at {width} × {height}");
    Ok(bytes)
}

/// The level in an uncompressed `format`.
fn store(level: &Level, format: TextureFormat) -> Vec<u8> {
    let pixels = &level.pixels;
    match format {
        TextureFormat::Rgba8UnormSrgb => rgba8(level, true).into_iter().flat_map(u32::to_le_bytes).collect(),
        TextureFormat::R8Unorm => channels8(level, 1),
        TextureFormat::Rg8Unorm => channels8(level, 2),
        TextureFormat::R16Unorm => pixels.iter().flat_map(|p| unorm16(p[0])).collect(),
        TextureFormat::Rg16Unorm => {
            pixels.iter().flat_map(|p| [unorm16(p[0]), unorm16(p[1])]).flatten().collect()
        }
        TextureFormat::Rgba16Float => {
            pixels.iter().flatten().flat_map(|&c| f16::from_f32(c).to_le_bytes()).collect()
        }
        TextureFormat::Rgb9e5Ufloat => pixels.iter().flat_map(|&p| rgb9e5(p).to_le_bytes()).collect(),
        _ => unreachable!("{format:?} is block-compressed"),
    }
}

/// The level's first `n` channels as 8-bit linear unorms, texel after texel.
fn channels8(level: &Level, n: usize) -> Vec<u8> {
    level.pixels.iter().flat_map(|pixel| pixel[..n].iter().map(|&c| unorm8(c))).collect()
}

/// The level as RGBA8 words (R in the low byte): RGB sRGB-encoded if `srgb`, alpha linear.
fn rgba8(level: &Level, srgb: bool) -> Vec<u32> {
    level
        .pixels
        .iter()
        .map(|&[r, g, b, a]| {
            let bytes = if srgb {
                [srgb8(r), srgb8(g), srgb8(b), to_byte(a.clamp(0.0, 1.0))]
            } else {
                [unorm8(r), unorm8(g), unorm8(b), unorm8(a)]
            };
            u32::from_le_bytes(bytes)
        })
        .collect()
}

/// A linear value as an 8-bit unorm, rounding half away from zero.
fn unorm8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// A linear value as a 16-bit unorm's little-endian bytes, rounding half away from zero.
fn unorm16(c: f32) -> [u8; 2] {
    ((c.clamp(0.0, 1.0) * 65535.0).round() as u16).to_le_bytes()
}

/// `[0, 1]` to a byte, rounding half to even.
fn to_byte(c: f32) -> u8 {
    (c * 255.0).round_ties_even() as u8
}

// ctt 0.6.1's minimax fit of the sRGB encode (`processing/kernels/srgb.rs`, MIT or Apache-2.0):
// `1 / (c^-1/8 - B)^3 - C` with `c^-1/8` as `1 / sqrt(sqrt(sqrt(c)) - A)`, within half a code value of
// the exact curve.
const OETF_A: f32 = 0.075_058_33;
const OETF_B: f32 = 0.048_553_98;
const OETF_C: f32 = 0.027_579_91;
/// Where the sRGB encode's linear segment ends.
const OETF_LINEAR_MAX: f32 = 0.003_130_8;

/// A linear value sRGB-encoded to a byte: square roots, divides and products only, each exactly
/// rounded, so the same on every machine.
fn srgb8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let encoded = if c <= OETF_LINEAR_MAX {
        c * 12.92
    } else {
        let r3 = 1.0 / (c.sqrt().sqrt() - OETF_A).sqrt();
        let inner = r3 - OETF_B;
        1.0 / (inner * inner * inner) - OETF_C
    };
    to_byte(encoded)
}

/// RGB with a shared exponent (`VK_FORMAT_E5B9G9R9_UFLOAT_PACK32`), R in the low bits, alpha dropped:
/// the largest channel picks the exponent, and each mantissa rounds half up.
fn rgb9e5([r, g, b, _]: [f32; 4]) -> u32 {
    const MANTISSA_BITS: u32 = 9;
    /// The largest value the format holds: `(2^9 - 1) / 2^9 × 2^15`.
    const MAX: f32 = 65408.0;
    let clamp = |c: f32| c.clamp(0.0, MAX);
    let [r, g, b] = [clamp(r), clamp(g), clamp(b)];
    let max = r.max(g).max(b);
    // Biased by 15 and floored at 2^-16: `max(exponent field, 111) - 111`.
    let exponent = ((max.to_bits() >> 23) & 0xff).max(111) - 111;
    // 2^(24 - exponent) as bits: an exact power of two, so each product is exact and only the
    // `+ 0.5` rounds.
    let scale = |exponent: u32| f32::from_bits((151 - exponent) << 23);
    let rounded = |c: f32, exponent: u32| (c * scale(exponent) + 0.5) as i32 as u32;
    // Rounding the largest channel up into the next binade takes the next exponent.
    let exponent = if rounded(max, exponent) == 1 << MANTISSA_BITS { exponent + 1 } else { exponent };
    let mantissa = |c: f32| rounded(c, exponent) & 0x1ff;
    ((exponent & 0x1f) << 27) | (mantissa(b) << 18) | (mantissa(g) << 9) | mantissa(r)
}

/// `texels` (tightly packed, `bytes_per_texel` each) grown to whole 4 × 4 blocks by repeating the
/// last column and row, with its new size.
fn pad_to_blocks(texels: &[u8], width: u32, height: u32, bytes_per_texel: usize) -> (Vec<u8>, u32, u32) {
    let (padded_width, padded_height) = (width.next_multiple_of(4), height.next_multiple_of(4));
    if (padded_width, padded_height) == (width, height) {
        return (texels.to_vec(), width, height);
    }
    let row = width as usize * bytes_per_texel;
    let mut out = Vec::with_capacity(padded_width as usize * padded_height as usize * bytes_per_texel);
    for y in 0..padded_height {
        let source = &texels[y.min(height - 1) as usize * row..][..row];
        out.extend_from_slice(source);
        let last = &source[row - bytes_per_texel..];
        for _ in width..padded_width {
            out.extend_from_slice(last);
        }
    }
    (out, padded_width, padded_height)
}

/// The level through astcenc at its "medium" preset from RGBA8 texels, in strips of whole block
/// rows on up to `threads` threads, each strip a single-threaded context's image of its own. A
/// partial block takes edge texels from its own strip, which is the level's last.
fn astc(
    level: &Level,
    (block_x, block_y): (u32, u32),
    srgb: bool,
    usage: AstcUsage,
    threads: usize,
) -> Result<Vec<u8>, EncodeError> {
    let (profile, flags, swizzle) = match usage {
        AstcUsage::Color => (
            if srgb { astc::Profile::LdrSrgb } else { astc::Profile::Ldr },
            astc::Flags::USE_PERCEPTUAL,
            astc::Swizzle::IDENTITY,
        ),
        AstcUsage::Data => (astc::Profile::Ldr, astc::Flags::empty(), astc::Swizzle::IDENTITY),
        AstcUsage::OneChannel => (astc::Profile::Ldr, astc::Flags::empty(), astc::Swizzle::RRR1),
        AstcUsage::TwoChannels => (astc::Profile::Ldr, astc::Flags::empty(), astc::Swizzle::RRRG),
        AstcUsage::Normal => (astc::Profile::Ldr, astc::Flags::MAP_NORMAL, astc::Swizzle::RRRG),
    };
    // The context only ever compresses.
    let flags = flags | astc::Flags::SELF_DECOMPRESS_ONLY;
    let strips = kernels::strips(level.height.div_ceil(block_y), threads);
    let contexts = {
        let _tables = ASTC_TABLES.write().unwrap_or_else(PoisonError::into_inner);
        let config = astc::config_init(profile, block_x, block_y, 1, astc::Preset::Medium, flags)?;
        strips.iter().map(|_| astc::Context::new(&config)).collect::<Result<Vec<_>, _>>()?
    };
    let texels = rgba8(level, srgb);
    let row_words = level.width as usize;
    let _tables = ASTC_TABLES.read().unwrap_or_else(PoisonError::into_inner);
    let jobs = strips.into_iter().zip(contexts).collect();
    kernels::on_threads(jobs, |(rows, mut context)| -> Result<Vec<u8>, EncodeError> {
        let first = rows.start * block_y;
        let height = (rows.end * block_y).min(level.height) - first;
        // astcenc only reads the image, through a mutable pointer of the C header's; each strip
        // copies its rows so no two threads share one.
        let mut strip = texels[first as usize * row_words..][..height as usize * row_words].to_vec();
        let mut data = strip.as_mut_ptr().cast::<std::ffi::c_void>();
        let mut image = astc::bindings::astcenc_image {
            dim_x: level.width,
            dim_y: height,
            dim_z: 1,
            data_type: astc::bindings::astcenc_type_ASTCENC_TYPE_U8,
            data: &mut data,
        };
        let blocks = level.width.div_ceil(block_x) as usize * height.div_ceil(block_y) as usize;
        let mut out = vec![0; blocks * 16];
        context.compress(&mut image, swizzle, &mut out)?;
        Ok(out)
    })
}

#[cfg(test)]
mod tests {
    use sapphire_assets::texture::AstcBlock;

    use super::*;

    #[test]
    fn a_level_encodes_the_same_in_strips() {
        // Tall enough for several strips, and neither size a whole number of blocks, so the last
        // strip holds partial blocks; one strip has translucent texels and the rest are opaque.
        let (width, height) = (38, 301);
        let level = Level {
            width,
            height,
            pixels: (0..width * height)
                .map(|i| {
                    let (x, y) = ((i % width) as f32, (i / width) as f32);
                    let alpha = if (100.0..120.0).contains(&y) { 0.5 } else { 1.0 };
                    [(x * 0.37 + y * 0.11).sin() * 0.5 + 0.5, x / width as f32, y / height as f32, alpha]
                })
                .collect(),
        };
        let encodings = [
            (TextureFormat::Bc7RgbaUnormSrgb, Encoding::Bc7 { perceptual: true }),
            (TextureFormat::Bc7RgbaUnorm, Encoding::Bc7 { perceptual: false }),
            (TextureFormat::Bc4RUnorm, Encoding::Ispc),
            (TextureFormat::Bc5RgUnorm, Encoding::Ispc),
            (TextureFormat::Bc6hRgbUfloat, Encoding::Ispc),
            (TextureFormat::Astc { block: AstcBlock::B4x4, srgb: true }, Encoding::Astc(AstcUsage::Color)),
            (TextureFormat::Astc { block: AstcBlock::B6x6, srgb: false }, Encoding::Astc(AstcUsage::Normal)),
            (TextureFormat::Astc { block: AstcBlock::B8x8, srgb: false }, Encoding::Astc(AstcUsage::Data)),
        ];
        for (format, encoding) in encodings {
            let whole = encode_level_on(&level, format, encoding, 1).unwrap();
            for threads in [2, 3, 7] {
                let strips = encode_level_on(&level, format, encoding, threads).unwrap();
                assert!(strips == whole, "{format:?} on {threads} threads");
            }
        }
    }

    #[test]
    fn concurrent_astc_conversions_match_serial_output_for_all_tables() {
        let level = Level {
            width: 8,
            height: 8,
            pixels: (0..64).map(|i| [((i * 7) % 31) as f32 / 31.0, i as f32 / 64.0, 0.25, 1.0]).collect(),
        };
        let formats = [AstcBlock::B4x4, AstcBlock::B5x5, AstcBlock::B6x6, AstcBlock::B8x8]
            .map(|block| TextureFormat::Astc { block, srgb: true });
        let serial =
            formats.map(|format| encode_level(&level, format, Encoding::Astc(AstcUsage::Color)).unwrap());
        thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|i| {
                    let level = &level;
                    scope.spawn(move || {
                        encode_level(level, formats[i % 4], Encoding::Astc(AstcUsage::Color)).unwrap()
                    })
                })
                .collect();
            for (i, handle) in handles.into_iter().enumerate() {
                assert_eq!(handle.join().unwrap(), serial[i % 4]);
            }
        });
    }
}
