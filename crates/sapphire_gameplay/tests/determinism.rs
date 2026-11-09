//! One checksum for one set of inputs (spec §9, gate 2), through the public API.

mod support;

use std::io::Write as _;
use std::process::Command;

use sapphire_ecs::prelude::*;
use sapphire_gameplay::replay;
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, CombatSet, ControllerProfile, SimConfig, SimRng, Simulation,
    Stick, TickActions,
};
use sapphire_math::Vec3;
use sapphire_moves::{MoveSet, MoveSetDocument};
use support::{press, run, script, slice, slice_cooked, world};

/// Long enough for every button in the script to fire, and spawned actors to come and go.
const TICKS: u64 = 1200;

#[test]
fn the_script_exercises_what_it_claims_to() {
    let (mut sim, actors) = world(1);
    let mut playing = false;
    let mut frozen = false;
    let mut extra_actors = false;
    let mut hit_volumes = false;
    for tick in 0..TICKS {
        sim.step(&script(tick, &actors));
        playing |= sim.actors().any(|view| view.moves.current.is_some());
        frozen |= sim.actors().any(|view| view.hitstop_ticks > 0);
        extra_actors |= sim.actors().len() > actors.len();
        hit_volumes |= sim.actors().any(|view| !view.moves.volumes.is_empty());
    }
    assert!(
        playing && frozen && extra_actors && hit_volumes,
        "{playing} {frozen} {extra_actors} {hit_volumes}"
    );
    let stats = sim.stats();
    assert_eq!(stats.steps, TICKS);
    assert_eq!(sim.tick(), TICKS);
    assert!(stats.peak_actors > actors.len() as u32);
    assert_eq!(stats.spawns_refused, 0, "the pool has room for the script");
    assert!(stats.cancels_fired > 10, "{stats:?}");
    assert!(stats.cues_published > 10, "{stats:?}");
    assert!(stats.intents_expired > 0, "{stats:?}");
    assert!(stats.intents_pressed > stats.moves_started / 2, "{stats:?}");
    assert_eq!(stats.cues_dropped + stats.volumes_dropped + stats.intents_overwritten, 0, "{stats:?}");
}

/// Every tick's checksum agrees, not only the last: a difference shows where it began.
#[test]
fn the_same_inputs_give_the_same_checksum_in_one_process() {
    let (mut first, actors) = world(7);
    let (mut second, same_actors) = world(7);
    assert_eq!(actors, same_actors, "the same spawns give the same IDs");
    let (a, b) = (run(&mut first, &actors, TICKS), run(&mut second, &actors, TICKS));
    assert_eq!(a, b);
    // No checksum repeats while time passes: the tick count is in it.
    let mut sorted = a.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), a.len());
}

/// Set for the child process of [`the_same_inputs_give_the_same_checksum_across_processes`].
const CHILD: &str = "SAPPHIRE_GAMEPLAY_DETERMINISM_CHILD";

/// Prints the script's checksums when another run of this test binary asks for them. It writes past
/// the test harness's capture, so a child's stdout holds this line among the harness's.
#[test]
fn prints_checksums_for_a_parent_process() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let (mut sim, actors) = world(7);
    let checksums = run(&mut sim, &actors, TICKS);
    let line = format!("checksums {checksums:x?}\n");
    std::io::stdout().write_all(line.as_bytes()).expect("stdout");
}

/// A new process has new addresses and hash seeds, and threads that start otherwise: nothing of
/// that may reach a result.
#[test]
fn the_same_inputs_give_the_same_checksum_across_processes() {
    let (mut sim, actors) = world(7);
    let expected = format!("checksums {:x?}", run(&mut sim, &actors, TICKS));
    for _ in 0..2 {
        let output = Command::new(std::env::current_exe().expect("this test binary"))
            .args(["--exact", "prints_checksums_for_a_parent_process"])
            .env(CHILD, "1")
            .output()
            .expect("the test binary runs again");
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.lines().find(|line| line.contains("checksums [")).expect("the child's checksums");
        // The harness may print on the same line first ("test name ... ").
        let printed = &line[line.find("checksums [").expect("found above")..];
        assert_eq!(printed, expected);
    }
}

/// Time passing is a difference, with nothing in the world to show it.
#[test]
fn the_tick_count_is_in_the_checksum() {
    let mut sim = Simulation::new(SimConfig::default());
    let before = sim.checksum();
    sim.step(&TickActions::NEUTRAL);
    assert_ne!(sim.checksum(), before);
    assert_eq!(sim.tick(), 1);
}

/// One replay file holds one hash of the move data and controller profiles for every architecture, so
/// a document must cook to the same bytes on all of them, unlike per-tick checksums, which may differ.
/// This world holds what `melee` and `duel` hash, so it's what their replays are stamped with. CI's
/// `Test` job holds
/// this one constant on Linux, Windows and macOS. If only some of them fail, something on the bake's
/// path has reached glam's SIMD types or a C math library (see `sapphire_math::det`), and that's the
/// bug: don't record a value per architecture. If all of them fail, the test content or the format
/// changed: update this and run `cargo xtask replay --update`.
#[test]
fn the_test_contents_hash_is_the_same_on_every_architecture() {
    let mut sim = Simulation::new(SimConfig::default());
    sim.add_controller_profile(&ControllerProfile::default()).expect("a good profile");
    sim.add_move_set(replay::test_moves().expect("compiles"));
    let hash = sim.tables_hash();
    assert_eq!(hash, TEST_CONTENT_HASH, "the test content's hash is {hash:#x}");
}

const TEST_CONTENT_HASH: u64 = 0x0df2_0528_3f2a_718d;

#[test]
fn a_different_seed_changes_the_checksum_once_a_number_is_drawn() {
    let (mut a, actors) = world(7);
    let (mut b, _) = world(8);
    assert_ne!(a.checksum(), b.checksum(), "the generator's state is in the checksum from the start");
    let (a, b) = (run(&mut a, &actors, TICKS), run(&mut b, &actors, TICKS));
    assert!(a.iter().zip(&b).all(|(a, b)| a != b));
}

#[test]
fn any_difference_in_input_changes_the_checksum_from_that_tick() {
    let (mut plain, actors) = world(3);
    let baseline = run(&mut plain, &actors, 100);

    // One step of one stick's axis, on one tick, which nothing acts on yet: the state holds it as
    // this tick's input and then last tick's, and has nothing else to tell them apart after that.
    let (mut nudged, _) = world(3);
    for tick in 0..100 {
        let mut actions = script(tick, &actors);
        if tick == 40 {
            let was = actions.get(actors[1]);
            let stick = Stick::new(was.stick.x().wrapping_add(1), was.stick.y());
            actions.set(actors[1], ActorActions { stick, ..was }).expect("room");
        }
        nudged.step(&actions);
        let differs = (40..=41).contains(&tick);
        assert_eq!(nudged.checksum() != baseline[tick as usize], differs, "tick {tick}");
    }

    // A press of a button that starts something: everything differs from then on. It's the first
    // tick's, since the script's own presses and the reactions they bring keep this actor busy by
    // tick 40, and a press that nothing takes is gone with its buffer.
    let (mut pressed, _) = world(3);
    let actor = actors[1];
    for tick in 0..100 {
        let mut actions = script(tick, &actors);
        if tick == 0 {
            let was = actions.get(actor);
            assert!(!was.buttons.contains(Buttons::SKILL), "the script doesn't press it here");
            actions.set(actor, ActorActions { buttons: was.buttons | Buttons::SKILL, ..was }).expect("room");
        }
        pressed.step(&actions);
        assert_ne!(pressed.checksum(), baseline[tick as usize], "tick {tick}");
    }
}

/// The same IDs, with different entities behind them: the histories free the pool's entities in
/// opposite orders, so the actors spawned next take them the other way round. Every actor stands
/// somewhere of its own and presses at once, and each spawned actor takes the next random yaw and
/// its parent's position, so a parent's turn shows. Going through a bare query instead fails this.
#[test]
fn the_order_of_creation_doesnt_leak_through_iteration_order() {
    fn history(reverse_despawns: bool) -> (Vec<ActorId>, Vec<u64>) {
        let mut sim = Simulation::new(SimConfig { seed: 11, max_actors: 32, ..SimConfig::default() });
        let moves = sim.add_move_set(slice());
        replay::add_melee_buttons(&mut sim);
        let at = |x: u32| ActorSpawn {
            position: Vec3::new(x as f32, 0.0, 0.0),
            moves: Some(moves),
            ..ActorSpawn::default()
        };
        let mut ids: Vec<ActorId> = (0..6).map(|x| sim.spawn(at(x)).expect("room")).collect();
        let mut gone = [ids[1], ids[2], ids[4]];
        if reverse_despawns {
            gone.reverse();
        }
        for id in gone {
            assert!(sim.despawn(id));
        }
        ids.retain(|id| !gone.contains(id));
        ids.extend((6..9).map(|x| sim.spawn(at(x)).expect("room")));

        let mut checksums = Vec::new();
        for tick in 0..120 {
            let mut actions = TickActions::NEUTRAL;
            if tick % 20 == 0 {
                for &id in ids.iter().take(TickActions::CAPACITY) {
                    let actor =
                        ActorActions { buttons: Buttons::JUMP | Buttons::LIGHT, stick: Stick::CENTER };
                    actions.set(id, actor).expect("room");
                }
            }
            sim.step(&actions);
            checksums.push(sim.checksum());
        }
        (ids, checksums)
    }
    let (first_ids, first) = history(false);
    let (second_ids, second) = history(true);
    assert_eq!(first_ids, second_ids);
    assert_eq!(first, second);
}

/// The creation-order test above has teeth: these two histories put the same IDs in different
/// entities, so a system that draws numbers in a bare query's order, as the crate's rules forbid,
/// gives them different checksums, where ID order agrees.
#[test]
fn a_bare_querys_order_would_leak_into_the_checksum() {
    fn leaky(
        mut rng: ResMut<'_, SimRng>,
        mut transforms: Query<'_, '_, &mut sapphire_gameplay::ActorTransform>,
    ) {
        for mut transform in &mut transforms {
            transform.yaw += rng.0.unit_f32();
        }
    }
    fn history(reverse: bool, leak: bool) -> u64 {
        let mut sim = Simulation::new(SimConfig { max_actors: 8, ..SimConfig::default() });
        let moves = sim.add_move_set(slice());
        let spawn = ActorSpawn { moves: Some(moves), ..ActorSpawn::default() };
        let actors: Vec<ActorId> = (0..4).map(|_| sim.spawn(spawn).unwrap()).collect();
        let (a, b) = if reverse { (actors[2], actors[0]) } else { (actors[0], actors[2]) };
        assert!(sim.despawn(a));
        assert!(sim.despawn(b));
        for _ in 0..2 {
            sim.spawn(spawn).unwrap();
        }
        if leak {
            sim.add_combat_system(CombatSet::Moves, leaky);
        }
        for _ in 0..8 {
            sim.step(&TickActions::NEUTRAL);
        }
        sim.checksum()
    }
    assert_eq!(history(false, false), history(true, false), "ID order hides the entities' order");
    assert_ne!(history(false, true), history(true, true), "the two histories really differ");
}

/// The app may keep a simulation on any thread, and a replay job may run several.
#[test]
fn a_simulation_can_move_between_threads() {
    fn assert_send<T: Send>() {}
    assert_send::<Simulation>();
}

#[test]
fn actor_ids_count_up_and_are_never_reused() {
    let mut sim = Simulation::new(SimConfig { max_actors: 2, ..SimConfig::default() });
    let spawn = ActorSpawn::default();
    let first = sim.spawn(spawn).expect("room");
    let second = sim.spawn(spawn).expect("room");
    assert_eq!((first.raw(), second.raw()), (1, 2));
    assert!(sim.spawn(spawn).is_err(), "the pool is full");
    assert_eq!(sim.stats().spawns_refused, 0, "the stat updates with each step");
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(sim.stats().spawns_refused, 1);

    assert!(sim.despawn(first));
    assert!(!sim.despawn(first), "already gone");
    assert!(sim.actor(first).is_none());
    let third = sim.spawn(spawn).expect("room again");
    assert_eq!(third.raw(), 3, "a freed entity comes back with a new ID");
    // A refused spawn doesn't burn a number.
    assert!(sim.spawn(spawn).is_err());
    sim.despawn(second);
    assert_eq!(sim.spawn(spawn).expect("room").raw(), 4);
}

#[test]
fn actions_for_an_actor_that_is_gone_change_nothing() {
    let (mut a, actors) = world(5);
    let (mut b, _) = world(5);
    for sim in [&mut a, &mut b] {
        sim.despawn(actors[2]);
    }
    a.step(&TickActions::NEUTRAL);
    b.step(&press(actors[2], Buttons::LIGHT));
    assert_eq!(a.checksum(), b.checksum());
}

#[test]
fn stats_count_steps_against_the_budget() {
    let mut sim =
        Simulation::new(SimConfig { step_budget: std::time::Duration::ZERO, ..SimConfig::default() });
    assert_eq!(sim.stats().steps, 0);
    assert_eq!(sim.stats().mean_step(), std::time::Duration::ZERO);
    for _ in 0..4 {
        sim.step(&TickActions::NEUTRAL);
    }
    let stats = sim.stats();
    assert_eq!(stats.steps, 4);
    assert!(stats.longest_step >= stats.last_step);
    assert_eq!(stats.over_budget_steps, 4, "nothing runs in no time");
    assert!(stats.total_step_time >= stats.longest_step);
    assert!(stats.mean_step() <= stats.longest_step);
}

fn tweaked(edit: impl FnOnce(&mut MoveSetDocument)) -> MoveSet {
    let mut document = MoveSetDocument::parse(support::SLICE).expect("parses");
    edit(&mut document);
    document.compile().expect("compiles")
}

/// A move's data is part of the hash in every way a designer can change it: one edit per track.
#[test]
fn the_tables_hash_changes_with_the_move_data_and_nothing_else() {
    let hash_of = |sets: Vec<MoveSet>| {
        let mut sim = Simulation::new(SimConfig::default());
        for set in sets {
            sim.add_move_set(set);
        }
        sim.step(&TickActions::NEUTRAL);
        sim.tables_hash()
    };
    let base = hash_of(vec![slice()]);
    assert_eq!(base, hash_of(vec![slice()]));
    assert_ne!(base, hash_of(vec![]));
    assert_ne!(base, hash_of(vec![slice(), slice()]));

    type Edit = fn(&mut MoveSetDocument);
    let edits: &[(&str, Edit)] = &[
        ("an intent's buffer", |d| d.intents[0].buffer += 1),
        ("an intent's priority", |d| d.intents[0].priority += 1),
        ("a neutral start", |d| d.neutral[0].target = "skill".to_owned()),
        ("a phase", |d| d.moves[0].recovery += 1),
        ("the root motion", |d| d.moves[0].motion.root[0].offset[2] += 0.25),
        ("a path", |d| d.moves[0].motion.paths[0].keys[0].offset[0] += 0.25),
        ("a hitbox's window", |d| d.moves[0].hitboxes[0].to -= 1),
        ("a hitbox's shape", |d| {
            d.moves[0].hitboxes[0].shape = sapphire_moves::Shape::Sphere { radius: 0.125 };
        }),
        ("a cancel window", |d| d.moves[0].cancels[0].from += 1),
        ("a cancel's mode", |d| d.moves[0].cancels[0].allow[0].when = sapphire_moves::CancelMode::Instant),
        ("a cue", |d| d.moves[0].cues[0].at += 1),
        ("invulnerability", |d| d.moves[3].invulnerable[0].to -= 1),
        ("a perfect-dodge window", |d| d.moves[3].perfect_dodge[0].to -= 1),
        ("a perfect-dodge volume", |d| {
            d.moves[3].perfect_dodge[0].volume.as_mut().unwrap().offset[1] += 0.25;
        }),
        ("a slowdown's speed", |d| d.moves[3].perfect_dodge[0].slow_motion.as_mut().unwrap().per_mille = 250),
        ("a slowdown's length", |d| d.moves[3].perfect_dodge[0].slow_motion.as_mut().unwrap().steps += 1),
        ("a counter window", |d| d.moves[3].perfect_dodge[0].counter.as_mut().unwrap().ticks += 1),
        ("what a counter starts", |d| {
            d.moves[3].perfect_dodge[0].counter.as_mut().unwrap().allow[0].target = "dash_attack".to_owned();
        }),
        ("a hurtbox", |d| d.moves[3].hurtboxes[0].to -= 1),
        ("a warp's limit", |d| d.moves[4].warps[0].max_distance += 0.25),
        ("a parry window", |d| d.moves[5].parry[0].to -= 1),
    ];
    for (what, edit) in edits {
        assert_ne!(base, hash_of(vec![tweaked(edit)]), "{what}");
    }
}

#[test]
fn cooked_sets_load_and_bad_ones_are_refused() {
    let cooked = slice_cooked();
    let mut sim = Simulation::new(SimConfig::default());
    let id = sim.add_cooked_move_set(&cooked).expect("loads");
    let mut plain = Simulation::new(SimConfig::default());
    plain.add_move_set(slice());
    assert_eq!(sim.tables_hash(), plain.tables_hash());
    assert_eq!(id.raw(), 0);

    let mut stale = cooked.clone();
    stale[16] += 1;
    assert!(sim.add_cooked_move_set(&stale).is_err());
}
