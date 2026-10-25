//! Ramps: shadow-color gradients per material ID, authored as color stops and baked to rows.
//!
//! The anime character shader's ramp mode (Genshin, Star Rail, WuWa) takes the shadow side's
//! color from a ramp row picked by material ID, with a day and a night row that time of day
//! blends between (spec §5.12). A `.ramp.ron` document lists each material's ramps as color
//! stops, and the bake writes them into the row layout of `sapphire_assets::character_maps`:
//! day rows for IDs 0 to 7, then night rows, each [`RampBakeSettings::row_height`] texels
//! tall and [`RampBakeSettings::width`] wide.
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
//!   are those two blends (`Srgb` matches an image editor's gradient, for a painted ramp),
//!   and `Step` holds a stop's color up to the next stop, for banded ramps.
//! - **Rows** come from materials in order: the first is ID 0. A material without a night
//!   ramp uses its day ramp for both, and IDs past the last material repeat the last one, so
//!   any ID the shader reads (stray ones included, where filtering blends IDs) finds a ramp.
//!
//! Each texel is the ramp evaluated at its center, `u = (x + 0.5) / width`, so linear
//! filtering between texel centers follows the ramp.

use std::fmt;
use std::str::FromStr;

use bevy_reflect::TypePath;
use sapphire_assets::character_maps::ramp::{Half, ROWS, row};
use sapphire_assets::character_maps::{MATERIAL_IDS, MaterialId};
use sapphire_assets::texture::MAX_TEXTURE_DIMENSION;
use sapphire_reflect::Versioned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{GeneratedMap, MapError};
use crate::cache::Stage;
use crate::color::{linear_srgb_to_oklab, linear_to_srgb, oklab_to_linear_srgb, srgb_to_linear};
use crate::import::image::{ChannelLayout, SourceImage};
use crate::texture::{RampSettings, TextureClass};

/// This importer, as cache keys know it: reading the document and baking its rows. Bump the
/// version whenever the rows change for the same document.
pub const IMPORTER: Stage = Stage { name: "ramp", version: 1 };

/// A `.ramp.ron` document: the day and night ramps of up to [`MATERIAL_IDS`] materials.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct RampMap {
    /// How stops blend unless a ramp or stop says otherwise.
    #[serde(default)]
    pub interpolation: Interpolation,
    /// Material ID 0's ramps first, then ID 1's, and so on.
    pub materials: Vec<MaterialRamps>,
}

impl Versioned for RampMap {
    const VERSION: u32 = 1;
}

/// One material ID's day/night pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialRamps {
    /// What the material is (`"skin"`), for messages.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub day: Ramp,
    /// The day ramp when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub night: Option<Ramp>,
}

/// One ramp: color stops along `u`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ramp {
    /// Overrides the document's interpolation for this ramp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpolation: Option<Interpolation>,
    pub stops: Vec<ColorStop>,
}

/// A color at a position along a ramp.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorStop {
    /// From 0, the deepest shadow, to 1, the lit edge.
    pub at: f32,
    pub color: SrgbColor,
    /// How the ramp blends from this stop to the next, overriding the ramp's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpolation: Option<Interpolation>,
}

/// How a ramp blends between two stops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interpolation {
    /// In OkLab: even steps of perceived lightness and hue.
    #[default]
    OkLab,
    /// In linear light.
    Linear,
    /// On the sRGB-encoded values, as image editors' gradients do.
    Srgb,
    /// No blend: the stop's color up to the next stop.
    Step,
}

/// An 8-bit sRGB color with linear alpha, written `"#rrggbb"` or `"#rrggbbaa"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SrgbColor(pub [u8; 4]);

impl SrgbColor {
    /// The color in linear light, with its alpha.
    pub fn to_linear(self) -> [f32; 4] {
        let [r, g, b, a] = self.0.map(|v| f32::from(v) / 255.0);
        [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a]
    }
}

/// Returned for text that isn't a `#rrggbb` or `#rrggbbaa` color.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0:?} is not a color: expected sRGB hex like \"#e3a184\", or \"#e3a184c0\" with alpha")]
pub struct InvalidColor(pub String);

impl FromStr for SrgbColor {
    type Err = InvalidColor;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || InvalidColor(text.to_owned());
        let hex = text.strip_prefix('#').filter(|hex| hex.is_ascii()).ok_or_else(invalid)?;
        let byte = |i: usize| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| invalid());
        match hex.len() {
            6 => Ok(Self([byte(0)?, byte(1)?, byte(2)?, 255])),
            8 => Ok(Self([byte(0)?, byte(1)?, byte(2)?, byte(3)?])),
            _ => Err(invalid()),
        }
    }
}

impl fmt::Display for SrgbColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [r, g, b, a] = self.0;
        write!(f, "#{r:02x}{g:02x}{b:02x}")?;
        if a != 255 { write!(f, "{a:02x}") } else { Ok(()) }
    }
}

impl Serialize for SrgbColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SrgbColor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
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

/// Which ramp of a material an error is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RampName {
    pub material: usize,
    pub name: String,
    pub half: Half,
}

impl fmt::Display for RampName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let half = match self.half {
            Half::Day => "day",
            Half::Night => "night",
        };
        write!(f, "material {}", self.material)?;
        if !self.name.is_empty() {
            write!(f, " ({})", self.name)?;
        }
        write!(f, ", {half} ramp")
    }
}

/// Errors in a ramp document or its bake settings.
#[derive(Debug, thiserror::Error)]
pub enum RampError {
    #[error("the document lists no materials")]
    NoMaterials,
    #[error("the document lists {0} materials; ramps hold at most {MATERIAL_IDS}")]
    TooManyMaterials(usize),
    #[error("{0} has no stops")]
    NoStops(RampName),
    #[error("{ramp}: stop {index} is at {at}; stops go from 0 to 1")]
    StopRange { ramp: RampName, index: usize, at: f32 },
    #[error(
        "{ramp}: stop {index} at {at} comes before the stop before it at {previous}: list stops in order"
    )]
    StopOrder { ramp: RampName, index: usize, at: f32, previous: f32 },
    #[error(
        "a width of {width} and rows {row_height} texels tall don't fit a texture: both must be at least 1, \
         the width at most {MAX_TEXTURE_DIMENSION} and {ROWS} rows at most {MAX_TEXTURE_DIMENSION} tall"
    )]
    Size { width: u32, row_height: u32 },
}

impl Ramp {
    /// The color at `u`, in linear light with alpha, blending with `default` where neither the
    /// stop nor the ramp says how.
    pub fn evaluate(&self, u: f32, default: Interpolation) -> [f32; 4] {
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
        blend(stop.color, next.color, t, interpolation)
    }

    fn validate(&self, ramp: impl Fn() -> RampName) -> Result<(), RampError> {
        if self.stops.is_empty() {
            return Err(RampError::NoStops(ramp()));
        }
        for (index, stop) in self.stops.iter().enumerate() {
            if !(0.0..=1.0).contains(&stop.at) {
                return Err(RampError::StopRange { ramp: ramp(), index, at: stop.at });
            }
            if let Some(previous) = index.checked_sub(1).map(|i| self.stops[i].at)
                && stop.at < previous
            {
                return Err(RampError::StopOrder { ramp: ramp(), index, at: stop.at, previous });
            }
        }
        Ok(())
    }
}

/// `a` blended toward `b` by `t`, in linear light with alpha. Alpha always blends linearly,
/// except under `Step`.
fn blend(a: SrgbColor, b: SrgbColor, t: f32, interpolation: Interpolation) -> [f32; 4] {
    let (la, lb) = (a.to_linear(), b.to_linear());
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

impl RampMap {
    /// Checks the material list and every ramp's stops.
    pub fn validate(&self) -> Result<(), RampError> {
        match self.materials.len() {
            0 => return Err(RampError::NoMaterials),
            count if count > usize::from(MATERIAL_IDS) => return Err(RampError::TooManyMaterials(count)),
            _ => {}
        }
        for (material, ramps) in self.materials.iter().enumerate() {
            let name = |half| RampName { material, name: ramps.name.clone(), half };
            ramps.day.validate(|| name(Half::Day))?;
            if let Some(night) = &ramps.night {
                night.validate(|| name(Half::Night))?;
            }
        }
        Ok(())
    }

    /// The ramp in `id`'s `half` row: see the [module docs](self) for unlisted IDs and
    /// missing night ramps.
    pub fn ramp(&self, id: MaterialId, half: Half) -> &Ramp {
        let ramps = &self.materials[usize::from(id.get()).min(self.materials.len() - 1)];
        match half {
            Half::Day => &ramps.day,
            Half::Night => ramps.night.as_ref().unwrap_or(&ramps.day),
        }
    }
}

/// Bakes `map`'s rows.
pub(super) fn generate(map: &RampMap, settings: &RampBakeSettings) -> Result<GeneratedMap, MapError> {
    map.validate()?;
    let RampBakeSettings { width, row_height, mips } = *settings;
    let height = row_height.saturating_mul(ROWS);
    if !(1..=MAX_TEXTURE_DIMENSION).contains(&width) || row_height == 0 || height > MAX_TEXTURE_DIMENSION {
        return Err(RampError::Size { width, row_height }.into());
    }
    let mut rows = vec![Vec::new(); ROWS as usize];
    for half in [Half::Day, Half::Night] {
        for id in MaterialId::ALL {
            let ramp = map.ramp(id, half);
            rows[row(id, half) as usize] = (0..width)
                .map(|x| ramp.evaluate((x as f32 + 0.5) / width as f32, map.interpolation))
                .collect();
        }
    }
    let pixels =
        rows.iter().flat_map(|texels| (0..row_height).flat_map(move |_| texels.iter().copied())).collect();
    let image = SourceImage::new(width, height, ChannelLayout::Rgba, pixels).expect("one texel per pixel");
    Ok(GeneratedMap { image, class: TextureClass::Ramp(RampSettings { mips }) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::unorm8;

    fn color(text: &str) -> SrgbColor {
        text.parse().unwrap()
    }

    fn stop(at: f32, text: &str) -> ColorStop {
        ColorStop { at, color: color(text), interpolation: None }
    }

    fn ramp(stops: Vec<ColorStop>) -> Ramp {
        Ramp { interpolation: None, stops }
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
            day: ramp(vec![stop(0.0, day)]),
            night: night.map(|night| ramp(vec![stop(0.0, night)])),
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
        let GeneratedMap { image, class } = generate(&map, &settings).unwrap();
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
                day: ramp(vec![stop(0.0, "#000000"), stop(1.0, "#ffffff")]),
                night: None,
            }],
        };
        let settings = RampBakeSettings { width: 4, row_height: 1, mips: true };
        let GeneratedMap { image, class } = generate(&map, &settings).unwrap();
        let reds: Vec<f32> = image.pixels[..4].iter().map(|p| p[0]).collect();
        assert_eq!(reds, [0.125, 0.375, 0.625, 0.875]);
        assert_eq!(class, TextureClass::Ramp(RampSettings { mips: true }));
    }

    #[test]
    fn invalid_documents_and_sizes_are_errors() {
        let one = |stops| RampMap {
            interpolation: Interpolation::OkLab,
            materials: vec![MaterialRamps { name: "hair".into(), day: ramp(stops), night: None }],
        };
        let settings = RampBakeSettings::default();
        let error = |map: &RampMap| generate(map, &settings).unwrap_err().to_string();
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
            let result = generate(&valid, &RampBakeSettings { width, row_height, mips: false });
            assert!(matches!(result, Err(MapError::Ramp(RampError::Size { .. }))), "{width} x {row_height}");
        }
    }
}
