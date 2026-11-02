//! What a fixed step costs, against spec §3.2's budget (1.0 ms on Desktop-Low, 1.5 on Mobile-Low).
//!
//! ```text
//! cargo run --release -p sapphire_gameplay --example step_bench [steps]
//! ```
//!
//! Runs the drill at a duel's size, a busy fight's, and a full pool's, every actor following long
//! tables with a path and pressing a button every few ticks, in alternating rounds, and reports each
//! size's minimum median and worst step across rounds. It reads the steps through
//! `Simulation::stats`, as the overlay would, and adds its checksum cost, which a replay pays per
//! tick only when asked.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::time::Duration;

use sapphire_core::Name;
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, SimConfig, Simulation, Stick, TickActions,
};
use sapphire_moves::{BakedPath, BakedTables, PathSample, RootSample};
use sapphire_time::Stopwatch;

/// 2 seconds of move at 60 Hz with one path, a typical swing's size.
fn tables() -> BakedTables {
    let root = (0..=120)
        .map(|tick| {
            let t = tick as f32;
            RootSample { translation: [0.0, 0.0, t * 0.05], yaw: t * 0.01 }
        })
        .collect();
    let blade = (0..=120)
        .map(|tick| {
            let t = tick as f32 * 0.02;
            PathSample { translation: [t, 1.0, 0.5], rotation: [0.0, 0.0, 0.0, 1.0] }
        })
        .collect();
    BakedTables::new(root, vec![BakedPath::new(Name::new("blade"), blade)]).expect("a good table")
}

fn world(actors: u32) -> (Simulation, Vec<ActorId>) {
    // Room for every actor and the transient ones they spawn.
    let mut sim = Simulation::new(SimConfig { seed: 1, max_actors: actors * 2, ..SimConfig::default() });
    let table = sim.add_tables(tables());
    let ids = (0..actors)
        .map(|_| sim.spawn(ActorSpawn { table: Some(table), ..ActorSpawn::default() }).expect("room"))
        .collect();
    (sim, ids)
}

/// One tick: the first actors a tick can hold press something on their own rhythm.
fn actions(tick: u64, ids: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    for (index, &id) in ids.iter().take(TickActions::CAPACITY).enumerate() {
        let buttons = match (tick + index as u64 * 7) % 40 {
            0 => Buttons::LIGHT,
            13 => Buttons::HEAVY,
            27 => Buttons::SKILL,
            _ => Buttons::NONE,
        };
        let stick = Stick::new((tick % 127) as i8, 0);
        actions.set(id, ActorActions { buttons, stick }).expect("room");
    }
    actions
}

struct Round {
    median: Duration,
    worst: Duration,
    checksum: Duration,
}

fn round(actors: u32, steps: u64) -> Round {
    let (mut sim, ids) = world(actors);
    let mut times = Vec::with_capacity(steps as usize);
    for tick in 0..steps {
        let input = actions(tick, &ids);
        sim.step(&input);
        times.push(sim.stats().last_step);
    }
    times.sort_unstable();
    let stopwatch = Stopwatch::start();
    let checksum = sim.checksum();
    let checksum_time = stopwatch.elapsed();
    assert_ne!(checksum, 0);
    assert_eq!(sim.stats().steps, steps);
    Round { median: times[times.len() / 2], worst: sim.stats().longest_step, checksum: checksum_time }
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
                    worst: other.worst.min(this.worst),
                    checksum: other.checksum.min(this.checksum),
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
    println!("actors  median step  worst step  checksum");
    for (size, best) in SIZES.iter().zip(&best) {
        let best = best.as_ref().expect("every size ran");
        println!(
            "{size:6}  {:9.2} µs  {:7.2} µs  {:5.2} µs",
            best.median.as_secs_f64() * 1e6,
            best.worst.as_secs_f64() * 1e6,
            best.checksum.as_secs_f64() * 1e6,
        );
    }
    println!("budget: 1000 µs on Desktop-Low, 1500 µs on Mobile-Low");
}
