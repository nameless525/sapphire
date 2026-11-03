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
pub mod document;
mod file;
mod play;
mod scenario;
mod script;
mod tuning;
mod ward;

pub use checksums::{
    ARCHITECTURE, CHECKPOINT_EVERY, ChecksumFile, ChecksumsError, Divergence, RecordedTrail, Trail,
    checksums_file_name,
};
pub use file::{
    FORMAT_VERSION, MAX_ACTORS, MAX_NAME_LEN, MAX_TICKS, PLAIN_FORMAT_VERSION, Recorder, Replay, ReplayError,
    is_valid_name,
};
pub use play::{PlayError, Played, Player, Recording, StepAllocated};
pub use scenario::{
    FightWorld, PARTY_TUNING, PLAYERS, PreparedFight, RIVALS, SCENARIOS, Scenario, ScenarioError,
    ScenarioSetup, ULTIMATE_CINEMATIC, WARRIOR_MOVES, WARRIORS, add_melee_buttons, arena_fight,
    document_file_name, duel, find_scenario, melee, melee_buttons, party_document, party_moves, player_moves,
    test_moves, timelines, ultimate_cinematic, warrior_moves,
};
pub use script::{
    ARENA_ULTIMATE_AT, ARENA_WALK_JUMP, ARENA_WALK_LEGS, ATTEMPT_EVERY, ActionSource, CINEMATIC, CUTSCENE,
    SCRIPTS, Script, arena_ultimate, arena_walk, attempts, fight_back, find_script, idle, into_the_yard,
    melee_sweep,
};
pub use tuning::{LiveMoves, LiveSet, compile_document};
pub use ward::{
    COURT_GATE_ID, COURT_GATE_PATH, COURT_GATE_TICK, PLAYER_SPAWNER, WARD_COLLIDERS_PATH, WARD_COURT,
    WARD_ZONE_PATH, WARD_ZONE_STEM, WARRIOR_SPAWNER, ZoneFight, ward_colliders, ward_court, ward_spawns,
    zone_fight,
};
