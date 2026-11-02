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
    ActorActions, ActorId, ActorSpawn, ActorTransform, Buttons, CombatSet, HitQuery, HurtboxId,
    INTENT_RING_LEN, MAX_HIT_RECORDS, OwnerStep, SimConfig, Simulation, TickActions, VolumeSweep, WarpTarget,
    remove_hurtboxes,
};
use sapphire_math::{Vec3, det};
use sapphire_moves::{BakedPath, MoveSet, MoveSetDocument, PathSample};
use sapphire_physics::{
    ColliderId, Group, GroupMask, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape,
};
use sapphire_time::{ScaleSource, SimDuration};
use support::{SLICE, mesh_arena, press, script, slice, world};

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

/// Characters on the mesh arena, counted from the first step: walking, stairs, walls, jumps, falls and
/// landings, every one of them a move or a cast, plus a move's root motion through the controller.
#[test]
fn characters_do_not_allocate_from_the_first_tick() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let (mut sim, actors) = characters(6);
    let moves = sim.add_move_set(slice());
    let lunger = sim
        .spawn(ActorSpawn {
            position: Vec3::new(2.0, 0.3, 2.0),
            moves: Some(moves),
            controller: sim.actor(actors[0]).and_then(|view| view.controller.profile()),
            ..ActorSpawn::default()
        })
        .expect("room");
    let ticks: Vec<TickActions> = (0..3000)
        .map(|tick| {
            let mut actions = mesh_arena::tick_actions(tick, &actors);
            if tick % 101 == 0 {
                actions
                    .set(lunger, ActorActions { buttons: Buttons::LIGHT, ..Default::default() })
                    .expect("room");
            }
            actions
        })
        .collect();

    let ((), counts) = alloc::count_in_scope(|| {
        for actions in &ticks {
            sim.step(actions);
        }
    });

    assert_eq!(counts.allocations, 0, "3000 steps allocated {} times", counts.allocations);
    assert_eq!(sim.stats().allocations, 0, "and the stat agrees");
    assert!(sim.stats().moves_started > 0, "the moves ran");
    let moved = sim.actors().filter(|view| view.position != Vec3::ZERO).count();
    assert_eq!(moved, actors.len() + 1, "they all went somewhere");
}

/// A capsule comes and goes with its actor, which doesn't allocate either.
#[test]
fn a_characters_body_comes_and_goes_without_allocating() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let (mut sim, actors) = characters(2);
    let profile = sim.actor(actors[0]).and_then(|view| view.controller.profile());
    let spawn =
        ActorSpawn { position: Vec3::new(5.0, 1.0, 5.0), controller: profile, ..ActorSpawn::default() };
    let ((), counts) = alloc::count_in_scope(|| {
        for _ in 0..100 {
            let id = sim.spawn(spawn).expect("room");
            sim.step(&TickActions::NEUTRAL);
            assert!(sim.despawn(id));
        }
    });
    assert_eq!(counts.allocations, 0, "{} allocations", counts.allocations);
    assert_eq!(sim.physics().body_count(), 2);
}

/// `count` different characters on the mesh arena.
fn characters(count: usize) -> (Simulation, Vec<ActorId>) {
    let mut sim = Simulation::new(SimConfig { max_actors: 16, ..SimConfig::default() });
    let world = PhysicsWorld::new(mesh_arena::statics(), sim.body_capacity()).expect("the arena builds");
    sim.set_physics_world(world).expect("it has room");
    let profiles =
        mesh_arena::profiles().map(|profile| sim.add_controller_profile(&profile).expect("a good one"));
    let actors = (0..count)
        .map(|index| {
            let (position, profile) = mesh_arena::start(index);
            sim.spawn(ActorSpawn { position, controller: Some(profiles[profile]), ..ActorSpawn::default() })
                .expect("room")
        })
        .collect();
    (sim, actors)
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
    let cues: String = (0..48).map(|cue| format!(r#"(name: "cue_{cue}", at: {}),"#, cue % 12)).collect();
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

/// Hit queries, counted from the very first sweep, on everything a sweep can do: turning, stopping at
/// a wall, re-arming, freezing, hurtboxes moving, coming and going, and every limit past full. Plain
/// inputs, so this needs no move system.
#[test]
fn hit_queries_do_not_allocate_from_the_first_sweep() {
    if !PER_THREAD_COUNTS {
        return;
    }
    const ATTACKERS: u32 = 40;
    const CROWD: u32 = 24;
    let actor = |raw: u32| ActorId::from_raw(raw).expect("not zero");

    let wall = StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(0.1, 3.0, 3.0) }),
        pose: Pose::at(Vec3::new(1.5, 1.0, 0.0)),
        memberships: Group::World.mask(),
    };
    let mut physics = PhysicsWorld::new(vec![wall], CROWD as usize).expect("it builds");

    // One path around a half circle a tick at a time, turning as it goes.
    let samples = (0..=30)
        .map(|tick| {
            let angle = tick as f32 * 0.1;
            let (sin, cos) = det::sin_cos(angle);
            PathSample {
                translation: [2.0 * cos, 1.0, 2.0 * sin],
                rotation: det::quat_from_rotation_z(angle).to_array(),
            }
        })
        .collect();
    let path = BakedPath::new(Name::new("blade"), samples);
    let shapes = [
        Shape::Sphere { radius: 0.1 },
        Shape::Capsule { half_height: 0.5, radius: 0.05 },
        Shape::Box { half_extents: Vec3::new(0.3, 0.1, 0.02) },
    ];
    let groups = [Name::new("first"), Name::new("second")];
    let rearm = [10, 20];
    // All within the half circle's reach: rows of them from its middle out.
    let chest = |index: u32| {
        let (sin, cos) = det::sin_cos(index as f32 * 0.125);
        PathSample { translation: [2.0 * cos, 1.0, 2.0 * sin], ..PathSample::IDENTITY }
    };

    let mut hits = HitQuery::new();
    let ((), counts) = alloc::count_in_scope(|| {
        for tick in 0_u64..2000 {
            // Hurtboxes come and go, and move.
            let root = ActorTransform { position: Vec3::X * det::sin(tick as f32 * 0.05) * 0.1, yaw: 0.0 };
            for index in 0..CROWD {
                let owner = actor(1000 + index);
                let id = HurtboxId::new(owner, 0).expect("in range");
                if tick % 97 == u64::from(index) {
                    remove_hurtboxes(&mut physics, owner);
                }
                let shape = match (tick + u64::from(index)) % 11 {
                    0 => None,
                    1 => Some(Shape::Sphere { radius: 0.4 }),
                    _ => Some(Shape::Sphere { radius: 0.3 }),
                };
                id.set(&mut physics, shape, root, chest(index)).expect("fits");
            }

            // Every attacker's volumes along the table, at speeds that change.
            let speed = [1000, 3000, 100, 0][(tick / 40 % 4) as usize];
            for attacker in 0..ATTACKERS {
                let reading = (tick * 1000 + u64::from(attacker) * 500) % 30_000;
                let previous = reading.saturating_sub(speed);
                let delta = SimDuration::from_subticks(speed);
                let spin = ActorTransform { yaw: speed as f32 * 0.0002, ..root };
                for volume in 0..3_u16 {
                    hits.sweep(
                        &physics,
                        &VolumeSweep {
                            attacker: actor(1 + attacker),
                            instance: (tick / 40) as u32,
                            volume,
                            group: groups[usize::from(volume % 2)],
                            rearm: if volume == 0 { &rearm } else { &[] },
                            shape: shapes[usize::from(volume)],
                            path: &path,
                            blocked_by: if volume == 1 { Group::World.mask() } else { GroupMask::NONE },
                            owner: OwnerStep {
                                delta,
                                reading: SimDuration::from_subticks(reading),
                                root_before: root,
                                root_after: spin,
                            },
                            from: SimDuration::from_subticks(previous),
                            to: SimDuration::from_subticks(reading),
                        },
                    );
                }
            }
            hits.finish_step();
            if tick % 500 == 499 {
                hits.forget_actor(actor(1));
                hits.forget_actor(actor(1000));
            }
        }
    });

    assert_eq!(counts.allocations, 0, "{} bytes allocated in hit queries", counts.bytes);
    let stats = hits.stats();
    assert!(stats.frozen_skips > 0, "some steps were frozen: {stats:?}");
    assert!(stats.events > 1000, "the fight must land hits: {stats:?}");
    assert!(stats.volumes_stopped > 0, "and stop at walls: {stats:?}");
    assert!(stats.contacts_dropped > 0, "and overflow every limit: {stats:?}");
    assert!(stats.events_dropped > 0, "{stats:?}");
    assert!(stats.hits_refused > 0, "{stats:?}");
    assert!(hits.record_count() <= MAX_HIT_RECORDS);
}

/// Hashing what hit queries keep, through a simulation's checksum, allocates nothing either.
#[test]
fn a_simulations_hit_state_hashes_without_allocating() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let sim = Simulation::new(SimConfig::default());
    assert!(sim.hits().events().is_empty());
    let (checksum, counts) = alloc::count_in_scope(|| sim.checksum());
    assert_ne!(checksum, 0);
    assert_eq!(counts.allocations, 0);
}

/// A fight's worth of the test content's moves in real steps, from the first: hurtboxes placed, sweeps
/// and hits, reactions forced through whatever's running, hitstop, pushes, a poise break, a defeat with a
/// finish freeze, slow motion, cues, and a despawn with its records and bodies. Everyone's in reach, and
/// nobody's in charge.
#[test]
fn a_fight_with_hits_landing_allocates_nothing_from_the_first_step() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let mut sim = Simulation::new(SimConfig::default());
    // Few enough hit points and little enough poise that someone breaks, and someone falls, before
    // the pushes part them for good.
    let frail = SLICE.replacen("health: 300", "health: 40", 1).replacen("poise: 60", "poise: 20", 1);
    let frail = MoveSetDocument::parse(&frail).expect("parses");
    let moves = Some(sim.add_move_set(frail.compile().expect("compiles")));
    let attacker = sim.spawn(ActorSpawn { moves, ..ActorSpawn::default() }).expect("room");
    let position = Vec3::new(0.0, 0.0, 1.6);
    let target = sim.spawn(ActorSpawn { position, yaw: 3.0, moves, controller: None }).expect("room");
    let at = Vec3::X;
    let second = sim.spawn(ActorSpawn { position: at, yaw: -1.0, moves, controller: None }).expect("room");
    let scripted: Vec<TickActions> = (0..2400_u64)
        .map(|tick| {
            let mut actions = TickActions::NEUTRAL;
            let every = |period, offset| tick % period == offset;
            let mut press = |actor, buttons| {
                actions.set(actor, ActorActions { buttons, ..ActorActions::default() }).expect("room");
            };
            press(
                attacker,
                match () {
                    () if every(40, 0) => Buttons::LIGHT,
                    () if every(90, 20) => Buttons::HEAVY,
                    () if every(130, 70) => Buttons::ULTIMATE,
                    _ => Buttons::NONE,
                },
            );
            press(target, if every(90, 45) { Buttons::DODGE } else { Buttons::NONE });
            press(second, if every(55, 5) { Buttons::SKILL } else { Buttons::NONE });
            actions
        })
        .collect();

    let ((), counts) = alloc::count_in_scope(|| {
        for (tick, actions) in scripted.iter().enumerate() {
            if tick == 1200 {
                sim.request_slow_motion(200, 30);
            }
            sim.step(actions);
        }
    });
    assert_eq!(counts.allocations, 0);
    let stats = sim.stats();
    assert_eq!(stats.allocations, 0);
    assert!(stats.hits_applied > 5, "{stats:?}");
    assert!(stats.reactions_forced > 3, "{stats:?}");
    assert!(stats.poise_breaks > 0, "{stats:?}");
    assert!(stats.defeats > 0, "nobody fell, so that wasn't tested: {stats:?}");
    assert!(sim.actors().any(|view| view.fighter.is_defeated()), "{stats:?}");

    let ((), counts) = alloc::count_in_scope(|| {
        sim.despawn(target);
        sim.despawn(attacker);
        sim.step(&TickActions::NEUTRAL);
    });
    assert_eq!(counts.allocations, 0, "a despawn gives room back and takes none");
}
