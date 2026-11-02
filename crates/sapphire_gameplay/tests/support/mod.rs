//! A scripted fight for the simulation's tests, the same one the replay suite records: everything
//! the drill can do, on a fixed schedule.

use sapphire_gameplay::replay;
use sapphire_gameplay::{ActorId, SimConfig, Simulation, TickActions};
use sapphire_moves::BakedTables;

/// A table with no two readings alike.
pub fn curve(ticks: u32) -> BakedTables {
    replay::drill_curve(ticks)
}

/// A pool with room for all this script's transient actors at their peak.
pub fn world(seed: u64) -> (Simulation, Vec<ActorId>) {
    let scenario = replay::find_scenario(replay::SCENARIOS, "drill").expect("listed");
    scenario.start(SimConfig { seed, ..scenario.config }).expect("the drill sets up")
}

/// One tick of the script: every button on its own period, and sticks that sweep.
pub fn script(tick: u64, actors: &[ActorId]) -> TickActions {
    replay::drill_sweep(tick, actors)
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
