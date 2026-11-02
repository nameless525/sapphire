//! What hit queries cost per fixed step, against spec §3.2's budgets for Desktop-Low: the simulation's
//! 1.0 ms, which they count toward as combat, and physics' 0.7 ms, since most of what they do is casts.
//!
//! ```text
//! cargo run --release -p sapphire_gameplay --example hit_bench [steps]
//! ```
//!
//! Each row runs real work. Actors stand in a ring with two hurtboxes each, set every step. Each
//! attacker swings two volumes every 13 ticks: a blade laid out from the root through a 3 rad arc (three
//! casts a tick, as the sub-step rule asks of a blade that fast) and a fist running straight out (one).
//! Their reach takes in their neighbours, so hits land, records fill and retire and queues sort. The
//! world holds 225 static boxes, which sweeps' trees walk past. Rows run in alternating rounds, with a
//! warm-up each, and report the minimum median, 99th percentile and worst step across rounds, so
//! other work on the machine shows up as little as it can.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::hint::black_box;
use std::time::Duration;

use sapphire_core::Name;
use sapphire_gameplay::{ActorId, ActorTransform, HitQuery, HurtboxId, OwnerStep, VolumeSweep};
use sapphire_math::{Vec3, det};
use sapphire_moves::{BakedPath, PathSample};
use sapphire_physics::{
    ColliderId, Group, GroupMask, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape,
};
use sapphire_time::{SimDuration, Stopwatch};

const WARM_STEPS: u32 = 120;
const ROUNDS: u32 = 7;
const SWING_TICKS: u64 = 12;

/// How far apart neighbours stand on the ring, in metres: inside a blade's reach.
const SPACING: f32 = 1.5;

/// Bodies each actor has: two hurtboxes, and the capsule the controller will add.
const BODIES_PER_ACTOR: usize = 3;

const HURTBOX: Shape = Shape::Capsule { half_height: 0.3, radius: 0.25 };

/// One row of the table.
#[derive(Clone, Copy)]
struct Load {
    what: &'static str,
    attackers: usize,
    actors: usize,
    /// Ticks of table each step covers: a time scale.
    ticks_per_step: u64,
    /// Whether volumes stop at the world, which adds a cast of each sub-step against the scenery.
    stop_at_walls: bool,
}

struct Scene {
    load: Load,
    world: PhysicsWorld,
    hits: HitQuery,
    actors: Vec<(ActorId, ActorTransform)>,
    blade: BakedPath,
    fist: BakedPath,
    tick: u64,
}

/// A hurtbox's place on its actor.
fn hurtbox_local(index: u8) -> PathSample {
    PathSample { translation: [0.0, 0.6 + 0.8 * f32::from(index), 0.0], ..PathSample::IDENTITY }
}

fn scene(load: Load) -> Scene {
    let statics = (0..225_u32)
        .map(|index| StaticCollider {
            id: ColliderId(index),
            shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(1.0, 2.0, 1.0) }),
            pose: Pose::at(Vec3::new(
                (index % 15) as f32 * 8.0 - 57.0,
                2.0,
                (index / 15) as f32 * 8.0 - 57.0,
            )),
            memberships: Group::World.mask(),
        })
        .collect();
    let mut world = PhysicsWorld::new(statics, load.actors * BODIES_PER_ACTOR).expect("the scene builds");

    let radius = SPACING / (2.0 * det::sin(PI / load.actors as f32));
    let spot = |index: usize| {
        let (sin, cos) = det::sin_cos(index as f32 / load.actors as f32 * TAU);
        Vec3::new(radius * sin, 0.0, radius * cos)
    };
    let actors: Vec<(ActorId, ActorTransform)> = (0..load.actors)
        .map(|index| {
            let id = ActorId::from_raw(index as u32 + 1).expect("not zero");
            let position = spot(index);
            // Facing the next actor, so its swing takes in that one's hurtboxes.
            let yaw = det::yaw_of_direction(spot(index + 1) - position);
            let root = ActorTransform { position, yaw };
            for hurtbox in 0..2 {
                let hurtbox_id = HurtboxId::new(id, hurtbox).expect("in range");
                hurtbox_id.set(&mut world, Some(HURTBOX), root, hurtbox_local(hurtbox)).expect("it fits");
            }
            (id, root)
        })
        .collect();

    // Laid out from the root: a capsule's +Y turned onto +Z, then swept about the root.
    let lay_out = det::quat_from_rotation_x(FRAC_PI_2);
    let blade = (0..=SWING_TICKS)
        .map(|tick| {
            let angle = -1.5 + tick as f32 / SWING_TICKS as f32 * 3.0;
            let (sin, cos) = det::sin_cos(angle);
            PathSample {
                translation: [sin, 1.2, cos],
                rotation: (det::quat_from_rotation_y(angle) * lay_out).to_array(),
            }
        })
        .collect();
    let fist = (0..=SWING_TICKS)
        .map(|tick| PathSample { translation: [0.2, 1.3, 0.3 + tick as f32 * 0.14], ..PathSample::IDENTITY })
        .collect();

    Scene {
        load,
        world,
        hits: HitQuery::new(),
        actors,
        blade: BakedPath::new(Name::new("blade"), blade),
        fist: BakedPath::new(Name::new("fist"), fist),
        tick: 0,
    }
}

impl Scene {
    /// One step: what `CombatSet::Movement` does for hurtboxes, then `CombatSet::Hits`.
    fn step(&mut self) {
        self.tick += 1;
        let Load { attackers, ticks_per_step, stop_at_walls, .. } = self.load;
        for &(id, root) in &self.actors {
            for index in 0..2 {
                let hurtbox = HurtboxId::new(id, index).expect("in range");
                hurtbox.set(&mut self.world, Some(HURTBOX), root, hurtbox_local(index)).expect("it fits");
            }
        }
        let blocked_by = if stop_at_walls { Group::World.mask() } else { GroupMask::NONE };
        let delta = SimDuration::from_ticks(ticks_per_step);
        for (slot, &(attacker, root)) in self.actors.iter().enumerate().take(attackers) {
            // Each attacker on its own beat through its own swings.
            let beat = (self.tick + slot as u64 * 5) * ticks_per_step;
            let elapsed = beat % (SWING_TICKS + 1);
            let instance = (beat / (SWING_TICKS + 1)) as u32;
            let from = SimDuration::from_ticks(elapsed.saturating_sub(ticks_per_step));
            let to = SimDuration::from_ticks(elapsed);
            let owner = OwnerStep { delta, reading: to, root_before: root, root_after: root };
            let volumes = [
                (0, Name::new("slash"), Shape::Capsule { half_height: 0.5, radius: 0.05 }, &self.blade),
                (1, Name::new("jab"), Shape::Sphere { radius: 0.12 }, &self.fist),
            ];
            for (volume, group, shape, path) in volumes {
                self.hits.sweep(
                    &self.world,
                    &VolumeSweep {
                        attacker,
                        instance,
                        volume,
                        group,
                        rearm: &[],
                        shape,
                        path,
                        blocked_by,
                        owner,
                        from,
                        to,
                    },
                );
            }
        }
        self.hits.finish_step();
        black_box(self.hits.events());
    }
}

#[derive(Clone, Copy)]
struct Round {
    median: Duration,
    p99: Duration,
    worst: Duration,
    casts_per_step: f64,
    hits_per_step: f64,
}

fn round(load: Load, steps: u32) -> Round {
    let mut scene = scene(load);
    for _ in 0..WARM_STEPS {
        scene.step();
    }
    let before = scene.hits.stats();
    let mut times = Vec::with_capacity(steps as usize);
    for _ in 0..steps {
        let stopwatch = Stopwatch::start();
        scene.step();
        times.push(stopwatch.elapsed());
    }
    let after = scene.hits.stats();
    assert_eq!(after.results_dropped, 0, "the crowd must fit a cast");
    assert_eq!(after.events_dropped + after.contacts_dropped + after.hits_refused, 0, "and the queues");
    assert!(after.events > before.events, "and hits must land, or this measures too little: {after:?}");
    times.sort_unstable();
    Round {
        median: times[times.len() / 2],
        p99: times[times.len() * 99 / 100],
        worst: times[times.len() - 1],
        casts_per_step: (after.sub_steps - before.sub_steps) as f64 / f64::from(steps),
        hits_per_step: (after.events - before.events) as f64 / f64::from(steps),
    }
}

fn main() {
    let steps = std::env::args().nth(1).and_then(|text| text.parse().ok()).unwrap_or(4000_u32);
    let load = |what, attackers, actors, ticks_per_step, stop_at_walls| Load {
        what,
        attackers,
        actors,
        ticks_per_step,
        stop_at_walls,
    };
    let loads = [
        load("a duel, one swinging", 1, 2, 1, false),
        load("a duel, both swinging", 2, 2, 1, false),
        load("a busy fight", 4, 8, 1, false),
        load("16 attackers", 16, 16, 1, false),
        load("16 attackers among 32", 16, 32, 1, false),
        load("16 attackers at 3x", 16, 16, 3, false),
        load("16 attackers, walls can stop", 16, 16, 1, true),
    ];
    let mut best: Vec<Option<Round>> = vec![None; loads.len()];

    for _ in 0..ROUNDS {
        for (slot, &load) in loads.iter().enumerate() {
            let round = round(load, steps);
            let best = best[slot].get_or_insert(round);
            best.median = best.median.min(round.median);
            best.p99 = best.p99.min(round.p99);
            best.worst = best.worst.min(round.worst);
        }
    }

    println!("hit queries: minimums across {ROUNDS} alternating rounds of {steps} steps each");
    println!(
        "load                            casts  hits  median µs  p99 µs  worst µs  of 700 µs  of 1000 µs"
    );
    for (load, round) in loads.iter().zip(&best) {
        let round = round.expect("every load ran");
        let median = round.median.as_secs_f64() * 1e6;
        println!(
            "{:<30} {:>6.1} {:>5.2} {median:>10.1} {:>7.1} {:>9.1} {:>9.1}% {:>10.1}%",
            load.what,
            round.casts_per_step,
            round.hits_per_step,
            round.p99.as_secs_f64() * 1e6,
            round.worst.as_secs_f64() * 1e6,
            median / 7.0,
            median / 10.0,
        );
    }
}
