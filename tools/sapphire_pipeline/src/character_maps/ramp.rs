//! Ramps: shadow-color gradients per material ID, authored as color stops or painted as image
//! strips, and baked to rows.
//!
//! The anime character shader's ramp mode (Genshin, Star Rail, WuWa) takes the shadow side's
//! color from a ramp row picked by material ID, with a day and a night row that time of day
//! blends between (spec §5.12). A `.ramp.ron` document lists each material's ramps, and the bake
//! writes them into the row layout of `sapphire_assets::character_maps`: day rows for IDs 0 to 7,
//! then night rows, each [`RampBakeSettings::row_height`] texels tall and
//! [`RampBakeSettings::width`] wide. A ramp is color stops, `Stops((stops: […]))`, or one row of
//! a painted image, `Image((path: "skin_ramp.png", row: 0))`.
//!
//! - **Stops** sit at positions `at` from 0 (the deepest shadow) to 1 (the lit edge of the
//!   terminator), in order; two stops at one position make a hard edge. Before the first stop
//!   and after the last, their colors hold.
//! - **Colors** are 8-bit sRGB as artists pick them, `"#e3a184"`, with an optional linear
//!   alpha, `"#e3a184c0"`.
//! - **Interpolation** between a stop and the next is the stop's own, else its ramp's, else
//!   the document's, which defaults to OkLab: equal steps along the ramp look like equal
//!   steps of lightness and color, where blending the encoded sRGB values darkens the middle
//!   of a warm-to-cool shadow and blending linear light washes it out. `Linear` and `Srgb`
//!   are those two blends (`Srgb` matches an image editor's gradient), and `Step` holds a
//!   stop's color up to the next stop, for banded ramps.
//! - **Images** (M6 C7): a row of a PNG, TGA or EXR, named relative to the document as a mask
//!   is (the [parent module](super)'s "Inputs"), so it is part of the ramp's asset and a
//!   dependency of its cache key: editing the strip recooks the ramps that read it, and nothing
//!   else. Its RGB is sRGB-encoded color and its alpha linear, as a painted texture's are. Its
//!   texels are stops at their centers, `(i + 0.5) / image width`, blended in the document's
//!   interpolation, so a strip as wide as the bake copies texel for texel and a narrower one is
//!   blended between its texels as stops are. A strip wider than the bake, which would skip
//!   painted texels, and a row past the image's last are errors naming the ramp; one image may
//!   hold the rows of many ramps.
//! - **Rows** come from materials in order: the first is ID 0. A material without a night
//!   ramp uses its day ramp for both, and IDs past the last material repeat the last one, so
//!   any ID the shader reads (stray ones included, where filtering blends IDs) finds a ramp.
//!
//! Each texel is the ramp evaluated at its center, `u = (x + 0.5) / width`, so linear
//! filtering between texel centers follows the ramp.
//!
//! # Versions
//!
//! Version 2 (M6 C7) writes each ramp as `Stops((…))` or `Image((…))`. A version 1 document
//! (M1 A7), whose ramps are all stops written as `(stops: […])`, reads as it did and bakes the
//! same rows. Every committed document is at version 2 since M6 T4, which moved their keys and
//! none of their rows. This importer's stage carries the image importer's version, as the stages
//! that decode masks do ([`super::with_image_importer`]), so a change to the decoder recooks every
//! image ramp; folding it in (after C7's review) moved every ramp's key once, and none of its rows.
//!
//! An image ramp has no interpolation of its own: its texels blend in the document's, so a
//! hard-banded strip narrower than the bake softens at each band edge in a smooth document. Paint
//! it at the bake's width, or set the document to `Step`.
//!
//! # Where the types live
//!
//! The document's types and the checks that need nothing else are `sapphire_assets::ramp`'s (M6
//! T4), so the editor opens a ramp without linking the cooker; they're re-exported here. What needs
//! the cook's color math stays: [`Linear`] and [`Evaluate`], and the bake.

use std::collections::BTreeMap;

use sapphire_assets::character_maps::MaterialId;
use sapphire_assets::character_maps::ramp::{Half, ROWS, row};
pub use sapphire_assets::ramp::{
    ColorStop, ImageRamp, Interpolation, InvalidColor, MaterialRamps, Ramp, RampError, RampMap, RampName,
    SrgbColor, StopRamp, v1,
};
use sapphire_assets::texture::MAX_TEXTURE_DIMENSION;
use serde::{Deserialize, Serialize};

use super::{GeneratedMap, Inputs, MapError, with_image_importer};
use crate::cache::Stage;
use crate::color::{linear_srgb_to_oklab, linear_to_srgb, oklab_to_linear_srgb, srgb_to_linear};
use crate::import::image::{ChannelLayout, SourceImage, Transfer};
use crate::texture::{RampSettings, TextureClass};

/// This importer, as cache keys know it: reading the document and baking its rows, with the image
/// importer's version folded in ([`with_image_importer`]), as the face SDF's is: an image ramp's rows
/// are decoded by it. Bump the version whenever the rows change for the same document.
/// 3: the image importer's version folded in (M6 C7, fixed after its review), which moved every
/// ramp's key and none of its rows. 2: colors decode and encode through `sapphire_math::det` (M4 C3).
pub const IMPORTER: Stage = Stage { name: "ramp", version: with_image_importer(3) };

/// A color in linear light, with its alpha, by the cook's deterministic decode.
pub trait Linear {
    fn to_linear(self) -> [f32; 4];
}

impl Linear for SrgbColor {
    fn to_linear(self) -> [f32; 4] {
        let [r, g, b, a] = self.0.map(|v| f32::from(v) / 255.0);
        [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a]
    }
}

/// Import settings for ramp documents: the size of the baked rows, and how they cook.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RampBakeSettings {
    /// Texels along `u`.
    pub width: u32,
    /// Texels per row. At least 2 keeps bilinear filtering at a row's center inside the row
    /// even with some error in `v`.
    pub row_height: u32,
    /// Off by default: past the level where rows are one texel tall, mips blend materials.
    pub mips: bool,
}

impl Default for RampBakeSettings {
    fn default() -> Self {
        Self { width: 256, row_height: 4, mips: false }
    }
}

/// A ramp of stops evaluated as the bake evaluates it, by the cook's color math.
pub trait Evaluate {
    /// The color at `u`, in linear light with alpha, blending with `default` where neither the
    /// stop nor the ramp says how.
    fn evaluate(&self, u: f32, default: Interpolation) -> [f32; 4];
}

impl Evaluate for StopRamp {
    fn evaluate(&self, u: f32, default: Interpolation) -> [f32; 4] {
        let stops = &self.stops;
        // The last stop at or before `u`: with two stops at one position, the second wins
        // from there on.
        let Some(i) = stops.iter().rposition(|stop| stop.at <= u) else {
            return stops[0].color.to_linear();
        };
        let Some(next) = stops.get(i + 1) else {
            return stops[i].color.to_linear();
        };
        let stop = &stops[i];
        let t = (u - stop.at) / (next.at - stop.at);
        let interpolation = stop.interpolation.or(self.interpolation).unwrap_or(default);
        blend(stop.color.to_linear(), next.color.to_linear(), t, interpolation)
    }
}

/// Linear `la` blended toward linear `lb` by `t`, with alpha. Alpha always blends linearly,
/// except under `Step`.
fn blend(la: [f32; 4], lb: [f32; 4], t: f32, interpolation: Interpolation) -> [f32; 4] {
    let mix = |x: f32, y: f32| x + (y - x) * t;
    let alpha = mix(la[3], lb[3]);
    let rgb = |c: [f32; 4]| [c[0], c[1], c[2]];
    let [r, g, b] = match interpolation {
        Interpolation::Step => return la,
        Interpolation::Linear => [0, 1, 2].map(|c| mix(la[c], lb[c])),
        Interpolation::Srgb => {
            [0, 1, 2].map(|c| srgb_to_linear(mix(linear_to_srgb(la[c]), linear_to_srgb(lb[c]))))
        }
        Interpolation::OkLab => {
            let (ka, kb) = (linear_srgb_to_oklab(rgb(la)), linear_srgb_to_oklab(rgb(lb)));
            oklab_to_linear_srgb([0, 1, 2].map(|c| mix(ka[c], kb[c])))
        }
    };
    // OkLab blends of in-gamut colors can leave the gamut slightly; the RGBA8 cook clamps,
    // and so does this, so previews and cooks agree.
    [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0), alpha]
}

/// A ramp ready to bake: its stops, or its image's row read.
enum Baking<'a> {
    Stops(&'a StopRamp),
    /// The row's texels, linear with alpha, at most as many as the bake is wide.
    Texels(Vec<[f32; 4]>),
}

impl Baking<'_> {
    /// Texel `x` of a row `width` texels wide: the ramp at the texel's center, blended with
    /// `default`, the document's interpolation, where nothing else says how.
    fn texel(&self, x: u32, width: u32, default: Interpolation) -> [f32; 4] {
        match self {
            Self::Stops(stops) => stops.evaluate((x as f32 + 0.5) / width as f32, default),
            Self::Texels(texels) => {
                // The center's place among the image's texel centers, in image texels: exact when
                // the widths are equal, so the strip is copied texel for texel.
                let at = (f64::from(x) + 0.5) * texels.len() as f64 / f64::from(width) - 0.5;
                let i = at.max(0.0).floor() as usize;
                let t = (at - i as f64) as f32;
                match texels.get(i + 1) {
                    Some(&next) if t > 0.0 => blend(texels[i], next, t, default),
                    _ => texels[i.min(texels.len() - 1)],
                }
            }
        }
    }
}

/// The images a document's ramps read, each decoded once.
#[derive(Default)]
struct Images(BTreeMap<String, SourceImage>);

impl Images {
    /// `ramp` ready to bake `width` texels wide; `name` names it for errors.
    fn bake<'a>(
        &mut self,
        ramp: &'a Ramp,
        inputs: &Inputs<'_>,
        width: u32,
        name: impl Fn() -> RampName,
    ) -> Result<Baking<'a>, MapError> {
        Ok(match ramp {
            Ramp::Stops(stops) => Baking::Stops(stops),
            Ramp::Image(image) => Baking::Texels(self.row(inputs, image, width, name)?),
        })
    }

    /// The row `image` names, checked against the bake's `width`; `ramp` names the ramp for errors.
    fn row(
        &mut self,
        inputs: &Inputs<'_>,
        image: &ImageRamp,
        width: u32,
        ramp: impl Fn() -> RampName,
    ) -> Result<Vec<[f32; 4]>, MapError> {
        if !self.0.contains_key(&image.path) {
            // Painted colors, as a color texture's: sRGB-encoded RGB, linear alpha.
            let (_, decoded) = inputs.image_as(&image.path, Transfer::Srgb)?;
            self.0.insert(image.path.clone(), decoded);
        }
        let decoded = &self.0[&image.path];
        let path = image.path.clone();
        if image.row >= decoded.height {
            let (row, height) = (image.row, decoded.height);
            return Err(RampError::ImageRow { ramp: ramp(), path, row, height }.into());
        }
        if decoded.width > width {
            return Err(
                RampError::ImageWidth { ramp: ramp(), path, width: decoded.width, bake: width }.into()
            );
        }
        let start = image.row as usize * decoded.width as usize;
        Ok(decoded.pixels[start..start + decoded.width as usize].to_vec())
    }
}

/// Bakes `map`'s rows, reading the images its ramps name through `inputs`.
pub(super) fn generate(
    map: &RampMap,
    settings: &RampBakeSettings,
    inputs: &Inputs<'_>,
) -> Result<GeneratedMap, MapError> {
    map.validate()?;
    let RampBakeSettings { width, row_height, mips } = *settings;
    let height = row_height.saturating_mul(ROWS);
    if !(1..=MAX_TEXTURE_DIMENSION).contains(&width) || row_height == 0 || height > MAX_TEXTURE_DIMENSION {
        return Err(RampError::Size { width, row_height }.into());
    }
    let mut images = Images::default();
    let mut materials = Vec::with_capacity(map.materials.len());
    for (material, ramps) in map.materials.iter().enumerate() {
        let name = |half| move || RampName { material, name: ramps.name.clone(), half };
        let day = images.bake(&ramps.day, inputs, width, name(Half::Day))?;
        let night = ramps.night.as_ref().map(|night| images.bake(night, inputs, width, name(Half::Night)));
        materials.push((day, night.transpose()?));
    }
    let mut rows = vec![Vec::new(); ROWS as usize];
    for half in [Half::Day, Half::Night] {
        for id in MaterialId::ALL {
            // IDs past the list repeat the last material; a missing night ramp is the day's.
            let (day, night) = &materials[usize::from(id.get()).min(materials.len() - 1)];
            let ramp = match half {
                Half::Day => day,
                Half::Night => night.as_ref().unwrap_or(day),
            };
            rows[row(id, half) as usize] =
                (0..width).map(|x| ramp.texel(x, width, map.interpolation)).collect();
        }
    }
    let pixels =
        rows.iter().flat_map(|texels| (0..row_height).flat_map(move |_| texels.iter().copied())).collect();
    let image = SourceImage::new(width, height, ChannelLayout::Rgba, pixels).expect("one texel per pixel");
    Ok(GeneratedMap { image, class: TextureClass::Ramp(RampSettings { mips }) })
}

#[cfg(test)]
mod tests {
    use sapphire_assets::character_maps::MATERIAL_IDS;

    use super::*;
    use crate::color::unorm8;

    fn color(text: &str) -> SrgbColor {
        text.parse().unwrap()
    }

    fn stop(at: f32, text: &str) -> ColorStop {
        ColorStop { at, color: color(text), interpolation: None }
    }

    fn ramp(stops: Vec<ColorStop>) -> StopRamp {
        StopRamp { interpolation: None, stops }
    }

    /// Bakes a document that names no images.
    fn bake(map: &RampMap, settings: &RampBakeSettings) -> Result<GeneratedMap, MapError> {
        generate(map, settings, &Inputs { document: std::path::Path::new("stops.ramp.ron") })
    }

    /// 8-bit sRGB of a linear color, as the cook stores it.
    fn stored([r, g, b, a]: [f32; 4]) -> [u8; 4] {
        [unorm8(linear_to_srgb(r)), unorm8(linear_to_srgb(g)), unorm8(linear_to_srgb(b)), unorm8(a)]
    }

    #[test]
    fn colors_parse_and_print_as_hex() {
        assert_eq!(color("#e3a184"), SrgbColor([0xe3, 0xa1, 0x84, 0xff]));
        assert_eq!(color("#E3A184C0"), SrgbColor([0xe3, 0xa1, 0x84, 0xc0]));
        assert_eq!(color("#e3a184").to_string(), "#e3a184");
        assert_eq!(color("#e3a18400").to_string(), "#e3a18400");
        for bad in ["e3a184", "#e3a18", "#e3a1841", "#g3a184", "#é3a184", ""] {
            assert_eq!(bad.parse::<SrgbColor>(), Err(InvalidColor(bad.to_owned())), "{bad:?}");
        }
        let linear = color("#808080").to_linear();
        assert!((linear[0] - 0.215_860_5).abs() < 1e-6 && linear[3] == 1.0);
    }

    #[test]
    fn stops_hold_at_the_ends_and_blend_between() {
        let r = ramp(vec![stop(0.25, "#000000"), stop(0.75, "#ffffff")]);
        for interpolation in
            [Interpolation::OkLab, Interpolation::Linear, Interpolation::Srgb, Interpolation::Step]
        {
            assert_eq!(stored(r.evaluate(0.0, interpolation)), [0, 0, 0, 255]);
            assert_eq!(stored(r.evaluate(0.25, interpolation)), [0, 0, 0, 255]);
            assert_eq!(stored(r.evaluate(1.0, interpolation)), [255, 255, 255, 255]);
        }
        // 40% of the way from black to white: sRGB blending gives 40% of the encoded range,
        // linear blending 40% of the light, OkLab 40% of the lightness (linear 0.4³ for grey).
        assert_eq!(stored(r.evaluate(0.45, Interpolation::Srgb)), [102, 102, 102, 255]);
        assert_eq!(stored(r.evaluate(0.45, Interpolation::Linear)), [170, 170, 170, 255]);
        assert_eq!(stored(r.evaluate(0.45, Interpolation::OkLab)), stored([0.064, 0.064, 0.064, 1.0]));
        assert_eq!(stored(r.evaluate(0.7, Interpolation::Step)), [0, 0, 0, 255]);
        // Two stops at one place make a hard edge, the second color from there on.
        let hard = ramp(vec![
            stop(0.0, "#102030"),
            stop(0.5, "#102030"),
            stop(0.5, "#f0e0d0"),
            stop(1.0, "#f0e0d0"),
        ]);
        assert_eq!(stored(hard.evaluate(0.499, Interpolation::OkLab)), [0x10, 0x20, 0x30, 255]);
        assert_eq!(stored(hard.evaluate(0.5, Interpolation::OkLab)), [0xf0, 0xe0, 0xd0, 255]);
        // A single stop is a flat ramp.
        assert_eq!(
            stored(ramp(vec![stop(0.3, "#abcdef80")]).evaluate(0.9, Interpolation::OkLab)),
            [0xab, 0xcd, 0xef, 0x80]
        );
    }

    #[test]
    fn oklab_blends_lightness_evenly() {
        // A warm skin shadow to a cool one: halfway, OkLab is halfway in lightness, while
        // blending encoded sRGB comes out darker and blending linear light brighter.
        let r = ramp(vec![stop(0.0, "#e0784a"), stop(1.0, "#4a78e0")]);
        let lightness = |interpolation, u| {
            let [red, green, blue, _] = r.evaluate(u, interpolation);
            linear_srgb_to_oklab([red, green, blue])[0]
        };
        let middle = (lightness(Interpolation::OkLab, 0.0) + lightness(Interpolation::OkLab, 1.0)) / 2.0;
        assert!((lightness(Interpolation::OkLab, 0.5) - middle).abs() < 1e-3);
        assert!(lightness(Interpolation::Srgb, 0.5) < middle - 0.02);
        assert!(lightness(Interpolation::Linear, 0.5) > middle);
    }

    #[test]
    fn interpolation_comes_from_the_stop_then_the_ramp_then_the_document() {
        let mut r = ramp(vec![stop(0.0, "#000000"), stop(0.5, "#ffffff"), stop(1.0, "#000000")]);
        r.interpolation = Some(Interpolation::Srgb);
        r.stops[1].interpolation = Some(Interpolation::Step);
        assert_eq!(stored(r.evaluate(0.2, Interpolation::OkLab)), [102, 102, 102, 255], "the ramp's");
        assert_eq!(stored(r.evaluate(0.75, Interpolation::OkLab)), [255, 255, 255, 255], "the stop's");
        r.interpolation = None;
        assert_eq!(stored(r.evaluate(0.2, Interpolation::Linear)), [170, 170, 170, 255], "the document's");
    }

    fn material(name: &str, day: &str, night: Option<&str>) -> MaterialRamps {
        MaterialRamps {
            name: name.to_owned(),
            day: Ramp::Stops(ramp(vec![stop(0.0, day)])),
            night: night.map(|night| Ramp::Stops(ramp(vec![stop(0.0, night)]))),
        }
    }

    #[test]
    fn rows_follow_the_shared_layout() {
        let map = RampMap {
            interpolation: Interpolation::OkLab,
            materials: vec![
                material("skin", "#f0b090", Some("#806080")),
                material("cloth", "#3050a0", None),
                material("metal", "#a0a0a0", Some("#404060")),
            ],
        };
        let settings = RampBakeSettings { width: 8, row_height: 3, mips: false };
        let GeneratedMap { image, class } = bake(&map, &settings).unwrap();
        assert_eq!((image.width, image.height, image.layout), (8, 48, ChannelLayout::Rgba));
        assert_eq!(class, TextureClass::Ramp(RampSettings { mips: false }));
        let row_color = |row: u32| {
            // Every texel of a row's band is the same flat color here.
            let band = &image.pixels[(row * 3 * 8) as usize..((row + 1) * 3 * 8) as usize];
            assert!(band.iter().all(|texel| texel == &band[0]), "row {row} is one color");
            SrgbColor(stored(band[0]))
        };
        let id = |k| MaterialId::new(k).unwrap();
        assert_eq!(row_color(row(id(0), Half::Day)), color("#f0b090"));
        assert_eq!(row_color(row(id(0), Half::Night)), color("#806080"));
        assert_eq!(row_color(row(id(1), Half::Day)), color("#3050a0"));
        assert_eq!(row_color(row(id(1), Half::Night)), color("#3050a0"), "no night ramp: the day one");
        assert_eq!(row_color(row(id(2), Half::Night)), color("#404060"));
        for k in 3..MATERIAL_IDS {
            assert_eq!(
                row_color(row(id(k), Half::Day)),
                color("#a0a0a0"),
                "ID {k} repeats the last material"
            );
            assert_eq!(row_color(row(id(k), Half::Night)), color("#404060"));
        }
    }

    #[test]
    fn texels_sample_the_ramp_at_their_centers() {
        let map = RampMap {
            interpolation: Interpolation::Linear,
            materials: vec![MaterialRamps {
                name: String::new(),
                day: Ramp::Stops(ramp(vec![stop(0.0, "#000000"), stop(1.0, "#ffffff")])),
                night: None,
            }],
        };
        let settings = RampBakeSettings { width: 4, row_height: 1, mips: true };
        let GeneratedMap { image, class } = bake(&map, &settings).unwrap();
        let reds: Vec<f32> = image.pixels[..4].iter().map(|p| p[0]).collect();
        assert_eq!(reds, [0.125, 0.375, 0.625, 0.875]);
        assert_eq!(class, TextureClass::Ramp(RampSettings { mips: true }));
    }

    #[test]
    fn invalid_documents_and_sizes_are_errors() {
        let one = |stops| RampMap {
            interpolation: Interpolation::OkLab,
            materials: vec![MaterialRamps {
                name: "hair".into(),
                day: Ramp::Stops(ramp(stops)),
                night: None,
            }],
        };
        let settings = RampBakeSettings::default();
        let error = |map: &RampMap| bake(map, &settings).unwrap_err().to_string();
        assert_eq!(
            error(&RampMap { interpolation: Interpolation::OkLab, materials: Vec::new() }),
            "the document lists no materials"
        );
        let nine = RampMap {
            interpolation: Interpolation::OkLab,
            materials: (0..9).map(|_| material("", "#000000", None)).collect(),
        };
        assert!(error(&nine).contains("at most 8"));
        assert_eq!(error(&one(Vec::new())), "material 0 (hair), day ramp has no stops");
        assert!(error(&one(vec![stop(-0.1, "#000000")])).contains("stop 0 is at -0.1"));
        assert!(
            error(&one(vec![stop(0.0, "#000000"), stop(f32::NAN, "#000000")])).contains("stop 1 is at NaN")
        );
        assert!(
            error(&one(vec![stop(0.6, "#000000"), stop(0.5, "#000000")])).contains("list stops in order")
        );
        let valid = one(vec![stop(0.0, "#000000")]);
        for (width, row_height) in
            [(0, 4), (256, 0), (MAX_TEXTURE_DIMENSION + 1, 4), (256, MAX_TEXTURE_DIMENSION / 8)]
        {
            let result = bake(&valid, &RampBakeSettings { width, row_height, mips: false });
            assert!(matches!(result, Err(MapError::Ramp(RampError::Size { .. }))), "{width} x {row_height}");
        }
    }
}
