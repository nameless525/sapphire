//! The block encoders' kernels, each run at one instruction set per architecture, so a cook's
//! compressed texels are the same on every machine of an architecture (M4 C3).
//!
//! The kernels come prebuilt: bc7enc-rdo's `bc7e` for BC7 and Intel's for BC4, BC5 and BC6H, both
//! ISPC compiled with fast math for SSE2, SSE4, AVX, AVX2 and AVX-512 on x86-64, behind a dispatcher
//! that picks the widest one the CPU has. Fast math lets each fuse and estimate differently, so the
//! dispatcher made a texture's blocks depend on the CPU: CI's Linux runners in one region have
//! AVX-512 and in another don't, and the same commit packed BC7 two ways (PR #290's two runs). Here
//! the AVX2 entry points are called by name instead, never the dispatcher, and a CPU without AVX2
//! can't cook block-compressed textures at all ([`EncodeError::Cpu`]). AArch64's kernels are built
//! for NEON alone, so there is nothing to choose. astcenc picks AVX2 when the CPU has it and nothing
//! wider, so requiring AVX2 pins it too; its own dispatch stays in `ctt-astcenc`.
//!
//! The two architectures' kernels don't round alike (NEON fuses where SSE wouldn't), so an
//! AArch64 cook and an x86-64 cook differ in compressed blocks; each is pinned on its own
//! (`cooks_the_same_bytes_on_every_machine_of_an_architecture`).

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("the texture kernels are pinned for x86-64 and AArch64 only");

use ctt_bc7enc_rdo::bindings::bc7e_compress_block_params;
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
        bc7e_compress_block_init as bc7_init, bc7e_compress_block_params_init_basic as bc7_params_basic,
        bc7e_compress_blocks as bc7,
    };
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
        use ctt_bc7enc_rdo::bindings::bc7e_compress_block_params;
        use ctt_intel_texture_compressor::bindings::kernel::{bc6h_enc_settings, rgba_surface};

        unsafe extern "C" {
            #[link_name = "bc7e_compress_block_init_avx2"]
            pub fn bc7_init();
            #[link_name = "bc7e_compress_block_params_init_basic_avx2"]
            pub fn bc7_params_basic(params: *mut bc7e_compress_block_params, perceptual: bool);
            #[link_name = "bc7e_compress_blocks_avx2"]
            pub fn bc7(
                num_blocks: u32,
                blocks: *mut u64,
                pixels: *const u32,
                params: *const bc7e_compress_block_params,
            );
            #[link_name = "CompressBlocksBC4_ispc_avx2"]
            pub fn bc4(src: *mut rgba_surface, dst: *mut u8);
            #[link_name = "CompressBlocksBC5_ispc_avx2"]
            pub fn bc5(src: *mut rgba_surface, dst: *mut u8);
            #[link_name = "CompressBlocksBC6H_ispc_avx2"]
            pub fn bc6h(src: *mut rgba_surface, dst: *mut u8, settings: *mut bc6h_enc_settings);
        }
    }
}

/// `bc7e`'s tables, built once by the pinned build before any of its blocks encode.
static BC7_TABLES: std::sync::Once = std::sync::Once::new();

/// BC7 blocks from `pixels`: whole 4 × 4 blocks of RGBA8 texels, each block's 16 in row order, as
/// packed little-endian words. `bc7e`'s "basic" preset, with perceptual (BT.601-weighted) error or
/// uniform. Call [`check_cpu`] first.
pub fn bc7(pixels: &[u32], perceptual: bool) -> Vec<u8> {
    assert_eq!(pixels.len() % 16, 0, "whole blocks");
    let blocks = pixels.len() / 16;
    // SAFETY: the init takes no arguments and only fills the kernel's own tables; `Once` runs it
    // exactly once, and every encode below waits for it.
    BC7_TABLES.call_once(|| unsafe { ffi::bc7_init() });
    // SAFETY: the params struct is plain data the preset fills in whole; zeroed is a valid value
    // of it (integers and bools) before that.
    let params = unsafe {
        let mut params: bc7e_compress_block_params = std::mem::zeroed();
        ffi::bc7_params_basic(&mut params, perceptual);
        params
    };
    let mut out = vec![0u64; blocks * 2];
    let count = u32::try_from(blocks).expect("a level's block count fits in u32");
    // SAFETY: `pixels` holds `count` blocks of 16 texels and `out` room for `count` blocks of 16
    // bytes, as the kernel reads and writes; neither is touched elsewhere meanwhile.
    unsafe { ffi::bc7(count, out.as_mut_ptr(), pixels.as_ptr(), &params) };
    bytemuck::cast_slice(&out).to_vec()
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
