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
//! size's rounds as the `rounds` module reads them. Each round warms up first.
//!
//! Given a collider document, it runs the same moves and queries on that document's world instead:
//! what a stage's own collider set costs (`content/arena/square.colliders.ron`, M2 O5).

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "support/rounds.rs"]
mod rounds;

use std::hint::black_box;
use std::time::Duration;

use common::{Script, bodies, statics};
use glam::Vec3;
use rounds::Rounds;
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

/// A round's steps.
fn round(document: Option<&ColliderSet>, extra_hurtboxes: u32, steps: u32) -> Vec<Duration> {
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
    times
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

    let mut rows = CROWDS.map(|_| Rounds::new("p99", 990));
    for _ in 0..ROUNDS {
        for (crowd, row) in CROWDS.iter().zip(&mut rows) {
            row.add(round(document, *crowd, steps));
        }
    }

    println!("building the world of {colliders} colliders: {:.0} µs", micros(build));
    println!("{steps} steps a round, {ROUNDS} alternating rounds, in µs");
    for (crowd, row) in CROWDS.iter().zip(&mut rows) {
        println!("{:4} bodies: {}", bodies().len() as u32 + crowd, row.summary());
    }
    println!(
        "budget: {} µs on Desktop-Low, {} µs on Mobile-Low",
        sapphire_physics::STEP_BUDGET_DESKTOP_LOW_US,
        sapphire_physics::STEP_BUDGET_MOBILE_LOW_US,
    );
}
