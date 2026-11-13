//! The app's icon (M4 S1): every platform's icon set, made from one master.
//!
//! The master, `platform/icon/icon.png`, is Sapphire's icon: the maintainer's own artwork, given on
//! 2026-11-13, a cluster of four faceted blue gems with a cyan outline around a white four-pointed glint, on
//! white. It's authored, not generated: a new icon replaces the file, byte for byte as it's given.
//!
//! A master must be square, at least [`MASTER_SIZE`] px, and opaque, as App Store icons must be
//! ([`read_master`] refuses anything else). Its art must sit inside the middle circle, of a diameter 72/108
//! of the side, on a flat ground that fills the rest: Android's adaptive icons show the middle 72 of 108 dp,
//! masked to a circle at their smallest, and iOS rounds the corners. A test holds the committed master to
//! these rules.
//!
//! `sapphire_pipeline icons --source <png> --out <dir>` makes every platform's set from a master, resized
//! with Lanczos3 ([`write_sets`]). Phones mask the icon themselves, so they take the square as it is.
//! Desktops show an icon unmasked, so theirs is the master on Apple's macOS grid: an 824 px rounded square,
//! corner radius 185, centered on a transparent 1024 px canvas ([`desktop_master`]). xtask compiles the Apple
//! catalogs with `actool` (and `package` makes the sets from the committed master); the rest are used as
//! written.

use std::fs;
use std::io;
use std::path::Path;

use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};

/// The master's file name, in the icon directory (`platform/icon/`).
pub const MASTER_FILE_NAME: &str = "icon.png";
/// The master's side, and the least a replacement may have: App Store Connect's icon is 1024 px.
pub const MASTER_SIZE: u32 = 1024;
/// Samples per pixel along each axis, for the desktop body's antialiased edge.
const SAMPLES: u32 = 4;

/// Why a master can't make icons.
#[derive(Debug, thiserror::Error)]
pub enum IconError {
    #[error("failed to read the icon {path}")]
    Read {
        path: String,
        #[source]
        source: image::ImageError,
    },
    #[error("the icon is {width}×{height}: it must be square, at least {MASTER_SIZE} px")]
    Size { width: u32, height: u32 },
    #[error("the icon has transparent pixels: App Store icons must be opaque, and phones mask the square")]
    Transparent,
    #[error("failed to write {path}")]
    Write {
        path: String,
        #[source]
        source: io::Error,
    },
}

/// Reads a master: square, at least [`MASTER_SIZE`], and opaque.
pub fn read_master(path: &Path) -> Result<RgbImage, IconError> {
    let read = |source| IconError::Read { path: path.display().to_string(), source };
    let image = image::ImageReader::open(path)
        .map_err(|source| read(image::ImageError::IoError(source)))?
        .with_guessed_format()
        .map_err(|source| read(image::ImageError::IoError(source)))?
        .decode()
        .map_err(read)?;
    let (width, height) = (image.width(), image.height());
    if width != height || width < MASTER_SIZE {
        return Err(IconError::Size { width, height });
    }
    if image.color().has_alpha() && image.to_rgba8().pixels().any(|pixel| pixel[3] != 255) {
        return Err(IconError::Transparent);
    }
    Ok(image.to_rgb8())
}

/// The Apple catalogs' sizes: (point size, scale) for macOS, as Xcode's mac app icon set lists them.
const MAC_SIZES: [(u32, u32); 10] =
    [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)];
/// Android's densities, and the adaptive icon's 108 dp layer at each.
const ANDROID_DENSITIES: [(&str, u32); 5] =
    [("mdpi", 108), ("hdpi", 162), ("xhdpi", 216), ("xxhdpi", 324), ("xxxhdpi", 432)];
/// The Windows icon's sizes: what Explorer and the taskbar ask for at 100 to 400 % scale.
const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
/// Google Play's store listing icon.
const PLAY_SIZE: u32 = 512;
/// The Linux icon (hicolor's largest common raster size), and Steam's library one.
const LINUX_SIZE: u32 = 512;

/// Every platform's icon set from `master`, by path under the output directory:
///
/// | Path | What |
/// |---|---|
/// | `ios/Assets.xcassets/` | An asset catalog with one 1024 px universal icon, `AppIcon` |
/// | `macos/Assets.xcassets/` | An asset catalog with `AppIcon`'s ten mac images, 16 to 1024 px, on the desktop grid |
/// | `android/res/mipmap-*/ic_launcher_background.png`, `android/res/mipmap-anydpi-v26/ic_launcher.xml` | An adaptive icon: the master as its background layer at each density, and a transparent foreground |
/// | `android/play_store_512.png` | Play's listing icon, which Play Console takes as an upload |
/// | `windows/app.ico` | Seven sizes, 16 to 256 px, each a PNG, on the desktop grid |
/// | `linux/icon_512.png` | 512 px, on the desktop grid |
pub fn sets(master: &RgbImage) -> io::Result<Vec<(String, Vec<u8>)>> {
    let square = |side: u32| imageops::resize(master, side, side, FilterType::Lanczos3);
    let desktop = desktop_master(master);
    let desktop_at = |side: u32| resize_straight(&desktop, side);
    let mut files = vec![
        ("ios/Assets.xcassets/Contents.json".to_owned(), CATALOG_CONTENTS.as_bytes().to_vec()),
        ("ios/Assets.xcassets/AppIcon.appiconset/Contents.json".to_owned(), ios_contents().into_bytes()),
        (
            "ios/Assets.xcassets/AppIcon.appiconset/icon_1024.png".to_owned(),
            encode(square(MASTER_SIZE), ImageFormat::Png)?,
        ),
    ];

    files.push(("macos/Assets.xcassets/Contents.json".to_owned(), CATALOG_CONTENTS.as_bytes().to_vec()));
    files.push((
        "macos/Assets.xcassets/AppIcon.appiconset/Contents.json".to_owned(),
        mac_contents().into_bytes(),
    ));
    for (points, scale) in MAC_SIZES {
        let name = mac_file_name(points, scale);
        let path = format!("macos/Assets.xcassets/AppIcon.appiconset/{name}");
        files.push((path, encode(desktop_at(points * scale), ImageFormat::Png)?));
    }

    for (density, side) in ANDROID_DENSITIES {
        let path = format!("android/res/mipmap-{density}/ic_launcher_background.png");
        files.push((path, encode(square(side), ImageFormat::Png)?));
    }
    files.push((
        "android/res/mipmap-anydpi-v26/ic_launcher.xml".to_owned(),
        ADAPTIVE_ICON.as_bytes().to_vec(),
    ));
    files.push(("android/play_store_512.png".to_owned(), encode(square(PLAY_SIZE), ImageFormat::Png)?));

    let ico_images = ICO_SIZES
        .iter()
        .map(|&side| encode(desktop_at(side), ImageFormat::Png))
        .collect::<io::Result<Vec<_>>>()?;
    files.push(("windows/app.ico".to_owned(), ico(&ICO_SIZES, &ico_images)));
    files.push((format!("linux/icon_{LINUX_SIZE}.png"), encode(desktop_at(LINUX_SIZE), ImageFormat::Png)?));
    Ok(files)
}

/// Writes [`sets`] of the master at `source` under `out`, replacing what's there.
pub fn write_sets(source: &Path, out: &Path) -> Result<usize, IconError> {
    let master = read_master(source)?;
    let write = |path: &Path, source| IconError::Write { path: path.display().to_string(), source };
    let files = sets(&master).map_err(|source| write(out, source))?;
    for (name, bytes) in &files {
        let path = out.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| write(parent, source))?;
        }
        fs::write(&path, bytes).map_err(|source| write(&path, source))?;
    }
    Ok(files.len())
}

/// Apple's macOS icon grid, in 1024 px units: the body's side, its offset and its corner radius.
const DESKTOP_BODY: f32 = 824.0;
const DESKTOP_OFFSET: f32 = 100.0;
const DESKTOP_RADIUS: f32 = 185.0;

/// The master on the desktop grid: resized into the rounded body, transparent outside it, its edge
/// antialiased with 4×4 samples. Straight (not premultiplied) alpha, as PNGs hold it.
pub fn desktop_master(master: &RgbImage) -> RgbaImage {
    let body = DESKTOP_BODY as u32;
    let inner = imageops::resize(master, body, body, FilterType::Lanczos3);
    let offset = DESKTOP_OFFSET as u32;
    RgbaImage::from_fn(MASTER_SIZE, MASTER_SIZE, |x, y| {
        let mut covered = 0;
        for sy in 0..SAMPLES {
            for sx in 0..SAMPLES {
                let px = x as f32 + (sx as f32 + 0.5) / SAMPLES as f32;
                let py = y as f32 + (sy as f32 + 0.5) / SAMPLES as f32;
                covered += u32::from(in_rounded_body(px, py));
            }
        }
        if covered == 0 {
            return Rgba([0, 0, 0, 0]);
        }
        // Edge pixels take the nearest body pixel's color, so resizing has no dark fringe to blend in.
        let ix = x.clamp(offset, offset + body - 1) - offset;
        let iy = y.clamp(offset, offset + body - 1) - offset;
        let Rgb([r, g, b]) = *inner.get_pixel(ix, iy);
        let alpha = (covered * 255 + (SAMPLES * SAMPLES) / 2) / (SAMPLES * SAMPLES);
        Rgba([r, g, b, alpha as u8])
    })
}

fn in_rounded_body(x: f32, y: f32) -> bool {
    let (low, high) = (DESKTOP_OFFSET, DESKTOP_OFFSET + DESKTOP_BODY);
    if x < low || x > high || y < low || y > high {
        return false;
    }
    let corner = |v: f32| {
        if v < low + DESKTOP_RADIUS {
            low + DESKTOP_RADIUS - v
        } else if v > high - DESKTOP_RADIUS {
            v - (high - DESKTOP_RADIUS)
        } else {
            0.0
        }
    };
    let (dx, dy) = (corner(x), corner(y));
    dx * dx + dy * dy <= DESKTOP_RADIUS * DESKTOP_RADIUS
}

/// Resizes an image with straight alpha through premultiplied alpha, so transparent pixels' color
/// doesn't bleed into the edge.
fn resize_straight(image: &RgbaImage, side: u32) -> RgbaImage {
    if image.width() == side {
        return image.clone();
    }
    let premultiplied =
        image::ImageBuffer::<Rgba<f32>, Vec<f32>>::from_fn(image.width(), image.height(), |x, y| {
            let Rgba([r, g, b, a]) = *image.get_pixel(x, y);
            let alpha = f32::from(a) / 255.0;
            Rgba([
                f32::from(r) / 255.0 * alpha,
                f32::from(g) / 255.0 * alpha,
                f32::from(b) / 255.0 * alpha,
                alpha,
            ])
        });
    let resized = imageops::resize(&premultiplied, side, side, FilterType::Lanczos3);
    RgbaImage::from_fn(side, side, |x, y| {
        let Rgba([r, g, b, a]) = *resized.get_pixel(x, y);
        let alpha = a.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return Rgba([0, 0, 0, 0]);
        }
        let channel = |c: f32| ((c / alpha).clamp(0.0, 1.0) * 255.0).round() as u8;
        Rgba([channel(r), channel(g), channel(b), (alpha * 255.0).round() as u8])
    })
}

fn encode(image: impl Into<DynamicImage>, format: ImageFormat) -> io::Result<Vec<u8>> {
    let mut bytes = io::Cursor::new(Vec::new());
    image.into().write_to(&mut bytes, format).map_err(io::Error::other)?;
    Ok(bytes.into_inner())
}

/// An `.ico` holding `images`, each a PNG `sizes[i]` px square (Windows Vista and later read PNG
/// entries at every size). A side of 256 is written as 0, as the format has it.
fn ico(sizes: &[u32], images: &[Vec<u8>]) -> Vec<u8> {
    const HEADER: usize = 6;
    const ENTRY: usize = 16;
    let count = u16::try_from(images.len()).expect("a handful of sizes");
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]); // Reserved, then type 1: an icon.
    out.extend_from_slice(&count.to_le_bytes());
    let mut offset = HEADER + ENTRY * images.len();
    for (&side, image) in sizes.iter().zip(images) {
        let side = if side >= 256 { 0 } else { side as u8 };
        out.extend_from_slice(&[side, side, 0, 0]); // Width, height, no palette, reserved.
        out.extend_from_slice(&1u16.to_le_bytes()); // Color planes.
        out.extend_from_slice(&32u16.to_le_bytes()); // Bits per pixel.
        out.extend_from_slice(&u32::try_from(image.len()).expect("small").to_le_bytes());
        out.extend_from_slice(&u32::try_from(offset).expect("small").to_le_bytes());
        offset += image.len();
    }
    for image in images {
        out.extend_from_slice(image);
    }
    out
}

/// An asset catalog's own `Contents.json`.
const CATALOG_CONTENTS: &str =
    "{\n  \"info\" : {\n    \"author\" : \"xcode\",\n    \"version\" : 1\n  }\n}\n";

/// The iOS icon set: one 1024 px image for every size, which Xcode 14 and later accept.
fn ios_contents() -> String {
    "{\n  \"images\" : [\n    {\n      \"filename\" : \"icon_1024.png\",\n      \"idiom\" : \"universal\",\n      \
     \"platform\" : \"ios\",\n      \"size\" : \"1024x1024\"\n    }\n  ],\n  \"info\" : {\n    \"author\" : \
     \"xcode\",\n    \"version\" : 1\n  }\n}\n"
        .to_owned()
}

fn mac_file_name(points: u32, scale: u32) -> String {
    if scale == 1 { format!("icon_{points}.png") } else { format!("icon_{points}@{scale}x.png") }
}

/// The macOS icon set: ten images, each a size at a scale.
fn mac_contents() -> String {
    let images: Vec<String> = MAC_SIZES
        .iter()
        .map(|&(points, scale)| {
            format!(
                "    {{\n      \"filename\" : \"{}\",\n      \"idiom\" : \"mac\",\n      \"scale\" : \"{scale}x\",\n      \
                 \"size\" : \"{points}x{points}\"\n    }}",
                mac_file_name(points, scale)
            )
        })
        .collect();
    format!(
        "{{\n  \"images\" : [\n{}\n  ],\n  \"info\" : {{\n    \"author\" : \"xcode\",\n    \"version\" : 1\n  }}\n}}\n",
        images.join(",\n")
    )
}

/// Android's adaptive icon: the master is the background layer, and the foreground is empty, since the
/// master is one picture with nothing to lift apart for the launcher's parallax.
const ADAPTIVE_ICON: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!-- Made by `sapphire_pipeline icons` from platform/icon/icon.png (M4 S1). -->
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@mipmap/ic_launcher_background" />
    <foreground android:drawable="@android:color/transparent" />
</adaptive-icon>
"#;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn icon_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../platform/icon")
    }

    /// The committed master is one every store takes, and no platform's mask cuts its art: every pixel
    /// whose center lies outside the circle Android keeps at its smallest (the middle 72 of 108 dp, so a
    /// diameter of 2/3 of the side) is exactly the ground, the color of the corner pixel.
    #[test]
    fn the_committed_master_is_taken_by_every_store_and_kept_whole_by_every_mask() {
        let image = read_master(&icon_dir().join(MASTER_FILE_NAME)).unwrap();
        let side = image.width();
        assert_eq!(image.dimensions(), (MASTER_SIZE, MASTER_SIZE));
        let ground = *image.get_pixel(0, 0);
        // In half pixels from the center, so the test is exact in integers: a pixel center is at
        // (2x + 1 - side) / 2, and the circle's radius is side / 3, so outside is 9 d² > 4 side².
        let outside = |x: u32, y: u32| {
            let (dx, dy) = (i64::from(2 * x + 1) - i64::from(side), i64::from(2 * y + 1) - i64::from(side));
            9 * (dx * dx + dy * dy) > 4 * i64::from(side) * i64::from(side)
        };
        let (mut cut, mut art) = (Vec::new(), 0);
        for (x, y, pixel) in image.enumerate_pixels() {
            if *pixel != ground {
                art += 1;
                if outside(x, y) {
                    cut.push((x, y));
                }
            }
        }
        assert!(
            cut.is_empty(),
            "{} art pixels lie outside the circle every mask keeps, such as {:?}",
            cut.len(),
            &cut[..cut.len().min(8)]
        );
        assert!(art > 0, "the master is all ground");
    }

    #[test]
    fn refuses_masters_no_store_takes() {
        let dir = std::env::temp_dir().join(format!("icon-masters-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let small = dir.join("small.png");
        RgbImage::new(512, 512).save(&small).unwrap();
        assert!(matches!(read_master(&small), Err(IconError::Size { width: 512, .. })));
        let wide = dir.join("wide.png");
        RgbImage::new(1024, 1000).save(&wide).unwrap();
        assert!(matches!(read_master(&wide), Err(IconError::Size { .. })));
        let clear = dir.join("clear.png");
        RgbaImage::from_pixel(1024, 1024, Rgba([10, 20, 30, 254])).save(&clear).unwrap();
        assert!(matches!(read_master(&clear), Err(IconError::Transparent)));
        // Opaque RGBA is fine.
        let opaque = dir.join("opaque.png");
        RgbaImage::from_pixel(1024, 1024, Rgba([10, 20, 30, 255])).save(&opaque).unwrap();
        assert!(read_master(&opaque).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Every set has its files at their sizes, and the desktop's are transparent outside the rounded body.
    #[test]
    fn makes_every_platforms_set() {
        // A small flat master keeps the test quick; sizes come from the sets, not the master.
        let files = sets(&RgbImage::from_pixel(MASTER_SIZE, MASTER_SIZE, Rgb([40, 80, 200]))).unwrap();
        let file = |name: &str| {
            &files.iter().find(|(path, _)| path == name).unwrap_or_else(|| panic!("no {name}")).1
        };
        let png = |name: &str| image::load_from_memory(file(name)).unwrap();
        assert_eq!(png("ios/Assets.xcassets/AppIcon.appiconset/icon_1024.png").width(), 1024);
        assert!(!png("ios/Assets.xcassets/AppIcon.appiconset/icon_1024.png").color().has_alpha());
        for (points, scale) in MAC_SIZES {
            let image =
                png(&format!("macos/Assets.xcassets/AppIcon.appiconset/{}", mac_file_name(points, scale)));
            assert_eq!(image.width(), points * scale);
            let rgba = image.to_rgba8();
            assert_eq!(rgba.get_pixel(0, 0)[3], 0, "the corner is transparent at {points}@{scale}x");
            let middle = rgba.get_pixel(image.width() / 2, image.height() / 2);
            assert_eq!(*middle, Rgba([40, 80, 200, 255]), "{points}@{scale}x");
        }
        let contents: serde_json::Value =
            serde_json::from_slice(file("macos/Assets.xcassets/AppIcon.appiconset/Contents.json")).unwrap();
        assert_eq!(contents["images"].as_array().unwrap().len(), MAC_SIZES.len());
        let _: serde_json::Value =
            serde_json::from_slice(file("ios/Assets.xcassets/AppIcon.appiconset/Contents.json")).unwrap();
        for (density, side) in ANDROID_DENSITIES {
            assert_eq!(
                png(&format!("android/res/mipmap-{density}/ic_launcher_background.png")).width(),
                side
            );
        }
        assert!(
            String::from_utf8_lossy(file("android/res/mipmap-anydpi-v26/ic_launcher.xml"))
                .contains("adaptive-icon")
        );
        assert_eq!(png("android/play_store_512.png").width(), PLAY_SIZE);
        assert_eq!(png("linux/icon_512.png").width(), LINUX_SIZE);

        // The icon file's directory, read back: each entry a PNG of its size.
        let ico = file("windows/app.ico");
        assert_eq!(&ico[..6], &[0, 0, 1, 0, ICO_SIZES.len() as u8, 0]);
        for (i, &side) in ICO_SIZES.iter().enumerate() {
            let entry = &ico[6 + 16 * i..6 + 16 * (i + 1)];
            assert_eq!(u32::from(entry[0]), side % 256);
            let size = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
            let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
            let image = image::load_from_memory(&ico[offset..offset + size]).unwrap();
            assert_eq!(image.width(), side);
        }
    }
}
