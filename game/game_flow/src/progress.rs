//! What a game saves of a run (M3 G1): which zone it's in, B4's [`ZoneStart`] there (its latest checkpoint
//! and the encounters it has cleared) and Q2's story flags. Never a fight in progress (m3-plan §3): a run
//! continued from it starts at that checkpoint with those encounters cleared and those flags set, as a new
//! run, and its file says so ([`RunStart`]).
//!
//! It lives in a [`ProgressStore`], apart from the settings, and a [`Kept`](crate::kept::Kept) writes it
//! off the frame. [`Progress::after`] says when a run has moved on enough to write it again: a checkpoint
//! reached, an encounter cleared or a flag changed.

use std::path::Path;

use bevy_reflect::TypePath;
use sapphire_core::Name;
use sapphire_gameplay::replay::RunStart;
use sapphire_gameplay::{FlagValues, Flags, ZoneStart};
use sapphire_reflect::Versioned;
use sapphire_save::ProgressStore;
use sapphire_serialize::AssetId;
use serde::{Deserialize, Serialize};

use crate::LOG_TARGET;

/// A run's place in the game, as a save holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypePath)]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::game"]
pub struct Progress {
    /// The zone the run is in, by its document's ID.
    pub zone: AssetId,
    /// Where in it a run continued starts.
    pub start: ZoneStart,
    /// The story so far, including flags a newer registry no longer names, which stay in the file.
    pub flags: FlagValues,
}

impl Versioned for Progress {
    /// 1: M3 G1.
    const VERSION: u32 = 1;
}

impl Progress {
    /// A new game's, in `zone`: from where it starts, with nothing cleared and no flag set.
    #[must_use]
    pub fn new_game(zone: AssetId) -> Self {
        Self { zone, start: ZoneStart::default(), flags: FlagValues::default() }
    }

    /// What a run continued from it starts from, if it's a save of `zone`, the flow's: a save of another
    /// zone can't be continued here, and the log says why. Of its flags, those `registered` still names; the
    /// log counts the rest, which the save keeps.
    #[must_use]
    pub fn run_start(&self, zone: AssetId, registered: &[Name]) -> Option<RunStart> {
        if self.zone != zone {
            tracing::warn!(
                target: LOG_TARGET,
                saved = %self.zone,
                flow = %zone,
                "the save is of another zone than the flow's, so there's nothing to continue"
            );
            return None;
        }
        let starting = self.flags.starting(registered);
        if !starting.ignored.is_empty() {
            let ignored = starting.ignored.len();
            tracing::warn!(target: LOG_TARGET, ignored, "the save has flags the registry no longer names");
        }
        Some(RunStart { zone: Some(self.start.clone()), flags: starting.flags })
    }

    /// The progress a run in its zone has made, if that's moved on from this: `start` and `flags` are what the
    /// run holds now (`Simulation::zone_start` and `Simulation::flags`). `None` for nothing new, which writes
    /// nothing.
    #[must_use]
    pub fn after(&self, start: &ZoneStart, flags: &Flags) -> Option<Self> {
        let values = flags.values(Some(&self.flags));
        (self.start != *start || self.flags != values).then(|| Self {
            zone: self.zone,
            start: start.clone(),
            flags: values,
        })
    }
}

/// The game's save, `progress` in `dir`, written by `write`, which a game makes durable.
pub fn progress_store(dir: &Path, write: fn(&Path, &[u8]) -> std::io::Result<()>) -> ProgressStore<Progress> {
    ProgressStore::new(dir, "progress", write)
}

/// The save `store` holds, if it holds one it can read: a damaged or newer one is logged and left alone,
/// and the title offers no continue.
#[must_use]
pub fn load(store: &ProgressStore<Progress>) -> Option<Progress> {
    match store.load() {
        Ok(Some(loaded)) => {
            if loaded.fell_back() {
                tracing::warn!(target: LOG_TARGET, "the newest save didn't read, so the one before it did");
            }
            Some(loaded.value)
        }
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(target: LOG_TARGET, %error, "the save didn't read, so there's nothing to continue");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WARD: AssetId = AssetId::from_text("0b1c2d3e-4f50-4617-8a9b-acbdcedf0f10");
    const COURT: AssetId = AssetId::from_text("1c2d3e4f-5061-4728-9bac-bdcedf0f1021");

    fn reached(checkpoint: &str, cleared: &[AssetId]) -> ZoneStart {
        ZoneStart { checkpoint: Some(checkpoint.to_owned()), cleared: cleared.to_vec() }
    }

    #[test]
    fn a_new_game_starts_where_the_zone_does_and_moves_on_as_the_run_does() {
        let none = Flags::default();
        let new = Progress::new_game(WARD);
        let fresh = RunStart { zone: Some(ZoneStart::default()), flags: Vec::new() };
        assert_eq!(new.run_start(WARD, &[]), Some(fresh));
        assert_eq!(new.after(&ZoneStart::default(), &none), None, "nothing new, nothing written");
        let market = new.after(&reached("market", &[]), &none).expect("a checkpoint reached");
        assert_eq!(market.start.checkpoint.as_deref(), Some("market"));
        let cleared = market.after(&reached("market", &[COURT]), &none).expect("an encounter cleared");
        assert_eq!(cleared.start.cleared, [COURT]);
        assert_eq!(cleared.run_start(COURT, &[]), None, "another zone's save continues nothing here");
    }

    #[test]
    fn a_flag_moves_the_save_on_and_starts_a_continued_run() {
        let answered = Name::new("ward_gate_answered");
        let mut flags = Flags::default();
        let new = Progress::new_game(WARD);
        assert!(flags.set(answered, 2));
        let set = new.after(&ZoneStart::default(), &flags).expect("a flag changed");
        assert_eq!(set.after(&ZoneStart::default(), &flags), None);
        let start = set.run_start(WARD, &[answered]).expect("the flow's zone");
        assert_eq!(start.flags, [(answered, 2)]);
        let start = set.run_start(WARD, &[]).expect("the flow's zone");
        assert!(start.flags.is_empty(), "a flag the registry dropped starts nothing");
    }

    #[test]
    fn a_save_comes_back_from_its_store_as_it_went() {
        let dir = std::env::temp_dir().join(format!("game_flow-progress-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory of its own");
        let store = progress_store(&dir, |path, bytes| std::fs::write(path, bytes));
        assert_eq!(load(&store), None, "nothing saved yet");
        let flags = FlagValues { flags: vec![(Name::new("ward_gate_answered").as_u64(), 1)] };
        let progress = Progress { zone: WARD, start: reached("court", &[COURT]), flags };
        store.write(&store.snapshot(&progress).expect("snapshots")).expect("writes");
        let loaded = load(&store);
        std::fs::remove_dir_all(&dir).expect("cleans up");
        assert_eq!(loaded, Some(progress));
    }
}
