//! Version 9 of the cooked zone (M6 W2), frozen, read for one release after version 10 (cooker rule 2): the
//! same halves with no rooms and no weather plan. The simulation's half and every part kept their layout, so it shares them.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::AssetId;

use super::{
    CookedInstance, CookedLight, CookedSpline, PlaceContents, Zone, ZoneCell, ZoneError, ZonePresentation,
    ZoneSimulation,
};
use crate::atmosphere::CookedAtmosphere;
use crate::look::{ZoneFog, ZoneWeather, ZoneWind};
use crate::water::CookedWaterSurface;

/// The version this reads.
pub(super) const VERSION: u32 = 9;

/// [`Zone`] as version 9 wrote it, under its name.
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[type_path = "sapphire::zone"]
#[type_name = "Zone"]
pub(super) struct ZoneV9 {
    pub simulation: ZoneSimulation,
    pub presentation: PresentationV9,
}

impl Versioned for ZoneV9 {
    const VERSION: u32 = VERSION;
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(super) struct PresentationV9 {
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
}

/// Version 9 bytes as a zone of today's version, unchecked past the archive: the caller writes and reads it
/// back.
pub(super) fn read(bytes: &[u8]) -> Result<Zone, ZoneError> {
    let archived = sapphire_serialize::access::<ZoneV9>(bytes)?;
    let old = rkyv::deserialize::<ZoneV9, rkyv::rancor::Error>(archived)
        .map_err(|error| ZoneError::Cooked(error.into()))?;
    let PresentationV9 {
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
            rooms: Vec::new(),
            weather_plan: None,
            foliage: None,
            atmosphere: CookedAtmosphere::default(),
        },
    })
}

/// `zone` written as version 9 would have, for tests: its rooms dropped.
#[cfg(test)]
pub(super) fn write(zone: &Zone) -> sapphire_serialize::cooked::AlignedVec {
    let Zone { simulation, presentation } = zone.clone();
    let old = ZoneV9 {
        simulation,
        presentation: PresentationV9 {
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
        },
    };
    sapphire_serialize::to_cooked(&old).expect("writes")
}
