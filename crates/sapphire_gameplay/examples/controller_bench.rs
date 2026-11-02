//! What character controllers cost, physics included, against spec §3.2's budget for physics in a step
//! (0.7 ms on Desktop-Low, 1.2 on Mobile-Low).
//!
//! ```text
//! cargo run --release -p sapphire_gameplay --example controller_bench [steps]
//! ```
//!
//! Characters walk, run, jump, climb, fall and meet each other and the walls of an arena with an
//! 8,192-triangle ground, in alternating rounds of a duel's size and a crowd's. Two figures for each:
//!
//! - **Controllers:** every character steps through `step_controller` directly, each on its own stick,
//!   timed around the loop. Nothing else runs, so this is the figure for §3.2's line.
//! - **A whole step:** `Simulation::step`, and the part of it `SimStats` says went to controllers. A
//!   tick holds input for 8 actors, so past that the rest stand idle.
//!
//! Reports each size's minimum median, 99th percentile and worst step across rounds. Each round warms
//! up first, though nothing here needs it for allocation: only for the CPU's caches.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

#[path = "../tests/support/mesh_arena.rs"]
mod mesh_arena;

use std::time::Duration;

use sapphire_gameplay::{
    ActorId, ActorSpawn, ActorTransform, ControllerInput, ControllerState, GroundMode, ResolvedProfile,
    SimConfig, Simulation, step_controller,
};
use sapphire_physics::{BodyId, PhysicsStats, PhysicsWorld};
use sapphire_time::{SimDuration, Stopwatch};

const WARM_STEPS: u64 = 300;
const SIZES: [usize; 2] = [2, 16];
const ROUNDS: usize = 7;

#[derive(Clone, Copy)]
struct Figure {
    median: Duration,
    p99: Duration,
    worst: Duration,
}

impl Figure {
    fn of(mut times: Vec<Duration>) -> Self {
        times.sort_unstable();
        Self {
            median: times[times.len() / 2],
            p99: times[times.len() * 99 / 100],
            worst: times[times.len() - 1],
        }
    }

    fn at_least(self, other: Option<Self>) -> Self {
        let Some(other) = other else { return self };
        Self {
            median: self.median.min(other.median),
            p99: self.p99.min(other.p99),
            worst: self.worst.min(other.worst),
        }
    }
}

struct Character {
    transform: ActorTransform,
    state: ControllerState,
    profile: ResolvedProfile,
}

/// `size` characters stepped directly: the figure for §3.2's line.
fn direct_round(size: usize, steps: u64) -> (Figure, Tally) {
    let profiles = mesh_arena::profiles().map(|profile| ResolvedProfile::new(&profile).expect("a good profile"));
    let mut world = PhysicsWorld::new(mesh_arena::statics(), size).expect("the arena builds");
    let mut characters: Vec<Character> = (0..size)
        .map(|index| {
            let (position, profile) = mesh_arena::start(index);
            let profile = profiles[profile];
            world.add_body(profile.body(body(index), position, 0.0)).expect("it fits");
            Character {
                transform: ActorTransform { position, yaw: 0.0 },
                state: ControllerState::default(),
                profile,
            }
        })
        .collect();
    let mut step = |world: &mut PhysicsWorld, tick: u64, tally: &mut Tally| {
        for (index, character) in characters.iter_mut().enumerate() {
            let (stick, jump) = mesh_arena::input(tick, index);
            let input = ControllerInput { stick, jump, ..ControllerInput::default() };
            step_controller(
                world,
                body(index),
                &mut character.transform,
                &mut character.state,
                &character.profile,
                &input,
                SimDuration::from_ticks(1),
            )
            .expect("the body is there");
            tally.airborne += u64::from(character.state.mode != GroundMode::Grounded);
        }
    };
    let mut tally = Tally::default();
    for tick in 0..WARM_STEPS {
        step(&mut world, tick, &mut tally);
    }
    world.take_stats();
    tally = Tally::default();
    let mut times = Vec::with_capacity(steps as usize);
    for tick in WARM_STEPS..WARM_STEPS + steps {
        let stopwatch = Stopwatch::start();
        step(&mut world, tick, &mut tally);
        times.push(stopwatch.elapsed());
    }
    tally.physics = world.take_stats();
    tally.steps = steps;
    (Figure::of(times), tally)
}

const fn body(index: usize) -> BodyId {
    BodyId(index as u32 + 1)
}

#[derive(Clone, Copy, Default)]
struct Tally {
    airborne: u64,
    steps: u64,
    physics: PhysicsStats,
}

/// `size` characters in a simulation, the whole step and its controllers' part of it.
fn simulation_round(size: usize, steps: u64) -> (Figure, Figure) {
    let mut sim = Simulation::new(SimConfig { seed: 1, max_actors: 16, ..SimConfig::default() });
    let world = PhysicsWorld::new(mesh_arena::statics(), sim.body_capacity()).expect("the arena builds");
    sim.set_physics_world(world).expect("it has room");
    let profiles = mesh_arena::profiles().map(|profile| sim.add_controller_profile(&profile).expect("good"));
    let actors: Vec<ActorId> = (0..size)
        .map(|index| {
            let (position, profile) = mesh_arena::start(index);
            let controller = Some(profiles[profile]);
            sim.spawn(ActorSpawn { position, controller, ..ActorSpawn::default() }).expect("room")
        })
        .collect();
    for tick in 0..WARM_STEPS {
        sim.step(&mesh_arena::tick_actions(tick, &actors));
    }
    let mut whole = Vec::with_capacity(steps as usize);
    let mut controllers = Vec::with_capacity(steps as usize);
    for tick in WARM_STEPS..WARM_STEPS + steps {
        sim.step(&mesh_arena::tick_actions(tick, &actors));
        whole.push(sim.stats().last_step);
        controllers.push(sim.stats().controllers_last);
    }
    assert_eq!(sim.stats().allocations, 0, "no counting allocator here, so this only reads 0");
    (Figure::of(whole), Figure::of(controllers))
}

fn micros(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e6
}

fn main() {
    let steps = std::env::args().nth(1).and_then(|steps| steps.parse().ok()).unwrap_or(2_000);
    let mut direct: [Option<Figure>; SIZES.len()] = [None; SIZES.len()];
    let mut whole: [Option<Figure>; SIZES.len()] = [None; SIZES.len()];
    let mut part: [Option<Figure>; SIZES.len()] = [None; SIZES.len()];
    let mut tallies = [Tally::default(); SIZES.len()];
    for _ in 0..ROUNDS {
        for (slot, &size) in SIZES.iter().enumerate() {
            let (figure, tally) = direct_round(size, steps);
            direct[slot] = Some(figure.at_least(direct[slot]));
            tallies[slot] = tally;
            let (step, controllers) = simulation_round(size, steps);
            whole[slot] = Some(step.at_least(whole[slot]));
            part[slot] = Some(controllers.at_least(part[slot]));
        }
    }

    println!(
        "{steps} steps, {ROUNDS} rounds, minimums across rounds. Budget: 700 µs on Desktop-Low, 1,200 on Mobile-Low."
    );
    println!("actors  what                     median       p99      worst");
    let row = |size: usize, what: &str, figure: Option<Figure>| {
        let figure = figure.expect("every size ran");
        println!(
            "{size:6}  {what:<22} {:7.1} µs {:7.1} µs {:7.1} µs",
            micros(figure.median),
            micros(figure.p99),
            micros(figure.worst),
        );
    };
    for (slot, &size) in SIZES.iter().enumerate() {
        row(size, "controllers, direct", direct[slot]);
        row(size, "controllers, in a step", part[slot]);
        row(size, "whole step", whole[slot]);
        let tally = tallies[slot];
        let per = |count: u32| f64::from(count) / tally.steps as f64 / size as f64;
        println!(
            "        per character per step: {:.2} moves, {:.2} sweeps, {:.2} casts, {:.2} rays, {:.1} narrow tests; {:.0}% of the time off the ground",
            per(tally.physics.moves),
            per(tally.physics.move_sweeps),
            per(tally.physics.shape_casts),
            per(tally.physics.ray_casts),
            per(tally.physics.narrow_tests),
            tally.airborne as f64 / tally.steps as f64 / size as f64 * 100.0,
        );
    }
}
