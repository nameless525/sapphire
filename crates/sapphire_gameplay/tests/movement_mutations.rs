//! A fighter's `movement` under attack (M2 T2), the way the review attacked documents: every number of both
//! real documents is rewritten in the text a person edits, to what a typo or a bad night could make it. Each
//! result is either refused when it compiles, naming the number, or it's a document the engine has to live
//! with, so it plays: from the start of a fight, and as an edit landing in the middle of one. Any panic, a
//! position that stops being finite, a character out of the arena or one that can no longer get anywhere
//! fails the test.
//!
//! The wider run takes random numbers from inside the limits, every one at once, as many as
//! `SAPPHIRE_MOVEMENT_PROFILES` says (500 by default) from `SAPPHIRE_MOVEMENT_SEED`:
//! `cargo test --release -p sapphire_gameplay --test movement_mutations -- --ignored --nocapture`.

#![allow(clippy::print_stdout, reason = "the attack reports what it did")]

use sapphire_gameplay::replay::{
    LiveMoves, SCENARIOS, Scenario, compile_document, find_scenario, player_moves,
};
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, ControllerProfile, GroundMode, MoveSetId, SimConfig,
    Simulation, Stick, TickActions,
};
use sapphire_math::{Pcg32, Vec3};
use sapphire_moves::{MOVEMENT_LIMITS, MoveSet, MoveSetDocument, MoveSetError};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};

const PLAYER: &str = include_str!("../../../content/combat/player/player.moves.ron");
const WARRIOR: &str = include_str!("../../../content/combat/enemy/warrior.moves.ron");
const SLICE: &str = include_str!("../../sapphire_moves/test_content/slice.moves.ron");

/// What each number is rewritten to, on top of what's worked out from its own limits.
const WILD: [&str; 17] = [
    "-1.0", "-0.0", "0.0", "1e-30", "0.001", "0.1", "0.5", "1.0", "2.0", "5.0", "10.0", "100.0", "1000.0",
    "1e30", "NaN", "inf", "-inf",
];
/// The same, for a count of ticks.
const WILD_TICKS: [&str; 7] = ["0", "1", "60", "61", "4000000000", "-1", "1.5"];

const FIGHT_TICKS: u64 = 500;

/// The furthest anyone gets, past the arena's paving and its back row's fronts by a margin for a thin capsule,
/// and the highest: the walls keeping everyone in stand 6 m tall.
const BOUNDS_X: f32 = 20.0;
const BOUNDS_FRONT_Z: f32 = 21.5;
const BOUNDS_BACK_Z: f32 = -19.5;
const HIGHEST: f32 = 6.0;
/// The most one tick can carry anyone: a fall at its limit, or a table.
const MOST_IN_A_TICK: f32 = 3.0;

/// One document under attack.
struct Target {
    name: &'static str,
    text: &'static str,
}

const TARGETS: [Target; 2] =
    [Target { name: "player.moves.ron", text: PLAYER }, Target { name: "warrior.moves.ron", text: WARRIOR }];

/// `text` with this number of its `movement` section rewritten, as a person would, in place.
fn rewritten(text: &str, field: &str, value: &str) -> String {
    let section = text.find("movement: (").expect("a section");
    let name = format!("\n                {field}: ");
    let start = section + text[section..].find(&name).expect("the number is written out") + name.len();
    let end = start + text[start..].find(',').expect("its comma");
    format!("{}{value}{}", &text[..start], &text[end..])
}

fn fights() -> [&'static Scenario; 2] {
    ["arena-fight", "party"].map(|name| find_scenario(SCENARIOS, name).expect("listed"))
}

/// What a set of a real document's name compiles against, as a running app has it.
fn running(name: &str) -> MoveSet {
    match name {
        "player.moves.ron" => player_moves().expect("the simulation's copy reads"),
        _ => sapphire_gameplay::replay::warrior_moves().expect("compiles"),
    }
}

/// A person with a pad and no plan: a seeded stick and every button, on and off.
struct Thumbs(Pcg32);

impl Thumbs {
    fn new(seed: u64) -> Self {
        Self(Pcg32::new(seed, 11))
    }

    fn actions(&mut self, tick: u64, player: ActorId) -> TickActions {
        // The stick holds for a few ticks at a time, and the walls get run into.
        let stick = Stick::new(
            ((tick / 40).wrapping_mul(2_654_435_761) >> 13) as i8,
            ((tick / 40).wrapping_mul(40_503) >> 3) as i8,
        );
        let buttons = match self.0.below(48) {
            0 => Buttons::JUMP,
            1 | 2 => Buttons::LIGHT,
            3 => Buttons::DODGE,
            4 => Buttons::SKILL,
            5 => Buttons::SWAP,
            _ => Buttons::NONE,
        };
        let mut actions = TickActions::NEUTRAL;
        actions.set(player, ActorActions { buttons, stick }).expect("room");
        actions
    }
}

/// Every tick, of everyone in the fight: nothing that isn't finite, nobody out of the arena and nobody
/// going further in a tick than anything can carry them.
fn hold_to_the_world(sim: &Simulation, cast: &[ActorId], was: &mut [Vec3], label: &str) {
    for (actor, was) in cast.iter().zip(was) {
        let actor = sim.party().on_field(*actor);
        let Some(view) = sim.actor(actor) else { continue };
        let feet = view.position;
        assert!(feet.is_finite(), "{label}: a position that isn't finite: {feet}");
        let velocity = view.controller.state.velocity;
        assert!(velocity.is_finite(), "{label}: a velocity that isn't finite: {velocity}");
        assert!(
            feet.x.abs() < BOUNDS_X && feet.z < BOUNDS_FRONT_Z && feet.z > BOUNDS_BACK_Z,
            "{label}: out of the arena: {feet}"
        );
        assert!(feet.y > -1.0 && feet.y < HIGHEST, "{label}: out of the world: {feet}");
        let moved = (feet - *was).length();
        assert!(moved < MOST_IN_A_TICK, "{label}: went {moved} m in a tick");
        *was = feet;
    }
    assert_eq!(sim.stats().controllers_refused, 0, "{label}");
}

/// A fight on a scenario, from `from` if that's given, taking `edit` partway if that's given.
fn play(
    scenario: &Scenario,
    from: Option<(&str, &MoveSet)>,
    edit: Option<(u64, MoveSetId, &MoveSet)>,
    seed: u64,
    label: &str,
) {
    let mut live = LiveMoves::default();
    if let Some((name, set)) = from {
        live.remember(name, set.clone());
    }
    let (mut sim, cast) = live.start(scenario, scenario.config).expect("a fight sets up");
    let mut thumbs = Thumbs::new(seed);
    let mut was: Vec<Vec3> = cast.iter().map(|actor| sim.actor(*actor).expect("alive").position).collect();
    for tick in 0..FIGHT_TICKS {
        if let Some((at, id, set)) = edit
            && at == tick
        {
            let positions: Vec<Vec3> =
                cast.iter().map(|actor| sim.actor(*actor).expect("alive").position).collect();
            sim.tune_move_set(id, set.clone()).expect("its set");
            let after: Vec<Vec3> =
                cast.iter().map(|actor| sim.actor(*actor).expect("alive").position).collect();
            assert_eq!(positions, after, "{label}: the edit moved someone");
        }
        sim.step(&thumbs.actions(tick, cast[0]));
        hold_to_the_world(&sim, &cast, &mut was, label);
    }
}

/// Alone on a floor: a full stick gets it somewhere, a jump brings it back, and it can do either after an
/// edit lands on it running.
fn gets_about(movement: ControllerProfile, label: &str) {
    let mut document = MoveSetDocument::parse(SLICE).expect("parses");
    let set = |movement: ControllerProfile, document: &mut MoveSetDocument| {
        document.fighter.as_mut().expect("in the content").movement = Some(movement);
        document.compile().expect("compiles")
    };
    let mut sim = Simulation::new(SimConfig { max_actors: 2, ..SimConfig::default() });
    let floor = StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(500.0, 0.5, 500.0) }),
        pose: Pose::at(Vec3::new(0.0, -0.5, 0.0)),
        memberships: Group::World.mask(),
    };
    let world = PhysicsWorld::new(vec![floor], sim.body_capacity()).expect("builds");
    sim.set_physics_world(world).expect("nobody's in it yet");
    let controller = Some(sim.add_controller_profile(&ControllerProfile::default()).expect("a good profile"));
    let id = sim.add_move_set(set(ControllerProfile::default(), &mut document));
    let actor =
        sim.spawn(ActorSpawn { position: Vec3::ZERO, yaw: 0.0, moves: Some(id), controller }).expect("room");
    let step = |sim: &mut Simulation, stick: Stick, buttons: Buttons| {
        let mut actions = TickActions::NEUTRAL;
        actions.set(actor, ActorActions { buttons, stick }).expect("room");
        sim.step(&actions);
    };
    for _ in 0..30 {
        step(&mut sim, Stick::new(0, 127), Buttons::NONE);
    }
    sim.tune_move_set(id, set(movement, &mut document)).expect("its set");

    let start = sim.actor(actor).expect("alive").position;
    for _ in 0..240 {
        step(&mut sim, Stick::new(127, 0), Buttons::NONE);
    }
    let there = sim.actor(actor).expect("alive").position;
    // Over the ground, counting what it was still carrying as the edit landed: a slow enough character
    // takes all of this just to stop what it was doing. The slowest a document can say goes 0.4 m in this.
    let got = Vec3::new(there.x - start.x, 0.0, there.z - start.z).length();
    assert!(got > 0.2, "{label}: four seconds of a full stick went {got} m");

    if movement.jump_speed <= 0.0 {
        return;
    }
    step(&mut sim, Stick::CENTER, Buttons::JUMP);
    let came_down = (0..1800).any(|_| {
        step(&mut sim, Stick::CENTER, Buttons::NONE);
        let state = sim.actor(actor).expect("alive").controller.state;
        state.mode == GroundMode::Grounded
    });
    assert!(came_down, "{label}: half a minute later it hasn't come down");
}

/// Plays a document that compiled, as everything it can be: where a fight starts, and an edit partway
/// through one, each landing somewhere different.
fn play_everywhere(target: &Target, set: &MoveSet, salt: u64, label: &str) {
    for (index, scenario) in fights().into_iter().enumerate() {
        let id = scenario.set_of(target.name).expect("it plays both");
        let seed = salt.wrapping_add(index as u64);
        play(scenario, Some((target.name, set)), None, seed, label);
        for at in [37 + salt % 90, 180 + salt % 200] {
            play(scenario, None, Some((at, id, set)), seed, label);
        }
    }
    let movement = *set.movement().expect("every document here has one");
    gets_about(movement, label);
}

/// Every number of both real documents, by a table of bad and good values and each end of its limits.
#[test]
fn every_number_of_a_document_is_refused_by_name_or_safe_to_play() {
    let (mut refused, mut played, mut unread) = (0, 0, 0);
    for target in &TARGETS {
        let running = running(target.name);
        for (index, limit) in MOVEMENT_LIMITS.iter().enumerate() {
            let field = limit.field;
            let mut values: Vec<String> = if field == "coyote_ticks" {
                WILD_TICKS.iter().map(ToString::to_string).collect()
            } else {
                let ends = [limit.min, limit.max, limit.max * 1.01, limit.min * 0.99];
                WILD.iter()
                    .map(ToString::to_string)
                    .chain(ends.iter().map(|end| format!("{end:?}")))
                    .collect()
            };
            values.dedup();
            for (place, value) in values.iter().enumerate() {
                let text = rewritten(target.text, field, value);
                let label = format!("{}: {field}: {value}", target.name);
                match compile_document(&text, Some(&running)) {
                    Err(MoveSetError::Invalid(issues)) => {
                        let said: Vec<String> = issues.iter().map(ToString::to_string).collect();
                        assert!(
                            said.iter().all(|issue| issue.starts_with("`movement`") && issue.contains(field)),
                            "{label}: refused without naming it: {said:?}"
                        );
                        refused += 1;
                    }
                    // What RON can't read at all, such as a tick count of 1.5, is refused where it stands.
                    Err(MoveSetError::Document(_)) => unread += 1,
                    Err(other) => panic!("{label}: {other}"),
                    Ok(live) => {
                        play_everywhere(target, &live.set, (index * 31 + place) as u64, &label);
                        played += 1;
                    }
                }
            }
        }
    }
    println!(
        "{refused} refused by name, {unread} that RON won't read, {played} played from the start and as edits"
    );
    assert!(refused > 300 && played > 50, "{refused}, {played}: the attack has stopped reaching anything");
}

/// A number from inside its limits: uniformly, or by powers of ten where the limits span several.
fn pick(rng: &mut Pcg32, min: f32, max: f32) -> f32 {
    match rng.below(4) {
        0 => min,
        1 => max,
        2 if min > 0.0 => {
            // Toward the low end: a person's typos are a digit lost more often than one gained.
            let step = rng.unit_f32();
            let share = step * step * step;
            min + (max - min) * share
        }
        _ => min + (max - min) * rng.unit_f32(),
    }
}

/// Numbers from inside the limits, every one at once, kept if they hold each other.
fn random_movement(rng: &mut Pcg32) -> ControllerProfile {
    loop {
        let mut numbers = MOVEMENT_LIMITS.iter().map(|limit| (limit.min, limit.max));
        let mut next = || {
            let (min, max) = numbers.next().expect("twenty numbers");
            pick(rng, min, max)
        };
        let profile = ControllerProfile {
            height: next(),
            radius: next(),
            skin: next(),
            walk_speed: next(),
            run_speed: next(),
            walk_stick: next(),
            run_stick: next(),
            acceleration: next(),
            deceleration: next(),
            turn_rate_degrees: next(),
            gravity: next(),
            jump_speed: next(),
            air_acceleration: next(),
            max_fall_speed: next(),
            coyote_ticks: next() as u32,
            slope_limit_degrees: next(),
            step_height: next(),
            ground_snap: next(),
            stance_radius: next(),
            max_slide_speed: next(),
        };
        if profile.document_issues().is_empty() {
            return profile;
        }
    }
}

fn random_sweep(profiles: u64, seed: u64) {
    let mut rng = Pcg32::new(seed, 23);
    for count in 0..profiles {
        for target in &TARGETS {
            let mut document = MoveSetDocument::parse(target.text).expect("parses");
            let movement = random_movement(&mut rng);
            document.fighter.as_mut().expect("a fighter").movement = Some(movement);
            let set = document.compile_over(&running(target.name)).expect("inside the limits, it compiles");
            let label = format!("{} #{count}: {movement:?}", target.name);
            play_everywhere(target, &set, count, &label);
        }
    }
}

/// A taste of [`every_random_character_is_safe_to_play`], on every run.
#[test]
fn numbers_at_random_are_safe_to_play() {
    random_sweep(12, 7);
}

/// See the crate's docs for how to run this. It takes a few minutes in a release build.
#[test]
#[ignore = "a long run, for when the limits or the controller change"]
fn every_random_character_is_safe_to_play() {
    let profiles =
        std::env::var("SAPPHIRE_MOVEMENT_PROFILES").map_or(500, |count| count.parse().expect("a count"));
    let seed = std::env::var("SAPPHIRE_MOVEMENT_SEED").map_or(99, |seed| seed.parse().expect("a seed"));
    random_sweep(profiles, seed);
    println!(
        "{profiles} random characters for each document, each a fresh fight and two edits on two scenarios"
    );
}
