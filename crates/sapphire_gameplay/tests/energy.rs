//! The ultimate's energy (M3 Q3c) through the public API: a fighter's meter fills as it lands and takes hits,
//! up to its top, and a move that costs more than it holds doesn't start, while one that starts spends it.

mod support;

use sapphire_core::Name;
use sapphire_gameplay::replay::{
    ARENA_ENERGY_TICKS, ARENA_ENERGY_TRIES_EVERY, ARENA_ULTIMATE_AT, Recording, SCENARIOS, arena_energy,
    find_scenario,
};
use sapphire_gameplay::{ActorId, ActorSpawn, Buttons, SimConfig, Simulation, TickActions};
use sapphire_math::Vec3;
use sapphire_moves::{EnergyDocument, MoveSet, MoveSetDocument};
use support::{SLICE, press};

/// The test content's meter, in the tests below.
const METER: EnergyDocument = EnergyDocument { max: 30, landed: 10, taken: 5 };
/// What the test content's `spin` costs.
const SPIN_COST: u32 = 20;

fn document() -> MoveSetDocument {
    let mut document = MoveSetDocument::parse(SLICE).expect("parses");
    document.fighter.as_mut().expect("the test content fights").energy = Some(METER);
    let spin = document.moves.iter_mut().find(|mv| mv.name == "spin").expect("the test content spins");
    spin.cost = SPIN_COST;
    document
}

/// Two fighters on the test content with a meter, face to face, inside a light cut's reach.
fn fight() -> (Simulation, ActorId, ActorId) {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_move_set(document().compile().expect("compiles"));
    let hitter = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).expect("room");
    let hit = sim
        .spawn(ActorSpawn {
            moves: Some(moves),
            position: Vec3::new(0.0, 0.0, 1.2),
            yaw: std::f32::consts::PI,
            ..ActorSpawn::default()
        })
        .expect("room");
    (sim, hitter, hit)
}

fn energy(sim: &Simulation, actor: ActorId) -> u32 {
    sim.actor(actor).expect("alive").fighter.energy()
}

fn current(sim: &Simulation, actor: ActorId) -> Option<Name> {
    sim.actor(actor).expect("alive").moves.current.map(|mv| mv.name)
}

/// Steps until `actor`'s health drops, at most `limit` steps, pressing `buttons` on the first.
fn until_hit(sim: &mut Simulation, by: ActorId, buttons: Buttons, target: ActorId, limit: u32) {
    let health = sim.actor(target).expect("alive").fighter.health();
    sim.step(&press(by, buttons));
    for _ in 0..limit {
        if sim.actor(target).expect("alive").fighter.health() < health {
            return;
        }
        sim.step(&TickActions::NEUTRAL);
    }
    panic!("no hit landed in {limit} steps");
}

#[test]
fn a_move_that_costs_more_than_the_meter_holds_does_not_start_and_one_that_starts_spends_it() {
    let (mut sim, hitter, _) = fight();
    assert_eq!(
        (energy(&sim, hitter), sim.actor(hitter).expect("alive").fighter.max_energy()),
        (0, METER.max)
    );
    sim.step(&press(hitter, Buttons::ULTIMATE));
    for _ in 0..20 {
        assert_ne!(current(&sim, hitter), Some(Name::intern("spin")), "it can't afford the spin");
        sim.step(&TickActions::NEUTRAL);
    }
    assert_eq!(energy(&sim, hitter), 0, "a refused press spends nothing");

    assert!(sim.fill_energy(hitter));
    sim.step(&press(hitter, Buttons::ULTIMATE));
    assert_eq!(current(&sim, hitter), Some(Name::intern("spin")));
    assert_eq!(energy(&sim, hitter), METER.max - SPIN_COST, "it spent the spin's cost as it started");
}

#[test]
fn a_press_made_while_short_fires_once_the_meter_fills_inside_its_buffer() {
    let (mut sim, hitter, _) = fight();
    sim.step(&press(hitter, Buttons::ULTIMATE));
    assert_eq!(current(&sim, hitter), None);
    // Filled between steps, with the press still buffered: the next reading fires it.
    sim.fill_energy(hitter);
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(current(&sim, hitter), Some(Name::intern("spin")));
    assert_eq!(energy(&sim, hitter), METER.max - SPIN_COST);
}

#[test]
fn a_hit_fills_the_meters_of_both_who_landed_it_and_who_took_it_up_to_their_tops() {
    let (mut sim, hitter, hit) = fight();
    until_hit(&mut sim, hitter, Buttons::LIGHT, hit, 30);
    assert_eq!((energy(&sim, hitter), energy(&sim, hit)), (METER.landed, METER.taken));

    sim.fill_energy(hitter);
    for _ in 0..60 {
        sim.step(&TickActions::NEUTRAL);
    }
    until_hit(&mut sim, hitter, Buttons::LIGHT, hit, 30);
    assert_eq!(energy(&sim, hitter), METER.max, "a full meter stays at its top");
    assert_eq!(energy(&sim, hit), 2 * METER.taken);
}

#[test]
fn a_set_that_spends_energy_it_cannot_hold_is_refused_by_name() {
    let refused = |document: MoveSetDocument| match document.compile() {
        Err(error) => error.to_string(),
        Ok(_) => panic!("compiled"),
    };
    let mut no_meter = document();
    no_meter.fighter.as_mut().expect("fights").energy = None;
    assert!(refused(no_meter).contains("cost"), "a cost with no meter");

    let mut too_dear = document();
    too_dear.moves.iter_mut().find(|mv| mv.name == "spin").expect("spins").cost = METER.max + 1;
    assert!(refused(too_dear).contains("cost"), "a cost past the top");

    let mut empty = document();
    empty.fighter.as_mut().expect("fights").energy = Some(EnergyDocument { max: 0, ..METER });
    assert!(refused(empty).contains("energy"), "a meter of nothing");

    // And none of it is a change for a set that says nothing of energy.
    let plain = MoveSetDocument::parse(SLICE).expect("parses").compile().expect("compiles");
    assert!(plain.moves().iter().all(|mv| mv.cost() == 0));
    assert_eq!(plain.fighter().and_then(|fighter| fighter.energy), None);
    let _: &MoveSet = &plain;
}

/// `arena-energy`, as it records: the arena's player starts full, the ultimate empties the meter, the presses
/// every third of a second fire nothing until the fight has filled it, and the first one after that which
/// finds the player between moves plays the ultimate again.
#[test]
#[allow(clippy::print_stdout, reason = "the ticks the spec's note reports")]
fn a_meter_the_fight_fills_casts_the_ultimate_again() {
    let scenario = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
    let mut recording = Recording::new(scenario, scenario.config).expect("sets up");
    let player = recording.cast()[0];
    let ultimate = Name::intern("ultimate");
    let full = recording.simulation().actor(player).expect("alive").fighter.max_energy();
    assert_eq!(energy(recording.simulation(), player), full, "a built-in fight starts full");

    let mut source = arena_energy;
    let mut casts = Vec::new();
    let mut was = None;
    let mut filled_at = None;
    while recording.recorded_ticks() < ARENA_ENERGY_TICKS {
        recording.step(&mut source).expect("short");
        let sim = recording.simulation();
        let now = sim.actor(player).expect("alive");
        let running = now.moves.current.map(|mv| mv.name);
        if running == Some(ultimate) && was != Some(ultimate) {
            casts.push((recording.recorded_ticks(), now.fighter.energy()));
        }
        if casts.len() == 1 && filled_at.is_none() && now.fighter.energy() == full {
            filled_at = Some(recording.recorded_ticks());
        }
        was = running;
    }
    let [(first, after_first), (second, after_second)] = casts[..] else {
        panic!("the ultimate should play twice: {casts:?}");
    };
    assert_eq!(u64::from(first), ARENA_ULTIMATE_AT + 1);
    assert_eq!((after_first, after_second), (0, 0), "each spent the full meter");
    let filled_at = filled_at.expect("the fight filled the meter");
    assert!(filled_at < second, "it filled at {filled_at}, before the second at {second}");
    // Between moves, as the neutral starts it: on a press, the step after it.
    let pressed = u64::from(second - 1) - (ARENA_ULTIMATE_AT + 150);
    assert_eq!(pressed % ARENA_ENERGY_TRIES_EVERY, 0, "cast at {second}, on no press");
    println!("arena-energy: cast at {first} and {second}, refilled at {filled_at}");
}
