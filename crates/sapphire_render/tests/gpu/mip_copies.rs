//! Raising and lowering a compressed texture one mip level at a time by copies (M5 M0's
//! spike, kept as a regression test). wgpu has no sparse textures, so m5-plan §3's mip
//! streaming changes a texture's top level by making a new texture with one more or one
//! fewer level, copying every kept level across with `copy_texture_to_texture`, writing a
//! new top with `write_texture`, and rebinding.
//!
//! Each case cooks an image as the cooks do (BC7 for the desktop, ASTC for mobile), uploads
//! a tail (here the deepest level it can stream down to, far under the plan's 128 px, so the
//! small levels are kept levels at every step), raises it a level at a time to the whole
//! texture and lowers it back, and reads
//! every level back after each step: the blocks must be the cooked bytes, and sampling must
//! match the texture as uploaded whole. The sizes aren't powers of two and the tails reach
//! one texel, so levels smaller than a block and levels that aren't whole blocks are copied
//! at every step: wgpu copies their whole blocks, passing Metal and DX12 the padded size
//! and Vulkan the level's own, which is where backends could differ.
//!
//! A texture's top must be whole blocks (wgpu refuses others, and so does
//! `TextureDesc::validate`), so the deepest level a texture can stream down to is the last of
//! the leading levels that are whole blocks: level k of a base W × H is a top only when the
//! block's size times 2^k divides W and H.

use std::f32::consts::TAU;

use sapphire_assets::AssetId;
use sapphire_assets::texture::{AstcBlock, Texture, TextureDesc, TextureFormat, TextureTail};
use sapphire_assets::texture_level::TextureLevel;
use sapphire_pipeline::cook::Cook;
use sapphire_pipeline::import::image::{ChannelLayout, SourceImage};
use sapphire_pipeline::texture::{self, ColorSettings, CookedTexture, TextureClass, TextureSettings};
use sapphire_render::texture::{GpuTexture, TextureError, check_format, wgpu_format};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use super::texture::LevelSampler;

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

/// What a streamed texture needs: sampled, written, and a copy's source when it's replaced.
const USAGE: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING
    .union(wgpu::TextureUsages::COPY_DST)
    .union(wgpu::TextureUsages::COPY_SRC);

/// A level's extent in whole blocks, as copies of a compressed format take it.
fn copy_extent(desc: &TextureDesc, level: u32) -> wgpu::Extent3d {
    let (width, height) = desc.level_extent(level);
    wgpu::Extent3d { width, height, depth_or_array_layers: 1 }.physical_size(wgpu_format(desc.format))
}

/// `desc` from `level` down: the shape of a texture whose top is that level.
fn from_level(desc: &TextureDesc, level: u32) -> TextureDesc {
    let (width, height) = desc.level_extent(level);
    TextureDesc { width, height, mip_level_count: desc.mip_level_count - level, ..*desc }
}

/// The deepest level such that it and every level above it are whole blocks.
fn deepest_whole_level(desc: &TextureDesc) -> u32 {
    let (block_width, block_height) = desc.format.block_dimensions();
    (0..desc.mip_level_count)
        .take_while(|&level| {
            let (width, height) = desc.level_extent(level);
            width.is_multiple_of(block_width) && height.is_multiple_of(block_height)
        })
        .last()
        .expect("a cooked base is whole blocks")
}

fn create(gpu: &GpuContext, desc: &TextureDesc) -> wgpu::Texture {
    gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: desc.width, height: desc.height, depth_or_array_layers: 1 },
        mip_level_count: desc.mip_level_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu_format(desc.format),
        usage: USAGE,
        view_formats: &[],
    })
}

/// Writes `data` to `level` of `raw`, as `GpuTexture::upload` writes each level.
fn write_level(gpu: &GpuContext, raw: &wgpu::Texture, desc: &TextureDesc, level: u32, data: &[u8]) {
    let (block_width, block_height) = desc.format.block_dimensions();
    let (width, height) = desc.level_extent(level);
    gpu.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: raw,
            mip_level: level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width.div_ceil(block_width) * desc.format.block_bytes()),
            rows_per_image: Some(height.div_ceil(block_height)),
        },
        copy_extent(desc, level),
    );
}

/// A texture of shape `desc` holding `levels`, written level by level.
fn upload(gpu: &GpuContext, desc: &TextureDesc, levels: &[Vec<u8>]) -> wgpu::Texture {
    assert_eq!(levels.len() as u32, desc.mip_level_count);
    let raw = create(gpu, desc);
    for (level, data) in (0..).zip(levels) {
        write_level(gpu, &raw, desc, level, data);
    }
    raw
}

/// Copies `count` levels of `from`, starting at `from_level`, into `to` from `to_level`.
fn copy_levels(
    encoder: &mut wgpu::CommandEncoder,
    (from, from_desc, from_level): (&wgpu::Texture, &TextureDesc, u32),
    (to, to_level): (&wgpu::Texture, u32),
    count: u32,
) {
    for i in 0..count {
        let texture = |texture, mip_level| wgpu::TexelCopyTextureInfo {
            texture,
            mip_level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        };
        encoder.copy_texture_to_texture(
            texture(from, from_level + i),
            texture(to, to_level + i),
            copy_extent(from_desc, from_level + i),
        );
    }
}

/// The design's raise: a texture of shape `to` (one level more than `from`), with `from`'s
/// levels copied one level down and `top` written as its new level 0.
fn raise(
    gpu: &GpuContext,
    from: &wgpu::Texture,
    from_desc: &TextureDesc,
    to: &TextureDesc,
    top: &[u8],
) -> wgpu::Texture {
    assert_eq!(to.mip_level_count, from_desc.mip_level_count + 1);
    let raw = create(gpu, to);
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    copy_levels(&mut encoder, (from, from_desc, 0), (&raw, 1), from_desc.mip_level_count);
    write_level(gpu, &raw, to, 0, top);
    gpu.queue().submit([encoder.finish()]);
    raw
}

/// The design's lower: a texture one level smaller, holding `from`'s levels below its top.
fn lower(gpu: &GpuContext, from: &wgpu::Texture, from_desc: &TextureDesc) -> wgpu::Texture {
    let to = from_level(from_desc, 1);
    let raw = create(gpu, &to);
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    copy_levels(&mut encoder, (from, from_desc, 1), (&raw, 0), to.mip_level_count);
    gpu.queue().submit([encoder.finish()]);
    raw
}

/// Every level's blocks, read back and packed tightly, as cooked levels are.
fn read_levels(gpu: &GpuContext, raw: &wgpu::Texture, desc: &TextureDesc) -> Vec<Vec<u8>> {
    let device = gpu.device();
    let (block_width, block_height) = desc.format.block_dimensions();
    let mut encoder = device.create_command_encoder(&Default::default());
    let mut readbacks = Vec::new();
    for level in 0..desc.mip_level_count {
        let (width, height) = desc.level_extent(level);
        let row = width.div_ceil(block_width) * desc.format.block_bytes();
        let rows = height.div_ceil(block_height);
        let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * rows),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: raw,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(rows),
                },
            },
            copy_extent(desc, level),
        );
        readbacks.push((buffer, row as usize, padded as usize));
    }
    gpu.queue().submit([encoder.finish()]);
    for (buffer, ..) in &readbacks {
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    }
    device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    readbacks
        .iter()
        .map(|(buffer, row, padded)| {
            let mapped = buffer.get_mapped_range(..).expect("mapped");
            mapped.chunks(*padded).flat_map(|line| &line[..*row]).copied().collect()
        })
        .collect()
}

/// A smooth image whose every channel varies, so every block differs from its neighbours.
fn image(width: u32, height: u32) -> SourceImage {
    let pixels = (0..width * height)
        .map(|i| {
            let (u, v) = ((i % width) as f32 / width as f32, (i / width) as f32 / height as f32);
            let t = 0.5 + 0.3 * (u * TAU).sin() * (v * TAU * 0.5).cos() + 0.2 * (u - v);
            [0.1 + 0.8 * t, 0.9 - 0.6 * t, 0.3 + 0.5 * v, 0.4 + 0.5 * u]
        })
        .collect();
    SourceImage::new(width, height, ChannelLayout::Rgba, pixels).expect("a valid image")
}

struct Case {
    name: &'static str,
    cook: Cook,
    block: AstcBlock,
    /// Level 0's size: whole blocks, and not a power of two where the block allows it.
    size: (u32, u32),
}

fn cases() -> [Case; 4] {
    let case = |name, cook, block, size| Case { name, cook, block, size };
    [
        // 20×12 is the deepest whole-block level: 10×6 and 5×3 aren't whole blocks, 2×1 and
        // 1×1 are smaller than one.
        case("BC7 sRGB", Cook::Desktop, AstcBlock::B4x4, (160, 96)),
        // What the mobile cook uses for every class it compresses.
        case("ASTC 4x4 sRGB", Cook::Mobile, AstcBlock::B4x4, (160, 96)),
        // The coarser blocks a texture's settings may choose: down to 12×6 with 6×6 blocks, and
        // to 40×24 with 8×8, keeping 20×12, which is over one block and not whole blocks.
        case("ASTC 6x6 sRGB", Cook::Mobile, AstcBlock::B6x6, (192, 96)),
        case("ASTC 8x8 sRGB", Cook::Mobile, AstcBlock::B8x8, (320, 192)),
    ]
}

/// Checks the texture `raw` (shape `desc`, the cooked texture's levels from `top` down)
/// against the cook's bytes and against the same levels uploaded whole and sampled.
fn check(
    gpu: &GpuContext,
    sampler: &LevelSampler,
    case: &Case,
    cooked: &CookedTexture,
    top: u32,
    raw: &wgpu::Texture,
) {
    let desc = from_level(&cooked.desc, top);
    let read = read_levels(gpu, raw, &desc);
    for (level, (read, cooked)) in read.iter().zip(&cooked.levels[top as usize..]).enumerate() {
        assert!(
            read == cooked,
            "{}: top {top}, level {level} ({:?}) differs from the cook's blocks",
            case.name,
            desc.level_extent(level as u32),
        );
    }
    let whole = Texture { desc, levels: cooked.levels[top as usize..].iter().map(Vec::as_slice).collect() };
    let uploaded = GpuTexture::upload(gpu, &whole, case.name).expect("upload");
    let expected = sampler.sample(gpu, uploaded.texture());
    let sampled = sampler.sample(gpu, raw);
    for (level, (expected, sampled)) in expected.iter().zip(&sampled).enumerate() {
        let same =
            expected.iter().flatten().map(|v| v.to_bits()).eq(sampled.iter().flatten().map(|v| v.to_bits()));
        assert!(same, "{}: top {top}, level {level} samples differently from a whole upload", case.name);
    }
}

#[test]
#[ignore = "needs a GPU"]
fn compressed_textures_stream_level_by_level_by_copies() {
    let gpu = gpu();
    let sampler = LevelSampler::new(&gpu);
    let mut skipped = Vec::new();
    for case in cases() {
        let settings = TextureSettings {
            class: TextureClass::Color(ColorSettings { mips: true, astc_block: case.block }),
        };
        let cooked = texture::cook(&image(case.size.0, case.size.1), &settings, case.cook).expect("cook");
        let desc = cooked.desc;
        if let Err(error @ TextureError::Unsupported { .. }) = check_format(&gpu, desc.format) {
            skipped.push(format!("{}: {error}", case.name));
            continue;
        }
        let tail = deepest_whole_level(&desc);
        assert!(tail >= 2, "{}: the case should stream at least two levels", case.name);
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);

        // The tail as it's uploaded today, then each level above it in turn.
        let mut raw = upload(&gpu, &from_level(&desc, tail), &cooked.levels[tail as usize..]);
        check(&gpu, &sampler, &case, &cooked, tail, &raw);
        for top in (0..tail).rev() {
            raw = raise(
                &gpu,
                &raw,
                &from_level(&desc, top + 1),
                &from_level(&desc, top),
                &cooked.levels[top as usize],
            );
            check(&gpu, &sampler, &case, &cooked, top, &raw);
        }
        // And back down to the tail.
        for top in 1..=tail {
            raw = lower(&gpu, &raw, &from_level(&desc, top - 1));
            check(&gpu, &sampler, &case, &cooked, top, &raw);
        }
        assert!(pollster::block_on(scope.pop()).is_none(), "{}: validation error", case.name);

        // The level below the tail isn't whole blocks, and a texture can't start there.
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        let _ = create(&gpu, &from_level(&desc, tail + 1));
        assert!(
            pollster::block_on(scope.pop()).is_some(),
            "{}: a texture whose top isn't whole blocks should be refused",
            case.name
        );
        eprintln!(
            "{:<14} {:?}: {}x{} streamed from {:?} up and down, every level matches",
            case.name,
            gpu.caps().backend(),
            desc.width,
            desc.height,
            desc.level_extent(tail),
        );
    }
    for skip in &skipped {
        eprintln!("skipped {skip}");
    }
    // A skip must be a device without the format, never a lost check: CI's CPU rasterizers
    // (lavapipe and WARP) take the desktop cook's BC7, and every Apple GPU takes ASTC.
    let caps = gpu.caps();
    if caps.adapter.device_type == wgpu::DeviceType::Cpu {
        assert!(!skipped.iter().any(|skip| skip.starts_with("BC7")), "{skipped:?}");
    }
    if caps.backend() == wgpu::Backend::Metal {
        assert!(!skipped.iter().any(|skip| skip.starts_with("ASTC")), "{skipped:?}");
    }
}

/// `count` valid blocks of `format` that all differ: BC7 mode 6 blocks, or ASTC void-extent
/// (one color) blocks, from a fixed sequence.
fn blocks(format: TextureFormat, count: u64, mut state: u64) -> Vec<u8> {
    let mut next = move || {
        // SplitMix64.
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    (0..count)
        .flat_map(|_| {
            let (low, high) = match format {
                // Mode 6 is a 1 bit and six 0 bits; any bits after them are a valid block.
                TextureFormat::Bc7RgbaUnorm | TextureFormat::Bc7RgbaUnormSrgb => {
                    ((next() & !0x7f) | 0x40, next())
                }
                // A 2D LDR void-extent block with no extent: its color is the upper 64 bits.
                TextureFormat::Astc { .. } => (0xffff_ffff_ffff_fdfc, next()),
                _ => unreachable!("{format:?} isn't a streamed compressed format"),
            };
            [low.to_le_bytes(), high.to_le_bytes()].concat()
        })
        .collect()
}

/// The cost of one raise of a 2048² texture from its 1024² top: create, eleven copies and a
/// 4 MiB write. Logged for the spike's report and checked byte for byte; time it alone, on a
/// quiet machine.
#[test]
#[ignore = "needs a GPU"]
#[allow(clippy::disallowed_methods, reason = "timing needs the OS clock, and nothing here ships")]
fn raising_a_2k_texture_by_one_level_keeps_every_block() {
    use std::time::Instant;
    const RUNS: usize = 9;
    let gpu = gpu();
    for format in
        [TextureFormat::Bc7RgbaUnormSrgb, TextureFormat::Astc { block: AstcBlock::B4x4, srgb: true }]
    {
        if let Err(error @ TextureError::Unsupported { .. }) = check_format(&gpu, format) {
            eprintln!("skipped {format:?}: {error}");
            continue;
        }
        let desc = TextureDesc {
            format,
            width: 2048,
            height: 2048,
            mip_level_count: 12,
            swizzle: Default::default(),
        };
        let levels: Vec<Vec<u8>> = (0..desc.mip_level_count)
            .map(|level| blocks(format, desc.level_byte_len(level) / 16, u64::from(level)))
            .collect();
        let (from, to) = (from_level(&desc, 1), desc);
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        // Milliseconds: create, copies recorded, top written, submitted, done on the GPU.
        let mut phases = vec![[0.0f64; 5]; RUNS];
        let mut raised = None;
        for phase in &mut phases {
            let tail = upload(&gpu, &from, &levels[1..]);
            gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
            let start = Instant::now();
            let mut lap = {
                let mut last = start;
                move || {
                    let now = Instant::now();
                    let ms = (now - last).as_secs_f64() * 1e3;
                    last = now;
                    ms
                }
            };
            let raw = create(&gpu, &to);
            phase[0] = lap();
            let mut encoder = gpu.device().create_command_encoder(&Default::default());
            copy_levels(&mut encoder, (&tail, &from, 0), (&raw, 1), from.mip_level_count);
            let commands = encoder.finish();
            phase[1] = lap();
            write_level(&gpu, &raw, &to, 0, &levels[0]);
            phase[2] = lap();
            gpu.queue().submit([commands]);
            phase[3] = lap();
            gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
            phase[4] = lap();
            raised = Some(raw);
        }
        assert!(pollster::block_on(scope.pop()).is_none(), "{format:?}: validation error");
        let read = read_levels(&gpu, &raised.expect("raised"), &to);
        assert!(read == levels, "{format:?}: the raised texture differs from its levels");

        let median = |i: usize| {
            let mut times: Vec<f64> = phases[1..].iter().map(|phase| phase[i]).collect();
            times.sort_by(f64::total_cmp);
            times[times.len() / 2]
        };
        let total = {
            let mut totals: Vec<f64> = phases[1..].iter().map(|phase| phase.iter().sum()).collect();
            totals.sort_by(f64::total_cmp);
            totals[totals.len() / 2]
        };
        eprintln!(
            "{format:?} on {} ({:?}): raise 1024 to 2048, median of {} after a first of {:.3} ms: \
             create {:.3}, copies {:.3}, write {:.3}, submit {:.3}, GPU wait {:.3}, total {total:.3} ms",
            gpu.caps().adapter.name,
            gpu.caps().backend(),
            RUNS - 1,
            phases[0].iter().sum::<f64>(),
            median(0),
            median(1),
            median(2),
            median(3),
            median(4),
        );
    }
}

/// Whether `a` and `b` sample alike at every level, bit for bit.
fn sample_alike(gpu: &GpuContext, sampler: &LevelSampler, a: &wgpu::Texture, b: &wgpu::Texture) -> bool {
    let (a, b) = (sampler.sample(gpu, a), sampler.sample(gpu, b));
    a.len() == b.len()
        && a.iter().flatten().map(|v| v.map(f32::to_bits)).eq(b.iter().flatten().map(|v| v.map(f32::to_bits)))
}

/// M5 M3's entry for the app: a texture cooked as a tail and levels, uploaded as its tail with the whole
/// texture made at once ([`GpuTexture::upload_tail`]) and its levels written in place as they arrive, in
/// any order, samples bit for bit as the whole texture uploaded at once. A new version of the same shape
/// (hot reload) is written over it in place, and what isn't one of its levels is refused.
#[test]
#[ignore = "needs a GPU"]
fn a_tail_and_its_levels_written_in_place_sample_as_the_whole_texture() {
    let gpu = gpu();
    let sampler = LevelSampler::new(&gpu);
    let id = AssetId::from_bytes([5; 16]);
    let mut skipped = Vec::new();
    for case in cases() {
        let settings = TextureSettings {
            class: TextureClass::Color(ColorSettings { mips: true, astc_block: case.block }),
        };
        let (width, height) = case.size;
        let cooked = texture::cook(&image(width, height), &settings, case.cook).expect("cook");
        if let Err(error @ TextureError::Unsupported { .. }) = check_format(&gpu, cooked.desc.format) {
            skipped.push(format!("{}: {error}", case.name));
            continue;
        }
        let above = cooked.levels_above(&settings);
        assert!(above >= 1, "{}: the case should split", case.name);
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);

        let tail_file = cooked.tail(above);
        let tail = TextureTail::from_ktx2(&tail_file).expect("a tail");
        let mut split = GpuTexture::upload_tail(&gpu, &tail, case.name).expect("upload the tail");
        assert_eq!((split.desc(), split.is_whole()), (cooked.desc, false), "{}", case.name);
        // The levels arrive deepest first, as a streamer raises a texture.
        for level in (0..above).rev() {
            let bytes = cooked.level(id, level).expect("a level");
            split.write_level(&gpu, TextureLevel::read(&bytes).expect("valid")).expect("write the level");
        }
        let whole = GpuTexture::upload(&gpu, &cooked.texture(), case.name).expect("upload whole");
        assert!(split.is_whole(), "{}", case.name);
        assert_eq!(split.bytes(), whole.bytes(), "{}", case.name);
        assert!(sample_alike(&gpu, &sampler, split.texture(), whole.texture()), "{}: differs", case.name);

        // A new version of the same shape, written over it in place: the tail, then its levels again.
        let mut source = image(width, height);
        source.pixels.reverse();
        let next = texture::cook(&source, &settings, case.cook).expect("cook");
        let next_tail_file = next.tail(above);
        split.write_tail(&gpu, &TextureTail::from_ktx2(&next_tail_file).expect("a tail")).expect("in place");
        for level in 0..above {
            let bytes = next.level(id, level).expect("a level");
            split.write_level(&gpu, TextureLevel::read(&bytes).expect("valid")).expect("write the level");
        }
        let next_whole = GpuTexture::upload(&gpu, &next.texture(), case.name).expect("upload whole");
        assert_eq!(split.bytes(), whole.bytes(), "{}: rewriting counts nothing twice", case.name);
        assert!(sample_alike(&gpu, &sampler, split.texture(), next_whole.texture()), "{}: reload", case.name);

        // The tail's top isn't a level above it, and a whole texture isn't this one's tail.
        let top = cooked.level(id, above).expect("a level");
        let refused = split.write_level(&gpu, TextureLevel::read(&top).expect("valid"));
        assert!(matches!(refused, Err(TextureError::NotALevel { .. })), "{}: {refused:?}", case.name);
        let refused = split.write_tail(&gpu, &TextureTail { texture: cooked.texture(), above: 0 });
        assert!(matches!(refused, Err(TextureError::OtherShape { .. })), "{}: {refused:?}", case.name);
        assert!(pollster::block_on(scope.pop()).is_none(), "{}: validation error", case.name);
    }
    for skip in &skipped {
        eprintln!("skipped {skip}");
    }
    if gpu.caps().adapter.device_type == wgpu::DeviceType::Cpu {
        assert!(!skipped.iter().any(|skip| skip.starts_with("BC7")), "{skipped:?}");
    }
}
