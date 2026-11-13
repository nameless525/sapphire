use super::*;

/// Intel's BC7 kernel restricted to mode 6 (one subset, RGBA endpoints), which [`decode_mode6`]
/// reads back.
fn mode6_only(channels: i32) -> bc7_enc_settings {
    bc7_enc_settings { mode_selection: [false, false, false, true], channels, ..bc7_basic(channels == 3) }
}

/// A BC7 mode 6 block's 16 texels in row order: 7-bit RGBA endpoints with a P-bit each, and 4-bit
/// indices (the first one 3 bits) weighting between them, as the format defines.
fn decode_mode6(block: &[u8; 16]) -> [[u8; 4]; 16] {
    let bits = u128::from_le_bytes(*block);
    assert_eq!(bits & 0x7f, 0x40, "a mode 6 block");
    let mut at = 7;
    let mut take = |count: u32| {
        let value = (bits >> at) & ((1 << count) - 1);
        at += count;
        value as u32
    };
    let mut endpoints = [[0u32; 4]; 2];
    for channel in 0..4 {
        for endpoint in &mut endpoints {
            endpoint[channel] = take(7);
        }
    }
    let p_bits = [take(1), take(1)];
    for (endpoint, p) in endpoints.iter_mut().zip(p_bits) {
        endpoint.iter_mut().for_each(|c| *c = *c << 1 | p);
    }
    const WEIGHTS: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];
    std::array::from_fn(|texel| {
        let weight = WEIGHTS[take(if texel == 0 { 3 } else { 4 }) as usize];
        std::array::from_fn(|c| {
            (((64 - weight) * endpoints[0][c] + weight * endpoints[1][c] + 32) >> 6) as u8
        })
    })
}

/// A 16 × 8 RGBA8 image whose eight blocks lie far apart in colour, each texel on its block's line.
fn eight_blocks(alpha: bool) -> Vec<u8> {
    let (width, height) = (16, 8);
    let mut texels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let block = (y / 4) * 4 + x / 4;
            let t = (x % 4 + (y % 4) * 4) as u8;
            let base = 20 + 28 * block as u8;
            let a = if alpha { 200 - 2 * t - block as u8 * 10 } else { u8::MAX };
            texels.extend([base + t, 255 - base - t, base / 2 + t / 2, a]);
        }
    }
    texels
}

#[test]
fn intel_bc7_encodes_each_block_where_its_texels_are() {
    for alpha in [false, true] {
        let texels = eight_blocks(alpha);
        let image = Blocks { texels: &texels, width: 16, height: 8, bytes_per_texel: 4 };
        assert_eq!(opaque(&image), !alpha);
        let encoded = intel_bc7_with(&image, mode6_only(if alpha { 4 } else { 3 }));
        assert_eq!(encoded.len(), 8 * 16);
        let mut worst = 0;
        for (block, bytes) in encoded.as_chunks::<16>().0.iter().enumerate() {
            let (bx, by) = (block % 4, block / 4);
            for (i, decoded) in decode_mode6(bytes).iter().enumerate() {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                let source = &texels[(y * 16 + x) * 4..][..4];
                if !alpha {
                    assert_eq!(decoded[3], u8::MAX, "an opaque image stays opaque");
                }
                let error = (0..4).map(|c| decoded[c].abs_diff(source[c])).max().unwrap();
                worst = worst.max(error);
            }
        }
        // Blocks in the wrong place or channels out of order would be tens of values off.
        assert!(worst <= 3, "alpha {alpha}: a texel decodes {worst} off");
    }
}

#[test]
fn the_bc7_presets_are_intels_basic_ones() {
    use ctt_intel_texture_compressor::bc7;
    for (opaque, preset) in [(true, bc7::opaque_basic_settings()), (false, bc7::alpha_basic_settings())] {
        let ours = bc7_basic(opaque);
        assert_eq!(ours.mode_selection, preset.mode_selection);
        assert_eq!(ours.refineIterations, preset.refine_iterations.map(|i| i as i32));
        assert_eq!(ours.skip_mode2, preset.skip_mode2);
        assert_eq!(ours.fastSkipThreshold_mode1, preset.fast_skip_threshold_mode1 as i32);
        assert_eq!(ours.fastSkipThreshold_mode3, preset.fast_skip_threshold_mode3 as i32);
        assert_eq!(ours.fastSkipThreshold_mode7, preset.fast_skip_threshold_mode7 as i32);
        assert_eq!(ours.mode45_channel0, preset.mode45_channel0 as i32);
        assert_eq!(ours.refineIterations_channel, preset.refine_iterations_channel as i32);
        assert_eq!(ours.channels, preset.channels);
    }
}

#[test]
fn the_cook_encodes_bc7_with_its_architectures_encoder() {
    let texels = eight_blocks(true);
    let image = Blocks { texels: &texels, width: 16, height: 8, bytes_per_texel: 4 };
    check_cpu().unwrap();
    let cooked = bc7(&image, false, 1);
    if cfg!(target_arch = "x86_64") {
        assert_eq!(cooked, intel_bc7(&image));
    } else {
        assert_ne!(cooked, intel_bc7(&image), "AArch64 keeps bc7e");
    }
}

#[test]
fn intels_bc7_in_strips_keeps_the_whole_levels_preset() {
    // 64 rows of blocks, so up to four strips; only the last strip's texels are translucent, so the
    // other three are opaque on their own and would pick the opaque preset, which encodes otherwise.
    let (width, height) = (8u32, 256u32);
    let texels: Vec<u8> = (0..width * height)
        .flat_map(|i| {
            let (x, y) = (i % width, i / width);
            let alpha = if y >= 192 { 128 } else { u8::MAX };
            [(x * 29 + y * 7) as u8, (y * 3) as u8, (x * y) as u8, alpha]
        })
        .collect();
    let image = Blocks { texels: &texels, width, height, bytes_per_texel: 4 };
    check_cpu().unwrap();
    let whole = intel_bc7(&image);
    for threads in [1, 2, 4] {
        assert!(intel_bc7_in_strips(&image, threads) == whole, "{threads} threads");
    }
    let opaque_rows = intel_bc7(&image.rows(0..48));
    assert!(opaque_rows[..] != whole[..opaque_rows.len()], "the opaque preset encodes these rows otherwise");
}

/// The other x86-64 builds of the BC7 kernels, for the probe below.
#[cfg(target_arch = "x86_64")]
mod x86_64 {
    use ctt_bc7enc_rdo::bindings::bc7e_compress_block_params;
    use ctt_intel_texture_compressor::bindings::kernel::{bc7_enc_settings, rgba_surface};

    unsafe extern "C" {
        #[link_name = "bc7e_compress_block_init_avx2"]
        pub fn bc7e_init_avx2();
        #[link_name = "bc7e_compress_block_params_init_basic_avx2"]
        pub fn bc7e_params_avx2(params: *mut bc7e_compress_block_params, perceptual: bool);
        #[link_name = "bc7e_compress_blocks_avx2"]
        pub fn bc7e_avx2(
            count: u32,
            blocks: *mut u64,
            pixels: *const u32,
            params: *const bc7e_compress_block_params,
        );
        #[link_name = "bc7e_compress_block_init_avx512skx"]
        pub fn bc7e_init_avx512();
        #[link_name = "bc7e_compress_block_params_init_basic_avx512skx"]
        pub fn bc7e_params_avx512(params: *mut bc7e_compress_block_params, perceptual: bool);
        #[link_name = "bc7e_compress_blocks_avx512skx"]
        pub fn bc7e_avx512(
            count: u32,
            blocks: *mut u64,
            pixels: *const u32,
            params: *const bc7e_compress_block_params,
        );
        #[link_name = "CompressBlocksBC7_ispc_avx512skx"]
        pub fn intel_bc7_avx512(src: *mut rgba_surface, dst: *mut u8, settings: *mut bc7_enc_settings);
    }

    /// Whether this CPU runs ISPC's `avx512skx` target.
    pub fn has_avx512() -> bool {
        std::arch::is_x86_feature_detected!("avx512f")
            && std::arch::is_x86_feature_detected!("avx512bw")
            && std::arch::is_x86_feature_detected!("avx512cd")
            && std::arch::is_x86_feature_detected!("avx512dq")
            && std::arch::is_x86_feature_detected!("avx512vl")
    }

    type Init = unsafe extern "C" fn();
    type Params = unsafe extern "C" fn(*mut bc7e_compress_block_params, bool);
    type Encode = unsafe extern "C" fn(u32, *mut u64, *const u32, *const bc7e_compress_block_params);

    /// `bc7e`'s "basic" preset, perceptual, through one build's entry points.
    pub fn bc7e(words: &[u32], (init, params_basic, encode): (Init, Params, Encode)) -> Vec<u8> {
        let blocks = words.len() / 16;
        let mut out = vec![0u64; blocks * 2];
        // SAFETY: as in the cook's `bc7e`: the build's own init and preset, then `blocks` blocks of
        // 16 texels in and 16 bytes out.
        unsafe {
            init();
            let mut params: bc7e_compress_block_params = std::mem::zeroed();
            params_basic(&mut params, true);
            encode(blocks as u32, out.as_mut_ptr(), words.as_ptr(), &params);
        }
        bytemuck::cast_slice(&out).to_vec()
    }

    pub const BC7E_AVX2: (Init, Params, Encode) = (bc7e_init_avx2, bc7e_params_avx2, bc7e_avx2);
    pub const BC7E_AVX512: (Init, Params, Encode) = (bc7e_init_avx512, bc7e_params_avx512, bc7e_avx512);

    /// blake3 of `_mm_rcp_ps` at every float in `[1, 2)`: one value per design of the estimate.
    pub fn rcpps_table() -> String {
        use std::arch::x86_64::{_mm_cvtss_f32, _mm_rcp_ps, _mm_set1_ps};
        let mut bytes = Vec::with_capacity(4 << 23);
        for mantissa in 0..1u32 << 23 {
            let x = f32::from_bits(0x3f80_0000 | mantissa);
            #[allow(unused_unsafe)]
            // SAFETY: SSE is part of x86-64's baseline.
            let estimate = unsafe { _mm_cvtss_f32(_mm_rcp_ps(_mm_set1_ps(x))) };
            bytes.extend(estimate.to_bits().to_le_bytes());
        }
        blake3::hash(&bytes).to_hex()[..16].to_string()
    }
}

/// M4 C4's evidence, run on CI's x86-64 runners by `.github/workflows/bc7-probe.yml`: one line per
/// committed texture and BC7 encoder build available here, `bc7-probe <texture> <encoder> <blake3>
/// <ms>`, and on x86-64 the hash of this CPU's `rcpps` table. Across runners of both makers, `bc7e`'s
/// x86-64 builds split with the table and Intel's AVX2 build doesn't.
/// An encoder's name, and the encode.
type Encoder<'a> = (&'static str, Box<dyn Fn() -> Vec<u8> + 'a>);

#[test]
#[allow(clippy::print_stdout, reason = "with --nocapture, the probe's lines are its evidence")]
#[ignore = "evidence for CI's BC7 probe; prints, asserts nothing"]
fn prints_this_machines_bc7_encodings() {
    use std::time::Instant;

    let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let sources = [
        "combat/lancer/lancer_matcap.png",
        "lookdev/town/textures/stone.png",
        "third_party/vrm1_constraint_twist_sample/extracted/Body_00.png",
    ];
    println!("bc7-probe arch {}", std::env::consts::ARCH);
    #[cfg(target_arch = "x86_64")]
    println!("bc7-probe rcpps {}", x86_64::rcpps_table());
    for source in sources {
        let decoded =
            ::image::load_from_memory(&std::fs::read(content.join(source)).unwrap()).unwrap().into_rgba8();
        let (width, height) = (decoded.width() / 4 * 4, decoded.height() / 4 * 4);
        let texels: Vec<u8> = decoded
            .rows()
            .take(height as usize)
            .flat_map(|row| row.take(width as usize).flat_map(|texel| texel.0))
            .collect();
        let image = Blocks { texels: &texels, width, height, bytes_per_texel: 4 };
        let words = image.tiled_words();
        let mut encoders: Vec<Encoder<'_>> =
            vec![("cook", Box::new(|| bc7(&image, true, 1))), ("intel", Box::new(|| intel_bc7(&image)))];
        #[cfg(target_arch = "x86_64")]
        {
            encoders.push(("bc7e-avx2", Box::new(|| x86_64::bc7e(&words, x86_64::BC7E_AVX2))));
            if x86_64::has_avx512() {
                encoders.push(("bc7e-avx512", Box::new(|| x86_64::bc7e(&words, x86_64::BC7E_AVX512))));
                encoders.push((
                    "intel-avx512",
                    Box::new(|| {
                        let mut surface = image.surface();
                        let mut settings = bc7_basic(opaque(&image));
                        let mut out = vec![0u8; image.block_count() * 16];
                        // SAFETY: as in `intel_bc7_with`, through the AVX-512 build this CPU runs.
                        unsafe { x86_64::intel_bc7_avx512(&mut surface, out.as_mut_ptr(), &mut settings) };
                        out
                    }),
                ));
            }
        }
        #[cfg(target_arch = "aarch64")]
        encoders.push(("bc7e", Box::new(|| bc7e(&image, true))));
        let _ = &words;
        for (name, encode) in encoders {
            let start = Instant::now();
            let blocks = encode();
            let ms = start.elapsed().as_millis();
            println!("bc7-probe {source} {name} {} {ms}", &blake3::hash(&blocks).to_hex()[..16]);
        }
    }
}
