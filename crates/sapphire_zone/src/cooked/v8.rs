//! Version 8 of the cooked zone (M5 L16), frozen, read for one release after version 9 (cooker rule 2): the
//! same halves with no water. Every part kept its layout, so it shares them.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::AssetId;

use super::{
    CookedCheckpoint, CookedExit, CookedInstance, CookedLight, CookedPatrol, CookedSpawner, CookedSpline,
    CookedTrigger, PlaceContents, Zone, ZoneCell, ZoneError, ZonePresentation, ZoneSimulation,
};
use crate::atmosphere::CookedAtmosphere;
use crate::look::{ZoneFog, ZoneWeather, ZoneWind};

/// The version this reads.
pub(super) const VERSION: u32 = 8;

/// [`Zone`] as version 8 wrote it, under its name.
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[type_path = "sapphire::zone"]
#[type_name = "Zone"]
pub(super) struct ZoneV8 {
    pub simulation: SimulationV8,
    pub presentation: PresentationV8,
}

impl Versioned for ZoneV8 {
    const VERSION: u32 = VERSION;
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(super) struct SimulationV8 {
    pub colliders: Option<AssetId>,
    pub spawners: Vec<CookedSpawner>,
    pub patrols: Vec<CookedPatrol>,
    pub triggers: Vec<CookedTrigger>,
    pub checkpoints: Vec<CookedCheckpoint>,
    pub navigation: Option<AssetId>,
    pub encounters: Vec<AssetId>,
    pub exits: Vec<CookedExit>,
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub(super) struct PresentationV8 {
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
}

/// Version 8 bytes as a zone of today's version, unchecked past the archive: the caller writes and reads it
/// back.
pub(super) fn read(bytes: &[u8]) -> Result<Zone, ZoneError> {
    let archived = sapphire_serialize::access::<ZoneV8>(bytes)?;
    let old = rkyv::deserialize::<ZoneV8, rkyv::rancor::Error>(archived)
        .map_err(|error| ZoneError::Cooked(error.into()))?;
    let SimulationV8 { colliders, spawners, patrols, triggers, checkpoints, navigation, encounters, exits } =
        old.simulation;
    let PresentationV8 {
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
    } = old.presentation;
    Ok(Zone {
        simulation: ZoneSimulation {
            colliders,
            spawners,
            patrols,
            triggers,
            checkpoints,
            navigation,
            encounters,
            exits,
            water: Vec::new(),
        },
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
            water: Vec::new(),
            rooms: Vec::new(),
            weather_plan: None,
            foliage: None,
            atmosphere: CookedAtmosphere::default(),
        },
    })
}

/// `zone` written as version 8 would have, for tests: its water dropped.
#[cfg(test)]
pub(super) fn write(zone: &Zone) -> sapphire_serialize::cooked::AlignedVec {
    let Zone { simulation, presentation } = zone.clone();
    let old = ZoneV8 {
        simulation: SimulationV8 {
            colliders: simulation.colliders,
            spawners: simulation.spawners,
            patrols: simulation.patrols,
            triggers: simulation.triggers,
            checkpoints: simulation.checkpoints,
            navigation: simulation.navigation,
            encounters: simulation.encounters,
            exits: simulation.exits,
        },
        presentation: PresentationV8 {
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
        },
    };
    sapphire_serialize::to_cooked(&old).expect("writes")
}
