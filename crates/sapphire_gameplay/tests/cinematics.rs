//! A move's cinematic (M3 Q3b) through the public API: the move names it, the fight loads it by that name
//! and refuses a set naming one it hasn't, and the run starts on the move's first tick, holding the others
//! while the move plays through at its own speed.

mod support;

use sapphire_core::Name;
use sapphire_gameplay::replay::{
    ARENA_ULTIMATE_AT, PlayError, Recording, SCENARIOS, arena_ultimate, find_scenario,
};
use sapphire_gameplay::{
    ActorId, ActorSpawn, Buttons, CinematicError, SimConfig, Simulation, TickActions, TimelineProgress,
};
use sapphire_math::Vec3;
use sapphire_moves::{MoveSet, MoveSetDocument};
use sapphire_time::SUBTICKS_PER_TICK;
use sapphire_timeline::{TimelineDocument, TimelineSimulation};
use support::{SLICE, press};

const CINEMATIC_DOCUMENT: &str = include_str!("../../sapphire_timeline/test_content/cinematic.timeline.ron");
/// How long `CINEMATIC_DOCUMENT` runs, in steps.
const CINEMATIC_TICKS: u32 = 150;
/// How long the player's ultimate's cinematic runs, in steps: 2.4 s.
const ULTIMATE_TICKS: u32 = 144;
/// What the test content's `spin` names, in the tests below.
const FINISHER: &str = "finisher";

fn cinematic() -> TimelineSimulation {
    TimelineDocument::from_ron(CINEMATIC_DOCUMENT).expect("reads").cook().expect("cooks").simulation
}

/// The test content, with its `spin` (on the ultimate button) naming `cinematic`.
fn slice_naming(cinematic: &str) -> MoveSet {
    let mut document = MoveSetDocument::parse(SLICE).expect("parses");
    let spin = document.moves.iter_mut().find(|mv| mv.name == "spin").expect("the test content spins");
    spin.cinematic = Some(cinematic.to_owned());
    document.compile().expect("compiles")
}

/// Two actors on the set naming the finisher, twenty metres apart, out of the spin's reach, with it loaded if `loaded`.
fn fight(loaded: bool) -> (Simulation, ActorId, ActorId) {
    fight_on(slice_naming(FINISHER), loaded)
}

fn fight_on(set: MoveSet, loaded: bool) -> (Simulation, ActorId, ActorId) {
    let mut sim = Simulation::new(SimConfig::default());
    if loaded {
        sim.add_cinematic(FINISHER, cinematic()).expect("loads");
    }
    let moves = sim.add_move_set(set);
    let owner = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).expect("room");
    let other = sim
        .spawn(ActorSpawn {
            moves: Some(moves),
            position: Vec3::new(0.0, 0.0, 20.0),
            ..ActorSpawn::default()
        })
        .expect("room");
    (sim, owner, other)
}

fn current(sim: &Simulation, actor: ActorId) -> Option<Name> {
    sim.actor(actor).expect("alive").moves.current.map(|mv| mv.name)
}

fn delta(sim: &Simulation, actor: ActorId) -> u64 {
    sim.actor(actor).expect("alive").clock_delta.as_subticks()
}

#[test]
fn a_move_starts_its_cinematic_on_its_first_tick_and_it_plays_to_its_end() {
    let (mut sim, owner, _) = fight(true);
    sim.check_cinematics().expect("it's loaded");
    sim.step(&press(owner, Buttons::ULTIMATE));
    assert_eq!(current(&sim, owner), Some(Name::intern("spin")));
    let id = sim.timeline().expect("playing").id;
    assert_eq!(
        sim.timeline(),
        Some(TimelineProgress { id, tick: 0, ticks: CINEMATIC_TICKS, owner: Some(owner) })
    );
    assert_eq!(sim.timelines_played(), 1);
    assert_eq!(sim.timeline_steps_held(), CINEMATIC_TICKS, "it holds input from the move's first tick");

    for _ in 1..CINEMATIC_TICKS {
        sim.step(&press(owner, Buttons::ULTIMATE));
    }
    assert!(sim.timeline().is_some(), "still on its last step");
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(sim.timeline(), None);
    assert_eq!(sim.timelines_played(), 1, "pressing all through it started nothing more");
    assert_eq!(sim.cinematics_refused(), 0);
}

/// Its time scale slows the world and counter-scales the owner's clock, as a perfect dodge does, so the
/// owner's move plays through at its own speed while the other is held; and both come back at its end.
#[test]
fn the_others_are_held_while_the_owners_move_plays_through() {
    let (mut sim, owner, other) = fight(true);
    sim.step(&press(owner, Buttons::ULTIMATE));
    let tick = u64::from(SUBTICKS_PER_TICK);
    let mut slowed = None;
    for step in 1..CINEMATIC_TICKS {
        sim.step(&TickActions::NEUTRAL);
        if delta(&sim, other) < tick {
            slowed = Some(step);
            break;
        }
    }
    let slowed = slowed.expect("the cinematic slows the world");
    assert!((45..=47).contains(&slowed), "slowed on step {slowed}");
    assert_eq!(delta(&sim, other), 300, "the other is held at 0.3x");
    assert!(delta(&sim, owner).abs_diff(tick) <= 1, "the owner plays through: {}", delta(&sim, owner));

    while sim.timeline().is_some() {
        sim.step(&TickActions::NEUTRAL);
    }
    sim.step(&TickActions::NEUTRAL);
    assert_eq!((delta(&sim, owner), delta(&sim, other)), (tick, tick), "both come back at its end");
    // Over the whole run, the other lost about half a second to it, and the owner a sub-tick a step at most.
    let now = |actor| sim.actor(actor).expect("alive").clock_now.as_subticks();
    let steps = sim.tick() * tick;
    assert!(steps - now(owner) <= u64::from(CINEMATIC_TICKS), "the owner lost {}", steps - now(owner));
    assert!(steps - now(other) > 30 * tick, "the other lost {}", steps - now(other));
}

#[test]
fn a_set_naming_a_cinematic_the_fight_has_not_loaded_is_refused_as_it_is_prepared() {
    let (sim, _, _) = fight(false);
    match sim.check_cinematics() {
        Err(CinematicError::Missing { set, mv, name }) => {
            assert_eq!((set, mv.as_str(), name), (0, "spin", Name::intern(FINISHER)));
        }
        other => panic!("refused by name, not {other:?}"),
    }

    let mut sim = Simulation::new(SimConfig::default());
    sim.add_cinematic(FINISHER, cinematic()).expect("loads");
    assert!(
        matches!(sim.add_cinematic(FINISHER, cinematic()), Err(CinematicError::Taken(name)) if name == FINISHER)
    );
    let mut held = cinematic();
    held.ticks = 0;
    assert!(matches!(sim.add_cinematic("held", held), Err(CinematicError::Held(name)) if name == "held"));
}

/// A fight that skipped the check, or a move that starts while another timeline plays, plays the move and
/// counts the cinematic it couldn't play, in the checksum.
#[test]
fn a_cinematic_that_cannot_start_is_counted_and_moves_the_checksum() {
    let (mut unloaded, owner, _) = fight(false);
    unloaded.step(&press(owner, Buttons::ULTIMATE));
    assert_eq!(current(&unloaded, owner), Some(Name::intern("spin")));
    assert_eq!(unloaded.timeline(), None);
    assert_eq!(unloaded.cinematics_refused(), 1);

    // Busy: a timeline started between steps, and a move forced under it.
    let (mut busy, owner, _) = fight(true);
    let other = busy.add_timeline(cinematic()).expect("loads");
    busy.play_timeline(other).expect("plays");
    assert!(busy.force_move(owner, Name::intern("spin")));
    busy.step(&TickActions::NEUTRAL);
    assert_eq!(current(&busy, owner), Some(Name::intern("spin")));
    let progress = busy.timeline().expect("still playing");
    assert_eq!((progress.id, progress.owner), (other, None));
    assert_eq!(busy.cinematics_refused(), 1);

    // The count is in the checksum: the same spin on a set naming nothing differs only by it.
    let (mut plain, plain_owner, _) =
        fight_on(MoveSetDocument::parse(SLICE).expect("parses").compile().expect("compiles"), false);
    plain.step(&press(plain_owner, Buttons::ULTIMATE));
    assert_eq!(current(&plain, plain_owner), Some(Name::intern("spin")));
    assert_eq!(plain.cinematics_refused(), 0);
    assert_ne!(plain.checksum(), unloaded.checksum());
}

/// The player's ultimate in the arena (M3 Q3b), as `arena-ultimate` records it: the cinematic starts on the
/// move's first step with the player as its owner, holds the warrior at a fifth of its speed while the chop
/// lands for the move's 60, can't be skipped, and lets everyone go at its end.
#[test]
fn the_players_ultimate_holds_the_warrior_while_its_chop_lands() {
    let scenario = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
    let mut recording = Recording::new(scenario, scenario.config).expect("sets up");
    let player = recording.cast()[0];
    let warrior =
        recording.simulation().actors().map(|view| view.id).find(|&id| id != player).expect("a foe");
    let ultimate = recording.simulation().timelines_played();
    assert_eq!(ultimate, 0);
    let mut source = arena_ultimate;
    let at = ARENA_ULTIMATE_AT as u32;
    while recording.recorded_ticks() <= at {
        recording.step(&mut source).expect("short");
    }
    let run = recording.simulation().timeline().expect("the ultimate's cinematic plays");
    assert_eq!((run.tick, run.ticks, run.owner), (0, ULTIMATE_TICKS, Some(player)), "2.4 s on its steps");
    assert_eq!(
        recording.simulation().actor(player).expect("alive").moves.current.map(|mv| mv.name),
        Some(Name::intern("ultimate"))
    );
    let health = recording.simulation().actor(warrior).expect("alive").fighter.health();
    assert!(
        matches!(recording.skip_timeline(500), Err(PlayError::Unskippable { .. })),
        "it can't be skipped"
    );

    let mut held = 0;
    while recording.simulation().timeline().is_some() {
        recording.step(&mut source).expect("short");
        held += u32::from(delta(recording.simulation(), warrior) < 1000);
        assert!(
            delta(recording.simulation(), player).abs_diff(1000) <= 1
                || delta(recording.simulation(), player) == 0,
            "the player plays at its own speed, or a hit's freeze holds it"
        );
    }
    assert_eq!(recording.recorded_ticks(), at + 1 + ULTIMATE_TICKS);
    assert!(held > 100, "the warrior was held for {held} steps");
    let landed = health - recording.simulation().actor(warrior).expect("alive").fighter.health();
    assert_eq!(landed, 60, "the chop landed, once");
    assert_eq!(recording.simulation().cinematics_refused(), 0);
}
