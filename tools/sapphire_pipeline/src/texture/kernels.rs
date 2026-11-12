//! The block encoders' kernels, each run at one instruction set per architecture and none whose
//! results depend on the CPU's maker, so a cook's compressed texels are the same on every machine
//! of an architecture (M4 C3, C4).
//!
//! The kernels come prebuilt: bc7enc-rdo's `bc7e` for BC7 and Intel's for BC4, BC5, BC6H and BC7,
//! both ISPC compiled for SSE2, SSE4, AVX, AVX2 and AVX-512 on x86-64, behind a dispatcher that
//! picks the widest one the CPU has. CI's Linux runners in one region have AVX-512 and in another
//! don't, and the same commit packed BC7 two ways (PR #290's two runs), so the AVX2 entry points
//! are called by name instead, never the dispatcher, and a CPU without AVX2 can't cook
//! block-compressed textures at all ([`EncodeError::Cpu`]) (C3).
//!
//! **x86-64 encodes BC7 with Intel's kernel, not `bc7e` (C4).** `bc7e` is built with ISPC's fast
//! math, which turns its divides into `rcpps` reciprocal estimates (41 in its AVX2 build, in the
//! endpoint fits and the colour cells' error), and an estimate's low bits are each CPU design's
//! own: Intel's and AMD's differ, so pinning AVX2 still left two BC7 cooks, one per maker of
//! runner (main's CI at 83dc6273, and C3's "other" hashes before it). Intel's kernels are built
//! without fast math and hold no estimate (`rcpps`, `rsqrtps`) in any x86-64 build; their divides
//! and square roots are IEEE's, exact on every CPU, as are AVX2's fused multiply-adds. AArch64 keeps
//! `bc7e`, built for NEON alone: its estimate (`frecpe`) is defined bit for bit by the architecture,
//! and keeping it keeps every texture on the Mac, and the Metal goldens, as they were. Intel's BC7
//! has no perceptual weighting, so x86-64's colours are fitted by plain RGB error.
//!
//! astcenc picks AVX2 when the CPU has it and nothing wider, so requiring AVX2 pins it too; its own
//! dispatch stays in `ctt-astcenc`, and it holds no reciprocal estimate either.
//!
//! The two architectures' kernels differ (NEON fuses where SSE wouldn't, and BC7 is two encoders),
//! so an AArch64 cook and an x86-64 cook differ in compressed blocks; each is pinned on its own
//! (`cooks_the_same_bytes_on_every_machine_of_an_architecture`).

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("the texture kernels are pinned for x86-64 and AArch64 only");

#[cfg(any(target_arch = "x86_64", test))]
use ctt_intel_texture_compressor::bindings::kernel::bc7_enc_settings;
use ctt_intel_texture_compressor::bindings::kernel::{bc6h_enc_settings, rgba_surface};

/// Why a level couldn't encode.
#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    /// The CPU lacks the instruction set the kernels are pinned to.
    #[error(
        "this CPU lacks {0}, which the texture kernels are pinned to so every machine cooks the same \
         bytes; cook on an x86-64 CPU with AVX2, FMA, F16C, BMI1 and BMI2 (2013 or later), or on AArch64"
    )]
    Cpu(&'static str),
    /// astcenc refused the configuration or the image.
    #[error("astcenc: {0}")]
    Astc(#[from] ctt_astcenc::Error),
}

/// Fails unless this CPU runs the instruction set the kernels are pinned to: always on AArch64.
pub fn check_cpu() -> Result<(), EncodeError> {
    #[cfg(target_arch = "x86_64")]
    {
        // What ISPC's AVX2 target and astcenc's AVX2 build assume.
        let required = [
            ("AVX2", std::arch::is_x86_feature_detected!("avx2")),
            ("FMA", std::arch::is_x86_feature_detected!("fma")),
            ("F16C", std::arch::is_x86_feature_detected!("f16c")),
            ("BMI1", std::arch::is_x86_feature_detected!("bmi1")),
            ("BMI2", std::arch::is_x86_feature_detected!("bmi2")),
        ];
        if let Some(&(name, _)) = required.iter().find(|(_, present)| !present) {
            return Err(EncodeError::Cpu(name));
        }
    }
    Ok(())
}

/// The pinned entry points: on x86-64 the AVX2 builds by their own names, on AArch64 the only build.
mod ffi {
    #[cfg(target_arch = "aarch64")]
    pub use ctt_bc7enc_rdo::bindings::{
        bc7e_compress_block_init as bc7e_init, bc7e_compress_block_params_init_basic as bc7e_params_basic,
        bc7e_compress_blocks as bc7e,
    };
    /// Intel's BC7, which AArch64's cook doesn't use; its tests do.
    #[cfg(all(target_arch = "aarch64", test))]
    pub use ctt_intel_texture_compressor::bindings::kernel::CompressBlocksBC7_ispc as bc7;
    #[cfg(target_arch = "aarch64")]
    pub use ctt_intel_texture_compressor::bindings::kernel::{
        CompressBlocksBC4_ispc as bc4, CompressBlocksBC5_ispc as bc5, CompressBlocksBC6H_ispc as bc6h,
    };

    #[cfg(target_arch = "x86_64")]
    pub use self::x86_64::*;

    /// ISPC names each target's build of an exported function `<name>_<target>`, beside the
    /// dispatcher under the plain name; the prebuilt archives for Linux, macOS and Windows all carry
    /// them. Signatures are the dispatchers', from the crates' generated bindings.
    #[cfg(target_arch = "x86_64")]
    mod x86_64 {
        use ctt_intel_texture_compressor::bindings::kernel::{
            bc6h_enc_settings, bc7_enc_settings, rgba_surface,
        };

        unsafe extern "C" {
            #[link_name = "CompressBlocksBC4_ispc_avx2"]
            pub fn bc4(src: *mut rgba_surface, dst: *mut u8);
            #[link_name = "CompressBlocksBC5_ispc_avx2"]
            pub fn bc5(src: *mut rgba_surface, dst: *mut u8);
            #[link_name = "CompressBlocksBC6H_ispc_avx2"]
            pub fn bc6h(src: *mut rgba_surface, dst: *mut u8, settings: *mut bc6h_enc_settings);
            #[link_name = "CompressBlocksBC7_ispc_avx2"]
            pub fn bc7(src: *mut rgba_surface, dst: *mut u8, settings: *mut bc7_enc_settings);
        }
    }
}

/// BC7 blocks (16 bytes each) from an RGBA8 image (R first), at the architecture's pinned encoder:
/// Intel's kernel on x86-64, `bc7e` on AArch64. `perceptual` weighs colour error by BT.601 luma, as
/// suits colours and is wrong for data; only `bc7e` can, so x86-64 ignores it. Call [`check_cpu`]
/// first.
pub fn bc7(image: &Blocks<'_>, perceptual: bool) -> Vec<u8> {
    #[cfg(target_arch = "x86_64")]
    {
        let _ = perceptual;
        intel_bc7(image)
    }
    #[cfg(target_arch = "aarch64")]
    {
        bc7e(image, perceptual)
    }
}

/// `bc7e`'s tables, built once by the pinned build before any of its blocks encode.
#[cfg(target_arch = "aarch64")]
static BC7E_TABLES: std::sync::Once = std::sync::Once::new();

/// BC7 through `bc7e`'s "basic" preset, with perceptual or uniform error.
#[cfg(target_arch = "aarch64")]
fn bc7e(image: &Blocks<'_>, perceptual: bool) -> Vec<u8> {
    use ctt_bc7enc_rdo::bindings::bc7e_compress_block_params;

    let pixels = image.tiled_words();
    let blocks = pixels.len() / 16;
    // SAFETY: the init takes no arguments and only fills the kernel's own tables; `Once` runs it
    // exactly once, and every encode below waits for it.
    BC7E_TABLES.call_once(|| unsafe { ffi::bc7e_init() });
    // SAFETY: the params struct is plain data the preset fills in whole; zeroed is a valid value
    // of it (integers and bools) before that.
    let params = unsafe {
        let mut params: bc7e_compress_block_params = std::mem::zeroed();
        ffi::bc7e_params_basic(&mut params, perceptual);
        params
    };
    let mut out = vec![0u64; blocks * 2];
    let count = u32::try_from(blocks).expect("a level's block count fits in u32");
    // SAFETY: `pixels` holds `count` blocks of 16 texels and `out` room for `count` blocks of 16
    // bytes, as the kernel reads and writes; neither is touched elsewhere meanwhile.
    unsafe { ffi::bc7e(count, out.as_mut_ptr(), pixels.as_ptr(), &params) };
    bytemuck::cast_slice(&out).to_vec()
}

/// BC7 through Intel's kernel at its "basic" preset: the opaque one when every texel's alpha is
/// 255, so alpha costs no bits, and the alpha one otherwise.
#[cfg(any(target_arch = "x86_64", test))]
fn intel_bc7(image: &Blocks<'_>) -> Vec<u8> {
    intel_bc7_with(image, bc7_basic(opaque(image)))
}

/// BC7 through Intel's kernel at `settings`; with three channels (an opaque image), every block
/// decodes alpha 255.
#[cfg(any(target_arch = "x86_64", test))]
fn intel_bc7_with(image: &Blocks<'_>, mut settings: bc7_enc_settings) -> Vec<u8> {
    assert_eq!(image.bytes_per_texel, 4);
    let mut surface = image.surface();
    let mut out = vec![0u8; image.block_count() * 16];
    // SAFETY: as in `bc4`, with 16 bytes a block; the settings are read only.
    unsafe { ffi::bc7(&mut surface, out.as_mut_ptr(), &mut settings) };
    if settings.channels == 3 {
        opaque_mode6(&mut out);
    }
    out
}

/// Makes every mode 6 block decode alpha 255. Mode 6 keeps one P-bit per endpoint as the low bit
/// of all four of its channels, and Intel's opaque preset picks it by RGB error alone, so an opaque
/// block can decode alpha 254, which data packed into alpha (ILM maps) would read as another value.
/// Both alpha endpoints become 127 and both P-bits 1, which moves an endpoint's colour by one code
/// value where its P-bit was 0. Every other mode it picks stores opaque alpha exactly.
#[cfg(any(target_arch = "x86_64", test))]
fn opaque_mode6(blocks: &mut [u8]) {
    /// Mode 6's alpha endpoints (bits 49 to 62) and P-bits (63 and 64).
    const ALPHA_AND_P_BITS: u128 = 0xffff << 49;
    let (blocks, rest) = blocks.as_chunks_mut::<16>();
    assert!(rest.is_empty(), "whole blocks");
    for block in blocks {
        let bits = u128::from_le_bytes(*block);
        if bits & 0x7f == 0x40 {
            *block = (bits | ALPHA_AND_P_BITS).to_le_bytes();
        }
    }
}

/// Whether every texel of an RGBA8 image has alpha 255.
#[cfg(any(target_arch = "x86_64", test))]
fn opaque(image: &Blocks<'_>) -> bool {
    image.texels.as_chunks::<4>().0.iter().all(|texel| texel[3] == u8::MAX)
}

/// Intel's "basic" BC7 presets, `ctt_intel_texture_compressor::bc7::opaque_basic_settings` and
/// `alpha_basic_settings`, which a test holds this to.
#[cfg(any(target_arch = "x86_64", test))]
fn bc7_basic(opaque: bool) -> bc7_enc_settings {
    bc7_enc_settings {
        mode_selection: [true; 4],
        refineIterations: [2, 2, 2, 2, 2, 2, 2, if opaque { 0 } else { 2 }],
        skip_mode2: true,
        fastSkipThreshold_mode1: 12,
        fastSkipThreshold_mode3: 8,
        fastSkipThreshold_mode7: if opaque { 0 } else { 4 },
        mode45_channel0: 0,
        refineIterations_channel: 2,
        channels: if opaque { 3 } else { 4 },
    }
}

/// A tightly packed image in whole blocks: `width` and `height` multiples of 4, `bytes_per_texel`
/// bytes a texel, row after row.
pub struct Blocks<'a> {
    pub texels: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub bytes_per_texel: u32,
}

impl Blocks<'_> {
    /// The kernels' view of it. They never write through the pointer, whose `mut` is the C
    /// header's.
    fn surface(&self) -> rgba_surface {
        assert!(
            self.width.is_multiple_of(4)
                && self.height.is_multiple_of(4)
                && self.width > 0
                && self.height > 0
        );
        let stride = self.width * self.bytes_per_texel;
        assert_eq!(self.texels.len(), stride as usize * self.height as usize, "a tightly packed image");
        let int = |value: u32| i32::try_from(value).expect("a level's size fits in i32");
        rgba_surface {
            ptr: self.texels.as_ptr().cast_mut(),
            width: int(self.width),
            height: int(self.height),
            stride: int(stride),
        }
    }

    fn block_count(&self) -> usize {
        (self.width / 4) as usize * (self.height / 4) as usize
    }

    /// An RGBA8 image as whole 4 × 4 blocks in row order, each block's 16 texels in row order, as
    /// packed little-endian words: what `bc7e` reads.
    #[cfg(any(target_arch = "aarch64", test))]
    fn tiled_words(&self) -> Vec<u32> {
        assert_eq!(self.bytes_per_texel, 4);
        let _ = self.surface();
        let row = self.width as usize * 4;
        let mut out = Vec::with_capacity(self.block_count() * 16);
        for by in 0..(self.height / 4) as usize {
            for bx in 0..(self.width / 4) as usize {
                for y in by * 4..by * 4 + 4 {
                    let start = y * row + bx * 16;
                    let words = self.texels[start..start + 16].as_chunks::<4>().0;
                    out.extend(words.iter().map(|&texel| u32::from_le_bytes(texel)));
                }
            }
        }
        out
    }
}

/// BC4 blocks (8 bytes each) from an R8 image. Call [`check_cpu`] first.
pub fn bc4(image: &Blocks<'_>) -> Vec<u8> {
    assert_eq!(image.bytes_per_texel, 1);
    let mut surface = image.surface();
    let mut out = vec![0u8; image.block_count() * 8];
    // SAFETY: `surface` describes `image`'s whole-block texels, which the kernel only reads, and
    // `out` has the 8 bytes per block it writes.
    unsafe { ffi::bc4(&mut surface, out.as_mut_ptr()) };
    out
}

/// BC5 blocks (16 bytes each) from an RG8 image. Call [`check_cpu`] first.
pub fn bc5(image: &Blocks<'_>) -> Vec<u8> {
    assert_eq!(image.bytes_per_texel, 2);
    let mut surface = image.surface();
    let mut out = vec![0u8; image.block_count() * 16];
    // SAFETY: as in `bc4`, with 16 bytes a block.
    unsafe { ffi::bc5(&mut surface, out.as_mut_ptr()) };
    out
}

/// BC6H unsigned-float blocks (16 bytes each) from an RGBA16F image with no negative halves, at
/// the kernel's "basic" settings. Call [`check_cpu`] first.
pub fn bc6h(image: &Blocks<'_>) -> Vec<u8> {
    assert_eq!(image.bytes_per_texel, 8);
    let mut surface = image.surface();
    // Intel's "basic" preset (`ctt_intel_texture_compressor::bc6h::basic_settings`).
    let mut settings = bc6h_enc_settings {
        slow_mode: false,
        fast_mode: false,
        refineIterations_1p: 2,
        refineIterations_2p: 2,
        fastSkipThreshold: 4,
    };
    let mut out = vec![0u8; image.block_count() * 16];
    // SAFETY: as in `bc4`, with 16 bytes a block; the settings are read only.
    unsafe { ffi::bc6h(&mut surface, out.as_mut_ptr(), &mut settings) };
    out
}

#[cfg(test)]
mod tests;
