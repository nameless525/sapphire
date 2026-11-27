//! Version 12 of the cooked zone (M6 G1a), frozen, read for one release after version 13 (cooker rule 2): the
//! same halves with no atmosphere volumes. The simulation's half and every part kept their layout, so it shares
//! them.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::AssetId;

use super::{
    CookedInstance, CookedLight, CookedSpline, CookedZoneFoliage, PlaceContents, Zone, ZoneCell, ZoneError,
    ZonePresentation, ZoneSimulation,
};
use crate::atmosphere::CookedAtmosphere;
use crate::look::{ZoneFog, ZoneWeather, ZoneWind};
use crate::rooms::CookedRoom;
use crate::water::CookedWaterSurface;
use crate::weather::CookedWeatherPlan;

/// The version this reads.
pub(super) const VERSION: u32 = 12;

/// [`Zone`] as version 12 wrote it, under its name.
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[type_path = "sapphire::zone"]
#[type_name = "Zone"]
pub(super) struct ZoneV12 {
    pub simulation: ZoneSimulation,
    pub presentation: PresentationV12,
}

impl Versioned for ZoneV12 {
    const VERSION: u32 = VERSION;
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(super) struct PresentationV12 {
    pub cell_size: f32,
    pub backdrop: Option<PlaceContents>,
    pub cells: Vec<ZoneCell>,
    pub instances: Vec<CookedInstance>,
    pub lights: Vec<CookedLight>,
    pub probes: Option<AssetId>,
    pub reflections: Option<AssetId>,
    pub day: Option<String>,
    pub rigs: Option<String>,
    pub splines: Vec<CookedSpline>,
    pub sky: Option<AssetId>,
    pub wind: Option<ZoneWind>,
    pub weather: Option<ZoneWeather>,
    pub fog: Option<ZoneFog>,
    pub water: Vec<CookedWaterSurface>,
    pub rooms: Vec<CookedRoom>,
    pub weather_plan: Option<CookedWeatherPlan>,
    pub foliage: Option<CookedZoneFoliage>,
}

/// Version 12 bytes as a zone of today's version, unchecked past the archive: the caller writes and reads it
/// back.
pub(super) fn read(bytes: &[u8]) -> Result<Zone, ZoneError> {
    let archived = sapphire_serialize::access::<ZoneV12>(bytes)?;
    let old = rkyv::deserialize::<ZoneV12, rkyv::rancor::Error>(archived)
        .map_err(|error| ZoneError::Cooked(error.into()))?;
    let PresentationV12 {
        cell_size,
        backdrop,
        cells,
        instances,
        lights,
        probes,
        reflections,
        day,
        rigs,
        splines,
        sky,
        wind,
        weather,
        fog,
        water,
        rooms,
        weather_plan,
        foliage,
    } = old.presentation;
    Ok(Zone {
        simulation: old.simulation,
        presentation: ZonePresentation {
            cell_size,
            backdrop,
            cells,
            instances,
            lights,
            probes,
            reflections,
            day,
            rigs,
            splines,
            sky,
            wind,
            weather,
            fog,
            water,
            rooms,
            weather_plan,
            foliage,
            atmosphere: CookedAtmosphere::default(),
        },
    })
}

/// `zone` written as version 12 would have, for tests: its atmosphere volumes dropped.
#[cfg(test)]
pub(super) fn write(zone: &Zone) -> sapphire_serialize::cooked::AlignedVec {
    let Zone { simulation, presentation } = zone.clone();
    let old = ZoneV12 {
        simulation,
        presentation: PresentationV12 {
            cell_size: presentation.cell_size,
            backdrop: presentation.backdrop,
            cells: presentation.cells,
            instances: presentation.instances,
            lights: presentation.lights,
            probes: presentation.probes,
            reflections: presentation.reflections,
            day: presentation.day,
            rigs: presentation.rigs,
            splines: presentation.splines,
            sky: presentation.sky,
            wind: presentation.wind,
            weather: presentation.weather,
            fog: presentation.fog,
            water: presentation.water,
            rooms: presentation.rooms,
            weather_plan: presentation.weather_plan,
            foliage: presentation.foliage,
        },
    };
    sapphire_serialize::to_cooked(&old).expect("writes")
}
