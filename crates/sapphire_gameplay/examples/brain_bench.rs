//! What a brain costs, against spec §3.2's fixed step (1.0 ms on Desktop-Low, 1.5 on Mobile-Low).
//!
//! ```text
//! cargo run --release -p sapphire_gameplay --example brain_bench [steps]
//! ```
//!
//! Three answers, in alternating rounds:
//!
//! - **A fight:** whole steps of the shipped warrior's moves and brain, as the `rounds` module reads
//!   them. Everyone but the first actor thinks, each fighting the next, on a floor, with health enough that
//!   nobody falls, so hits land and reactions cut moves off for the whole run. A duel is what the slice
//!   ships.
//! - **Thinking alone:** brains that have nothing to do but stand and face, in a world where nothing else
//!   moves, against the same world with their brains off: the difference between the quietest rounds'
//!   medians is `think`'s own cost.
//! - **One call:** `step_brain` by itself, the least of its rounds, on the tick it's busiest (scoring every
//!   attack and drawing) and an ordinary one (pacing).

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

#[path = "../../sapphire_physics/examples/support/rounds.rs"]
mod rounds;

use std::alloc::System;
use std::time::Duration;

use rounds::Rounds;
use sapphire_core::alloc::{self, CountingAllocator};
use sapphire_gameplay::brain::{Seen, SeenTarget, step_brain};
use sapphire_gameplay::{ActorId, ActorSpawn, Brain, ControllerProfile, SimConfig, Simulation, TickActions};
use sapphire_math::{Pcg32, Vec3};
use sapphire_moves::{
    BrainDocument, EnemyDocument, EnemyMind, MoveSet, MoveSetDocument, TargetDoing, TickRange,
};
use sapphire_nav::Way;
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};
use sapphire_time::{SimDuration, SimTime, Stopwatch};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const WARRIOR: &str = include_str!("../../../content/combat/enemy/warrior.moves.ron");
const WARRIOR_ENEMY: &str = include_str!("../../../content/combat/enemy/warrior.enemy.ron");

/// The warrior's moves, with health enough for a bench, and its kind of enemy with its brain edited.
fn warrior(edit: impl FnOnce(&mut BrainDocument)) -> (MoveSet, EnemyMind) {
    let lasting = WARRIOR.replacen("health: 240,", "health: 4000000000,", 1);
    let moves = MoveSetDocument::parse(&lasting).expect("parses");
    let mut enemy = EnemyDocument::parse(WARRIOR_ENEMY).expect("parses");
    edit(enemy.brains.get_mut(&enemy.brain).expect("its first"));
    let mind = enemy.compile(&moves).expect("compiles").mind;
    (moves.compile().expect("compiles"), mind)
}

/// Everyone on a floor, far enough apart to pace and close enough to reach.
fn world(actors: u32, spacing: f32, (set, mind): (MoveSet, EnemyMind)) -> (Simulation, Vec<ActorId>) {
    let mut sim = Simulation::new(SimConfig { seed: 1, max_actors: actors, ..SimConfig::default() });
    let floor = StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(500.0, 0.5, 500.0) }),
        pose: Pose::at(Vec3::new(0.0, -0.5, 0.0)),
        memberships: Group::World.mask(),
    };
    let physics = PhysicsWorld::new(vec![floor], sim.body_capacity()).expect("a good world");
    sim.set_physics_world(physics).expect("before any spawn");
    let controller = Some(sim.add_controller_profile(&ControllerProfile::default()).expect("a good profile"));
    let moves = sim.add_move_set(set);
    sim.add_enemy(moves, mind).expect("its brain fits its moves");
    let moves = Some(moves);
    let ids: Vec<ActorId> = (0..actors)
        .map(|index| {
            let position = Vec3::new(index as f32 * spacing, 0.0, 0.0);
            sim.spawn(ActorSpawn { position, moves, controller, ..ActorSpawn::default() }).expect("room")
        })
        .collect();
    (sim, ids)
}

/// Each thinks, picking its own foe: the first has no brain in a duel, as a player has none.
fn give_brains(sim: &mut Simulation, ids: &[ActorId], from: usize) {
    for &id in &ids[from..] {
        assert!(sim.give_brain(id));
    }
}

/// A round's steps.
fn run(sim: &mut Simulation, steps: u64) -> Vec<Duration> {
    let mut times = Vec::with_capacity(steps as usize);
    for _ in 0..steps {
        sim.step(&TickActions::NEUTRAL);
        times.push(sim.stats().last_step);
    }
    assert_eq!(sim.stats().allocations, 0, "rule R6");
    times
}

fn fight(actors: u32, steps: u64) -> (Vec<Duration>, u64) {
    let (mut sim, ids) = world(actors, 2.0, warrior(|_| {}));
    give_brains(&mut sim, &ids, usize::from(ids.len() > 1));
    let round = run(&mut sim, steps);
    assert!(
        sim.stats().hits_applied > steps / 500,
        "a fight with no hits measures nothing: {:?}",
        sim.stats()
    );
    (round, sim.stats().hits_applied)
}

/// Brains standing in their bands facing each other, or the same world with them off.
fn standing(actors: u32, steps: u64, thinking: bool) -> Vec<Duration> {
    let quiet = warrior(|brain| {
        brain.patience = TickRange { min: 3600, max: 3600 };
        brain.pace.circle_share = 0.0;
    });
    let (mut sim, ids) = world(actors, 2.2, quiet);
    if thinking {
        give_brains(&mut sim, &ids, 0);
    }
    run(&mut sim, steps.min(3000))
}

/// The ns of one `step_brain` by itself: with its patience out and every attack scoring and a draw, or
/// with all of that still to wait for.
fn one_call(deciding: bool, calls: u32) -> f64 {
    let (_, mind) = warrior(|brain| {
        for attack in &mut brain.attacks {
            attack.reach.near = 0.0;
            attack.reach.far = 9.0;
        }
    });
    let def = mind.brain(0).expect("the warrior has one");
    let mut rng = Pcg32::new(1, 1);
    let until = if deciding { SimTime::ZERO } else { SimTime::from_subticks(u64::MAX / 2) };
    let seen = Seen {
        now: SimTime::from_subticks(1_000_000),
        delta: SimDuration::from_ticks(1),
        position: Vec3::ZERO,
        yaw: 0.0,
        defeated: false,
        started: 0,
        running: None,
        may_attack: true,
        reaction_due: false,
        target: Some(SeenTarget {
            position: Vec3::new(1.0, 0.0, 2.0),
            doing: TargetDoing::Free,
            defeated: false,
        }),
        way: Way::Straight,
        point: None,
    };
    let stopwatch = Stopwatch::start();
    let mut pressed = 0_u32;
    for _ in 0..calls {
        let mut brain = Brain::bench(until);
        let output = step_brain(&mut brain, def, std::hint::black_box(&seen), &mut rng);
        pressed += u32::from(output.press.is_some());
    }
    let elapsed = stopwatch.elapsed();
    assert_eq!(pressed > 0, deciding);
    elapsed.as_secs_f64() * 1e9 / f64::from(calls)
}

fn main() {
    assert!(alloc::is_installed(), "the step's allocation count would read 0 whatever it did");
    let steps: u64 = std::env::args().nth(1).and_then(|steps| steps.parse().ok()).unwrap_or(20_000);
    const SIZES: [u32; 3] = [2, 16, 64];
    const ROUNDS: usize = 7;

    let mut fights = SIZES.map(|_| (Rounds::new("p99.9", 999), 0));
    let mut idle = SIZES.map(|_| [false, true].map(|_| Rounds::new("p99.9", 999)));
    let (mut deciding, mut pacing) = (f64::MAX, f64::MAX);
    for _ in 0..ROUNDS {
        for (index, &size) in SIZES.iter().enumerate() {
            let (this, hits) = fight(size, steps);
            fights[index].0.add(this);
            fights[index].1 = hits;
            for (slot, thinking) in [(0, false), (1, true)] {
                idle[index][slot].add(standing(size, steps, thinking));
            }
        }
        deciding = deciding.min(one_call(true, 200_000));
        pacing = pacing.min(one_call(false, 200_000));
    }

    println!("{steps} steps a round, {ROUNDS} alternating rounds, in µs");
    println!("a fight: everyone but the first thinks (a duel: one brain)");
    for (size, (rounds, hits)) in SIZES.iter().zip(&mut fights) {
        println!("{size:2} actors, {hits} hits: {}", rounds.summary());
    }
    println!("thinking alone: brains that stand and face, against the same world with them off");
    println!("actors  off's quietest median  on's  each brain");
    for (size, [off, on]) in SIZES.iter().zip(&mut idle) {
        let median = |rounds: &mut Rounds| rounds.summary().quietest.1.median.as_secs_f64() * 1e6;
        let (off, on) = (median(off), median(on));
        println!("{size:6}  {off:9.2} µs  {on:8.2} µs  {:7.3} µs", (on - off) / f64::from(*size));
    }
    println!("one call of step_brain: {deciding:.0} ns choosing among 3 attacks, {pacing:.0} ns pacing");
    println!("budget: 1000 µs a step on Desktop-Low, 1500 µs on Mobile-Low");
}
