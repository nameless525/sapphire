//! Replays: a recorded run's actions, what it must reach, and how to run it (spec §9, gates 2 and 4).
//!
//! A [`Replay`] is a versioned file of per-tick actions plus what makes the run reproducible: the
//! [`Scenario`] it starts from, the seed and pool, and the move data's hash. It holds no results, so
//! only an intended change moves anything. What a run must reach is a [`ChecksumFile`], one per
//! architecture, whose checkpoints name the first tick a run differs at.
//!
//! [`Player`] runs a replay a tick at a time, and [`Recording`] makes one from an [`ActionSource`]. A
//! headless runner (`tools/sapphire_replay`, `cargo xtask replay`) and the sandbox beside its renderer
//! both step these, so they can't disagree about what running a replay means.
//!
//! This module reads and writes bytes and text and never touches the file system.
//!
//! ```
//! use sapphire_gameplay::replay::{Player, RecordedTrail, Recording, Replay, SCENARIOS, find_script};
//!
//! // Record two seconds of a script from a scenario.
//! let scenario = &SCENARIOS[0];
//! let mut recording = Recording::new(scenario, scenario.config).unwrap();
//! let mut source = find_script("melee-play").unwrap().actions;
//! for _ in 0..120 {
//!     recording.step(&mut source).unwrap();
//! }
//! let (replay, played) = recording.finish();
//! let recorded = RecordedTrail::from_trail(&played.trail);
//!
//! // The file, read back and run, reaches the same checksums.
//! let bytes = replay.encode();
//! let mut player = Player::new(Replay::decode(&bytes).unwrap(), &SCENARIOS).unwrap();
//! player.run_to_end();
//! assert_eq!(recorded.first_difference(&player.finish().trail), None);
//! ```

mod chase;
mod checksums;
pub mod cinematics;
pub mod document;
mod file;
mod play;
mod scenario;
mod script;
mod tuning;
pub mod walks;
mod zone_fight;

pub use chase::{CHASERS, chase_world};
pub use checksums::{
    ARCHITECTURE, CHECKPOINT_EVERY, ChecksumFile, ChecksumsError, Divergence, RecordedTrail, Trail,
    checksums_file_name,
};
pub use cinematics::{Cinematic, add_cinematics, cinematics_of};
pub use document::KindOfEnemy;
pub use file::{
    COMMANDS_FORMAT_VERSION, FLAGS_FORMAT_VERSION, FORMAT_VERSION, MAX_ACTORS, MAX_NAME_LEN, MAX_TICKS,
    PLAIN_FORMAT_VERSION, Recorder, Replay, ReplayCommand, ReplayError, is_valid_name,
};
pub use play::{PlayError, Played, Player, Recording, RunStart, StepAllocated};
pub use scenario::{
    BAKED_ZONES, BakedZone, FightWorld, PARTY_TUNING, PLAYER_MOVES_PATH, PLAYERS, PreparedFight, RIVALS,
    SCENARIOS, Scenario, ScenarioError, ScenarioSetup, WARRIOR_ENEMY, WARRIOR_MOVES, WARRIORS,
    add_melee_buttons, add_warrior, arena_fight, document_file_name, duel, find_scenario, melee,
    melee_buttons, party_document, party_moves, player_moves, test_moves, timelines, warrior_enemy,
    warrior_moves,
};
pub use script::{
    ARENA_ENERGY_TICKS, ARENA_ENERGY_TRIES_EVERY, ARENA_ULTIMATE_AT, ARENA_WALK_JUMP, ARENA_WALK_LEGS,
    ATTEMPT_EVERY, ActionSource, CHOICE_AT, CINEMATIC, COURT_GATE_ID, COURT_GATE_TICK, CUTSCENE,
    GROUND_START, HUB_FROM_GROUND, HUB_START, LIGHT_THE_WARD, PROVING_GROUND, PROVING_HUB, SCRIPTS, Script,
    ZONE_TRIP_ENTRIES, arena_energy, arena_ultimate, arena_walk, attempts, fight_back, find_script, idle,
    into_the_yard, melee_sweep, up_the_street,
};
pub use tuning::{LiveMoves, LiveSet, compile_document};
pub use zone_fight::{ZoneFight, zone_fight};
