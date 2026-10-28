//! Golden images (spec §9 gate 3; M1 plan §3, open question Q6).
//!
//! A capture is compared with the reference image for its **backend family** ([`backend_key`]):
//! CPU rasterizers and GPUs disagree in the last bit, so each family keeps its own goldens.
//! Comparison uses NVIDIA's FLIP, a perceptual per-pixel error in [0, 1], through `nv-flip`
//! (as wgpu's own tests do), and gates on the mean and a high percentile rather than the
//! maximum: two-tone shading's hard terminators and outlines turn tiny precision differences into
//! whole-pixel flips along edges, which a max would always fail on.

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

/// When a capture still matches its golden.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// The highest allowed mean FLIP error over all pixels.
    pub mean: f32,
    /// A percentile of the per-pixel errors, in (0, 100], and the highest value it may take.
    /// Edge flips stay above it; broken shading doesn't.
    pub percentile: f32,
    pub percentile_max: f32,
}

impl Default for Thresholds {
    /// A backend family renders its own golden again nearly exactly, so the mean gate is
    /// tight. FLIP spreads each pixel's error over its neighbors (it models what the eye
    /// sees), so a handful of flipped edge pixels lifts p99 to about 0.2; 0.3 still fails a
    /// shifted shading band or a one-pixel change in outline width.
    fn default() -> Self {
        Self { mean: 0.01, percentile: 99.0, percentile_max: 0.3 }
    }
}

/// How a capture differs from its golden.
#[derive(Clone, Debug)]
pub struct Comparison {
    pub mean: f32,
    /// The [`Thresholds::percentile`] value it was compared at.
    pub percentile: f32,
    pub percentile_value: f32,
    pub max: f32,
    /// Per-pixel FLIP error, colored with the magma map (black: identical).
    pub error_map: Image,
}

impl Comparison {
    pub fn passes(&self, thresholds: &Thresholds) -> bool {
        self.mean <= thresholds.mean && self.percentile_value <= thresholds.percentile_max
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
        max: pool.max_value(),
        error_map: Image {
            width,
            height,
            rgba: colored.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect(),
        },
    })
}

/// Compare only complete, nonempty images: public `Image` fields can also describe malformed
/// buffers, which must not bypass the ordinary comparison's checks just because they are equal.
fn same_rgb(golden: &Image, actual: &Image) -> bool {
    let bytes = (golden.width as usize).checked_mul(golden.height as usize).and_then(|n| n.checked_mul(4));
    if bytes == Some(0) || bytes != Some(golden.rgba.len()) || bytes != Some(actual.rgba.len()) {
        return false;
    }
    golden.rgba == actual.rgba
        || golden.rgba.chunks_exact(4).zip(actual.rgba.chunks_exact(4)).all(|(a, b)| a[..3] == b[..3])
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
                for pixel in actual.rgba.chunks_exact_mut(4) {
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

    #[test]
    fn a_few_edge_pixels_pass_the_percentile_gate() {
        let (size, cell) = (200, 20);
        let golden = checker(size, size, cell, 0);
        let mut actual = golden.clone();
        // Flip 20 pixels along one cell edge (0.05%), as precision differences do on terminators.
        for y in 0..20usize {
            let edge = y * size as usize + cell as usize;
            actual.rgba[edge * 4..edge * 4 + 4].copy_from_slice(&golden.rgba[(edge - 1) * 4..edge * 4]);
        }
        let comparison = compare(&golden, &actual, &Thresholds::default()).unwrap();
        assert!(comparison.max > 0.3, "the flipped pixels are clearly different");
        assert!(comparison.passes(&Thresholds::default()), "{comparison:?}");
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
