//! How the simulation reads a baked table: root motion on an actor's own clock.
//!
//! This is the plan's "the simulation reads baked tables, never poses" at its smallest. Each step
//! the reading is taken at the clock's whole sub-ticks since the move began, so hitstop and time
//! scales reach the table exactly, and what moves the actor is the difference between two readings,
//! which telescopes: however a clock stops and slows, a table's whole length moves an actor by the
//! table's own last sample.

use sapphire_ecs::prelude::*;
use sapphire_math::{Vec3, det};
use sapphire_moves::{BakedTables, RootSample};
use sapphire_time::{SimDuration, SimTime};

use crate::actors::{ActorRequests, ActorTable};
use crate::drill::Drill;

/// Which baked tables, in a [`TableLibrary`]. Handed out in the order tables were added, so a replay
/// that adds them in the same order gets the same IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TableId(pub(crate) u32);

impl TableId {
    /// This ID's number, as state checksums and tools show it.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// The baked tables a simulation has loaded. Added between steps, read in them.
#[derive(Resource, Debug, Default)]
pub struct TableLibrary {
    tables: Vec<BakedTables>,
}

impl TableLibrary {
    pub(crate) fn add(&mut self, tables: BakedTables) -> TableId {
        let id = TableId(u32::try_from(self.tables.len()).expect("fewer than 2³² moves"));
        self.tables.push(tables);
        id
    }

    /// The tables behind an ID.
    #[must_use]
    pub fn get(&self, id: TableId) -> Option<&BakedTables> {
        self.tables.get(id.0 as usize)
    }

    /// Every table, in ID order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &BakedTables> {
        self.tables.iter()
    }
}

/// Where an actor is, as the fixed step left it. Scalar on purpose: `Vec3` adds the same way on every
/// architecture, which `Vec3A` doesn't promise (spec §5.4).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct ActorTransform {
    pub position: Vec3,
    /// Radians about +Y, unwrapped; 0 faces +Z.
    pub yaw: f32,
}

/// An actor's progress along a baked table.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct RootMotion {
    table: Option<TableId>,
    started_at: SimTime,
    /// The yaw at the move's first tick: the table's root space (see `sapphire_moves`).
    start_yaw: f32,
    elapsed: SimDuration,
    reading: RootSample,
}

impl RootMotion {
    /// Begins `table` at the owner's time `now`. The first reading is the table's zero, this same
    /// step, and it moves the actor from the next step on.
    pub fn start(&mut self, table: TableId, now: SimTime, yaw: f32) {
        *self = Self { table: Some(table), started_at: now, start_yaw: yaw, ..Self::default() };
    }

    /// The table being followed, or `None` once it has ended.
    #[must_use]
    pub const fn table(&self) -> Option<TableId> {
        self.table
    }

    /// The owner's time when the table began.
    #[must_use]
    pub const fn started_at(&self) -> SimTime {
        self.started_at
    }

    /// The yaw at the table's first tick.
    #[must_use]
    pub const fn start_yaw(&self) -> f32 {
        self.start_yaw
    }

    /// The owner's time from the table's first tick to the last reading.
    #[must_use]
    pub const fn elapsed(&self) -> SimDuration {
        self.elapsed
    }

    /// The last reading of the table.
    #[must_use]
    pub const fn reading(&self) -> RootSample {
        self.reading
    }
}

/// [`crate::CombatSet::Movement`]: each actor reads its table at its clock's time and moves by the
/// difference from its last reading. Drill actors that end a transient table queue their despawn.
pub(crate) fn follow_root_motion(
    table: Res<'_, ActorTable>,
    library: Res<'_, TableLibrary>,
    mut requests: ResMut<'_, ActorRequests>,
    mut actors: Query<'_, '_, (&EntityClock, &mut RootMotion, &mut ActorTransform, &Drill)>,
) {
    for (id, entity) in table.iter() {
        let Ok((clock, mut motion, mut transform, drill)) = actors.get_mut(entity) else { continue };
        let Some(tables) = motion.table.and_then(|table| library.get(table)) else { continue };

        let elapsed = clock.0.now().since(motion.started_at);
        let reading = tables.root_at(elapsed);
        let moved = reading.offset() - motion.reading.offset();
        transform.position += det::rotate_y(moved, motion.start_yaw);
        transform.yaw += reading.yaw - motion.reading.yaw;
        motion.elapsed = elapsed;
        motion.reading = reading;

        if elapsed >= tables.duration() {
            motion.table = None;
            if drill.transient {
                requests.despawn(id, &table);
            }
        }
    }
}
