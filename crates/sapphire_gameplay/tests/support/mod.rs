//! A scripted fight for the simulation's tests: everything the drill can do, on a fixed schedule.

use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, SimConfig, Simulation, Stick, TickActions,
};
use sapphire_math::det;
use sapphire_moves::{BakedTables, RootSample};

/// A table with no two readings alike: forward by the tick, sideways by its square, turning slowly,
/// with a pinned sine in it so the reading depends on `libm`.
pub fn curve(ticks: u32) -> BakedTables {
    let root = (0..=ticks)
        .map(|tick| {
            let t = tick as f32;
            RootSample {
                translation: [t * t * 0.01, det::sin(t * 0.3) - det::sin(0.0), t * 0.25],
                yaw: t * 0.02,
            }
        })
        .collect();
    BakedTables::new(root, vec![]).expect("a good table")
}

/// A pool with room for all this script's transient actors at their peak.
pub fn world(seed: u64) -> (Simulation, Vec<ActorId>) {
    let mut sim = Simulation::new(SimConfig { seed, max_actors: 16, ..SimConfig::default() });
    let short = sim.add_tables(curve(20));
    let long = sim.add_tables(curve(90));
    let actors = [(short, -2.0), (long, 0.0), (short, 2.0)]
        .map(|(table, x)| {
            let spawn = ActorSpawn {
                position: sapphire_math::Vec3::new(x, 0.0, 0.0),
                yaw: x * 0.5,
                table: Some(table),
                transient: false,
            };
            sim.spawn(spawn).expect("room in the pool")
        })
        .to_vec();
    (sim, actors)
}

/// One tick of the script: every button on its own period, and sticks that sweep.
pub fn script(tick: u64, actors: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    for (index, &actor) in actors.iter().enumerate() {
        let shifted = tick + index as u64 * 13;
        let every = |period: u64, offset: u64| shifted % period == offset;
        let mut buttons = Buttons::NONE;
        if every(53, 3) {
            buttons = buttons | Buttons::LIGHT;
        }
        if every(71, 9) {
            buttons = buttons | Buttons::HEAVY;
        }
        if every(97, 21) {
            buttons = buttons | Buttons::DODGE;
        }
        if every(131, 33) {
            buttons = buttons | Buttons::SKILL;
        }
        if index == 0 && every(211, 77) {
            buttons = buttons | Buttons::ULTIMATE;
        }
        let stick = Stick::new((shifted % 255) as u8 as i8, (shifted % 101) as u8 as i8);
        actions.set(actor, ActorActions { buttons, stick }).expect("few enough actors");
    }
    actions
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
