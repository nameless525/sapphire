//! What a fixed step costs, against spec §3.2's budget (1.0 ms on Desktop-Low, 1.5 on Mobile-Low).
//!
//! ```text
//! cargo run --release -p sapphire_gameplay --example step_bench [steps]
//! ```
//!
//! Runs the test content's moves at a duel's size, a busy fight's and a full pool's, in alternating
//! rounds, and reports each size's rounds as the `rounds` module reads them. The worst is mostly the
//! scheduler while the machine's busy, on a different tick each run, so read the quietest round's
//! percentile for what a step's work costs. Every actor plays
//! strings, dodge cancels and dash attacks with a warp target and its ring in use, with its neighbours
//! in reach and a capsule on a floor, so hits land, reactions cut moves off, and hurtboxes and capsules
//! move, which is more than most of a real fight's actors do at once. Nobody falls: they're given health
//! for it, so the last steps cost what the first do. In the crowds, dodges go through swings, so perfect
//! dodges fire and counters start, and the world spends about half the run slowed. That costs more, not
//! less: a slowed swing stays live five times as many steps, and each of those sweeps. It reads the steps
//! through `Simulation::stats`, as the overlay would, and adds its checksum cost, which a replay pays per
//! tick only when asked. It fails if a step allocates: a counting allocator only costs what's allocated,
//! so it adds nothing to steps that don't.
//!
//! Its last row is a party's (M2 G7): three members and an opponent, on the same rhythm, with the party
//! swapping every second and a half, so every swap takes one member's capsule and hurtboxes out of the
//! world and puts another's in. Its worst step is a swap's, or a swap under the rest of the step's work.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

#[path = "../../sapphire_physics/examples/support/rounds.rs"]
mod rounds;

use std::alloc::System;
use std::time::Duration;

use rounds::Rounds;
use sapphire_core::alloc::{self, CountingAllocator};
use sapphire_gameplay::replay::{PARTY_TUNING, party_document};
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, ControllerProfile, PARTY_SLOTS, SimConfig, Simulation, Stick,
    TickActions, WarpTarget,
};
use sapphire_math::Vec3;
use sapphire_moves::{MoveSet, MoveSetDocument};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};
use sapphire_time::Stopwatch;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const SLICE: &str = include_str!("../../sapphire_moves/test_content/slice.moves.ron");

fn slice() -> MoveSet {
    let lasting = SLICE.replacen("health: 300,", "health: 4000000000,", 1);
    MoveSetDocument::parse(&lasting).expect("parses").compile().expect("compiles")
}

/// A simulation of this size on a floor, with a profile for everyone on it.
fn on_floor(actors: u32) -> (Simulation, Option<sapphire_gameplay::ProfileId>) {
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
    (sim, controller)
}

fn world(actors: u32) -> (Simulation, Vec<ActorId>) {
    let (mut sim, controller) = on_floor(actors);
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

/// A party of three, none of whom falls, and an opponent a metre and a bit ahead of them: the party's
/// input and the opponent's are all a tick has, as in a real fight.
fn party_world() -> (Simulation, Vec<ActorId>) {
    let (mut sim, controller) = on_floor(PARTY_SLOTS as u32 + 1);
    let members: Vec<ActorId> = (0..PARTY_SLOTS)
        .map(|member| {
            let mut document = party_document(member).expect("parses");
            document.fighter.as_mut().expect("a fighter").health = 4_000_000_000;
            let moves = Some(sim.add_move_set(document.compile().expect("compiles")));
            sim.spawn(ActorSpawn { moves, controller, ..ActorSpawn::default() }).expect("room")
        })
        .collect();
    let position = Vec3::new(0.0, 0.0, 1.2);
    let moves = Some(sim.add_move_set(slice()));
    let yaw = std::f32::consts::PI;
    let opponent = sim.spawn(ActorSpawn { position, yaw, moves, controller }).expect("room");
    sim.form_party(&members, PARTY_TUNING).expect("a good party");
    for &member in &members {
        sim.set_warp_target(member, WarpTarget::Actor(opponent));
    }
    sim.set_warp_target(opponent, WarpTarget::Actor(members[0]));
    (sim, vec![members[0], opponent])
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
    times: Vec<Duration>,
    checksum: Duration,
    /// Moves started, cancels fired, cues published, hits applied, perfect dodges fired and counters
    /// started, to show the steps did the work.
    work: (u64, u64, u64, u64, u64, u64),
}

/// What a row's rounds came to.
struct Row {
    rounds: Rounds,
    checksum: Duration,
    work: (u64, u64, u64, u64, u64, u64),
}

impl Default for Row {
    fn default() -> Self {
        Self { rounds: Rounds::new("p99.9", 999), checksum: Duration::MAX, work: (0, 0, 0, 0, 0, 0) }
    }
}

/// A party's tick: [`actions`], and a swap press every second and a half.
fn party_actions(tick: u64, ids: &[ActorId]) -> TickActions {
    let mut input = actions(tick, ids);
    if tick % 90 == 45 {
        let was = input.get(ids[0]);
        let pressed = ActorActions { buttons: was.buttons | Buttons::SWAP, ..was };
        input.set(ids[0], pressed).expect("room");
    }
    input
}

fn round(actors: u32, steps: u64) -> Round {
    let (sim, ids) = world(actors);
    run(sim, &ids, steps, actions)
}

fn party_round(steps: u64) -> Round {
    let (sim, ids) = party_world();
    let round = run(sim, &ids, steps, party_actions);
    assert!(round.work.0 > steps / 1000, "or starting moves: {:?}", round.work);
    round
}

fn run(
    mut sim: Simulation,
    ids: &[ActorId],
    steps: u64,
    actions: fn(u64, &[ActorId]) -> TickActions,
) -> Round {
    let actors = ids.len();
    let mut times = Vec::with_capacity(steps as usize);
    for tick in 0..steps {
        let input = actions(tick, ids);
        // Everyone past the tick's few plays what the first does, as a brain would have them.
        if tick % 48 == 0 {
            for &id in ids.iter().skip(TickActions::CAPACITY) {
                sim.force_move(id, sapphire_core::Name::new("dash_attack"));
            }
        }
        sim.step(&input);
        times.push(sim.stats().last_step);
    }
    let stopwatch = Stopwatch::start();
    let checksum = sim.checksum();
    let checksum_time = stopwatch.elapsed();
    assert_ne!(checksum, 0);
    let stats = sim.stats();
    assert_eq!(stats.steps, steps);
    assert_eq!((stats.allocations, stats.cues_dropped, stats.volumes_dropped), (0, 0, 0));
    assert!(stats.hits_applied > steps / 100, "a load that stops landing hits stops measuring: {stats:?}");
    // A duel's two dodge on a rhythm that misses each other's swings, and the crowds' don't.
    if sim.party().is_formed() {
        assert!(stats.swaps > (steps / 200) as u32, "or swapping: {stats:?}");
    } else if actors > 2 {
        assert!(stats.perfect_dodges > steps / 1000, "or firing perfect dodges: {stats:?}");
        assert!(stats.counters_fired > 0, "or starting counters: {stats:?}");
    }
    Round {
        times,
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
    assert!(alloc::is_installed(), "the step's allocation count would read 0 whatever it did");
    let steps = std::env::args().nth(1).and_then(|steps| steps.parse().ok()).unwrap_or(20_000);
    const SIZES: [u32; 3] = [2, 16, 64];
    const ROUNDS: usize = 7;

    let keep = |row: &mut Row, round: Round| {
        row.rounds.add(round.times);
        row.checksum = row.checksum.min(round.checksum);
        row.work = round.work;
    };
    let mut rows = SIZES.map(|_| Row::default());
    let mut party = Row::default();
    for _ in 0..ROUNDS {
        for (size, row) in SIZES.iter().zip(&mut rows) {
            keep(row, round(*size, steps));
        }
        keep(&mut party, party_round(steps));
    }

    let stopwatch = Stopwatch::start();
    let created = Simulation::new(SimConfig { max_actors: 128, ..SimConfig::default() });
    let creation = stopwatch.elapsed();
    drop(created);

    println!("creating a simulation with 128 pooled actors: {:.0} µs", creation.as_secs_f64() * 1e6);
    println!("{steps} steps a round, {ROUNDS} alternating rounds, in µs, with the least checksum of them");
    let rows = SIZES.iter().map(|size| format!("{size:6}")).zip(&mut rows);
    let party_row = std::iter::once(("party4".to_owned(), &mut party));
    for (size, row) in rows.chain(party_row) {
        let work = row.work;
        println!(
            "{size}: {}; checksum {:.2}; {} moves, {} cancels, {} cues, {} hits, {} perfect, {} counters",
            row.rounds.summary(),
            row.checksum.as_secs_f64() * 1e6,
            work.0,
            work.1,
            work.2,
            work.3,
            work.4,
            work.5,
        );
    }
    println!("budget: 1000 µs on Desktop-Low, 1500 µs on Mobile-Low");
}
