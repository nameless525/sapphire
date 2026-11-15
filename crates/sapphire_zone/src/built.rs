//! A zone that code builds rather than a document (M5 B5): what a scenario naming it stands its simulation on.
//!
//! The benchmark district's simulation half is built in code (m5-plan §3, "The simulation's data"), in a crate
//! no shipped build links, so a scenario that fights there can't find it among the documents a build compiles
//! in. Whoever sets such a scenario up hands over the builder; this is what a builder makes, in the types the
//! simulation already reads: the zone's document (its simulation half), the encounters it lists, and its collider
//! set and navmesh, cooked.

use sapphire_serialize::AssetId;
use sapphire_serialize::cooked::AlignedVec;

use crate::{EncounterDocument, ZoneDocument};

/// What a zone's builder makes (see the [module docs](self)).
#[derive(Clone, Debug)]
pub struct BuiltZone {
    /// The zone, of which the simulation reads its half: its `colliders` names [`Self::colliders`], and its
    /// `encounters` are [`Self::encounters`]' IDs.
    pub document: ZoneDocument,
    /// Each encounter the zone lists, by its ID.
    pub encounters: Vec<(AssetId, EncounterDocument)>,
    /// The collider set the zone's `colliders` names, cooked.
    pub colliders: AlignedVec,
    /// Its navmesh, cooked, if it has one: given to the simulation beside the zone, since a cook bakes a document
    /// zone's, which nothing here does.
    pub navmesh: Option<AlignedVec>,
}

/// A zone's builder: given the kinds of enemy a scenario names, by their names, it builds the zone with its
/// encounters' spawners spawning those, or says why it can't.
pub type BuildZone = fn(enemies: &[&str]) -> Result<BuiltZone, String>;
