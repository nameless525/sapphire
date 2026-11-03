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
use std::sync::{Condvar, Mutex, PoisonError};

use sapphire_assets::texture::{AstcBlock, TextureFormat};

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

/// Which levels astcenc may encode at once. `astcenc_config_init` (`cpp/astcenc_entry.cpp`) points a
/// function-local `static` at the preset table for its block size, then reads its tuning through that:
/// a thread with another size between the two takes the wrong table's tuning, and encodes different
/// bytes without a word. Built with clang at `-O2` or more the read is gone, and at `-O1` or `-O0` it
/// isn't; other compilers are unread, and it can't be serialized from outside `ctt::convert`. So levels
/// of one table run together, since a write of the same pointer changes nothing, and another table's
/// wait for them to finish. Nothing holding this waits for anything else, so every wait ends.
struct AstcPresets {
    /// The table in use, and how many levels are using it.
    using: Mutex<(usize, usize)>,
    finished: Condvar,
}

/// The one astcenc reads through.
static ASTC_PRESETS: AstcPresets = AstcPresets::new();

/// Which of astcenc's preset tables a block size takes, as `astcenc_config_init` chooses.
fn preset_table(block: AstcBlock) -> usize {
    let (width, height) = block.dimensions();
    match width * height {
        ..25 => 0,
        25..64 => 1,
        64.. => 2,
    }
}

impl AstcPresets {
    const fn new() -> Self {
        Self { using: Mutex::new((0, 0)), finished: Condvar::new() }
    }

    /// Waits until no level is using another table, and uses `table` until this drops.
    fn enter(&self, table: usize) -> UsingPresets<'_> {
        let mut using = self.using.lock().unwrap_or_else(PoisonError::into_inner);
        while using.1 > 0 && using.0 != table {
            using = self.finished.wait(using).unwrap_or_else(PoisonError::into_inner);
        }
        *using = (table, using.1 + 1);
        UsingPresets(self)
    }
}

/// A level using a preset table, a panicking one's included, until this drops.
struct UsingPresets<'a>(&'a AstcPresets);

impl Drop for UsingPresets<'_> {
    fn drop(&mut self) {
        self.0.using.lock().unwrap_or_else(PoisonError::into_inner).1 -= 1;
        self.0.finished.notify_all();
    }
}

/// Encodes one level. `level`'s texels are linear and carry the format's channels first.
pub(super) fn encode_level(
    level: &Level,
    format: TextureFormat,
    encoding: Encoding,
) -> Result<Vec<u8>, ctt::Error> {
    let _presets = match format {
        TextureFormat::Astc { block, .. } => Some(ASTC_PRESETS.enter(preset_table(block))),
        _ => None,
    };
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn block_sizes_take_the_preset_table_astcenc_gives_them() {
        use AstcBlock::*;
        assert_eq!([B4x4, B5x5, B6x6, B8x8].map(preset_table), [0, 1, 1, 2]);
    }

    /// One table's levels run together, and another's wait for all of them, a panicking one's too.
    #[test]
    fn levels_of_another_preset_table_wait_for_those_using_one() {
        let presets = AstcPresets::new();
        let (first, second) = (presets.enter(0), presets.enter(0));
        let started = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                let other = presets.enter(2);
                started.store(true, Ordering::SeqCst);
                drop(other);
            });
            thread::sleep(Duration::from_millis(20));
            assert!(!started.load(Ordering::SeqCst), "it waits for both");
            drop(first);
            thread::sleep(Duration::from_millis(20));
            assert!(!started.load(Ordering::SeqCst), "it waits for the last");
            drop(second);
        });
        assert!(started.load(Ordering::SeqCst));
        let panicking = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _using = presets.enter(1);
            panic!("an encoder's panic");
        }));
        assert!(panicking.is_err());
        drop(presets.enter(0));
    }
}
