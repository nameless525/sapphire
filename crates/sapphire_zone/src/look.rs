//! A zone's air (M5 M6): the wind its plants sway in, how wet it is, and its fog over the day's. With its
//! day, its light rigs, its probe grid and its sky material, what a zone looks like as its own, which the
//! stage takes when the zone opens and when a trip arrives in it.
//!
//! Each is the same plain value in the document and in the cooked zone, in [`ZonePresentation`], so no
//! replay's digest sees it. A zone that names none keeps what the stage has: its wind, a dry world and the
//! day's fog as the day has it.
//!
//! ```ron
//! wind: Some((direction: (0.94, 0.0, 0.33), strength: 0.6, gustiness: 0.3)),
//! weather: Some((wetness: 0.7, puddles: 0.4)),
//! fog: Some((density: 1.6, height_base: Some(-0.5))),
//! ```
//!
//! [`ZonePresentation`]: crate::ZonePresentation

use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

use crate::grid::MAX_COORDINATE;

/// The strongest wind a zone may blow: a gale is a few times the strength plants are authored at.
pub const MAX_WIND: f32 = 8.0;
/// The most a zone may thicken its day's fog.
pub const MAX_FOG_DENSITY: f32 = 16.0;

/// The wind a zone's plants sway in: the renderer's `Wind`, as a document writes it.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
#[rkyv(derive(Debug))]
pub struct ZoneWind {
    /// Which way it blows toward. Only its horizontal part counts, and it must have one.
    pub direction: [f32; 3],
    /// 0 is calm and 1 the strength plants' responses are authored at, up to [`MAX_WIND`].
    pub strength: f32,
    /// How much gusts swell and ease it as they travel downwind: 0 steady, 1 from calm to double.
    #[serde(default)]
    pub gustiness: f32,
}

/// How wet a zone is: what the wet surface model reads (`FramePacket::weather`), the same at every hour.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
#[rkyv(derive(Debug))]
pub struct ZoneWeather {
    /// How wet its surfaces are, 0 dry to 1 soaked.
    pub wetness: f32,
    /// How far its puddles have filled, 0 none to 1 every one a material's mask allows.
    #[serde(default)]
    pub puddles: f32,
}

/// A zone's fog over its day's: the day says what colour the air is at each hour and how thick, and the zone
/// how much thicker it is here, and where its ground lies for the height fog to pool on.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
#[rkyv(derive(Debug))]
pub struct ZoneFog {
    /// Times the day's density, of its near, far and height layers alike: 1 is the day's, 0 clears the air,
    /// up to [`MAX_FOG_DENSITY`].
    #[serde(default = "one")]
    pub density: f32,
    /// The height fog's base, in metres in the zone's space, in place of the day's: a hollow's floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_base: Option<f32>,
}

fn one() -> f32 {
    1.0
}

impl ZoneWind {
    /// Whether it's a wind the renderer takes, or what's wrong with it.
    pub fn check(&self) -> Result<(), String> {
        let [x, y, z] = self.direction;
        let across = x * x + z * z;
        if !(across.is_finite() && y.is_finite()) || across == 0.0 {
            return Err(format!("direction {:?} has no horizontal part", self.direction));
        }
        if !(self.strength.is_finite() && (0.0..=MAX_WIND).contains(&self.strength)) {
            return Err(format!("strength {} isn't from 0 to {MAX_WIND}", self.strength));
        }
        check_share("gustiness", self.gustiness)
    }
}

impl ZoneWeather {
    /// Whether its shares are shares, or what's wrong with them.
    pub fn check(&self) -> Result<(), String> {
        check_share("wetness", self.wetness)?;
        check_share("puddles", self.puddles)
    }
}

impl ZoneFog {
    /// Whether it's a fog the renderer takes, or what's wrong with it.
    pub fn check(&self) -> Result<(), String> {
        if !(self.density.is_finite() && (0.0..=MAX_FOG_DENSITY).contains(&self.density)) {
            return Err(format!("density {} isn't from 0 to {MAX_FOG_DENSITY}", self.density));
        }
        match self.height_base {
            Some(base) if !(base.is_finite() && base.abs() <= MAX_COORDINATE) => {
                Err(format!("height_base {base} isn't within {MAX_COORDINATE} m"))
            }
            _ => Ok(()),
        }
    }
}

fn check_share(name: &str, value: f32) -> Result<(), String> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(format!("{name} {value} isn't from 0 to 1"))
    }
}

impl ArchivedZoneWind {
    #[must_use]
    pub fn to_native(&self) -> ZoneWind {
        ZoneWind {
            direction: self.direction.each_ref().map(|value| value.to_native()),
            strength: self.strength.to_native(),
            gustiness: self.gustiness.to_native(),
        }
    }
}

impl ArchivedZoneWeather {
    #[must_use]
    pub fn to_native(&self) -> ZoneWeather {
        ZoneWeather { wetness: self.wetness.to_native(), puddles: self.puddles.to_native() }
    }
}

impl ArchivedZoneFog {
    #[must_use]
    pub fn to_native(&self) -> ZoneFog {
        ZoneFog {
            density: self.density.to_native(),
            height_base: self.height_base.as_ref().map(|base| base.to_native()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each refuses what the renderer couldn't take, and takes the edges of what it can.
    #[test]
    fn each_refuses_what_the_renderer_cannot_take() {
        let wind = ZoneWind { direction: [1.0, 0.0, 0.0], strength: 1.0, gustiness: 0.3 };
        assert_eq!(wind.check(), Ok(()));
        assert_eq!(ZoneWind { strength: 0.0, gustiness: 1.0, ..wind }.check(), Ok(()));
        for bad in [
            ZoneWind { direction: [0.0, 1.0, 0.0], ..wind },
            ZoneWind { direction: [f32::NAN, 0.0, 1.0], ..wind },
            ZoneWind { strength: -0.1, ..wind },
            ZoneWind { strength: MAX_WIND + 1.0, ..wind },
            ZoneWind { gustiness: 1.5, ..wind },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
        assert_eq!(ZoneWeather { wetness: 1.0, puddles: 0.0 }.check(), Ok(()));
        assert!(ZoneWeather { wetness: 1.2, puddles: 0.0 }.check().is_err());
        assert!(ZoneWeather { wetness: 0.5, puddles: f32::INFINITY }.check().is_err());
        assert_eq!(ZoneFog { density: 0.0, height_base: Some(-3.0) }.check(), Ok(()));
        assert!(ZoneFog { density: -1.0, height_base: None }.check().is_err());
        assert!(ZoneFog { density: 1.0, height_base: Some(f32::NAN) }.check().is_err());
        assert!(ZoneFog { density: 1.0, height_base: Some(2.0 * MAX_COORDINATE) }.check().is_err());
    }

    /// What a document leaves out is what changes nothing: no gusts, no puddles, the day's density.
    #[test]
    fn what_a_document_leaves_out_changes_nothing() {
        let wind: ZoneWind = ron::from_str("(direction: (1.0, 0.0, 0.0), strength: 1.0)").expect("reads");
        assert_eq!(wind.gustiness, 0.0);
        let weather: ZoneWeather = ron::from_str("(wetness: 0.5)").expect("reads");
        assert_eq!(weather.puddles, 0.0);
        let fog: ZoneFog = ron::from_str("()").expect("reads");
        assert_eq!(fog, ZoneFog { density: 1.0, height_base: None });
        assert!(ron::from_str::<ZoneFog>("(density: 1.0, rain: 1.0)").is_err(), "unknown fields are refused");
    }
}
