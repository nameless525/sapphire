//! Zones as plain data: what the cooker, the simulation and the world agree on (spec §5.21).
//!
//! A zone is a cooked level: geometry cut into cells on a grid, the lights and probes that light it,
//! its spawners, triggers and rails, and references to its collision, navigation and encounters. This
//! crate holds the document and the cooked form and nothing that acts on them, so the cooker can link
//! it without a renderer, a world or a physics engine (#74's lesson), and a replay can link it without
//! a GPU.
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
//! - **Encounters** ([`encounter`], M3 B2): `.encounter.ron`, [`EncounterDocument`], and its cooked form
//!   (`.sencounter`), which a zone lists.
//!
//! # Does not own
//!
//! - Reading sources and sidecars, prefabs, merging placed pieces into a mesh per cell, cache keys and
//!   packs: `sapphire_pipeline`'s `zone` module.
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
//! hashes, so it cooks to the same values on every OS and architecture, and the digest hashes values,
//! not an archive's layout. Presentation's bounds are computed from geometry, whose cook goes through
//! each platform's C math library, so they stay out of the digest and may differ by a few bits.
//! `[simulation]` in `xtask/layers.toml` holds this crate to the simulation's `clippy.toml`.

pub mod cooked;
pub mod document;
pub mod encounter;
pub mod grid;

pub use cooked::{
    ArchivedCookedInstance, ArchivedCookedLight, ArchivedCookedLightShape, ArchivedCookedSpawner,
    ArchivedCookedSpline, ArchivedCookedTrigger, ArchivedCookedVolume, ArchivedPlaceContents, ArchivedZone,
    ArchivedZoneCell, ArchivedZonePresentation, ArchivedZoneSimulation, CookedInstance, CookedLight,
    CookedLightShape, CookedSpawner, CookedSpline, CookedTrigger, CookedVolume, PlaceContents,
    ZONE_EXTENSION, Zone, ZoneCell, ZonePresentation, ZoneSimulation,
};
pub use document::{
    Instance, Light, LightShape, Placement, Spawner, Spline, Split, Trigger, Volume, ZoneDocument, ZoneError,
    ZoneGeometry, ZoneNavigation,
};
pub use encounter::{
    ArchivedCookedWave, ArchivedEncounter, ArchivedWaveStart, CookedWave, ENCOUNTER_EXTENSION, Encounter,
    EncounterDocument, MAX_FACTION, MAX_TOKENS, MAX_WAVE_SPAWNS, MAX_WAVES, Wave, WaveStart,
};
pub use grid::{Bounds, CellCoord, Grid, Place};

/// The pack a zone's own assets go in, from the zone source's file stem (`town.zone.ron` →
/// `zone_town`): the packer writes it and whoever loads the zone mounts it.
#[must_use]
pub fn pack_name(stem: &str) -> String {
    format!("zone_{stem}")
}
