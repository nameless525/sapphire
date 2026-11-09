//! Checkpoints and teleports (spec §5.22, M3 B4): where a walk through a zone starts again, and how.
//!
//! A zone's document lists its checkpoints, and [`crate::Simulation::set_zone`] copies them with the rest of
//! what the simulation reads. From then on:
//!
//! - **Reaching one:** a player, which is to say a soft-locked actor on the field, as a trigger watches,
//!   standing in a checkpoint's volume while no encounter of the zone is running makes it the latest. It's
//!   decided in the step, with the triggers, so it's in the checksum. A save is never of a fight in progress
//!   (m3-plan §3), so a checkpoint inside a running fight waits for the fight to end.
//! - **A save names it:** [`ZoneStart`] is what a zone start reads: the latest checkpoint by name, and the
//!   encounters cleared by ID, names and IDs that outlive a cook's order. [`crate::Simulation::zone_start`]
//!   says what one written now would hold. A game writes it through `sapphire_save`'s `ProgressStore`,
//!   beside the zone's ID and its flags.
//! - **A load restores it:** a new simulation is given its zone, then [`crate::Simulation::start_at`], before
//!   the first step and before the player spawns: the cleared encounters stay cleared, the checkpoint is the
//!   latest, and it says where the player stands. Spawn them there.
//! - **A teleport returns to it,** in the same simulation, between steps:
//!   [`crate::Simulation::return_to_checkpoint`] puts the zone's player at a checkpoint's place. Any actor
//!   can be put anywhere by [`crate::Simulation::teleport`], which leaves it as a spawn there would, by what a
//!   zone entry stands its travellers with (W4's `zone::place`): placed, turned, its controller's state, its
//!   running move, its intents, its push, its input and what its swings remember having hit started over,
//!   its capsule moved with it and no heap taken. Its health, side, clock, brain and soft lock stay, and so
//!   does everything else in the zone. So what it does from
//!   there can't depend on where it came from, and it walks exactly as the player of a load at that
//!   checkpoint does.
//!
//! A replay records a return as a command between two ticks ([`crate::replay::ReplayCommand::ReturnTo`]), by the
//! checkpoint's place in the zone's list, which the zone's digest in `tables_hash` pins.
//!
//! **What a load can't do** is reach the checksum of the run that walked to the checkpoint: a checksum holds
//! the tick count, the RNG, the next actor ID and every clock, and a save holds a zone start, never a whole
//! simulation. What it does match, to the bit, is how the player moves from there (`tests/walks.rs`).

use bevy_reflect::TypePath;
use sapphire_core::Name;
use sapphire_math::Vec3;
use sapphire_reflect::Versioned;
use sapphire_serialize::AssetId;
use sapphire_zone::{ArchivedZoneSimulation, CookedVolume};
use serde::{Deserialize, Serialize};

use crate::actors::ActorId;
use crate::checksum::StateHasher;

/// Where an actor stands and which way it faces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    /// Its feet, in metres.
    pub position: Vec3,
    /// Radians about +Y; 0 faces +Z.
    pub yaw: f32,
}

impl Place {
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.position.is_finite() && self.yaw.is_finite()
    }
}

/// A checkpoint of the simulation's zone, by its place in the zone's list, which is by its name's hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CheckpointId(u32);

impl CheckpointId {
    /// The checkpoint at this place in the list, as a replay's command names it.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// What a zone start reads, and so what a save holds of a zone (m3-plan §3): never a fight in progress. See
/// the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::gameplay"]
pub struct ZoneStart {
    /// The checkpoint to stand at, by name, or `None` for wherever the zone starts a new game.
    pub checkpoint: Option<String>,
    /// The encounters already fought, by their IDs, ascending.
    pub cleared: Vec<AssetId>,
}

impl Versioned for ZoneStart {
    /// 1: M3 B4.
    const VERSION: u32 = 1;
}

/// Why a teleport didn't happen. Nothing moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TeleportError {
    #[error("actor {} isn't alive", .0.raw())]
    UnknownActor(ActorId),
    #[error("a teleport needs a finite place")]
    NotFinite,
    #[error("the simulation has no zone, so no checkpoints")]
    NoZone,
    #[error("the zone has no checkpoint {}", .0.raw())]
    UnknownCheckpoint(CheckpointId),
    #[error("nobody's playing: no actor on the field has a soft lock")]
    NoPlayer,
}

/// Whether `at` stands in `volume` about `position`: plain comparisons, in scalars, axis-aligned.
pub(crate) fn holds(position: [f32; 3], volume: CookedVolume, at: Vec3) -> bool {
    let (x, y, z) = (at.x - position[0], at.y - position[1], at.z - position[2]);
    match volume {
        CookedVolume::Box { half_extents } => {
            x.abs() <= half_extents[0] && y.abs() <= half_extents[1] && z.abs() <= half_extents[2]
        }
        CookedVolume::Sphere { radius } => x * x + y * y + z * z <= radius * radius,
    }
}

#[derive(Clone, Debug)]
struct SimCheckpoint {
    /// Its name's hash, which the list is sorted by.
    id: u64,
    name: String,
    position: [f32; 3],
    yaw: f32,
    volume: CookedVolume,
}

/// A zone's checkpoints as a simulation holds them, and which was reached last.
#[derive(Clone, Debug, Default)]
pub(crate) struct Checkpoints {
    list: Box<[SimCheckpoint]>,
    latest: Option<CheckpointId>,
    /// Times the latest has changed: a report, not in the checksum.
    pub(crate) reached: u64,
}

impl Checkpoints {
    pub(crate) fn new(zone: &ArchivedZoneSimulation) -> Self {
        let list = zone
            .checkpoints
            .iter()
            .map(|checkpoint| SimCheckpoint {
                id: checkpoint.id.to_native(),
                name: checkpoint.name.as_str().to_owned(),
                position: checkpoint.position(),
                yaw: checkpoint.yaw(),
                volume: checkpoint.volume.to_native(),
            })
            .collect();
        Self { list, latest: None, reached: 0 }
    }

    pub(crate) fn len(&self) -> usize {
        self.list.len()
    }

    /// The checkpoint named `name`.
    pub(crate) fn find(&self, name: &str) -> Option<CheckpointId> {
        let index = self.list.binary_search_by_key(&Name::new(name).as_u64(), |entry| entry.id).ok()?;
        (self.list[index].name == name).then_some(CheckpointId(index as u32))
    }

    pub(crate) fn name(&self, id: CheckpointId) -> Option<&str> {
        self.list.get(id.0 as usize).map(|entry| entry.name.as_str())
    }

    pub(crate) fn place(&self, id: CheckpointId) -> Option<Place> {
        let entry = self.list.get(id.0 as usize)?;
        Some(Place { position: Vec3::from_array(entry.position), yaw: entry.yaw })
    }

    pub(crate) const fn latest(&self) -> Option<CheckpointId> {
        self.latest
    }

    /// None reached, for a fight that starts over.
    pub(crate) fn restart(&mut self) {
        self.latest = None;
        self.reached = 0;
    }

    pub(crate) fn set_latest(&mut self, id: CheckpointId) {
        if self.latest != Some(id) {
            self.latest = Some(id);
            self.reached += 1;
        }
    }

    /// The first checkpoint in the list whose volume `at` stands in.
    pub(crate) fn at(&self, at: Vec3) -> Option<CheckpointId> {
        let index = self.list.iter().position(|entry| holds(entry.position, entry.volume, at))?;
        Some(CheckpointId(index as u32))
    }

    /// Nothing until one's been reached, so a zone's run that never reaches one hashes what it did before
    /// there were checkpoints.
    pub(crate) fn write_state(&self, hasher: &mut StateHasher) {
        if let Some(latest) = self.latest {
            hasher.u8(1);
            hasher.u32(latest.0);
        }
    }
}
