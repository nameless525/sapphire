//! What a crowd of brains costs, against spec §3.2's fixed step (1.0 ms on Desktop-Low, 1.5 on Mobile-Low).
//!
//! ```text
//! cargo run --release -p sapphire_gameplay --example chase_bench [rounds]
//! ```
//!
//! Whole steps of `nav-chase` on the arena's real navmesh: 64 brains after a player running `chase-laps`,
//! each with its own controller, in alternating rounds, with a crowd and with `apart` at 0, which is what
//! keeping them apart costs. They don't walk the same way, so the difference is a fair reading of the crowd
//! rather than of [`Crowd::lean`](sapphire_gameplay) alone.
//!
//! Beside each, what the navigation did: searches and nodes don't depend on a machine's load at all. And a
//! tick's least across the rounds, as `nav_bench` reads one: the same tick of the same run each time, so
//! what's left is what that tick's work costs, and a busy machine's scheduler mostly isn't in it.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

#[path = "../../sapphire_physics/examples/support/rounds.rs"]
mod rounds;

use std::alloc::System;
use std::time::Duration;

use rounds::Rounds;
use sapphire_core::alloc::{self, CountingAllocator};
use sapphire_gameplay::Simulation;
use sapphire_gameplay::replay::{SCENARIOS, chase_world, find_scenario, find_script};
use sapphire_moves::{EnemyDocument, EnemyMind, MoveSet, MoveSetDocument};
use sapphire_nav::NavStats;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const CHASER: &str = include_str!("../../sapphire_moves/test_content/chaser.moves.ron");
const CHASER_KIND: &str = include_str!("../../sapphire_moves/test_content/chaser.enemy.ron");

fn chasers(apart: f32) -> (MoveSet, EnemyMind) {
    let moves = MoveSetDocument::parse(CHASER).expect("it parses");
    let mut kind = EnemyDocument::parse(CHASER_KIND).expect("it parses");
    kind.brains.values_mut().for_each(|brain| brain.apart = apart);
    let mind = kind.compile(&moves).expect("it compiles").mind;
    (moves.compile().expect("it compiles"), mind)
}

/// One run of the script: each step's time, and what the navigation spent.
fn round(apart: f32) -> (Vec<Duration>, NavStats) {
    let scenario = find_scenario(&SCENARIOS, "nav-chase").expect("listed");
    let script = find_script("chase-laps").expect("listed");
    let mut sim = Simulation::new(scenario.config);
    let (player, _) = chase_world(&mut sim, chasers(apart), false).expect("it sets up");
    let cast = [player];
    let mut times = Vec::with_capacity(script.ticks as usize);
    let mut stats = NavStats::default();
    for tick in 0..u64::from(script.ticks) {
        sim.step(&script.actions(tick, &cast));
        times.push(sim.stats().last_step);
        let spent = sim.navigation_mut().expect("it has one").take_stats();
        stats.searches += spent.searches;
        stats.expanded += spent.expanded;
        stats.deferred += spent.deferred;
        stats.worst_tick = stats.worst_tick.max(spent.worst_tick);
    }
    assert_eq!(sim.stats().allocations, 0, "rule R6");
    (times, stats)
}

fn main() {
    assert!(alloc::is_installed(), "the step's allocation count would read 0 whatever it did");
    let rounds: usize = std::env::args().nth(1).and_then(|rounds| rounds.parse().ok()).unwrap_or(9);
    let modes = [("a crowd, apart 1.2 m", 1.2), ("a queue, apart 0", 0.0)];
    let script = find_script("chase-laps").expect("listed");
    let mut times = modes.map(|_| Rounds::new("p99", 990));
    let mut spent = modes.map(|_| NavStats::default());
    let mut least = modes.map(|_| vec![Duration::MAX; script.ticks as usize]);
    for _ in 0..rounds {
        for (index, (_, apart)) in modes.iter().enumerate() {
            let (steps, stats) = round(*apart);
            for (least, step) in least[index].iter_mut().zip(&steps) {
                *least = (*least).min(*step);
            }
            times[index].add(steps);
            spent[index] = stats;
        }
    }
    println!("64 brains and a player, {} steps a round, {rounds} alternating rounds, in µs", script.ticks);
    for (index, (name, _)) in modes.iter().enumerate() {
        let stats = spent[index];
        println!(
            "{name}: {} searches, {} nodes, worst tick {} nodes, {} deferred\n  {}",
            stats.searches,
            stats.expanded,
            stats.worst_tick,
            stats.deferred,
            times[index].summary(),
        );
        let slowest = least[index].iter().enumerate().max_by_key(|(_, time)| **time).expect("it ran").0;
        let ticks = &mut least[index];
        let at_slowest = ticks[slowest];
        ticks.sort_unstable();
        let share = |share: f64| ticks[((ticks.len() - 1) as f64 * share) as usize].as_secs_f64() * 1e6;
        println!(
            "  each tick's least: median {:.1}, p99 {:.1}, worst {:.1} at tick {slowest}",
            share(0.5),
            share(0.99),
            at_slowest.as_secs_f64() * 1e6,
        );
    }
    println!("budget: 1000 µs a step on Desktop-Low, 1500 µs on Mobile-Low");
}
