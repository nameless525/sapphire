//! Version 13 of the cooked zone (M6 V1), frozen, read for one release after version 14 (cooker rule 2): the
//! same halves with no drawn water, each surface without its mesh. The simulation's half and every other part
//! kept their layout, so it shares them. Versions 9 to 12 hold its surfaces too.

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
pub(super) const VERSION: u32 = 13;

/// [`Zone`] as version 13 wrote it, under its name.
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[type_path = "sapphire::zone"]
#[type_name = "Zone"]
pub(super) struct ZoneV13 {
    pub simulation: ZoneSimulation,
    pub presentation: PresentationV13,
}

impl Versioned for ZoneV13 {
    const VERSION: u32 = VERSION;
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(super) struct PresentationV13 {
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
    pub water: Vec<SurfaceV13>,
    pub rooms: Vec<CookedRoom>,
    pub weather_plan: Option<CookedWeatherPlan>,
    pub foliage: Option<CookedZoneFoliage>,
    pub atmosphere: CookedAtmosphere,
}

/// [`CookedWaterSurface`] as versions 9 to 13 wrote it: with no mesh.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(super) struct SurfaceV13 {
    pub id: u64,
    pub material: Option<AssetId>,
    pub probe: Option<u32>,
    pub shore_margin: f32,
}

/// Old surfaces as today's: none drawn.
pub(super) fn surfaces(old: Vec<SurfaceV13>) -> Vec<CookedWaterSurface> {
    let surface = |SurfaceV13 { id, material, probe, shore_margin }| CookedWaterSurface {
        id,
        material,
        probe,
        shore_margin,
        mesh: None,
    };
    old.into_iter().map(surface).collect()
}

/// Today's surfaces as old ones, for tests: their meshes dropped.
#[cfg(test)]
pub(super) fn old_surfaces(surfaces: Vec<CookedWaterSurface>) -> Vec<SurfaceV13> {
    let surface = |CookedWaterSurface { id, material, probe, shore_margin, .. }| SurfaceV13 {
        id,
        material,
        probe,
        shore_margin,
    };
    surfaces.into_iter().map(surface).collect()
}

/// Version 13 bytes as a zone of today's version, unchecked past the archive: the caller writes and reads it
/// back.
pub(super) fn read(bytes: &[u8]) -> Result<Zone, ZoneError> {
    let archived = sapphire_serialize::access::<ZoneV13>(bytes)?;
    let old = rkyv::deserialize::<ZoneV13, rkyv::rancor::Error>(archived)
        .map_err(|error| ZoneError::Cooked(error.into()))?;
    let PresentationV13 {
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
        atmosphere,
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
            water: surfaces(water),
            rooms,
            weather_plan,
            foliage,
            atmosphere,
        },
    })
}

/// `zone` written as version 13 would have, for tests: its surfaces' meshes dropped.
#[cfg(test)]
pub(super) fn write(zone: &Zone) -> sapphire_serialize::cooked::AlignedVec {
    let Zone { simulation, presentation } = zone.clone();
    let old = ZoneV13 {
        simulation,
        presentation: PresentationV13 {
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
            water: old_surfaces(presentation.water),
            rooms: presentation.rooms,
            weather_plan: presentation.weather_plan,
            foliage: presentation.foliage,
            atmosphere: presentation.atmosphere,
        },
    };
    sapphire_serialize::to_cooked(&old).expect("writes")
}
