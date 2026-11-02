//! What a fixed step costs, against spec §3.2's budget (1.0 ms on Desktop-Low, 1.5 on Mobile-Low).
//!
//! ```text
//! cargo run --release -p sapphire_gameplay --example step_bench [steps]
//! ```
//!
//! Runs the test content's moves at a duel's size, a busy fight's and a full pool's, in alternating
//! rounds, and reports each size's minimum median, 99.9th percentile and worst step across rounds. The
//! worst is mostly the scheduler while the machine's busy, on a different tick each run, so read the
//! percentile for what a step's work costs. Every actor plays
//! strings, dodge cancels and dash attacks with a warp target and its ring in use, with its neighbours
//! in reach and a capsule on a floor, so hits land, reactions cut moves off, and hurtboxes and capsules
//! move, which is more than most of a real fight's actors do at once. Nobody falls: they're given health
//! for it, so the last steps cost what the first do. Dodges go through swings, so perfect dodges fire and
//! counters start, with their slowdown taken out: a slowed world does a fifth of the work a step, which
//! would flatter every number here. It reads the steps through `Simulation::stats`, as the overlay would,
//! and adds its checksum cost, which a replay pays per tick only when asked.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::time::Duration;

use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, ControllerProfile, SimConfig, Simulation, Stick, TickActions,
    WarpTarget,
};
use sapphire_math::Vec3;
use sapphire_moves::{MoveSet, MoveSetDocument};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};
use sapphire_time::Stopwatch;

const SLICE: &str = include_str!("../../sapphire_moves/test_content/slice.moves.ron");

fn slice() -> MoveSet {
    let lasting = SLICE
        .replacen("health: 300,", "health: 4000000000,", 1)
        .replacen("slow_motion: (per_mille: 200, steps: 30),", "", 1);
    MoveSetDocument::parse(&lasting).expect("parses").compile().expect("compiles")
}

fn world(actors: u32) -> (Simulation, Vec<ActorId>) {
    let mut sim = Simulation::new(SimConfig { seed: 1, max_actors: actors, ..SimConfig::default() });
    let floor = StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(500.0, 0.5, 500.0) }),
        pose: Pose::at(Vec3::new(0.0, -0.5, 0.0)),
        memberships: Group::World.mask(),
    };
    let world = PhysicsWorld::new(vec![floor], sim.body_capacity()).expect("a good world");
    sim.set_physics_world(world).expect("before any spawn");
    let controller = Some(sim.add_controller_profile(&ControllerProfile::default()).expect("a good profile"));
    let moves = Some(sim.add_move_set(slice()));
    let ids: Vec<ActorId> = (0..actors)
        .map(|index| {
            let position = Vec3::new(index as f32, 0.0, 0.0);
            sim.spawn(ActorSpawn { position, moves, controller, ..ActorSpawn::default() }).expect("room")
        })
        .collect();
    // Each aims at the next, so warp windows do their work against moving targets.
    for (index, &id) in ids.iter().enumerate() {
        let next = ids[(index + 1) % ids.len()];
        sim.set_warp_target(id, WarpTarget::Actor(next));
    }
    (sim, ids)
}

/// One tick: the first actors a tick can hold press on their own rhythm, a string with a dodge and a
/// dash attack. Actors past those a tick holds would be a brain's, so they're left to play a force.
fn actions(tick: u64, ids: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    for (index, &id) in ids.iter().take(TickActions::CAPACITY).enumerate() {
        let buttons = match (tick + index as u64 * 7) % 96 {
            0 | 8 | 22 | 60 | 70 => Buttons::LIGHT,
            40 | 66 => Buttons::DODGE,
            88 => Buttons::SKILL,
            _ => Buttons::NONE,
        };
        let stick = Stick::new((tick % 127) as i8, 0);
        actions.set(id, ActorActions { buttons, stick }).expect("room");
    }
    actions
}

struct Round {
    median: Duration,
    p999: Duration,
    worst: Duration,
    checksum: Duration,
    /// Moves started, cancels fired, cues published, hits applied, perfect dodges fired and counters
    /// started, to show the steps did the work.
    work: (u64, u64, u64, u64, u64, u64),
}

fn round(actors: u32, steps: u64) -> Round {
    let (mut sim, ids) = world(actors);
    let mut times = Vec::with_capacity(steps as usize);
    for tick in 0..steps {
        let input = actions(tick, &ids);
        // Everyone past the tick's few plays what the first does, as a brain would have them.
        if tick % 48 == 0 {
            for &id in ids.iter().skip(TickActions::CAPACITY) {
                sim.force_move(id, sapphire_core::Name::new("dash_attack"));
            }
        }
        sim.step(&input);
        times.push(sim.stats().last_step);
    }
    times.sort_unstable();
    let stopwatch = Stopwatch::start();
    let checksum = sim.checksum();
    let checksum_time = stopwatch.elapsed();
    assert_ne!(checksum, 0);
    let stats = sim.stats();
    assert_eq!(stats.steps, steps);
    assert_eq!((stats.allocations, stats.cues_dropped, stats.volumes_dropped), (0, 0, 0));
    assert!(stats.hits_applied > steps / 100, "a load that stops landing hits stops measuring: {stats:?}");
    // A duel's two dodge on a rhythm that misses each other's swings, and the crowds' don't.
    if actors > 2 {
        assert!(stats.perfect_dodges > steps / 1000, "or firing perfect dodges: {stats:?}");
        assert!(stats.counters_fired > 0, "or starting counters: {stats:?}");
    }
    Round {
        median: times[times.len() / 2],
        p999: times[times.len() * 999 / 1000],
        worst: stats.longest_step,
        checksum: checksum_time,
        work: (
            stats.moves_started,
            stats.cancels_fired,
            stats.cues_published,
            stats.hits_applied,
            stats.perfect_dodges,
            stats.counters_fired,
        ),
    }
}

fn main() {
    let steps = std::env::args().nth(1).and_then(|steps| steps.parse().ok()).unwrap_or(20_000);
    const SIZES: [u32; 3] = [2, 16, 64];
    const ROUNDS: usize = 7;

    let mut best: [Option<Round>; SIZES.len()] = [const { None }; SIZES.len()];
    for _ in 0..ROUNDS {
        for (size, best) in SIZES.iter().zip(&mut best) {
            let this = round(*size, steps);
            *best = Some(match best.take() {
                None => this,
                Some(other) => Round {
                    median: other.median.min(this.median),
                    p999: other.p999.min(this.p999),
                    worst: other.worst.min(this.worst),
                    checksum: other.checksum.min(this.checksum),
                    work: this.work,
                },
            });
        }
    }

    let stopwatch = Stopwatch::start();
    let created = Simulation::new(SimConfig { max_actors: 128, ..SimConfig::default() });
    let creation = stopwatch.elapsed();
    drop(created);

    println!("creating a simulation with 128 pooled actors: {:.0} µs", creation.as_secs_f64() * 1e6);
    println!("{steps} steps, {ROUNDS} rounds, minimums across rounds");
    println!(
        "actors  median step  99.9% step  worst step  checksum  moves  cancels   cues   hits  perfect  counters"
    );
    for (size, best) in SIZES.iter().zip(&best) {
        let best = best.as_ref().expect("every size ran");
        println!(
            "{size:6}  {:9.2} µs  {:7.2} µs  {:7.2} µs  {:5.2} µs  {:5}  {:7}  {:5}  {:5}  {:7}  {:8}",
            best.median.as_secs_f64() * 1e6,
            best.p999.as_secs_f64() * 1e6,
            best.worst.as_secs_f64() * 1e6,
            best.checksum.as_secs_f64() * 1e6,
            best.work.0,
            best.work.1,
            best.work.2,
            best.work.3,
            best.work.4,
            best.work.5,
        );
    }
    println!("budget: 1000 µs on Desktop-Low, 1500 µs on Mobile-Low");
}
