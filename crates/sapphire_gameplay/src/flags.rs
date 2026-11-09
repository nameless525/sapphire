//! Flags (spec §5.24, M3 Q2): whole numbers by name that a game's story keeps, which dialogue sets and
//! scripts read. They're the simulation's, so a fight that reads one replays: a fixed table of at most
//! [`MAX_FLAGS`], sorted by name, in the checksum once anything is in it, and so a world that never
//! touches a flag hashes as it always did.
//!
//! **Where a flag changes:**
//!
//! - Before a run, from a save: [`Simulation::start_with_flags`](crate::Simulation::start_with_flags). A
//!   replay keeps those in its header, as the state it starts from, not as something anyone did.
//! - Between steps, from a dialogue's effect:
//!   [`Simulation::apply_dialogue_effect`](crate::Simulation::apply_dialogue_effect), which a replay
//!   records as a command.
//! - Inside a step, from a script's `set_flag`, applied with the rest of a hook's commands.
//!
//! A flag nobody set reads 0. The cook holds a game to [`MAX_FLAGS`] names (`content/flags.ron`), so a
//! full table is a script naming flags the registry doesn't: [`Flags::refused`] counts those writes, in
//! the checksum, rather than dropping one silently.

use bevy_reflect::TypePath;
use sapphire_core::Name;
pub use sapphire_dialogue::MAX_FLAGS;
use sapphire_ecs::prelude::*;
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use crate::checksum::StateHasher;

/// The simulation's flags: see the `flags` module's docs.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct Flags {
    /// By name's hash, ascending, up to `len`.
    entries: [(u64, i32); MAX_FLAGS],
    len: usize,
    refused: u64,
}

impl Default for Flags {
    fn default() -> Self {
        Self { entries: [(0, 0); MAX_FLAGS], len: 0, refused: 0 }
    }
}

impl Flags {
    /// A flag's value, or 0 if nothing has set it.
    #[must_use]
    pub fn get(&self, name: Name) -> i32 {
        let live = &self.entries[..self.len];
        live.binary_search_by_key(&name.as_u64(), |&(hash, _)| hash).map_or(0, |at| live[at].1)
    }

    /// Sets a flag, in place: no allocation, so a step can. `false`, counted, if the table is full of
    /// other flags.
    pub fn set(&mut self, name: Name, value: i32) -> bool {
        let hash = name.as_u64();
        match self.entries[..self.len].binary_search_by_key(&hash, |&(hash, _)| hash) {
            Ok(at) => {
                self.entries[at].1 = value;
                true
            }
            Err(_) if self.len == MAX_FLAGS => {
                self.refused += 1;
                false
            }
            Err(at) => {
                self.entries.copy_within(at..self.len, at + 1);
                self.entries[at] = (hash, value);
                self.len += 1;
                true
            }
        }
    }

    /// Every flag something has set, by name's hash, ascending.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (Name, i32)> + '_ {
        self.entries[..self.len].iter().map(|&(hash, value)| (Name::from_u64(hash), value))
    }

    /// How many flags something has set.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing has set any flag, nor tried: then the checksum holds nothing of them.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0 && self.refused == 0
    }

    /// Writes to a full table that went nowhere (rule R9).
    #[must_use]
    pub const fn refused(&self) -> u64 {
        self.refused
    }

    /// Makes this the same table as `other`, copying only what's used: what a hook's frame reads.
    pub fn copy_from(&mut self, other: &Self) {
        self.entries[..other.len].copy_from_slice(&other.entries[..other.len]);
        self.len = other.len;
        self.refused = other.refused;
    }

    /// What a save keeps of them: everything set, and whatever `previous` held that isn't, so a flag a
    /// newer registry dropped, and which this run never set, isn't lost from the player's file.
    #[must_use]
    pub fn values(&self, previous: Option<&FlagValues>) -> FlagValues {
        let mut flags: Vec<(u64, i32)> = self.entries[..self.len].to_vec();
        for &(hash, value) in previous.map_or(&[][..], |previous| &previous.flags) {
            if let Err(at) = flags.binary_search_by_key(&hash, |&(hash, _)| hash) {
                flags.insert(at, (hash, value));
            }
        }
        FlagValues { flags }
    }

    pub(crate) fn write_canonical(&self, hash: &mut StateHasher) {
        hash.u32(self.len as u32);
        for &(name, value) in &self.entries[..self.len] {
            hash.u64(name);
            hash.u32(value as u32);
        }
        hash.u64(self.refused);
    }
}

/// Flags as a save keeps them (`sapphire_save`'s `ProgressStore`): each name's hash and its value,
/// ascending. A release build keeps no names' text, so a save can't either.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::gameplay"]
#[serde(deny_unknown_fields)]
pub struct FlagValues {
    pub flags: Vec<(u64, i32)>,
}

impl Versioned for FlagValues {
    /// 1: M3 Q2.
    const VERSION: u32 = 1;
}

/// What a run starts from of a save's flags: [`FlagValues::starting`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartingFlags {
    /// For [`Simulation::start_with_flags`](crate::Simulation::start_with_flags), and a replay's header.
    pub flags: Vec<(Name, i32)>,
    /// Flags the save has and the registry doesn't any more, by hash: kept in the save, set nowhere, and
    /// for the caller to warn of.
    pub ignored: Vec<u64>,
}

impl FlagValues {
    /// Splits a save's flags into those `registered` still names, which a run starts from, and those it
    /// doesn't, which it ignores. A value of 0 is what an unset flag reads, so it starts nothing.
    #[must_use]
    pub fn starting(&self, registered: &[Name]) -> StartingFlags {
        let mut starting = StartingFlags::default();
        for &(hash, value) in &self.flags {
            if !registered.iter().any(|name| name.as_u64() == hash) {
                starting.ignored.push(hash);
            } else if value != 0 {
                starting.flags.push((Name::from_u64(hash), value));
            }
        }
        starting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_reads_what_was_set_and_zero_before() {
        let mut flags = Flags::default();
        assert!(flags.is_empty());
        assert_eq!(flags.get(Name::new("ward_lit")), 0);
        for (name, value) in [("c", 3), ("a", 1), ("b", -2), ("a", 5)] {
            assert!(flags.set(Name::new(name), value));
        }
        assert_eq!(flags.len(), 3);
        assert_eq!(flags.get(Name::new("a")), 5);
        assert_eq!(flags.get(Name::new("b")), -2);
        let hashes: Vec<u64> = flags.iter().map(|(name, _)| name.as_u64()).collect();
        assert!(hashes.is_sorted(), "kept in order, whatever order they came in");
    }

    #[test]
    fn a_full_table_refuses_and_counts_rather_than_drops_one_silently() {
        let mut flags = Flags::default();
        for index in 0..MAX_FLAGS {
            assert!(flags.set(Name::new(&format!("flag_{index}")), 1));
        }
        assert!(flags.set(Name::new("flag_7"), 2), "one it has still sets");
        assert!(!flags.set(Name::new("one_more"), 1));
        assert_eq!((flags.len(), flags.refused()), (MAX_FLAGS, 1));
    }

    #[test]
    fn the_order_flags_were_set_in_is_not_in_the_checksum_and_everything_else_is() {
        let hash = |flags: &Flags| {
            let mut hasher = StateHasher::new();
            flags.write_canonical(&mut hasher);
            hasher.finish()
        };
        let (mut one, mut two) = (Flags::default(), Flags::default());
        one.set(Name::new("a"), 1);
        one.set(Name::new("b"), 2);
        two.set(Name::new("b"), 2);
        two.set(Name::new("a"), 1);
        assert_eq!(hash(&one), hash(&two));
        two.set(Name::new("a"), 0);
        assert_ne!(hash(&one), hash(&two), "a value");
        let mut copied = Flags::default();
        copied.copy_from(&one);
        assert_eq!(copied, one);
    }

    #[test]
    fn a_save_keeps_flags_the_registry_dropped_and_a_run_ignores_them() {
        let mut flags = Flags::default();
        flags.set(Name::new("ward_lit"), 1);
        let gone = Name::new("old_quest").as_u64();
        let previous = FlagValues { flags: vec![(gone, 4)] };
        let values = flags.values(Some(&previous));
        assert_eq!(values.flags.len(), 2);
        let starting = values.starting(&[Name::new("ward_lit"), Name::new("met_guide")]);
        assert_eq!(starting.flags, [(Name::new("ward_lit"), 1)]);
        assert_eq!(starting.ignored, [gone]);
    }
}
