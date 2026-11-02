//! Rule R6, from the first tick: no heap allocation on the simulation thread inside
//! [`Simulation::step`], with spawns, despawns, hitstop and scales all happening inside it. Installs
//! `CountingAllocator` as this test binary's global allocator.

mod support;

use std::alloc::System;

use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_ecs::prelude::*;
use sapphire_gameplay::replay::{
    Player, Recording, Replay, SCENARIOS, Scenario, ScenarioError, StepAllocated, drill, find_script,
};
use sapphire_gameplay::{ActorId, ActorSpawn, Buttons, CombatSet, SimConfig, Simulation, TickActions};
use support::{curve, script, world};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// The whole script, 2000 ticks, counted from the first step: `Simulation::new` builds what would
/// otherwise allocate later. The inputs are built ahead of time because filling one in isn't part of
/// a step, though it wouldn't allocate either.
#[test]
fn steps_do_not_allocate_from_the_first_tick() {
    // Counts are exact per thread only where Rust uses native TLS (see `sapphire_core::alloc`).
    if !PER_THREAD_COUNTS {
        return;
    }
    let (mut sim, actors) = world(1);
    let ticks: Vec<TickActions> = (0..2000).map(|tick| script(tick, &actors)).collect();

    let ((), counts) = alloc::count_in_scope(|| {
        for actions in &ticks {
            sim.step(actions);
        }
    });

    assert!(alloc::is_installed());
    assert_eq!(counts.allocations, 0, "2000 steps allocated {} times", counts.allocations);
    assert_eq!(sim.stats().allocations, 0, "and the stat agrees");
    assert_eq!(sim.stats().steps, 2000);
    assert!(sim.stats().peak_actors > actors.len() as u32, "transient actors came and went");
}

/// A pool that's too small refuses inside a step, and that doesn't allocate either.
#[test]
fn a_full_pool_refuses_without_allocating() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let mut sim = Simulation::new(SimConfig { max_actors: 2, ..SimConfig::default() });
    let table = sim.add_tables(curve(30));
    let actor = sim.spawn(ActorSpawn { table: Some(table), ..ActorSpawn::default() }).expect("room");
    let mut press = TickActions::NEUTRAL;
    press
        .set(actor, sapphire_gameplay::ActorActions { buttons: Buttons::SKILL, ..Default::default() })
        .expect("room");

    let ((), counts) = alloc::count_in_scope(|| {
        for tick in 0..240 {
            // Pressed every other tick, so it's a new press each time.
            sim.step(if tick % 2 == 0 { &press } else { &TickActions::NEUTRAL });
        }
    });

    assert_eq!(counts.allocations, 0, "{} allocations", counts.allocations);
    assert!(sim.stats().spawns_refused > 0, "the pool filled");
}

/// A replay may check every tick, under the same count.
#[test]
fn checksums_do_not_allocate() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let (mut sim, actors) = world(1);
    for tick in 0..200 {
        sim.step(&script(tick, &actors));
    }
    let (checksum, counts) = alloc::count_in_scope(|| sim.checksum());
    assert_ne!(checksum, 0);
    assert_eq!(counts.allocations, 0);
}

/// A recording of the sweep script from the drill's scenario.
fn recorded(ticks: u32) -> Replay {
    let scenario = &SCENARIOS[0];
    let mut recording = Recording::new(scenario, scenario.config).expect("sets up");
    let mut source = find_script("drill-sweep").expect("listed").actions;
    for _ in 0..ticks {
        recording.step(&mut source).expect("short");
    }
    recording.finish().0
}

/// Spec §9's gate 4, as a player sees it: every tick of a replay, the file's cursor and the
/// checksums around each step included.
#[test]
fn a_replay_runs_with_no_allocation_in_any_step() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let mut player = Player::new(recorded(2000), SCENARIOS).expect("starts");
    let ((), counts) = alloc::count_in_scope(|| {
        // Past where a trail's reserved room would run out, it's the trail that grows, outside the
        // step. This run stays within it, so the whole of a tick costs nothing.
        while player.step() {}
    });
    assert_eq!(counts.allocations, 0, "2000 ticks allocated {} times", counts.allocations);
    let played = player.finish();
    assert_eq!(played.allocation_gate(), Ok(()));
    assert_eq!(played.stats.allocations, 0);
    assert_eq!(played.stats.steps, 2000);
}

/// The world's own system, added the way a scenario can: it allocates from its 40th run.
fn allocates_from_the_40th_step(mut runs: Local<'_, u32>) {
    *runs += 1;
    if *runs >= 40 {
        std::hint::black_box(Vec::<u8>::with_capacity(64));
    }
}

fn nothing() {}

fn with_an_allocating_system(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = drill(sim)?;
    sim.add_combat_system(CombatSet::Moves, allocates_from_the_40th_step);
    Ok(cast)
}

fn with_a_system_that_does_nothing(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = drill(sim)?;
    sim.add_combat_system(CombatSet::Moves, nothing);
    Ok(cast)
}

/// The gate bites: a step that allocates fails the run, and names the first tick that did.
#[test]
fn the_allocation_gate_bites_a_step_that_allocates() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let scenarios = [Scenario { setup: with_an_allocating_system, ..SCENARIOS[0] }];
    let mut player = Player::new(recorded(100), &scenarios).expect("starts");
    player.run_to_end();
    let played = player.finish();
    let failure = played.allocation_gate().expect_err("a step allocated");
    assert_eq!(failure, StepAllocated { first_tick: 40, allocations: 61 });
    assert!(failure.to_string().contains("first at tick 40"), "{failure}");

    // The same world without the allocation passes, so it's the allocation it bit on, and adding a
    // system costs its first step nothing.
    let scenarios = [Scenario { setup: with_a_system_that_does_nothing, ..SCENARIOS[0] }];
    let mut player = Player::new(recorded(100), &scenarios).expect("starts");
    player.run_to_end();
    assert_eq!(player.finish().allocation_gate(), Ok(()));
}
