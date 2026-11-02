//! Golden images (spec §9 gate 3; M1 plan §3, open question Q6).
//!
//! A capture is compared with the reference image for its **backend family** ([`backend_key`]):
//! CPU rasterizers and GPUs disagree in the last bit, so each family keeps its own goldens.
//! Comparison uses NVIDIA's FLIP, a perceptual per-pixel error in [0, 1], through `nv-flip`
//! (as wgpu's own tests do), and gates on the mean, a high percentile and the worst small tile
//! rather than the maximum: two-tone shading's hard terminators and outlines turn tiny precision
//! differences into whole-pixel flips along edges, which a max would always fail on.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

/// An 8-bit RGBA image, rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Why a golden image couldn't be read, written or compared.
#[derive(Debug, thiserror::Error)]
pub enum GoldenError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{}: not a PNG this harness reads: {source}", path.display())]
    Decode {
        path: PathBuf,
        #[source]
        source: png::DecodingError,
    },
    #[error("{}: {source}", path.display())]
    Encode {
        path: PathBuf,
        #[source]
        source: png::EncodingError,
    },
    #[error("{}: expected 8-bit RGBA, found {color:?} at {depth:?}", path.display())]
    Format { path: PathBuf, color: png::ColorType, depth: png::BitDepth },
    #[error("size mismatch: the golden is {golden:?}, the capture {actual:?}")]
    SizeMismatch { golden: (u32, u32), actual: (u32, u32) },
}

impl Image {
    pub fn read_png(path: &Path) -> Result<Self, GoldenError> {
        let io = |source| GoldenError::Io { path: path.to_owned(), source };
        let decode = |source| GoldenError::Decode { path: path.to_owned(), source };
        let decoder = png::Decoder::new(std::io::BufReader::new(File::open(path).map_err(io)?));
        let mut reader = decoder.read_info().map_err(decode)?;
        let (color, depth) = reader.output_color_type();
        if color != png::ColorType::Rgba || depth != png::BitDepth::Eight {
            return Err(GoldenError::Format { path: path.to_owned(), color, depth });
        }
        let mut rgba = vec![0; reader.output_buffer_size().unwrap_or(0)];
        let frame = reader.next_frame(&mut rgba).map_err(decode)?;
        rgba.truncate(frame.buffer_size());
        Ok(Self { width: frame.width, height: frame.height, rgba })
    }

    pub fn write_png(&self, path: &Path) -> Result<(), GoldenError> {
        let encode = |source| GoldenError::Encode { path: path.to_owned(), source };
        let file = File::create(path).map_err(|source| GoldenError::Io { path: path.to_owned(), source })?;
        let mut encoder = png::Encoder::new(BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(encode)?;
        writer.write_image_data(&self.rgba).map_err(encode)?;
        writer.finish().map_err(encode)
    }

    /// The RGB channels, packed. FLIP ignores alpha.
    fn rgb(&self) -> Vec<u8> {
        self.rgba.as_chunks::<4>().0.iter().flat_map(|&[r, g, b, _]| [r, g, b]).collect()
    }
}

/// The golden-image family for a capture rendered by `adapter` on `backend` (wgpu's backend
/// name: `vulkan`, `metal`, `dx12`):
///
/// | Key | Renderer |
/// |---|---|
/// | `vulkan-lavapipe` | Mesa's CPU Vulkan driver (Linux CI) |
/// | `dx12-warp` | Microsoft's CPU D3D12 driver (Windows CI) |
/// | `metal` | every Apple GPU, including the hosted macOS runner's paravirtual device, which renders on the host's Apple GPU |
/// | `<backend>-<adapter>` | anything else, e.g. a developer's discrete GPU, with no committed goldens |
pub fn backend_key(backend: &str, adapter: &str) -> String {
    let adapter = adapter.to_lowercase();
    match backend {
        "vulkan" if adapter.contains("llvmpipe") => "vulkan-lavapipe".to_owned(),
        "dx12" if adapter.contains("microsoft basic render driver") => "dx12-warp".to_owned(),
        "metal" => "metal".to_owned(),
        _ => {
            let slug: String =
                adapter.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
            let slug: Vec<&str> = slug.split('-').filter(|part| !part.is_empty()).collect();
            format!("{backend}-{}", slug.join("-"))
        }
    }
}

/// The side in pixels of the block [`Comparison::worst_tile`] averages over.
pub const TILE: usize = 16;

/// When a capture still matches its golden.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// The highest allowed mean FLIP error over all pixels.
    pub mean: f32,
    /// A percentile of the per-pixel errors, in (0, 100], and the highest value it may take.
    /// Edge flips stay above it; broken shading doesn't.
    pub percentile: f32,
    pub percentile_max: f32,
    /// The highest allowed mean error of any [`TILE`] × [`TILE`] block of pixels.
    pub tile_max: f32,
}

impl Default for Thresholds {
    /// A backend family renders its own golden again nearly exactly, so each gate sits just above
    /// what CI's captures measure (spec §9 has the runs): the worst mean is 0.0088, the worst p99
    /// 0.084 and the worst tile 0.122, all on CI's Metal `foliage-ground`, whose capture is the
    /// same on every run; lavapipe's worst are 0.0059, 0.057 and 0.060, and WARP's 0.0015, 0.018
    /// and 0.038. A p99 of 0.12 fails MSAA lost, a render scale of 0.7 or the bangs' soft shadow
    /// lost (they read 0.135 to 0.155). The whole-image gates can't see a small defect: a
    /// 40 px patch of wrong pixels stays under both. The tile gate fails one of 5 px, and
    /// scattered edge flips stay under it.
    fn default() -> Self {
        Self { mean: 0.01, percentile: 99.0, percentile_max: 0.12, tile_max: 0.15 }
    }
}

/// How a capture differs from its golden.
#[derive(Clone, Debug)]
pub struct Comparison {
    pub mean: f32,
    /// The [`Thresholds::percentile`] value it was compared at.
    pub percentile: f32,
    pub percentile_value: f32,
    /// The highest mean error of any [`TILE`] × [`TILE`] block of pixels, wherever it lies.
    pub worst_tile: f32,
    pub max: f32,
    /// Per-pixel FLIP error, colored with the magma map (black: identical).
    pub error_map: Image,
}

impl Comparison {
    pub fn passes(&self, thresholds: &Thresholds) -> bool {
        self.mean <= thresholds.mean
            && self.percentile_value <= thresholds.percentile_max
            && self.worst_tile <= thresholds.tile_max
    }
}

/// Compares `actual` with `golden` at `thresholds.percentile`.
///
/// Identical RGB pixels have zero perceptual error, so they skip FLIP's filtering. The same
/// pooling and color map still produce the comparison; alpha is ignored on both paths.
pub fn compare(golden: &Image, actual: &Image, thresholds: &Thresholds) -> Result<Comparison, GoldenError> {
    if (golden.width, golden.height) != (actual.width, actual.height) {
        return Err(GoldenError::SizeMismatch {
            golden: (golden.width, golden.height),
            actual: (actual.width, actual.height),
        });
    }
    let (width, height) = (golden.width, golden.height);
    let errors = if same_rgb(golden, actual) {
        nv_flip::FlipImageFloat::new(width, height)
    } else {
        let reference = nv_flip::FlipImageRgb8::with_data(width, height, &golden.rgb());
        let test = nv_flip::FlipImageRgb8::with_data(width, height, &actual.rgb());
        nv_flip::flip(reference, test, nv_flip::DEFAULT_PIXELS_PER_DEGREE)
    };
    let mut pool = nv_flip::FlipPool::from_image(&errors);
    let colored = errors.apply_color_lut(&nv_flip::magma_lut()).to_vec();
    Ok(Comparison {
        mean: pool.mean(),
        percentile: thresholds.percentile,
        percentile_value: pool.get_percentile(thresholds.percentile / 100.0, false),
        worst_tile: worst_tile(&errors.to_vec(), width as usize, height as usize),
        max: pool.max_value(),
        error_map: Image {
            width,
            height,
            rgba: colored.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect(),
        },
    })
}

/// The highest mean of `errors` (`width` × `height`, rows in order) over every [`TILE`] × [`TILE`]
/// block at every position, or over the whole image where it is smaller: a sliding window, so a
/// defect reads the same wherever it lands, where a fixed grid could split it across four tiles.
fn worst_tile(errors: &[f32], width: usize, height: usize) -> f32 {
    let (tile_width, tile_height) = (TILE.min(width), TILE.min(height));
    if tile_width == 0 || tile_height == 0 {
        return 0.0;
    }
    // Each column's sum over the last `tile_height` rows, then a run of `tile_width` of them.
    let mut columns = vec![0.0_f64; width];
    let mut worst = 0.0_f64;
    for y in 0..height {
        for (x, column) in columns.iter_mut().enumerate() {
            *column += f64::from(errors[y * width + x]);
            if y >= tile_height {
                *column -= f64::from(errors[(y - tile_height) * width + x]);
            }
        }
        if y + 1 < tile_height {
            continue;
        }
        let mut sum: f64 = columns[..tile_width].iter().sum();
        worst = worst.max(sum);
        for x in tile_width..width {
            sum += columns[x] - columns[x - tile_width];
            worst = worst.max(sum);
        }
    }
    (worst / (tile_width * tile_height) as f64) as f32
}

/// Compare only complete, nonempty images: public `Image` fields can also describe malformed
/// buffers, which must not bypass the ordinary comparison's checks just because they are equal.
fn same_rgb(golden: &Image, actual: &Image) -> bool {
    let bytes = (golden.width as usize).checked_mul(golden.height as usize).and_then(|n| n.checked_mul(4));
    if bytes == Some(0) || bytes != Some(golden.rgba.len()) || bytes != Some(actual.rgba.len()) {
        return false;
    }
    golden.rgba == actual.rgba
        || golden
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(actual.rgba.as_chunks::<4>().0)
            .all(|(a, b)| a[..3] == b[..3])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(width: u32, height: u32, cell: u32, offset: u32) -> Image {
        let rgba = (0..height)
            .flat_map(|y| (0..width).map(move |x| ((x + offset) / cell + y / cell) % 2))
            .flat_map(|on| if on == 1 { [230, 120, 40, 255] } else { [20, 30, 60, 255] })
            .collect();
        Image { width, height, rgba }
    }

    #[test]
    fn identical_images_pass_and_different_ones_fail() {
        let golden = checker(64, 48, 8, 0);
        let same = compare(&golden, &golden.clone(), &Thresholds::default()).unwrap();
        assert_eq!(same.mean, 0.0);
        assert!(same.max <= f32::MIN_POSITIVE, "FLIP's pool reports the smallest positive float for none");
        assert_eq!(same.worst_tile, 0.0);
        assert!(same.passes(&Thresholds::default()));
        assert_eq!((same.error_map.width, same.error_map.rgba.len()), (64, 64 * 48 * 4));

        let shifted = compare(&golden, &checker(64, 48, 8, 4), &Thresholds::default()).unwrap();
        assert!(shifted.mean > 0.05, "half of every cell changed: mean {}", shifted.mean);
        assert!(!shifted.passes(&Thresholds::default()));
    }

    #[test]
    fn identical_and_alpha_only_images_match_full_flip() {
        for (width, height) in [(1, 1), (7, 5), (64, 48)] {
            let golden = checker(width, height, 2, 0);
            let mut actual = golden.clone();
            for alpha in [255, 0, 127] {
                for pixel in actual.rgba.as_chunks_mut::<4>().0 {
                    pixel[3] = alpha;
                }
                let errors = nv_flip::flip(
                    nv_flip::FlipImageRgb8::with_data(width, height, &golden.rgb()),
                    nv_flip::FlipImageRgb8::with_data(width, height, &actual.rgb()),
                    nv_flip::DEFAULT_PIXELS_PER_DEGREE,
                );
                let mut pool = nv_flip::FlipPool::from_image(&errors);
                let expected: Vec<u8> = errors
                    .apply_color_lut(&nv_flip::magma_lut())
                    .to_vec()
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .flat_map(|&[r, g, b]| [r, g, b, 255])
                    .collect();
                for percentile in [1.0, 50.0, 99.0, 100.0] {
                    let thresholds = Thresholds { percentile, ..Thresholds::default() };
                    let comparison = compare(&golden, &actual, &thresholds).unwrap();
                    assert_eq!(comparison.mean.to_bits(), pool.mean().to_bits());
                    assert_eq!(comparison.max.to_bits(), pool.max_value().to_bits());
                    assert_eq!(
                        comparison.percentile_value.to_bits(),
                        pool.get_percentile(percentile / 100.0, false).to_bits(),
                    );
                    assert_eq!(comparison.worst_tile, 0.0);
                    assert!(errors.to_vec().iter().all(|&error| error == 0.0));
                    assert_eq!(comparison.error_map.rgba, expected);
                }
            }
        }
    }

    #[test]
    fn exact_comparison_keeps_single_channel_changes_and_rejects_incomplete_buffers() {
        let golden = checker(7, 5, 2, 0);
        for channel in 0..3 {
            let mut changed = golden.clone();
            let index = changed.rgba.len() - 4 + channel;
            changed.rgba[index] ^= 1;
            assert!(!same_rgb(&golden, &changed));
            let errors = nv_flip::flip(
                nv_flip::FlipImageRgb8::with_data(golden.width, golden.height, &golden.rgb()),
                nv_flip::FlipImageRgb8::with_data(changed.width, changed.height, &changed.rgb()),
                nv_flip::DEFAULT_PIXELS_PER_DEGREE,
            );
            let mut pool = nv_flip::FlipPool::from_image(&errors);
            let comparison = compare(&golden, &changed, &Thresholds::default()).unwrap();
            assert_eq!(comparison.mean.to_bits(), pool.mean().to_bits());
            assert_eq!(comparison.max.to_bits(), pool.max_value().to_bits());
            assert_eq!(comparison.percentile_value.to_bits(), pool.get_percentile(0.99, false).to_bits());
        }
        let mut truncated = golden.clone();
        truncated.rgba.pop();
        assert!(!same_rgb(&truncated, &truncated));
        let empty = Image { width: 0, height: 0, rgba: Vec::new() };
        assert!(!same_rgb(&empty, &empty));
    }

    /// `checker(200, 200, 20, 0)` with the pixels right of the first cell edge on `rows` flipped.
    fn with_flipped_edge(rows: impl Iterator<Item = usize>) -> (Image, Image) {
        let (size, cell) = (200, 20);
        let golden = checker(size, size, cell, 0);
        let mut actual = golden.clone();
        for y in rows {
            let edge = y * size as usize + cell as usize;
            actual.rgba[edge * 4..edge * 4 + 4].copy_from_slice(&golden.rgba[(edge - 1) * 4..edge * 4]);
        }
        (golden, actual)
    }

    #[test]
    fn scattered_edge_flips_pass_and_a_run_of_them_fails() {
        let thresholds = Thresholds::default();
        // 20 pixels (0.05%) spread down an edge, as precision differences flip pixels along terminators
        // and outlines: on CI's noisiest such tile, Metal's `debug-wireframe`, two flips of about 190
        // code values read 0.098. A max gate would always fail on them.
        let (golden, actual) = with_flipped_edge((0..200).step_by(10));
        let scattered = compare(&golden, &actual, &thresholds).unwrap();
        assert!(scattered.max > 0.3, "the flipped pixels are clearly different: {}", scattered.max);
        assert!(scattered.passes(&thresholds), "{}, {}", scattered.percentile_value, scattered.worst_tile);

        // A run of them is a line that moved: it stays under the whole-image gates.
        let (golden, actual) = with_flipped_edge(0..20);
        let run = compare(&golden, &actual, &thresholds).unwrap();
        assert!(run.mean <= thresholds.mean && run.percentile_value <= thresholds.percentile_max);
        assert!(run.worst_tile > thresholds.tile_max, "{}", run.worst_tile);
        assert!(!run.passes(&thresholds));
    }

    #[test]
    fn a_small_patch_fails_only_the_tile_gate() {
        let thresholds = Thresholds::default();
        let golden = Image { width: 200, height: 200, rgba: [90, 110, 160, 255].repeat(200 * 200) };
        let mut actual = golden.clone();
        for y in 100..106 {
            actual.rgba[(y * 200 + 100) * 4..(y * 200 + 106) * 4]
                .copy_from_slice(&[200, 200, 200, 255].repeat(6));
        }
        let comparison = compare(&golden, &actual, &thresholds).unwrap();
        assert!(
            comparison.mean <= thresholds.mean && comparison.percentile_value <= thresholds.percentile_max
        );
        assert!(comparison.worst_tile > thresholds.tile_max, "{}", comparison.worst_tile);
        assert!(!comparison.passes(&thresholds));
    }

    #[test]
    fn the_worst_tile_reads_a_block_the_same_wherever_it_lies() {
        let (width, height) = (64, 48);
        // Aligned with a 16 px grid, across its lines, and in the image's corners.
        for (left, top) in [(0, 0), (14, 14), (16, 16), (30, 18), (60, 0), (0, 44), (60, 44)] {
            let mut errors = vec![0.0; width * height];
            for y in top..top + 4 {
                errors[y * width + left..y * width + left + 4].fill(1.0);
            }
            assert_eq!(worst_tile(&errors, width, height), 16.0 / 256.0, "at ({left}, {top})");
        }
    }

    #[test]
    fn the_worst_tile_averages_only_inside_its_window() {
        let (width, height) = (64, 48);
        let mut errors = vec![0.25; width * height];
        // A strip one pixel short of a window's height, and a pixel over it at its other end: no
        // window holds both.
        for y in 8..23 {
            errors[y * width + 30] = 1.0;
        }
        errors[23 * width + 30 + 16] = 1.0;
        let expected = (15.0 + 241.0 * 0.25) / 256.0;
        assert!((worst_tile(&errors, width, height) - expected).abs() < 1e-6);
    }

    #[test]
    fn an_image_smaller_than_a_tile_averages_over_what_it_has() {
        assert_eq!(worst_tile(&[1.0, 0.0, 0.0, 1.0], 2, 2), 0.5);
        // Wider than a tile but shorter: the worst run of 16 columns, over its one row.
        let mut row = vec![0.0; 32];
        row[20..24].fill(1.0);
        assert_eq!(worst_tile(&row, 32, 1), 4.0 / 16.0);
        assert_eq!(worst_tile(&[], 0, 0), 0.0);
    }

    #[test]
    fn rejects_size_mismatches() {
        let error = compare(&checker(8, 8, 2, 0), &checker(8, 4, 2, 0), &Thresholds::default()).unwrap_err();
        assert!(matches!(error, GoldenError::SizeMismatch { golden: (8, 8), actual: (8, 4) }));
    }

    #[test]
    fn keys_backend_families() {
        assert_eq!(backend_key("vulkan", "llvmpipe (LLVM 19.1.7, 256 bits)"), "vulkan-lavapipe");
        assert_eq!(backend_key("dx12", "Microsoft Basic Render Driver"), "dx12-warp");
        assert_eq!(backend_key("metal", "Apple Paravirtual device"), "metal");
        assert_eq!(backend_key("metal", "Apple M5"), "metal");
        assert_eq!(backend_key("vulkan", "NVIDIA GeForce RTX 3070"), "vulkan-nvidia-geforce-rtx-3070");
    }

    #[test]
    fn png_round_trips() {
        let dir = std::env::temp_dir().join(format!("sapphire_testkit_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("image.png");
        let image = checker(16, 9, 3, 1);
        image.write_png(&path).unwrap();
        assert_eq!(Image::read_png(&path).unwrap(), image);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
