//! The state checksum: everything that can change a result, as canonical bytes, in a fixed hash.
//!
//! **A component that can change a result joins [`state_checksum`]'s loop.** Forgetting one doesn't
//! break a replay at once: the checksum only sees the difference when it reaches a field that's
//! written here, and later than it could.

use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_moves::{BakedTables, RootSample};
use xxhash_rust::xxh3::Xxh3;

use crate::actions::{ActorActions, Controls};
use crate::actors::{ActorRequests, ActorTable};
use crate::drill::Drill;
use crate::motion::{ActorTransform, RootMotion, TableId, TableLibrary};
use crate::simulation::SimRng;

/// Goes up when [`state_checksum`] starts writing something different, so a recorded checksum can't
/// match by accident. Recorded replays re-record then (`docs/m2-plan.md` §3).
const CHECKSUM_LAYOUT: u32 = 1;

/// A hasher over canonical bytes: XXH3-64, a published, frozen algorithm, unlike `DefaultHasher`,
/// so a recorded checksum stays good.
///
/// Canonical means nothing depends on how the state lies in memory: integers are little-endian,
/// floats go by their bits (one NaN for all, since Rust doesn't promise a NaN's payload), actors
/// are in ID order and a struct's fields in the order the checksum writes them.
pub struct StateHasher(Xxh3);

impl StateHasher {
    /// A hasher with nothing written. On the stack: no call here allocates.
    #[must_use]
    pub fn new() -> Self {
        Self(Xxh3::new())
    }

    /// Writes raw bytes.
    pub fn bytes(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn u8(&mut self, value: u8) {
        self.bytes(&[value]);
    }

    pub fn u16(&mut self, value: u16) {
        self.bytes(&value.to_le_bytes());
    }

    pub fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    /// Writes a float by its bits, so −0 and 0 differ, and any NaN as the same one.
    pub fn f32(&mut self, value: f32) {
        self.u32(if value.is_nan() { f32::NAN.to_bits() } else { value.to_bits() });
    }

    /// Writes each component by [`Self::f32`].
    pub fn f32s(&mut self, values: &[f32]) {
        for &value in values {
            self.f32(value);
        }
    }

    /// Writes a vector's components in order.
    pub fn vec3(&mut self, value: Vec3) {
        self.f32s(&value.to_array());
    }

    /// The hash of everything written so far.
    #[must_use]
    pub fn finish(&self) -> u64 {
        self.0.digest()
    }
}

impl Default for StateHasher {
    fn default() -> Self {
        Self::new()
    }
}

fn write_actions(hasher: &mut StateHasher, actions: ActorActions) {
    hasher.u16(actions.buttons.bits());
    hasher.bytes(&actions.stick.x().to_le_bytes());
    hasher.bytes(&actions.stick.y().to_le_bytes());
}

fn write_root_sample(hasher: &mut StateHasher, sample: RootSample) {
    hasher.f32s(&sample.translation);
    hasher.f32(sample.yaw);
}

/// An optional table, with `u32::MAX` standing for none, which [`TableId`] never reaches.
fn write_table_id(hasher: &mut StateHasher, table: Option<TableId>) {
    hasher.u32(table.map_or(u32::MAX, TableId::raw));
}

/// The checksum of a simulation's whole state.
pub(crate) fn state_checksum(world: &World) -> u64 {
    let mut hasher = StateHasher::new();
    hasher.u32(CHECKSUM_LAYOUT);

    // The tick count is in here, with the rest of the world clock's state.
    world.resource::<WorldClock>().0.write_canonical(&mut |bytes| hasher.bytes(bytes));
    for part in world.resource::<SimRng>().0.state() {
        hasher.u64(part);
    }
    hasher.u32(world.resource::<ActorRequests>().next_id());

    let table = world.resource::<ActorTable>();
    hasher.u32(table.len() as u32);
    for (id, entity) in table.iter() {
        hasher.u32(id.raw());
        let actor = world.entity(entity);
        if let Some(clock) = actor.get::<EntityClock>() {
            clock.0.write_canonical(&mut |bytes| hasher.bytes(bytes));
        }
        if let Some(controls) = actor.get::<Controls>() {
            write_actions(&mut hasher, controls.current);
            write_actions(&mut hasher, controls.previous);
        }
        if let Some(transform) = actor.get::<ActorTransform>() {
            hasher.vec3(transform.position);
            hasher.f32(transform.yaw);
        }
        if let Some(motion) = actor.get::<RootMotion>() {
            write_table_id(&mut hasher, motion.table());
            hasher.u64(motion.started_at().as_subticks());
            hasher.f32(motion.start_yaw());
            hasher.u64(motion.elapsed().as_subticks());
            write_root_sample(&mut hasher, motion.reading());
        }
        if let Some(drill) = actor.get::<Drill>() {
            write_table_id(&mut hasher, drill.table);
            hasher.u8(u8::from(drill.transient));
        }
    }
    hasher.finish()
}

/// The hash of loaded move data, for a replay to check it runs against what it recorded.
pub(crate) fn tables_hash(library: &TableLibrary) -> u64 {
    let mut hasher = StateHasher::new();
    hasher.u32(library.iter().len() as u32);
    for tables in library.iter() {
        write_tables(&mut hasher, tables);
    }
    hasher.finish()
}

fn write_tables(hasher: &mut StateHasher, tables: &BakedTables) {
    hasher.u32(tables.ticks());
    for &sample in tables.root_samples() {
        write_root_sample(hasher, sample);
    }
    hasher.u32(tables.paths().len() as u32);
    for path in tables.paths() {
        hasher.u64(path.name().as_u64());
        for sample in path.samples() {
            hasher.f32s(&sample.translation);
            hasher.f32s(&sample.rotation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(write: impl FnOnce(&mut StateHasher)) -> u64 {
        let mut hasher = StateHasher::new();
        write(&mut hasher);
        hasher.finish()
    }

    /// XXH3-64 of these bytes by the algorithm's definition: a change here moves every recorded
    /// checksum. Streaming in pieces gives the same as hashing at once.
    #[test]
    fn the_hash_is_xxh3_of_the_bytes_in_order() {
        let bytes = [1u8, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12];
        let expected = xxhash_rust::xxh3::xxh3_64(&bytes);
        let streamed = hash(|hasher| {
            hasher.u8(1);
            hasher.u16(0x1234);
            hasher.u32(0x1234_5678);
        });
        assert_eq!(streamed, expected);
        assert_eq!(hash(|_| {}), 0x2d06_8005_38d3_94c2, "XXH3-64 of nothing, as published");
    }

    #[test]
    fn floats_go_by_their_bits() {
        let bits = hash(|hasher| hasher.u32(1.5_f32.to_bits()));
        assert_eq!(hash(|hasher| hasher.f32(1.5)), bits);
        assert_ne!(hash(|hasher| hasher.f32(0.0)), hash(|hasher| hasher.f32(-0.0)), "−0 can diverge later");
        let one_nan = hash(|hasher| hasher.f32(f32::NAN));
        let other_nan = f32::from_bits(f32::NAN.to_bits() | 0x8000_0123);
        assert_eq!(hash(|hasher| hasher.f32(other_nan)), one_nan, "a NaN's payload isn't promised");
    }

    #[test]
    fn vectors_go_in_order() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(hash(|hasher| hasher.vec3(v)), hash(|hasher| hasher.f32s(&[1.0, 2.0, 3.0])));
        assert_ne!(hash(|hasher| hasher.vec3(v)), hash(|hasher| hasher.f32s(&[3.0, 2.0, 1.0])));
    }
}
