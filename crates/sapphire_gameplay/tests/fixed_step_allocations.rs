//! Rule R6, from the first tick: no heap allocation on the simulation thread inside
//! [`Simulation::step`], with spawns, despawns, hitstop and scales all happening inside it. Installs
//! `CountingAllocator` as this test binary's global allocator.

mod support;

use std::alloc::System;

use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_gameplay::{ActorSpawn, Buttons, SimConfig, Simulation, TickActions};
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
