//! What one climber's probes cost (M6 A16a), against the plan's kill line for climbing: 20 µs a step.
//!
//! ```text
//! cargo run --release -p sapphire_gameplay --example climb_bench [laps]
//! ```
//!
//! The heroine, alone, climbs a climbable wall 3 m tall with a wing standing out from its east end, two laps
//! of 400 ticks in turn, teleported back to the floor before each: one grabs the face, climbs up it and west
//! round its outside corner, lets go and falls, grabs the wall's west side from the floor and tops out over it;
//! the other grabs the face further east, climbs into the wing's inside corner and tops out over the wing. Every
//! tick of every lap is a `Simulation::step`. Three figures, each over the ticks she spends on a face (holding
//! one at the step's start or its end), which is where every climb cast falls:
//!
//! - **Controllers:** the part of the step `SimStats` gives the character controllers, where a held face's
//!   sweeps and rays run. One actor, so this is the climber's tick.
//! - **A whole step:** the grab's and the top-out's probes are in the step's intents, outside the
//!   controllers, so this holds them too.
//! - **Walking, controllers:** the same heroine walking to and fro on the floor for as many ticks, for what a
//!   tick costs her with no climb in it.
//!
//! And the casts: how many a climbing tick made, by `SimStats::climb_casts`. Each lap warms up first, for the
//! CPU's caches only: nothing here allocates (rule R6), which the bench asserts.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

#[path = "../../sapphire_physics/examples/support/rounds.rs"]
mod rounds;

use std::alloc::System;
use std::time::Duration;

use rounds::Rounds;
use sapphire_core::alloc::{self, CountingAllocator};
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, Place, SimConfig, Simulation, Stick, TickActions, replay,
};
use sapphire_math::Vec3;
use sapphire_physics::{
    ColliderId, Group, GroupMask, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape,
};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const ROUNDS: usize = 7;
const LAP: u32 = 400;

fn block(id: u32, min: Vec3, max: Vec3, memberships: GroupMask) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Convex(Shape::Box { half_extents: (max - min) * 0.5 }),
        pose: Pose::at((min + max) * 0.5),
        memberships,
    }
}

/// The floor, a climbable wall 3 m tall whose face is at z 1 facing −Z across x −3 to 3, and a climbable wing
/// standing out from its east end, its west side an inside corner with the face.
fn wall() -> Vec<StaticCollider> {
    let climbable = Group::World | Group::Climbable;
    vec![
        block(0, Vec3::new(-50.0, -1.0, -50.0), Vec3::new(50.0, 0.0, 50.0), Group::World.mask()),
        block(1, Vec3::new(-3.0, 0.0, 1.0), Vec3::new(3.0, 3.0, 3.0), climbable),
        block(2, Vec3::new(3.0, 0.0, -1.0), Vec3::new(5.0, 3.0, 1.0), climbable),
    ]
}

/// A lap's legs: the tick each lasts until, its stick, and whether it presses jump on its first tick.
type Legs = [(u32, Stick, bool)];

const STILL: Stick = Stick::new(0, 0);
const TOWARD: Stick = Stick::new(0, 127);
const WEST: Stick = Stick::new(-127, 0);
const EAST: Stick = Stick::new(127, 0);
const OUTSIDE: [(u32, Stick, bool); 8] = [
    (20, STILL, false),
    (60, TOWARD, true),
    (110, WEST, false),
    (150, STILL, false),
    (180, STILL, true),
    (200, STILL, true),
    (290, EAST, false),
    (LAP, STILL, false),
];
/// East into the corner and on along the wing, the stick's meaning latched; released, then east again, up it.
const INSIDE: [(u32, Stick, bool); 6] = [
    (20, STILL, false),
    (40, TOWARD, true),
    (100, EAST, false),
    (102, STILL, false),
    (165, EAST, false),
    (LAP, STILL, false),
];
/// To and fro along the floor, away from the wall (+Z is toward it) and well clear of it.
const WALK: [(u32, Stick, bool); 3] =
    [(200, Stick::new(127, -40), false), (380, Stick::new(-127, 40), false), (LAP, STILL, false)];

fn at(legs: &Legs, tick: u32) -> ActorActions {
    let index = legs.iter().position(|(until, ..)| tick < *until).expect("a leg");
    let (_, stick, press) = legs[index];
    let first = index.checked_sub(1).map_or(0, |before| legs[before].0);
    let buttons = if press && tick == first { Buttons::JUMP } else { Buttons::NONE };
    ActorActions { buttons, stick }
}

/// Where each lap starts: before the face, at `x`, facing it.
fn start(x: f32) -> Place {
    Place { position: Vec3::new(x, 0.0, 0.55), yaw: 0.0 }
}

struct Round {
    controllers: Vec<Duration>,
    whole: Vec<Duration>,
    walking: Vec<Duration>,
    casts: Vec<u64>,
}

fn heroine() -> (Simulation, ActorId) {
    let mut sim = Simulation::new(SimConfig::default());
    sim.set_physics_world(PhysicsWorld::new(wall(), sim.body_capacity()).expect("builds")).expect("room");
    let set = replay::player_moves().expect("the heroine's moves");
    let profile = set.movement().copied().unwrap_or_default();
    let controller = Some(sim.add_controller_profile(&profile).expect("a good one"));
    let moves = Some(sim.add_move_set(set));
    let place = start(-2.0);
    let spawn =
        ActorSpawn { position: place.position, yaw: place.yaw, moves, controller, ..ActorSpawn::default() };
    let id = sim.spawn(spawn).expect("room");
    (sim, id)
}

/// What a lap is, for which of a round's figures it adds to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lap {
    Warm,
    Climbing,
    Walking,
}

fn lap(sim: &mut Simulation, id: ActorId, legs: &Legs, x: f32, kind: Lap, round: &mut Round) {
    sim.teleport(id, start(x)).expect("on the floor");
    for tick in 0..LAP {
        let mut actions = TickActions::NEUTRAL;
        actions.set(id, at(legs, tick)).expect("room");
        let before = sim.actor(id).expect("alive").controller.state.climb.is_some();
        let casts = sim.stats().climb_casts;
        sim.step(&actions);
        let after = sim.actor(id).expect("alive").controller.state.climb.is_some();
        let stats = sim.stats();
        if kind == Lap::Walking {
            round.walking.push(stats.controllers_last);
        } else if kind == Lap::Climbing && (before || after) {
            round.controllers.push(stats.controllers_last);
            round.whole.push(stats.last_step);
            round.casts.push(stats.climb_casts - casts);
        }
    }
}

fn main() {
    assert!(alloc::is_installed(), "the step's allocation count would read 0 whatever it did");
    let laps: u32 = std::env::args().nth(1).and_then(|laps| laps.parse().ok()).unwrap_or(4);
    let mut controllers = Rounds::new("p99", 990);
    let mut whole = Rounds::new("p99", 990);
    let mut walking = Rounds::new("p99", 990);
    let mut casts = Vec::new();
    for _ in 0..ROUNDS {
        let (mut sim, id) = heroine();
        let mut round =
            Round { controllers: Vec::new(), whole: Vec::new(), walking: Vec::new(), casts: Vec::new() };
        lap(&mut sim, id, &OUTSIDE, -2.0, Lap::Warm, &mut round);
        lap(&mut sim, id, &INSIDE, 1.5, Lap::Warm, &mut round);
        for _ in 0..laps {
            lap(&mut sim, id, &OUTSIDE, -2.0, Lap::Climbing, &mut round);
            lap(&mut sim, id, &INSIDE, 1.5, Lap::Climbing, &mut round);
            lap(&mut sim, id, &WALK, -10.0, Lap::Walking, &mut round);
        }
        assert_eq!(sim.stats().allocations, 0, "rule R6");
        assert_eq!(sim.stats().climb_top_outs, u64::from(2 * (laps + 1)), "every lap topped out");
        controllers.add(round.controllers);
        whole.add(round.whole);
        walking.add(round.walking);
        casts.extend(round.casts);
    }

    println!("{laps} lap pairs a round, {ROUNDS} rounds, one climber, in µs. Kill line: 20 µs a step.");
    println!("climbing ticks, controllers: {}", controllers.summary());
    println!("climbing ticks, whole step: {}", whole.summary());
    println!("walking ticks, controllers: {}", walking.summary());
    let ticks = casts.len().max(1) as f64;
    let total: u64 = casts.iter().sum();
    let most = casts.iter().copied().max().unwrap_or(0);
    let mut by = [0_usize; 8];
    for &count in &casts {
        by[(count as usize).min(by.len() - 1)] += 1;
    }
    println!(
        "climb casts a climbing tick: mean {:.2}, most {most}; ticks by casts 0..7+: {by:?}",
        total as f64 / ticks
    );
}
