//! Zones as plain data: what the cooker, the simulation and the world agree on (spec §5.21).
//!
//! A zone is a cooked level: geometry cut into cells on a grid, the lights and probes that light it,
//! its spawners, triggers, checkpoints and rails, and references to its collision, navigation and
//! encounters. This crate holds the document and the cooked form and nothing that acts on them, so the
//! cooker can link it without a renderer, a world or a physics engine (#74's lesson), and a replay can
//! link it without a GPU.
//!
//! # Owns
//!
//! - **The document** ([`document`]): `.zone.ron`, [`ZoneDocument`], and its checks, which name the
//!   field.
//! - **The grid** ([`grid`]): which [`Place`] a position is in, and how far past it a place's content
//!   may reach.
//! - **The cooked format** ([`cooked`]): [`Zone`], an rkyv archive (`.szone`), read in place, split
//!   into what the simulation reads ([`ZoneSimulation`]) and what only presentation does
//!   ([`ZonePresentation`]).
//! - **The digest** ([`ArchivedZone::simulation_digest`]): what a replay pins.
//! - **A zone's air** ([`look`], M5 M6): its wind, weather and fog, beside its day, rigs, probes and sky in
//!   [`ZonePresentation`].
//! - **Reflection probes** ([`reflections`], M5 L2): where a zone's reflections are baked from and the box
//!   each is corrected against, at most [`reflections::MAX_PROBES`]. The cooked zone names the asset the cook
//!   bakes of them (`sapphire_assets::reflections`).
//! - **Ambient characters** ([`ambient`], M5 C10): who stands about a zone and walks it, as presentation
//!   alone, cooked to an asset of the zone's own (`.sambient`) beside its cells, never into [`Zone`].
//! - **Water** ([`water`], M6 W2): lakes, rivers and the sea, their volumes in [`ZoneSimulation`] and their
//!   surfaces in [`ZonePresentation`], and [`WaterVolumes::at`], the query a fixed step may ask.
//! - **Rooms** ([`rooms`], M6 U5): where a zone's sound takes a reverb, each a volume with a priority, a reverb
//!   of the mix's, an outdoor flag and a fade, in [`ZonePresentation`] alone.
//! - **A weather plan** ([`weather`], M6 W8a): which of a weather document's states a zone's sky shows by
//!   the hour (a seeded schedule) and by place (regions), in [`ZonePresentation`] alone.
//! - **The volume shape** ([`shape`], M6 W8a): [`ZoneShape`], the one shape a zone places a volume of air by
//!   (a sphere, or a box or an ellipsoid turned about +Y), which the world measures distances from.
//! - **Foliage** ([`foliage`], M6 G1a): `.foliage.ron`, [`FoliageDocument`], its cooked kinds (`.sfoliage`)
//!   and a cell's scatter set (`.sscatter`) of 16-byte instances with stable IDs, and the pure placement a cook
//!   makes them by ([`foliage::place_rules`], [`foliage::place_grass`]). A zone names one by its `foliage`;
//!   its sets are presentation alone.
//! - **Encounters** ([`encounter`], M3 B2): `.encounter.ron`, [`EncounterDocument`], and its cooked form
//!   (`.sencounter`), which a zone lists.
//! - **Prefabs** ([`prefab`], M6 T4): `.assembly.ron`, [`Assembly`], as plain data: what a zone places as a
//!   prefab, and what the cooker builds into a mesh.
//!
//! # Does not own
//!
//! - Reading sources and sidecars, merging placed pieces into a mesh per cell, cache keys and packs:
//!   `sapphire_pipeline`'s `zone` module; building a prefab's triangles, its `assembly` module.
//! - Asset kinds and residency: `sapphire_assets`, which reads this crate's format as
//!   `AssetKind::Zone`.
//! - Activating cells: `sapphire_world`. Spawning, firing triggers and running encounters:
//!   `sapphire_gameplay`.
//!
//! # The simulation streams nothing
//!
//! A zone's colliders, spawners, triggers, navmesh and encounters load before its first step
//! and stay (m3-plan §3). So they are a zone's *hard* references, which loading it loads
//! ([`ArchivedZone::references`]), and the cells' meshes are *soft* ones, which it only names
//! ([`ArchivedZone::presentation_references`]), for the world to ask for and let go of.
//!
//! # Determinism
//!
//! What the simulation reads is parsed from decimal text and copied, with nothing computed from it but
//! hashes and a water body's bounds (comparisons, a halving and a subtraction, each correctly rounded),
//! so it cooks to the same values on every OS and architecture, and the digest hashes values,
//! not an archive's layout. Presentation's bounds are computed from geometry, whose cook goes through
//! each platform's C math library, so they stay out of the digest and may differ by a few bits.
//! `[simulation]` in `xtask/layers.toml` holds this crate to the simulation's `clippy.toml`.

pub mod ambient;
pub mod built;
pub mod cooked;
pub mod document;
pub mod encounter;
pub mod foliage;
pub mod grid;
pub mod look;
pub mod prefab;
pub mod reflections;
pub mod rooms;
pub mod shape;
pub mod water;
pub mod weather;

pub use ambient::{
    AMBIENT_EXTENSION, AMBIENT_PART, Ambient, ArchivedAmbient, ArchivedCookedCastMember, ArchivedCookedPost,
    ArchivedCookedWalk, CastMember, CookedCastMember, CookedPost, CookedWalk, Post, Walk, ZoneAmbient,
};
pub use built::{BuildZone, BuiltZone};
pub use cooked::{
    ArchivedCookedCheckpoint, ArchivedCookedExit, ArchivedCookedInstance, ArchivedCookedLight,
    ArchivedCookedLightShape, ArchivedCookedPatrol, ArchivedCookedSpawner, ArchivedCookedSpline,
    ArchivedCookedTrigger, ArchivedCookedVolume, ArchivedPlaceContents, ArchivedZone, ArchivedZoneCell,
    ArchivedZonePresentation, ArchivedZoneSimulation, CookedCheckpoint, CookedExit, CookedInstance,
    CookedLight, CookedLightShape, CookedPatrol, CookedScatterCell, CookedSpawner, CookedSpline,
    CookedTrigger, CookedVolume, CookedZoneFoliage, PlaceContents, ZONE_EXTENSION, Zone, ZoneCell,
    ZonePresentation, ZoneSimulation,
};
pub use document::{
    Checkpoint, Exit, Instance, Light, LightShape, Placement, Spawner, Spline, Split, Trigger, Volume,
    ZoneDocument, ZoneError, ZoneGeometry, ZoneNavigation,
};
pub use encounter::{
    ArchivedCookedWave, ArchivedEncounter, ArchivedWaveStart, CookedWave, ENCOUNTER_EXTENSION, Encounter,
    EncounterDocument, MAX_FACTION, MAX_TOKENS, MAX_WAVE_SPAWNS, MAX_WAVES, Wave, WaveStart,
};
pub use foliage::{
    ArchivedFoliage, ArchivedScatterSet, FOLIAGE_EXTENSION, Foliage, FoliageDocument, SCATTER_EXTENSION,
    ScatterInstance, ScatterSet,
};
pub use grid::{Bounds, CellCoord, Grid, Place};
pub use look::{ArchivedZoneFog, ArchivedZoneWeather, ArchivedZoneWind, ZoneFog, ZoneWeather, ZoneWind};
pub use prefab::Assembly;
pub use reflections::{ParallaxBox, ReflectionProbe};
pub use rooms::{ArchivedCookedRoom, CookedRoom, ZoneRoom};
pub use shape::{ArchivedZoneShape, ZoneShape};
pub use water::{
    ArchivedCookedWaterSurface, ArchivedCookedWaterVolume, CookedWaterSurface, CookedWaterVolume, Floor,
    RiverPoint, WaterBody, WaterKind, WaterSample, WaterShape, WaterSurface, WaterVolumes,
};
pub use weather::{
    ArchivedCookedWeatherPlan, ArchivedCookedWeatherRegion, CookedWeatherPlan, CookedWeatherRegion,
    WeatherChoice, WeatherRegion, WeatherSlot, ZoneWeatherPlan,
};

/// The pack a zone's own assets go in, from the zone source's file stem (`town.zone.ron` →
/// `zone_town`): the packer writes it and whoever loads the zone mounts it.
#[must_use]
pub fn pack_name(stem: &str) -> String {
    format!("zone_{stem}")
}
