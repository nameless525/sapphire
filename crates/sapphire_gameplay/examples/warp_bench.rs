//! What a warp window costs a step, by how long the window is.
//!
//! ```text
//! cargo run --release -p sapphire_gameplay --example warp_bench [rounds]
//! ```
//!
//! Actors play one move whose whole length is a warp window, its root running a metre, with no physics
//! world and nothing else to do, and each row is whole playings: every step of the window, once the
//! first has warmed the step's queries. With no target the warp leaves the table alone, so the rows
//! with one less that row are the warp's own cost. Configurations take turns, and each row reports the
//! lowest of its rounds: this Mac is busy.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::time::Duration;

use sapphire_core::Name;
use sapphire_gameplay::{ActorId, ActorSpawn, SimConfig, Simulation, TickActions, WarpTarget};
use sapphire_math::Vec3;
use sapphire_moves::{MoveSet, MoveSetDocument};
use sapphire_time::Stopwatch;

/// A set of one move, `ticks` long, warping all the way.
fn set(ticks: u32) -> MoveSet {
    let text = format!(
        r#"(
            type: "sapphire::moves::MoveSetDocument",
            version: 1,
            data: (
                intents: [(name: "go", button: Light, buffer: 8, priority: 1)],
                neutral: [(intent: "go", move: "long")],
                moves: [(
                    name: "long",
                    startup: 0,
                    active: 1,
                    recovery: {recovery},
                    motion: (root: [(at: {ticks}, offset: (0.0, 0.0, 1.0), yaw: 0.0)]),
                    warps: [(from: 0, to: {ticks}, max_distance: 1.0, max_turn: 0.5, keep: 0.5)],
                )],
            ),
        )"#,
        recovery = ticks - 1,
    );
    MoveSetDocument::parse(&text).expect("parses").compile().expect("compiles")
}

fn playing(actors: u32, ticks: u32, warping: bool) -> (Simulation, Vec<ActorId>) {
    let mut sim = Simulation::new(SimConfig { seed: 1, max_actors: actors, ..SimConfig::default() });
    let moves = Some(sim.add_move_set(set(ticks)));
    let ids: Vec<ActorId> = (0..actors)
        .map(|index| {
            let position = Vec3::new(index as f32 * 4.0, 0.0, 0.0);
            let id = sim.spawn(ActorSpawn { position, moves, ..ActorSpawn::default() }).expect("room");
            if warping {
                sim.set_warp_target(id, WarpTarget::Point(position + Vec3::new(0.5, 0.0, 3.0)));
            }
            sim.force_move(id, Name::new("long"));
            id
        })
        .collect();
    // The first step starts what's forced: everything after is the window.
    sim.step(&TickActions::NEUTRAL);
    (sim, ids)
}

/// Every step of the window, as (median, mean, longest).
fn one_playing(actors: u32, ticks: u32, warping: bool) -> (Duration, Duration, Duration) {
    let (mut sim, ids) = playing(actors, ticks, warping);
    let mut steps = Vec::with_capacity(ticks as usize);
    for _ in 2..ticks {
        let stopwatch = Stopwatch::start();
        sim.step(&TickActions::NEUTRAL);
        steps.push(stopwatch.elapsed());
    }
    assert!(sim.actor(ids[0]).expect("alive").moves.current.is_some(), "it's still in the window");
    let mean = steps.iter().sum::<Duration>() / steps.len() as u32;
    steps.sort_unstable();
    (steps[steps.len() / 2], mean, steps[steps.len() - 1])
}

fn main() {
    let rounds: u32 = std::env::args().nth(1).and_then(|text| text.parse().ok()).unwrap_or(7);
    let rows: Vec<(u32, u32, bool)> = [1, 32]
        .into_iter()
        .flat_map(|actors| {
            [30, 120, 121, 600, 3600]
                .into_iter()
                .flat_map(move |ticks| [false, true].map(|warping| (actors, ticks, warping)))
        })
        .collect();
    let mut best = vec![(Duration::MAX, Duration::MAX, Duration::MAX); rows.len()];
    for _ in 0..rounds {
        for (row, &(actors, ticks, warping)) in rows.iter().enumerate() {
            let (median, mean, longest) = one_playing(actors, ticks, warping);
            let least = &mut best[row];
            *least = (least.0.min(median), least.1.min(mean), least.2.min(longest));
        }
    }
    println!("actors  window  target  median_us  mean_us  longest_us");
    for (&(actors, ticks, warping), (median, mean, longest)) in rows.iter().zip(&best) {
        println!(
            "{actors:>6}  {ticks:>6}  {target:>6}  {median:>9.2}  {mean:>7.2}  {longest:>10.2}",
            target = if warping { "yes" } else { "no" },
            median = median.as_secs_f64() * 1e6,
            mean = mean.as_secs_f64() * 1e6,
            longest = longest.as_secs_f64() * 1e6,
        );
    }
}
