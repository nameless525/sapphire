//! Ramp documents as plain data (M6 T4): a `.ramp.ron`'s shadow-color gradients per material ID, authored as
//! color stops or painted as image strips, and the checks that need nothing but the document.
//!
//! The cooker bakes them into the rows [`crate::character_maps::ramp`] lays out (`sapphire_pipeline`'s
//! `character_maps::ramp`, whose module docs say what each field does to a row), and the editor opens them
//! as documents; neither may depend on the other, so the types live here. Evaluating a ramp, which needs the
//! cook's deterministic color math, and reading its images stay in the pipeline.
//!
//! # Versions
//!
//! Version 2 (M6 C7) writes each ramp as `Stops((…))` or `Image((…))`. A version 1 document (M1 A7), whose
//! ramps are all stops written as `(stops: […])`, reads as it did and bakes the same rows.

use std::fmt;
use std::str::FromStr;

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_reflect::{Decoder, VersionError, Versioned};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::character_maps::MATERIAL_IDS;
use crate::character_maps::ramp::{Half, ROWS};
use crate::texture::MAX_TEXTURE_DIMENSION;

/// A `.ramp.ron` document: the day and night ramps of up to [`MATERIAL_IDS`] materials. Its stable name is
/// still the pipeline's, as every committed document's envelope says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct RampMap {
    /// How stops blend unless a ramp or stop says otherwise, and how an image's texels blend.
    #[serde(default)]
    pub interpolation: Interpolation,
    /// Material ID 0's ramps first, then ID 1's, and so on.
    pub materials: Vec<MaterialRamps>,
}

impl Versioned for RampMap {
    /// 1: M1 A7, every ramp stops. 2: M6 C7, a ramp is stops or a row of an image; a version 1
    /// document reads as it is.
    const VERSION: u32 = 2;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        match version {
            1 => data.decode::<v1::RampMap>().map(Into::into).map_err(VersionError::Decode),
            found => Err(VersionError::TooOld { found, oldest: 1 }),
        }
    }
}

/// One material ID's day/night pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::assets::ramp"]
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

/// One ramp: what its row holds along `u`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::assets::ramp"]
pub enum Ramp {
    /// Color stops.
    Stops(StopRamp),
    /// A row of a painted image (M6 C7).
    Image(ImageRamp),
}

/// A ramp of color stops along `u`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::assets::ramp"]
#[serde(deny_unknown_fields)]
pub struct StopRamp {
    /// Overrides the document's interpolation for this ramp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpolation: Option<Interpolation>,
    pub stops: Vec<ColorStop>,
}

/// A ramp painted as a row of an image, named relative to the document's directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::assets::ramp"]
#[serde(deny_unknown_fields)]
pub struct ImageRamp {
    /// The image, relative to the document's directory, `/`-separated.
    pub path: String,
    /// Which of its rows, from 0 at the top.
    pub row: u32,
}

/// A color at a position along a ramp.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::assets::ramp"]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::assets::ramp"]
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

/// An 8-bit sRGB color with linear alpha, written `"#rrggbb"` or `"#rrggbbaa"`. A value whole to
/// reflection, as its text is one string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Reflect)]
#[reflect(opaque, Clone, Debug, PartialEq, Hash, Serialize, Deserialize)]
#[type_path = "sapphire::assets::ramp"]
pub struct SrgbColor(pub [u8; 4]);

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

/// Errors in a ramp document, its images or its bake settings.
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
    #[error("{ramp}: `row: {row}` is past the last row of {path:?}, which is {height} texels tall")]
    ImageRow { ramp: RampName, path: String, row: u32, height: u32 },
    #[error(
        "{ramp}: {path:?} is {width} texels wide, wider than the bake's {bake}, which would skip painted \
         texels: paint it at most {bake} wide, or set the sidecar's `width` to {width}"
    )]
    ImageWidth { ramp: RampName, path: String, width: u32, bake: u32 },
    #[error(
        "a width of {width} and rows {row_height} texels tall don't fit a texture: both must be at least 1, \
         the width at most {MAX_TEXTURE_DIMENSION} and {ROWS} rows at most {MAX_TEXTURE_DIMENSION} tall"
    )]
    Size { width: u32, row_height: u32 },
}

impl StopRamp {
    /// Checks that it has stops, each from 0 to 1 and in order; `ramp` names it for errors.
    pub fn validate(&self, ramp: impl Fn() -> RampName) -> Result<(), RampError> {
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

impl RampMap {
    /// Checks the material list and every ramp's stops. An image ramp is checked as its image
    /// is read.
    pub fn validate(&self) -> Result<(), RampError> {
        match self.materials.len() {
            0 => return Err(RampError::NoMaterials),
            count if count > usize::from(MATERIAL_IDS) => return Err(RampError::TooManyMaterials(count)),
            _ => {}
        }
        for (material, ramps) in self.materials.iter().enumerate() {
            let name = |half| RampName { material, name: ramps.name.clone(), half };
            for (half, ramp) in [(Half::Day, Some(&ramps.day)), (Half::Night, ramps.night.as_ref())] {
                if let Some(Ramp::Stops(stops)) = ramp {
                    stops.validate(|| name(half))?;
                }
            }
        }
        Ok(())
    }

    /// The images the document's ramps read, as it names them (relative to its directory), in
    /// document order. Repeats are kept.
    pub fn inputs(&self) -> Vec<String> {
        let ramps = self.materials.iter().flat_map(|ramps| std::iter::once(&ramps.day).chain(&ramps.night));
        ramps
            .filter_map(|ramp| match ramp {
                Ramp::Image(image) => Some(image.path.clone()),
                Ramp::Stops(_) => None,
            })
            .collect()
    }
}

impl From<v1::RampMap> for RampMap {
    fn from(old: v1::RampMap) -> Self {
        let materials = old
            .materials
            .into_iter()
            .map(|material| MaterialRamps {
                name: material.name,
                day: Ramp::Stops(material.day),
                night: material.night.map(Ramp::Stops),
            })
            .collect();
        Self { interpolation: old.interpolation, materials }
    }
}

/// Version 1 (M1 A7 to M6 C7): every ramp is stops, written as `(stops: […])`. Its documents read as
/// they are.
pub mod v1 {
    use bevy_reflect::TypePath;
    use sapphire_reflect::Versioned;
    use serde::{Deserialize, Serialize};

    use super::{Interpolation, StopRamp};

    /// A version 1 `.ramp.ron` document.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
    #[type_path = "sapphire::pipeline"]
    #[serde(deny_unknown_fields)]
    pub struct RampMap {
        #[serde(default)]
        pub interpolation: Interpolation,
        pub materials: Vec<MaterialRamps>,
    }

    impl Versioned for RampMap {
        const VERSION: u32 = 1;
    }

    /// A version 1 material: stops by day, and at night when given.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct MaterialRamps {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        pub name: String,
        pub day: StopRamp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub night: Option<StopRamp>,
    }
}
