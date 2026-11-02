//! A scripted fight for the simulation's tests, the same one the replay suite records: the test
//! content's moves, with buttons that do to clocks and the pool what hits will, on a fixed schedule.

use sapphire_core::Name;
use sapphire_ecs::EntityClock;
use sapphire_ecs::prelude::*;
use sapphire_gameplay::replay;
use sapphire_gameplay::{
    ActorActions, ActorId, ActorTable, ActorTransform, Buttons, CombatSet, HitQuery, HurtboxId, MoveLibrary,
    MoveOutput, MoveRunner, PublishedMoves, SimConfig, Simulation, Stick, TickActions, published_hurtbox,
};
use sapphire_moves::{MoveSet, PathSample};
use sapphire_physics::{PhysicsWorld, Shape};

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

/// What M2 G3's system comes to: in [`CombatSet::Hits`], each actor's one hurtbox made what its moves
/// say it is, then everything they published swept. G3 sets hurtboxes in `Movement`, which this has
/// no way to order itself against, and keeps its physics world somewhere better than a closure.
#[allow(dead_code, reason = "not every test binary that shares this module sweeps")]
pub fn join_moves_to_hits(sim: &mut Simulation, name: Name, own: Shape, local: PathSample) {
    let mut world = PhysicsWorld::new(Vec::new(), 256).expect("no scenery to refuse");
    sim.add_combat_system(
        CombatSet::Hits,
        move |mut hits: ResMut<'_, HitQuery>,
              table: Res<'_, ActorTable>,
              library: Res<'_, MoveLibrary>,
              actors: Query<'_, '_, (&EntityClock, &MoveRunner, &MoveOutput, &ActorTransform)>| {
            for (id, entity) in table.iter() {
                let Ok((_, _, output, root)) = actors.get(entity) else { continue };
                let hurtbox = HurtboxId::new(id, 0).expect("a low ID");
                hurtbox.set(&mut world, published_hurtbox(name, own, output), *root, local).expect("valid");
            }
            for (attacker, entity) in table.iter() {
                let Ok((clock, runner, output, root)) = actors.get(entity) else { continue };
                let moves =
                    PublishedMoves { attacker, delta: clock.0.last_delta(), runner, output, root: *root };
                hits.sweep_published(&world, &library, &moves);
            }
            hits.finish_step();
        },
    );
}
