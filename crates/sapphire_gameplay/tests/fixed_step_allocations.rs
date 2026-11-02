//! Rule R6, from the first tick: no heap allocation on the simulation thread inside
//! [`Simulation::step`], with moves, cancels, cues, warp, spawns, despawns, hitstop and scales all
//! happening inside it. Installs `CountingAllocator` as this test binary's global allocator.

mod support;

use std::alloc::System;

use sapphire_core::Name;
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_ecs::prelude::*;
use sapphire_gameplay::replay::{
    Player, Recording, Replay, SCENARIOS, Scenario, ScenarioError, StepAllocated, find_script, melee,
    melee_buttons,
};
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, CombatSet, INTENT_RING_LEN, SimConfig, Simulation,
    TickActions, WarpTarget,
};
use sapphire_math::Vec3;
use sapphire_moves::{MoveSet, MoveSetDocument};
use sapphire_time::ScaleSource;
use support::{press, script, slice, world};

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
    let stats = sim.stats();
    assert_eq!(stats.allocations, 0, "and the stat agrees");
    assert_eq!(stats.steps, 2000);
    assert!(stats.peak_actors > actors.len() as u32, "spawned actors came and went");
    assert!(stats.cancels_fired > 0 && stats.cues_published > 0, "{stats:?}");
    assert!(stats.intents_expired > 0, "{stats:?}");
}

/// A pool that's too small refuses inside a step, and that doesn't allocate either.
#[test]
fn a_full_pool_refuses_without_allocating() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let mut sim = Simulation::new(SimConfig { max_actors: 2, ..SimConfig::default() });
    let moves = sim.add_move_set(slice());
    sim.add_combat_system(CombatSet::Resolve, melee_buttons);
    let actor = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).expect("room");
    let spawn = press(actor, Buttons::JUMP);

    let ((), counts) = alloc::count_in_scope(|| {
        for tick in 0..240 {
            // Pressed every other tick, so it's a new press each time.
            sim.step(if tick % 2 == 0 { &spawn } else { &TickActions::NEUTRAL });
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

/// A recording of the sweep script from the melee scenario.
fn recorded(ticks: u32) -> Replay {
    let scenario = &SCENARIOS[0];
    let mut recording = Recording::new(scenario, scenario.config).expect("sets up");
    let mut source = find_script("melee-sweep").expect("listed").actions;
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
    let cast = melee(sim)?;
    sim.add_combat_system(CombatSet::Moves, allocates_from_the_40th_step);
    Ok(cast)
}

fn with_a_system_that_does_nothing(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = melee(sim)?;
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

/// Every button at once on alternate ticks into a long move with nothing to take them: the ring fills
/// and gives up its oldest presses.
#[test]
fn a_full_ring_gives_up_its_oldest_without_allocating() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let intents: String = [("a", "Light"), ("b", "Heavy"), ("c", "Dodge"), ("d", "Skill"), ("e", "Ultimate")]
        .map(|(name, button)| format!(r#"(name: "{name}", button: {button}, buffer: 15),"#))
        .concat();
    let document = MoveSetDocument::parse(&format!(
        r#"(
            type: "sapphire::moves::MoveSetDocument",
            version: 1,
            data: (
                intents: [{intents}],
                neutral: [(intent: "a", move: "long")],
                moves: [(name: "long", startup: 1, active: 1, recovery: 400)],
            ),
        )"#
    ))
    .expect("parses");
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_move_set(document.compile().expect("compiles"));
    let actor = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).expect("room");
    let all = Buttons::LIGHT | Buttons::HEAVY | Buttons::DODGE | Buttons::SKILL | Buttons::ULTIMATE;
    let mash = press(actor, all);

    let ((), counts) = alloc::count_in_scope(|| {
        for tick in 0..200 {
            sim.step(if tick % 2 == 0 { &mash } else { &TickActions::NEUTRAL });
        }
    });

    assert_eq!(counts.allocations, 0, "{} allocations", counts.allocations);
    let stats = sim.stats();
    assert!(stats.intents_overwritten > 0, "{stats:?}");
    assert!(sim.intents(actor).expect("alive").len() <= INTENT_RING_LEN);
}

/// More cues in one step than the queue holds, hit volumes past an actor's limit, warp windows and
/// a forced start, at a high time scale that crosses several windows a step.
#[test]
fn an_overfull_step_counts_what_it_drops_without_allocating() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let cues: String = (0..12).map(|at| format!(r#"(name: "cue_{at}", at: {at}),"#)).collect();
    let hitboxes: String = (0..12)
        .map(|at| {
            format!(
                r#"(name: "hit_{at}", from: {at}, to: {}, path: "blade", shape: Sphere(radius: 0.5)),"#,
                at + 1
            )
        })
        .collect();
    let document = MoveSetDocument::parse(&format!(
        r#"(
            type: "sapphire::moves::MoveSetDocument",
            version: 1,
            data: (
                intents: [(name: "go", button: Light, buffer: 15)],
                neutral: [(intent: "go", move: "busy")],
                moves: [(
                    name: "busy",
                    startup: 2,
                    active: 10,
                    recovery: 2,
                    motion: (
                        root: [(at: 12, offset: (0.0, 0.0, 2.0))],
                        paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 1.0))])],
                    ),
                    hitboxes: [{hitboxes}],
                    hurtboxes: [
                        (name: "a", from: 0, to: 12, state: Off),
                        (name: "b", from: 0, to: 12, state: Off),
                        (name: "c", from: 0, to: 12, state: Off),
                        (name: "d", from: 0, to: 12, state: Off),
                        (name: "e", from: 0, to: 12, state: Off),
                    ],
                    warps: [(from: 0, to: 12, max_distance: 1.0, max_turn: 0.5, keep: 0.5)],
                    cues: [{cues}],
                )],
            ),
        )"#
    ))
    .expect("parses");
    let set: MoveSet = document.compile().expect("compiles");
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_move_set(set);
    let actors: Vec<_> = (0..8)
        .map(|x| {
            let position = Vec3::new(x as f32, 0.0, 0.0);
            sim.spawn(ActorSpawn { position, moves: Some(moves), ..ActorSpawn::default() }).expect("room")
        })
        .collect();
    let mut go = TickActions::NEUTRAL;
    for &actor in &actors {
        go.set(actor, ActorActions { buttons: Buttons::LIGHT, ..ActorActions::NEUTRAL }).expect("room");
        assert!(sim.set_warp_target(actor, WarpTarget::Point(Vec3::new(0.0, 0.0, 9.0))));
        let mut clock = sim.clock_mut(actor).expect("alive");
        clock.0.push_scale(ScaleSource::Debug, 12_000).expect("room");
    }

    let ((), counts) = alloc::count_in_scope(|| {
        for tick in 0..120 {
            sim.step(if tick % 4 == 0 { &go } else { &TickActions::NEUTRAL });
        }
    });

    assert_eq!(counts.allocations, 0, "{} allocations", counts.allocations);
    let stats = sim.stats();
    assert!(stats.cues_dropped > 0 && stats.volumes_dropped > 0, "{stats:?}");
    assert!(sim.force_move(actors[0], Name::new("busy")));
    let ((), counts) = alloc::count_in_scope(|| sim.step(&TickActions::NEUTRAL));
    assert_eq!(counts.allocations, 0);
}
