//! What a step of collision costs, against spec §3.2's budget for it (0.7 ms on Desktop-Low, 1.2 on
//! Mobile-Low).
//!
//! ```text
//! cargo run --release -p sapphire_physics --example physics_bench [steps] [colliders.ron]
//! ```
//!
//! Runs the scene R6's test and the checksum share (225 convex colliders and an 8,192-triangle mesh
//! the capsules walk on; 2 moves and 20 queries a step, as M2 D3's spike ran them over Rapier) with
//! a few sizes of hurtbox crowd, since bodies are scanned, in alternating rounds, and reports each
//! size's minimum median, 99th percentile and worst step across rounds. Each round warms up first.
//!
//! Given a collider document, it runs the same moves and queries on that document's world instead:
//! what a stage's own collider set costs (`content/arena/square.colliders.ron`, M2 O5).

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;
use std::time::Duration;

use common::{Script, bodies, statics};
use glam::Vec3;
use sapphire_colliders::{ColliderDocument, ColliderSet};
use sapphire_physics::{BodyDesc, BodyId, CollisionGroups, Group, GroupMask, PhysicsWorld, Pose, Shape};
use sapphire_time::Stopwatch;

const WARM_STEPS: u32 = 200;

/// A collider document's world, or the scene's, with room for `capacity` bodies.
fn statics_world(document: Option<&ColliderSet>, capacity: usize) -> PhysicsWorld {
    match document {
        Some(set) => {
            let bytes = set.to_cooked().expect("it writes");
            let set = ColliderSet::read(&bytes).expect("it reads");
            PhysicsWorld::from_cooked(set, capacity).expect("the document builds")
        }
        None => PhysicsWorld::new(statics(), capacity).expect("the scene builds"),
    }
}

/// Hurtboxes standing about besides the scene's own, spread over the arena's middle.
fn world(document: Option<&ColliderSet>, extra_hurtboxes: u32) -> PhysicsWorld {
    let own = bodies();
    let mut world = statics_world(document, own.len() + extra_hurtboxes as usize);
    for body in own {
        world.add_body(body).expect("it fits");
    }
    for index in 0..extra_hurtboxes {
        let position = Vec3::new((index % 12) as f32 * 2.0 - 11.0, 1.0, (index / 12) as f32 * 2.0 - 11.0);
        let body = BodyDesc {
            id: BodyId(1000 + index),
            shape: Shape::Sphere { radius: 0.3 },
            pose: Pose::at(position),
            groups: CollisionGroups::new(Group::Hurtbox.mask(), GroupMask::NONE),
        };
        world.add_body(body).expect("it fits");
    }
    world
}

#[derive(Clone, Copy)]
struct Round {
    median: Duration,
    p99: Duration,
    worst: Duration,
}

fn round(document: Option<&ColliderSet>, extra_hurtboxes: u32, steps: u32) -> Round {
    let mut world = world(document, extra_hurtboxes);
    let mut script = Script::default();
    let mut log = |bytes: &[u8]| {
        black_box(bytes);
    };
    for tick in 0..WARM_STEPS {
        script.step(&mut world, tick, &mut log);
    }
    let mut times = Vec::with_capacity(steps as usize);
    for tick in WARM_STEPS..WARM_STEPS + steps {
        let stopwatch = Stopwatch::start();
        script.step(&mut world, tick, &mut log);
        times.push(stopwatch.elapsed());
    }
    times.sort_unstable();
    Round {
        median: times[times.len() / 2],
        p99: times[times.len() * 99 / 100],
        worst: times[times.len() - 1],
    }
}

fn micros(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e6
}

fn main() {
    let steps = std::env::args().nth(1).and_then(|steps| steps.parse().ok()).unwrap_or(2_000);
    let document = std::env::args().nth(2).map(|path| {
        let text = std::fs::read_to_string(&path).expect("the collider document");
        ColliderDocument::from_ron(&text).expect("it reads").cook().expect("it cooks")
    });
    let document = document.as_ref();
    const CROWDS: [u32; 3] = [0, 28, 124];
    const ROUNDS: usize = 7;

    let stopwatch = Stopwatch::start();
    let built = world(document, 0);
    let build = stopwatch.elapsed();
    let colliders = built.static_count();
    drop(built);

    let mut best: [Option<Round>; CROWDS.len()] = [None; CROWDS.len()];
    for _ in 0..ROUNDS {
        for (crowd, best) in CROWDS.iter().zip(&mut best) {
            let this = round(document, *crowd, steps);
            *best = Some(match *best {
                None => this,
                Some(other) => Round {
                    median: other.median.min(this.median),
                    p99: other.p99.min(this.p99),
                    worst: other.worst.min(this.worst),
                },
            });
        }
    }

    println!("building the world of {colliders} colliders: {:.0} µs", micros(build));
    println!("{steps} steps, {ROUNDS} rounds, minimums across rounds");
    println!("bodies  median step  99th percentile  worst step");
    for (crowd, best) in CROWDS.iter().zip(&best) {
        let best = best.expect("every size ran");
        println!(
            "{:6}  {:8.1} µs  {:12.1} µs  {:7.1} µs",
            bodies().len() as u32 + crowd,
            micros(best.median),
            micros(best.p99),
            micros(best.worst),
        );
    }
    println!(
        "budget: {} µs on Desktop-Low, {} µs on Mobile-Low",
        sapphire_physics::STEP_BUDGET_DESKTOP_LOW_US,
        sapphire_physics::STEP_BUDGET_MOBILE_LOW_US,
    );
}
