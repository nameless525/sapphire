//! What a frame of the camera costs: M2 G5's "costs almost nothing".
//!
//! ```text
//! cargo run --release -p sapphire_camera --example camera_bench [frames]
//! ```
//!
//! Runs a director through the scripted fight, with look input and a stream of shakes, at 60 Hz in a world
//! of 227 colliders (a ground, 225 pillars and crates on a 4 m grid the fighters walk through, and an
//! 8,192-triangle mesh, the size of `sapphire_physics`' own bench scene), in alternating rounds of three
//! loads. It reports each load's minimum median, 99th percentile and worst frame across the rounds, how
//! many sphere casts a frame took, and on what share of frames the arm was pulled in.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::hint::black_box;
use std::time::Duration;

use sapphire_camera::{CameraDirector, CameraFrame, CameraTuning, Rig, Subject};
use sapphire_math::{Vec2, Vec3};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};
use sapphire_time::Stopwatch;

const ROUNDS: u32 = 7;

fn collider(id: u32, shape: StaticShape, position: Vec3) -> StaticCollider {
    StaticCollider { id: ColliderId(id), shape, pose: Pose::at(position), memberships: Group::World.mask() }
}

/// A ground, a grid of 15 × 15 pillars and crates among the fighters, and a bumpy mesh underneath them.
fn world() -> PhysicsWorld {
    let mut colliders = vec![collider(
        0,
        StaticShape::Convex(Shape::Box { half_extents: Vec3::new(200.0, 1.0, 200.0) }),
        Vec3::new(0.0, -1.0, 0.0),
    )];
    for index in 0..225_u32 {
        let (x, z) = ((index % 15) as f32 * 4.0 - 28.0, (index / 15) as f32 * 4.0 - 28.0);
        let shape = if index % 2 == 0 {
            Shape::Capsule { half_height: 1.5, radius: 0.4 }
        } else {
            Shape::Box { half_extents: Vec3::new(0.6, 1.0, 0.6) }
        };
        colliders.push(collider(index + 1, StaticShape::Convex(shape), Vec3::new(x, 1.9, z)));
    }
    let (side, mut vertices, mut indices) = (65_u32, Vec::new(), Vec::new());
    for row in 0..side {
        for column in 0..side {
            let (x, z) = (column as f32 - 32.0, row as f32 - 32.0);
            vertices.push(Vec3::new(x, -0.2 + 0.1 * (x * 0.7).sin() * (z * 0.9).cos(), z));
        }
    }
    for row in 0..side - 1 {
        for column in 0..side - 1 {
            let at = row * side + column;
            indices.push([at, at + side, at + 1]);
            indices.push([at + 1, at + side, at + side + 1]);
        }
    }
    colliders.push(collider(300, StaticShape::Mesh { vertices, indices }, Vec3::ZERO));
    PhysicsWorld::new(colliders, 0).expect("the bench's world builds")
}

/// The fighters at `t`: the player walking among the colliders, the enemy going round them.
fn fighters(t: f32) -> (Subject, Subject) {
    let player = Vec3::new(14.0 * (0.19 * t).sin(), 0.0, 12.0 * (0.27 * t + 1.0).sin());
    let angle = 0.9 * t;
    let reach = 5.0 + 3.0 * (0.4 * t).sin();
    (
        Subject { position: player, height: 1.7, radius: 0.35 },
        Subject {
            position: player + Vec3::new(reach * angle.sin(), 0.0, reach * angle.cos()),
            height: 2.0,
            radius: 0.45,
        },
    )
}

#[derive(Clone, Copy)]
enum Load {
    Explore,
    Combat,
    LockOn,
}

impl Load {
    fn name(self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::Combat => "combat",
            Self::LockOn => "lock-on",
        }
    }
}

fn camera(load: Load) -> CameraDirector {
    let mut camera =
        CameraDirector::new(CameraTuning::default_tuning(), 7).expect("the shipped tuning is valid");
    if matches!(load, Load::Combat | Load::LockOn) {
        camera.set_rig(Rig::Combat, true);
    }
    if matches!(load, Load::LockOn) {
        camera.set_rig(Rig::LockOn, true);
    }
    camera
}

fn frame(index: u32, load: Load) -> CameraFrame {
    let dt = 1.0 / 60.0;
    let t = index as f32 * dt;
    let (player, enemy) = fighters(t);
    CameraFrame {
        dt,
        player,
        player_yaw: 0.5 * t,
        target: if matches!(load, Load::Explore) { None } else { Some(enemy) },
        look: Vec2::new(1.1 * (0.7 * t).sin(), 0.4 * (1.1 * t).sin()) * dt,
        zoom: 0.0,
        aspect: 16.0 / 9.0,
    }
}

/// One round's median, 99th percentile and worst in microseconds, casts a frame and the share pulled in.
fn round(world: &PhysicsWorld, load: Load, frames: u32, times: &mut Vec<Duration>) -> [f64; 5] {
    let mut camera = camera(load);
    let shake = camera.tuning().shake_id("hit_light").expect("a shipped preset");
    for index in 0..120 {
        black_box(camera.update(&frame(index, load), world));
    }
    camera.take_stats();

    times.clear();
    for index in 0..frames {
        if index % 20 == 0 {
            camera.fire_shake(shake, 1.0);
        }
        let input = frame(120 + index, load);
        let clock = Stopwatch::start();
        black_box(camera.update(&input, world));
        times.push(clock.elapsed());
    }
    times.sort_unstable();
    let micros = |time: Duration| time.as_secs_f64() * 1e6;
    let stats = camera.take_stats();
    let per_frame = |count: u32| f64::from(count) / f64::from(frames);
    [
        micros(times[times.len() / 2]),
        micros(times[times.len() * 99 / 100]),
        micros(times[times.len() - 1]),
        per_frame(stats.casts),
        per_frame(stats.pulled_frames),
    ]
}

fn main() {
    let frames: u32 = std::env::args().nth(1).and_then(|text| text.parse().ok()).unwrap_or(4000);
    let world = world();
    println!("{} colliders, {frames} frames a round, {ROUNDS} alternating rounds", world.static_count());

    let loads = [Load::Explore, Load::Combat, Load::LockOn];
    let mut best = [[f64::MAX, f64::MAX, f64::MAX, 0.0, 0.0]; 3];
    let mut times = Vec::with_capacity(frames as usize);
    for _ in 0..ROUNDS {
        for (index, &load) in loads.iter().enumerate() {
            let result = round(&world, load, frames, &mut times);
            let kept = &mut best[index];
            *kept = [
                kept[0].min(result[0]),
                kept[1].min(result[1]),
                kept[2].min(result[2]),
                result[3],
                result[4],
            ];
        }
    }
    for (load, [median, p99, worst, casts, pulled]) in loads.iter().zip(best) {
        println!(
            "{:8} median {median:6.2} µs, p99 {p99:6.2} µs, worst {worst:6.2} µs, {casts:.2} casts a frame, pulled in on {:.0}%",
            load.name(),
            pulled * 100.0,
        );
    }
}
