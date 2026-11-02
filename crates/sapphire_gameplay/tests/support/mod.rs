//! A scripted fight for the simulation's tests, the same one the replay suite records: the test
//! content's moves, with buttons that slow clocks and spawn actors, on a fixed schedule.

pub mod mesh_arena;

use sapphire_gameplay::replay;
use sapphire_gameplay::{ActorActions, ActorId, Buttons, SimConfig, Simulation, Stick, TickActions};
use sapphire_moves::MoveSet;

/// The test content's document: see that file for its numbers.
#[allow(dead_code, reason = "not every test binary that shares this module edits a document")]
pub const SLICE: &str = include_str!("../../../sapphire_moves/test_content/slice.moves.ron");

/// The test content, as a cook writes it and the game reads it.
#[allow(dead_code, reason = "not every test binary that shares this module plays the test content")]
pub fn slice_cooked() -> sapphire_serialize::cooked::AlignedVec {
    replay::test_moves().expect("compiles").to_cooked().expect("cooks")
}

/// The test content, ready to load.
#[allow(dead_code, reason = "not every test binary that shares this module plays the test content")]
pub fn slice() -> MoveSet {
    MoveSet::from_cooked(&slice_cooked()).expect("reads back")
}

/// A pool with room for all this script's extra actors at their peak.
#[allow(dead_code, reason = "not every test binary that shares this module runs the script")]
pub fn world(seed: u64) -> (Simulation, Vec<ActorId>) {
    let scenario = replay::find_scenario(replay::SCENARIOS, "melee").expect("listed");
    scenario.start(SimConfig { seed, ..scenario.config }).expect("the melee sets up")
}

/// One tick of the script: every button on its own period, and sticks that sweep.
#[allow(dead_code, reason = "not every test binary that shares this module runs the script")]
pub fn script(tick: u64, actors: &[ActorId]) -> TickActions {
    replay::melee_sweep(tick, actors)
}

/// Runs the script for `ticks` and returns the checksum after every tick.
#[allow(dead_code, reason = "not every test binary that shares this module runs it whole")]
pub fn run(sim: &mut Simulation, actors: &[ActorId], ticks: u64) -> Vec<u64> {
    (0..ticks)
        .map(|tick| {
            sim.step(&script(tick, actors));
            sim.checksum()
        })
        .collect()
}

/// One actor pressing these buttons this tick, with the stick at rest.
#[allow(dead_code, reason = "not every test binary that shares this module runs it whole")]
pub fn press(actor: ActorId, buttons: Buttons) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    actions.set(actor, ActorActions { buttons, stick: Stick::CENTER }).expect("room");
    actions
}
