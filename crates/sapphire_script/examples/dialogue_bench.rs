//! What a dialogue costs between steps: `cargo run --release -p sapphire_script --example dialogue_bench`.
//!
//! On `dialogue-flag`'s fight, the Lantern Ward guide's dialogue: its conditions bound (compiled from the
//! source its cook kept, then loaded), a condition asked of the VM on a read-only frame, an effect applied,
//! and a panel's view of a line.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::time::Duration;

use sapphire_dialogue::{ConditionId, DialogueWalk, DialogueWorld, EffectId};
use sapphire_gameplay::replay::{LIGHT_THE_WARD, Recording, find_scenario};
use sapphire_script::scenarios::{SCRIPTED, ward_guide};
use sapphire_script::{ask, host_of};
use sapphire_time::Stopwatch;

const ROUNDS: usize = 20_000;
/// Bindings, each a compile and a load: fewer, since each costs more and a game binds once a dialogue.
const BINDS: usize = 500;

fn summary(what: &str, mut times: Vec<Duration>) {
    times.sort_unstable();
    let percentile = |share: f64| times[((times.len() - 1) as f64 * share) as usize];
    let mean = times.iter().sum::<Duration>() / times.len() as u32;
    println!(
        "{what}: mean {mean:?}, p50 {:?}, p99 {:?}, longest {:?}",
        percentile(0.5),
        percentile(0.99),
        percentile(1.0)
    );
}

/// A world whose conditions all hold and whose effects do nothing, for timing the view alone.
struct Open;

impl DialogueWorld for Open {
    fn ask(&mut self, _: ConditionId) -> bool {
        true
    }

    fn apply(&mut self, _: EffectId) {}
}

fn main() {
    let scenario = find_scenario(&SCRIPTED, "dialogue-flag").expect("listed");
    let mut recording = Recording::new(scenario, scenario.config).expect("starts");
    let dialogue = ward_guide();
    let mut binds = Vec::with_capacity(BINDS);
    for _ in 0..BINDS {
        let stopwatch = Stopwatch::start();
        let set = host_of(recording.simulation()).bind_conditions(&dialogue, "=ward_guide").expect("binds");
        binds.push(stopwatch.elapsed());
        drop(set);
        // A bind's functions stay in the VM until collected, as a game's would between steps: collect them
        // all here, outside what's timed, so a run of binds doesn't fill the VM's memory.
        let host = host_of(recording.simulation());
        while !host.gc_step().expect("collects") {}
    }
    let mut compiles = Vec::with_capacity(BINDS);
    for _ in 0..BINDS {
        let stopwatch = Stopwatch::start();
        let chunk = sapphire_script::cook(&dialogue.presentation.source).expect("compiles");
        compiles.push(stopwatch.elapsed());
        drop(chunk);
    }
    let mut scripts = Vec::with_capacity(BINDS);
    for _ in 0..BINDS {
        let stopwatch = Stopwatch::start();
        let chunk = sapphire_script::cook(sapphire_script::scenarios::ARENA_SCRIPTED).expect("compiles");
        scripts.push(stopwatch.elapsed());
        drop(chunk);
    }
    let mut set = host_of(recording.simulation()).bind_conditions(&dialogue, "=ward_guide").expect("binds");

    let mut asks = Vec::with_capacity(ROUNDS);
    let mut held = 0_u32;
    for round in 0..ROUNDS {
        let stopwatch = Stopwatch::start();
        held += u32::from(ask(&mut recording, &mut set, ConditionId((round % 2) as u16)));
        asks.push(stopwatch.elapsed());
    }

    // An effect is recorded as well as applied, so the recording's file grows by one command each.
    let mut effects = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let stopwatch = Stopwatch::start();
        recording.apply_dialogue_effect(LIGHT_THE_WARD).expect("loaded");
        effects.push(stopwatch.elapsed());
    }

    let walk = DialogueWalk::begin(&dialogue, &mut Open);
    let mut views = Vec::with_capacity(ROUNDS);
    let mut sink = 0_usize;
    for _ in 0..ROUNDS {
        let stopwatch = Stopwatch::start();
        let view = walk.view(&dialogue).expect("a line");
        sink += view.line.len() + view.choices.iter().flatten().count();
        views.push(stopwatch.elapsed());
    }

    summary("the guide's conditions bound (compiled, scanned and loaded)", binds);
    summary("the guide's conditions compiled and scanned alone", compiles);
    summary("arena-scripted's chunk compiled and scanned, for scale", scripts);
    summary("a condition asked", asks);
    summary("an effect applied and recorded", effects);
    summary("a line's view", views);
    let stats = host_of(recording.simulation()).stats();
    println!("({held} held, {} faulted, {sink})", stats.conditions_faulted);
}
