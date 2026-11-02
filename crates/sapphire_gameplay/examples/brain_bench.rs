//! What a brain costs, against spec §3.2's fixed step (1.0 ms on Desktop-Low, 1.5 on Mobile-Low).
//!
//! ```text
//! cargo run --release -p sapphire_gameplay --example brain_bench [steps]
//! ```
//!
//! Three answers, in alternating rounds, each reporting its minimum across rounds:
//!
//! - **A fight:** whole steps of the shipped warrior's moves and brain. Everyone but the first actor
//!   thinks, each fighting the next, on a floor, with health enough that nobody falls, so hits land and
//!   reactions cut moves off for the whole run. A duel is what the slice ships.
//! - **Thinking alone:** brains that have nothing to do but stand and face, in a world where nothing else
//!   moves, against the same world with their brains off: the difference is `think`'s own cost.
//! - **One call:** `step_brain` by itself, on the tick it's busiest (scoring every attack and drawing)
//!   and an ordinary one (pacing).

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::time::Duration;

use sapphire_gameplay::brain::{Seen, SeenTarget, step_brain};
use sapphire_gameplay::{ActorId, ActorSpawn, Brain, ControllerProfile, SimConfig, Simulation, TickActions};
use sapphire_math::{Pcg32, Vec3};
use sapphire_moves::{MoveSet, MoveSetDocument, TargetDoing, TickRange};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};
use sapphire_time::{SimDuration, SimTime, Stopwatch};

const WARRIOR: &str = include_str!("../../../content/combat/enemy/warrior.moves.ron");

/// The warrior's document, with health enough for a bench.
fn warrior(edit: impl FnOnce(&mut MoveSetDocument)) -> MoveSet {
    let lasting = WARRIOR.replacen("health: 240,", "health: 4000000000,", 1);
    let mut document = MoveSetDocument::parse(&lasting).expect("parses");
    edit(&mut document);
    document.compile().expect("compiles")
}

/// Everyone on a floor, far enough apart to pace and close enough to reach.
fn world(actors: u32, spacing: f32, set: MoveSet) -> (Simulation, Vec<ActorId>) {
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
    let moves = Some(sim.add_move_set(set));
    let ids: Vec<ActorId> = (0..actors)
        .map(|index| {
            let position = Vec3::new(index as f32 * spacing, 0.0, 0.0);
            sim.spawn(ActorSpawn { position, moves, controller, ..ActorSpawn::default() }).expect("room")
        })
        .collect();
    (sim, ids)
}

/// Each thinks, fighting the next: the first has no brain in a duel, as a player has none.
fn give_brains(sim: &mut Simulation, ids: &[ActorId], from: usize) {
    for index in from..ids.len() {
        assert!(sim.give_brain(ids[index], ids[(index + 1) % ids.len()]));
    }
}

#[derive(Clone, Copy, Debug)]
struct Round {
    median: Duration,
    p999: Duration,
    worst: Duration,
}

impl Round {
    fn best(self, other: Self) -> Self {
        Self {
            median: self.median.min(other.median),
            p999: self.p999.min(other.p999),
            worst: self.worst.min(other.worst),
        }
    }
}

fn run(sim: &mut Simulation, steps: u64) -> Round {
    let mut times = Vec::with_capacity(steps as usize);
    for _ in 0..steps {
        sim.step(&TickActions::NEUTRAL);
        times.push(sim.stats().last_step);
    }
    times.sort_unstable();
    assert_eq!(sim.stats().allocations, 0, "rule R6");
    Round {
        median: times[times.len() / 2],
        p999: times[times.len() * 999 / 1000],
        worst: sim.stats().longest_step,
    }
}

fn fight(actors: u32, steps: u64) -> (Round, u64) {
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
fn standing(actors: u32, steps: u64, thinking: bool) -> Round {
    let quiet = warrior(|document| {
        let brain = document.brain.as_mut().expect("the warrior has one");
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
    let set = warrior(|document| {
        let brain = document.brain.as_mut().expect("the warrior has one");
        for attack in &mut brain.attacks {
            attack.reach.near = 0.0;
            attack.reach.far = 9.0;
        }
    });
    let def = set.brain().expect("the warrior has one");
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
        reaction_due: false,
        target: Some(SeenTarget {
            position: Vec3::new(1.0, 0.0, 2.0),
            doing: TargetDoing::Free,
            defeated: false,
        }),
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
    let steps: u64 = std::env::args().nth(1).and_then(|steps| steps.parse().ok()).unwrap_or(20_000);
    const SIZES: [u32; 3] = [2, 16, 64];
    const ROUNDS: usize = 7;

    let mut fights: [Option<(Round, u64)>; SIZES.len()] = [None; SIZES.len()];
    let mut idle: [[Option<Round>; 2]; SIZES.len()] = [[None; 2]; SIZES.len()];
    let (mut deciding, mut pacing) = (f64::MAX, f64::MAX);
    for _ in 0..ROUNDS {
        for (index, &size) in SIZES.iter().enumerate() {
            let (this, hits) = fight(size, steps);
            let kept = fights[index].map_or(this, |(best, _)| best.best(this));
            fights[index] = Some((kept, hits));
            for (slot, thinking) in [(0, false), (1, true)] {
                let this = standing(size, steps, thinking);
                idle[index][slot] = Some(idle[index][slot].map_or(this, |best| best.best(this)));
            }
        }
        deciding = deciding.min(one_call(true, 200_000));
        pacing = pacing.min(one_call(false, 200_000));
    }

    println!("{steps} steps, {ROUNDS} rounds, minimums across rounds");
    println!("a fight: everyone but the first thinks (a duel: one brain)");
    println!("actors  median step  99.9% step  worst step   hits");
    for (size, best) in SIZES.iter().zip(&fights) {
        let (best, hits) = best.expect("every size ran");
        println!(
            "{size:6}  {:9.2} µs  {:7.2} µs  {:7.2} µs  {hits:5}",
            best.median.as_secs_f64() * 1e6,
            best.p999.as_secs_f64() * 1e6,
            best.worst.as_secs_f64() * 1e6,
        );
    }
    println!("thinking alone: brains that stand and face, against the same world with them off");
    println!("actors    off median    on median   each brain");
    for (size, best) in SIZES.iter().zip(&idle) {
        let (off, on) = (best[0].expect("ran"), best[1].expect("ran"));
        let (off, on) = (off.median.as_secs_f64() * 1e6, on.median.as_secs_f64() * 1e6);
        println!("{size:6}  {off:9.2} µs  {on:8.2} µs  {:7.3} µs", (on - off) / f64::from(*size),);
    }
    println!("one call of step_brain: {deciding:.0} ns choosing among 3 attacks, {pacing:.0} ns pacing");
    println!("budget: 1000 µs a step on Desktop-Low, 1500 µs on Mobile-Low");
}
