//! What a frame of the camera costs: M2 G5's "costs almost nothing".
//!
//! ```text
//! cargo run --release -p sapphire_camera --example camera_bench [frames]
//! ```
//!
//! Runs a director through the scripted fight, with look input and a stream of shakes, at 60 Hz in a world
//! of 227 colliders (a ground, 225 pillars and crates on a 4 m grid the fighters walk through, and an
//! 8,192-triangle mesh, the size of `sapphire_physics`' own bench scene), in alternating rounds of four
//! loads, each after `WARM_UP` frames it doesn't time. For each load it reports every round's frames
//! taken together, so `worst` is the slowest frame of them all, with how far the rounds' medians spread; the
//! quietest round, the one with the least median, whole, as what the code costs with the least else going
//! on; how many sphere casts a frame took; and on what share of frames the arm was pulled in.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::hint::black_box;
use std::time::Duration;

use sapphire_camera::{CameraDirector, CameraFrame, CameraTuning, Rig, Subject};
use sapphire_math::{Vec2, Vec3};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};
use sapphire_time::Stopwatch;

const ROUNDS: u32 = 7;
/// Frames a round runs before it starts timing, in which a fight gets going.
const WARM_UP: u32 = 120;

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
    /// Combat with no thumb on the view, so clearing's free to compare views.
    Turning,
}

impl Load {
    fn name(self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::Combat => "combat",
            Self::LockOn => "lock-on",
            Self::Turning => "turning",
        }
    }
}

fn camera(load: Load) -> CameraDirector {
    let mut camera =
        CameraDirector::new(CameraTuning::default_tuning(), 7).expect("the shipped tuning is valid");
    if matches!(load, Load::Combat | Load::LockOn | Load::Turning) {
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
        behind: 0.0,
        player: Some(player),
        player_yaw: 0.5 * t,
        target: if matches!(load, Load::Explore) { None } else { Some(enemy) },
        bystanders: [None; sapphire_camera::MAX_BYSTANDERS],
        look: if matches!(load, Load::Turning) {
            Vec2::ZERO
        } else {
            Vec2::new(1.1 * (0.7 * t).sin(), 0.4 * (1.1 * t).sin()) * dt
        },
        zoom: 0.0,
        aspect: 16.0 / 9.0,
    }
}

/// A set of frames' median, 99th percentile and slowest, in microseconds, all of the same frames.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Times {
    median: f64,
    p99: f64,
    worst: f64,
}

impl Times {
    /// Sorts `times` to read them.
    fn of(times: &mut [Duration]) -> Self {
        times.sort_unstable();
        let micros = |time: Duration| time.as_secs_f64() * 1e6;
        Self {
            median: micros(times[times.len() / 2]),
            p99: micros(times[times.len() * 99 / 100]),
            worst: micros(times[times.len() - 1]),
        }
    }
}

/// What one round came to.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Round {
    times: Times,
    casts: f64,
    pulled: f64,
    chose: u32,
}

/// What a load's rounds came to.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Summary {
    /// Every frame of every round, as one set.
    all: Times,
    frames: usize,
    /// The least and most of the rounds' medians.
    medians: (f64, f64),
    /// The round with the least median, counting from 1, and that round's own numbers.
    quietest: (usize, Round),
}

impl Summary {
    /// `all` holds every frame of `rounds`, which can't be empty.
    fn of(rounds: &[Round], all: &mut [Duration]) -> Self {
        let median = |round: &Round| round.times.median;
        let (index, quietest) = rounds
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| median(a).total_cmp(&median(b)))
            .expect("a round ran");
        Self {
            all: Times::of(all),
            frames: all.len(),
            medians: (
                median(quietest),
                rounds.iter().map(median).max_by(f64::total_cmp).expect("a round ran"),
            ),
            quietest: (index + 1, *quietest),
        }
    }
}

/// Runs a round, adding its frames' times to `all`.
fn round(world: &PhysicsWorld, load: Load, frames: u32, all: &mut Vec<Duration>) -> Round {
    let mut camera = camera(load);
    let shake = camera.tuning().shake_id("hit_light").expect("a shipped preset");
    for index in 0..WARM_UP {
        black_box(camera.update(&frame(index, load), world));
    }
    camera.take_stats();

    let first = all.len();
    for index in 0..frames {
        if index % 20 == 0 {
            camera.fire_shake(shake, 1.0);
        }
        let input = frame(WARM_UP + index, load);
        let clock = Stopwatch::start();
        black_box(camera.update(&input, world));
        all.push(clock.elapsed());
    }
    let stats = camera.take_stats();
    let per_frame = |count: u32| f64::from(count) / f64::from(frames);
    Round {
        times: Times::of(&mut all[first..]),
        casts: per_frame(stats.casts),
        pulled: per_frame(stats.pulled_frames),
        chose: stats.swings,
    }
}

fn main() {
    let frames: u32 = std::env::args().nth(1).and_then(|text| text.parse().ok()).unwrap_or(4000);
    let world = world();
    println!(
        "{} colliders, {frames} frames a round after {WARM_UP} to warm up, {ROUNDS} alternating rounds, in µs",
        world.static_count()
    );

    let loads = [Load::Explore, Load::Combat, Load::LockOn, Load::Turning];
    let mut rounds: [Vec<Round>; 4] = std::array::from_fn(|_| Vec::with_capacity(ROUNDS as usize));
    let mut all: [Vec<Duration>; 4] = std::array::from_fn(|_| Vec::with_capacity((ROUNDS * frames) as usize));
    for _ in 0..ROUNDS {
        for (index, &load) in loads.iter().enumerate() {
            rounds[index].push(round(&world, load, frames, &mut all[index]));
        }
    }
    for (index, load) in loads.iter().enumerate() {
        let Summary { all, frames, medians, quietest: (number, quietest) } =
            Summary::of(&rounds[index], &mut all[index]);
        println!(
            "{:8} all {frames} frames: median {:.2} ({:.2} to {:.2} by round), p99 {:.2}, worst {:.2}; \
             quietest round, {number}: median {:.2}, p99 {:.2}, worst {:.2}; \
             {:.2} casts a frame, pulled in on {:.0}%, chose {} times",
            load.name(),
            all.median,
            medians.0,
            medians.1,
            all.p99,
            all.worst,
            quietest.times.median,
            quietest.times.p99,
            quietest.times.worst,
            quietest.casts,
            quietest.pulled * 100.0,
            quietest.chose,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn micros(values: &[u64]) -> Vec<Duration> {
        values.iter().copied().map(Duration::from_micros).collect()
    }

    fn round_of(times: &[u64]) -> Round {
        Round { times: Times::of(&mut micros(times)), casts: 5.0, pulled: 0.1, chose: 0 }
    }

    /// One slow frame in a round that isn't the quietest is still the worst there was, and nothing from
    /// another round gets into the quietest round's numbers.
    #[test]
    fn the_worst_is_the_slowest_of_every_frame_and_the_quietest_round_is_one_round() {
        let (first, second, third): (Vec<u64>, Vec<u64>, Vec<u64>) = (
            (0..100).map(|_| 4).chain([500]).collect(),
            (0..100).map(|_| 2).chain([90]).collect(),
            (0..100).map(|_| 3).chain([7]).collect(),
        );
        let rounds = [round_of(&first), round_of(&second), round_of(&third)];
        let mut all = micros(&[first, second, third].concat());
        let summary = Summary::of(&rounds, &mut all);

        assert_eq!(summary.all.worst, 500.0, "the slowest frame of them all");
        assert_eq!(summary.frames, 303);
        assert_eq!(summary.medians, (2.0, 4.0));
        assert_eq!(summary.all.median, 3.0);
        assert_eq!(summary.quietest.0, 2, "the second, counting from 1");
        assert_eq!(summary.quietest.1, rounds[1]);
        assert_eq!(summary.quietest.1.times.worst, 90.0, "its own worst, not the least of the rounds' 7");
    }

    /// A round's percentile is of that round's frames, and adding them to the whole set loses none.
    #[test]
    fn rounds_keep_their_frames() {
        let world = world();
        let mut all = vec![Duration::from_secs(1)];
        let done = round(&world, Load::Explore, 8, &mut all);
        assert_eq!(all.len(), 9);
        assert!(done.times.worst < 1e6, "{done:?}: the frame already there isn't this round's");
        assert_eq!(all[0], Duration::from_secs(1));
    }
}
