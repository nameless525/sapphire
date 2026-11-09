//! A move's cinematic (M3 Q3b) through the public API: the move names it, the fight loads it by that name
//! and refuses a set naming one it hasn't, and the run starts on the move's first tick, holding the others
//! while the move plays through at its own speed.

mod support;

use sapphire_core::Name;
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
