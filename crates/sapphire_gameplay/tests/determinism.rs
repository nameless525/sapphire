//! One checksum for one set of inputs (spec §9, gate 2), through the public API.

mod support;

use std::io::Write as _;
use std::process::Command;

use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, DRILL_HITSTOP_TICKS, SimConfig, Simulation, Stick,
    TickActions,
};
use sapphire_math::Vec3;
use sapphire_moves::{BakedTables, RootSample};
use sapphire_time::SimDuration;
use support::{curve, run, script, world};

/// Long enough for every button in the script to fire, and transient actors to come and go.
const TICKS: u64 = 1200;

fn press(actor: ActorId, buttons: Buttons) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    actions.set(actor, ActorActions { buttons, stick: Stick::CENTER }).expect("room");
    actions
}

#[test]
fn the_script_exercises_what_it_claims_to() {
    let (mut sim, actors) = world(1);
    let mut followed = false;
    let mut frozen = false;
    let mut extra_actors = false;
    for tick in 0..TICKS {
        sim.step(&script(tick, &actors));
        followed |= sim.actors().any(|view| view.root_motion.table().is_some());
        frozen |= sim.actors().any(|view| view.hitstop_ticks > 0);
        extra_actors |= sim.actors().len() > actors.len();
    }
    assert!(followed && frozen && extra_actors, "{followed} {frozen} {extra_actors}");
    let stats = sim.stats();
    assert_eq!(stats.steps, TICKS);
    assert_eq!(sim.tick(), TICKS);
    assert!(stats.peak_actors > actors.len() as u32);
    assert_eq!(stats.spawns_refused, 0, "the pool has room for the script");
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

/// What the script's whole state hashes to, on every OS: `sapphire_gameplay`'s state is scalars and
/// `libm` so far, so CI's x86-64 runners and an arm64 Mac must all reach this. It stands until D2's
/// recorded replays take over. A package that adds state to the checksum updates it.
#[test]
fn the_script_reaches_its_recorded_checksum() {
    let (mut sim, actors) = world(7);
    let checksums = run(&mut sim, &actors, TICKS);
    let got = (checksums[0], checksums[119], checksums[TICKS as usize - 1]);
    assert_eq!(got, RECORDED, "the state's hash moved: {got:#x?}");
}

const RECORDED: (u64, u64, u64) = (0xd1e8_a770_c2ad_d49f, 0x6319_e472_3dd3_9429, 0x2a48_77fd_9e24_ca60);

#[test]
fn a_different_seed_changes_the_checksum_once_a_number_is_drawn() {
    let (mut a, actors) = world(7);
    let (mut b, _) = world(8);
    assert_ne!(a.checksum(), b.checksum(), "the generator's state is in the checksum from the start");
    let (a, b) = (run(&mut a, &actors, TICKS), run(&mut b, &actors, TICKS));
    assert!(a.iter().zip(&b).all(|(a, b)| a != b));
}

#[test]
fn any_difference_in_input_changes_the_checksum_at_that_tick() {
    let (mut plain, actors) = world(3);
    let baseline = run(&mut plain, &actors, 100);

    // One step of one stick's axis, on one tick.
    let (mut nudged, _) = world(3);
    for tick in 0..100 {
        let mut actions = script(tick, &actors);
        if tick == 40 {
            let was = actions.get(actors[1]);
            let stick = Stick::new(was.stick.x().wrapping_add(1), was.stick.y());
            actions.set(actors[1], ActorActions { stick, ..was }).expect("room");
        }
        nudged.step(&actions);
        assert_eq!(nudged.checksum() == baseline[tick as usize], tick < 40, "tick {tick}");
    }

    // One button on one tick, which nothing acts on yet: the state holds it as this tick's input and
    // then last tick's, and has nothing else to tell them apart after that.
    let (mut pressed, _) = world(3);
    for tick in 0..100 {
        let mut actions = script(tick, &actors);
        if tick == 40 {
            let was = actions.get(actors[1]);
            actions
                .set(actors[1], ActorActions { buttons: was.buttons | Buttons::JUMP, ..was })
                .expect("room");
        }
        pressed.step(&actions);
        let differs = (40..=41).contains(&tick);
        assert_eq!(pressed.checksum() != baseline[tick as usize], differs, "tick {tick}");
    }
}

/// The same IDs, with different entities behind them: the histories free the pool's entities in
/// opposite orders, so the actors spawned next take them the other way round. Every actor stands
/// somewhere of its own and presses at once, and each transient actor takes the next random yaw and
/// its parent's position, so a parent's turn shows. Going through a bare query instead fails this.
#[test]
fn the_order_of_creation_doesnt_leak_through_iteration_order() {
    fn history(reverse_despawns: bool) -> (Vec<ActorId>, Vec<u64>) {
        let mut sim = Simulation::new(SimConfig { seed: 11, max_actors: 32, ..SimConfig::default() });
        let table = sim.add_tables(curve(30));
        let at = |x: u32| ActorSpawn {
            position: Vec3::new(x as f32, 0.0, 0.0),
            table: Some(table),
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
                        ActorActions { buttons: Buttons::SKILL | Buttons::LIGHT, stick: Stick::CENTER };
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

/// A transient actor's whole table moves it by the table's last sample, turned by its starting yaw,
/// however a press froze or slowed it. The readings telescope, give or take float rounding.
#[test]
fn a_table_moves_an_actor_by_its_last_sample() {
    let mut sim = Simulation::new(SimConfig { seed: 1, ..SimConfig::default() });
    let table = curve(30);
    let last = *table.root_samples().last().expect("samples");
    let id = sim.add_tables(table);
    let yaw = 0.75;
    let actor = sim.spawn(ActorSpawn { yaw, table: Some(id), ..ActorSpawn::default() }).expect("room");

    sim.step(&press(actor, Buttons::LIGHT));
    let mut held = false;
    for tick in 0..80 {
        // Freeze it, then halve it, partway through.
        let actions = match tick {
            5 => press(actor, Buttons::HEAVY),
            15 => press(actor, Buttons::DODGE),
            _ => TickActions::NEUTRAL,
        };
        sim.step(&actions);
        held |= sim.actor(actor).expect("alive").hitstop_ticks == DRILL_HITSTOP_TICKS - 1;
    }
    assert!(held, "the freeze happened");

    let view = sim.actor(actor).expect("alive");
    assert_eq!(view.root_motion.table(), None, "the table has ended");
    let expected = sapphire_math::det::rotate_y(last.offset(), yaw);
    assert!(view.position.abs_diff_eq(expected, 1e-4), "{} against {expected}", view.position);
    assert!((view.yaw - (yaw + last.yaw)).abs() < 1e-5, "{}", view.yaw);
}

/// Hitstop and a time scale arrive at the table as exact whole sub-ticks, and the reading is the
/// table's own at that time, bit for bit.
#[test]
fn hitstop_and_time_scale_reach_the_tables_reading_exactly() {
    let mut sim = Simulation::new(SimConfig::default());
    let table = curve(60);
    let id = sim.add_tables(table.clone());
    let actor = sim.spawn(ActorSpawn { table: Some(id), ..ActorSpawn::default() }).expect("room");
    let reads = |sim: &Simulation, subticks: u64| {
        let motion = sim.actor(actor).expect("alive").root_motion;
        assert_eq!(motion.elapsed().as_subticks(), subticks, "tick {}", sim.tick());
        let expected = table.root_at(SimDuration::from_subticks(subticks));
        let actual = motion.reading();
        assert_eq!(actual.translation.map(f32::to_bits), expected.translation.map(f32::to_bits));
        assert_eq!(actual.yaw.to_bits(), expected.yaw.to_bits());
    };

    // The press reads the table's zero in its own step.
    sim.step(&press(actor, Buttons::LIGHT));
    reads(&sim, 0);
    assert_eq!(sim.actor(actor).expect("alive").root_motion.reading(), RootSample::ZERO);

    // Normal speed: a whole tick each step, reading exactly the table's samples.
    sim.step(&TickActions::NEUTRAL);
    reads(&sim, 1000);
    sim.step(&TickActions::NEUTRAL);
    reads(&sim, 2000);

    // Hitstop freezes the next six ticks exactly, the press's own step having already moved.
    sim.step(&press(actor, Buttons::HEAVY));
    reads(&sim, 3000);
    for _ in 0..DRILL_HITSTOP_TICKS {
        sim.step(&TickActions::NEUTRAL);
        reads(&sim, 3000);
    }
    sim.step(&TickActions::NEUTRAL);
    reads(&sim, 4000);

    // Half speed: half a tick each step, from the step after the press.
    sim.step(&press(actor, Buttons::DODGE));
    reads(&sim, 5000);
    sim.step(&TickActions::NEUTRAL);
    reads(&sim, 5500);
    let (a, b) = (table.root_samples()[5], table.root_samples()[6]);
    let halfway = sim.actor(actor).expect("alive").root_motion.reading();
    assert_eq!(halfway.translation[2], a.translation[2] + (b.translation[2] - a.translation[2]) * 0.5);
    sim.step(&TickActions::NEUTRAL);
    reads(&sim, 6000);

    // The world at a fifth as well: a tenth of a tick each step.
    sim.step(&press(actor, Buttons::ULTIMATE));
    reads(&sim, 6500);
    for step in 1..=10 {
        sim.step(&TickActions::NEUTRAL);
        reads(&sim, 6500 + step * 100);
    }

    // And back: another press of each, and a whole tick again from the next step.
    sim.step(&press(actor, Buttons::DODGE | Buttons::ULTIMATE));
    reads(&sim, 7600);
    sim.step(&TickActions::NEUTRAL);
    reads(&sim, 8600);
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

#[test]
fn the_tables_hash_changes_with_the_move_data_and_nothing_else() {
    let hash_of = |tables: Vec<BakedTables>| {
        let mut sim = Simulation::new(SimConfig::default());
        for table in tables {
            sim.add_tables(table);
        }
        sim.step(&TickActions::NEUTRAL);
        sim.tables_hash()
    };
    let base = hash_of(vec![curve(20), curve(30)]);
    assert_eq!(base, hash_of(vec![curve(20), curve(30)]));
    assert_ne!(base, hash_of(vec![curve(30), curve(20)]), "order is in the IDs");
    assert_ne!(base, hash_of(vec![curve(20), curve(31)]));
    assert_ne!(base, hash_of(vec![curve(20)]));
}

#[test]
fn cooked_tables_load_and_bad_ones_are_refused() {
    let table = curve(20);
    let cooked = table.to_cooked().expect("cooks");
    let mut sim = Simulation::new(SimConfig::default());
    let id = sim.add_cooked_tables(&cooked).expect("loads");
    let mut plain = Simulation::new(SimConfig::default());
    plain.add_tables(table);
    assert_eq!(sim.tables_hash(), plain.tables_hash());
    assert_eq!(id.raw(), 0);

    let mut stale = cooked.clone();
    stale[16] += 1;
    assert!(sim.add_cooked_tables(&stale).is_err());
}
