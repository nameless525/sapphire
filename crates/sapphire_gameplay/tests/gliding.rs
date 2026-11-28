//! Gliding (M6 A17), on the heroine's moves and numbers, one case a test: a press of jump in a fall with the ground
//! more than her `glide_min_height` under her opens a glide, and in a lower one doesn't; a glide sinks steadily at
//! the lower of `glide_sink_speed` and `max_fall_speed`; the stick steers it at `glide_speed` and the stated turn
//! rate; it lands out into walking; a press drops it into a fall; stamina's end drops it; a party won't swap in
//! one; a move taking the stick ends it; a defeat ends it; a press before a climbable face grabs it. Each case says what its ticks cast: an opening press one ray, a gliding tick nothing a fall doesn't.

use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, GLIDE_TURN_RATE, GroundMode, PartyTuning, SimConfig,
    Simulation, Stick, TickActions, TraversalMode, replay,
};
use sapphire_math::{Vec3, det};
use sapphire_moves::MoveSet;
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};

use std::f32::consts::PI;

/// The heroine's glide, her document's numbers.
const GLIDE_SPEED: f32 = 6.0;
const SINK_SPEED: f32 = 2.0;
const MIN_HEIGHT: f32 = 3.0;
/// One tick, in seconds.
const TICK: f32 = 1.0 / 60.0;

fn floor() -> StaticCollider {
    StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(200.0, 0.5, 200.0) }),
        pose: Pose::at(Vec3::new(0.0, -0.5, 0.0)),
        memberships: Group::World.mask(),
    }
}

/// The heroine's moves, her document's movement with `from` replaced by `to` if there's one.
fn heroine_moves(tuned: Option<(&str, &str)>) -> MoveSet {
    let running = replay::player_moves().unwrap();
    let Some((from, to)) = tuned else { return running };
    let text = include_str!("../../../content/combat/player/player.moves.ron");
    let tuned = text.replacen(from, to, 1);
    assert_ne!(tuned, text, "the document's movement still says {from:?}");
    replay::compile_document(&tuned, Some(&running)).unwrap().set
}

/// Her moves with `seconds` of stamina.
fn with_stamina(seconds: f32) -> MoveSet {
    let set = heroine_moves(Some((
        "swim_speed: 2.5,\n",
        &format!("swim_speed: 2.5,\n                stamina: Some({seconds:?}),\n"),
    )));
    assert_eq!(set.movement().unwrap().stamina, Some(seconds));
    set
}

/// A simulation over the floor with `count` heroines on `set`, spawned `height` over it facing +Z, as a party if
/// there's more than one.
fn heroines(set: MoveSet, height: f32, count: usize) -> (Simulation, Vec<ActorId>) {
    heroines_among(vec![floor()], set, height, count)
}

/// As [`heroines`], on `statics` instead of the floor alone, with room for an opponent.
fn heroines_among(
    statics: Vec<StaticCollider>,
    set: MoveSet,
    height: f32,
    count: usize,
) -> (Simulation, Vec<ActorId>) {
    let mut sim = Simulation::new(SimConfig { max_actors: 8, ..SimConfig::default() });
    sim.set_physics_world(PhysicsWorld::new(statics, sim.body_capacity()).unwrap()).unwrap();
    let profile = set.movement().copied().unwrap_or_default();
    let controller = Some(sim.add_controller_profile(&profile).unwrap());
    let moves = Some(sim.add_move_set(set));
    let at = Vec3::new(0.0, height, 0.0);
    let actors: Vec<ActorId> = (0..count)
        .map(|_| {
            sim.spawn(ActorSpawn { position: at, yaw: 0.0, moves, controller, ..ActorSpawn::default() })
                .unwrap()
        })
        .collect();
    if count > 1 {
        sim.form_party(&actors, PartyTuning { swap_cooldown: 10, rest: 10 }).unwrap();
    }
    (sim, actors)
}

/// One heroine on `set`, `height` over the floor: a spawn starts in the air, so she's falling.
fn heroine_on(set: MoveSet, height: f32) -> (Simulation, ActorId) {
    let (sim, actors) = heroines(set, height, 1);
    (sim, actors[0])
}

fn heroine(height: f32) -> (Simulation, ActorId) {
    heroine_on(heroine_moves(None), height)
}

fn act(actor: ActorId, buttons: Buttons, stick: Stick) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    actions.set(actor, ActorActions { buttons, stick }).unwrap();
    actions
}

const EAST: Stick = Stick::new(127, 0);

fn step(sim: &mut Simulation, actor: ActorId, buttons: Buttons, stick: Stick) {
    sim.step(&act(actor, buttons, stick));
}

fn feet(sim: &Simulation, actor: ActorId) -> Vec3 {
    sim.actor(actor).unwrap().position
}

fn gliding(sim: &Simulation, actor: ActorId) -> bool {
    sim.actor(actor).unwrap().controller.state.traversal == TraversalMode::Gliding
}

fn velocity(sim: &Simulation, actor: ActorId) -> Vec3 {
    sim.actor(actor).unwrap().controller.state.velocity
}

/// Falls for `ticks`, then presses jump: from a spawn high enough, that opens a glide.
fn open(sim: &mut Simulation, actor: ActorId, ticks: u32) {
    for _ in 0..ticks {
        sim.step(&TickActions::NEUTRAL);
    }
    let before = sim.stats().glide_casts;
    step(sim, actor, Buttons::JUMP, Stick::CENTER);
    assert_eq!(sim.stats().glide_casts - before, 1, "one ray down for the ground");
}

/// A press in a fall from 20 m opens a glide, on that tick: she sinks at her glide's speed from it, its one ray
/// having found no ground within her least height. The fall before it had no glide, and its ticks cast nothing
/// for one.
#[test]
fn a_press_in_a_high_fall_opens_a_glide() {
    let (mut sim, actor) = heroine(20.0);
    for _ in 0..20 {
        sim.step(&TickActions::NEUTRAL);
    }
    assert!(!gliding(&sim, actor), "a fall alone doesn't glide");
    assert!(velocity(&sim, actor).y < -5.0, "falling fast: {}", velocity(&sim, actor));
    assert_eq!(sim.stats().glide_casts, 0, "nothing pressed, nothing cast");
    open(&mut sim, actor, 0);
    assert!(gliding(&sim, actor), "the press opened a glide");
    assert_eq!(sim.stats().glides_started, 1);
    assert_eq!(velocity(&sim, actor).y, -SINK_SPEED, "sinking at the glide's speed from the press");
}

/// A press in a fall with the ground within her least height opens nothing: the ray finds it, and she falls on
/// and lands. A press in a high fall's last metres is as low.
#[test]
fn no_glide_opens_from_a_low_fall() {
    let (mut sim, actor) = heroine(MIN_HEIGHT - 0.5);
    open(&mut sim, actor, 5);
    assert!(feet(&sim, actor).y > 0.5, "still in the air: {}", feet(&sim, actor));
    assert!(!gliding(&sim, actor), "too low");
    assert_eq!(sim.stats().glides_started, 0);

    let (mut sim, actor) = heroine(10.0);
    let mut ticks = 0;
    while feet(&sim, actor).y > MIN_HEIGHT - 0.2 {
        sim.step(&TickActions::NEUTRAL);
        ticks += 1;
        assert!(ticks < 200, "she fell");
    }
    open(&mut sim, actor, 0);
    assert!(
        !gliding(&sim, actor) && sim.stats().glides_started == 0,
        "the last metres of a high fall are low"
    );
    for _ in 0..60 {
        sim.step(&TickActions::NEUTRAL);
    }
    assert_eq!(sim.actor(actor).unwrap().controller.state.mode, GroundMode::Grounded, "she landed");
}

/// A glide sinks steadily, every tick by the same speed: the glide's own 2 m/s, under her fastest fall of 30. With
/// her fastest fall tuned to 1.5 m/s, under the glide's sink, it sinks at 1.5 instead. Stamina off, a long glide
/// never tires.
#[test]
fn a_glide_sinks_at_the_lower_of_its_sink_and_the_fastest_fall() {
    for (set, sink) in [
        (heroine_moves(None), SINK_SPEED),
        (heroine_moves(Some(("max_fall_speed: 30.0,", "max_fall_speed: 1.5,"))), 1.5),
    ] {
        let (mut sim, actor) = heroine_on(set, 30.0);
        open(&mut sim, actor, 2);
        for tick in 0..600 {
            let before = feet(&sim, actor).y;
            sim.step(&TickActions::NEUTRAL);
            assert!(gliding(&sim, actor), "tick {tick}: stamina off, it never tires");
            assert_eq!(velocity(&sim, actor).y, -sink, "tick {tick}");
            let sank = before - feet(&sim, actor).y;
            assert!((sank - sink * TICK).abs() < 1e-5, "tick {tick}: sank {sank}");
        }
        assert!(feet(&sim, actor).y > 5.0, "ten seconds sank {sink} × 10 m: {}", feet(&sim, actor));
    }
}

/// A glide flies along her facing at her glide's speed, reached from the fall's standstill across at her air
/// acceleration, and a stick let go flies on straight. Held east, the stick turns her at the glide's rate, 120°
/// a second, until she heads east, and the speed across stays the glide's.
#[test]
fn the_stick_steers_a_glide() {
    let (mut sim, actor) = heroine(40.0);
    open(&mut sim, actor, 2);
    // 6 m/s at 18 m/s² is 20 ticks.
    for _ in 0..20 {
        sim.step(&TickActions::NEUTRAL);
    }
    let flying = velocity(&sim, actor);
    assert_eq!((flying.x, flying.z), (0.0, GLIDE_SPEED), "straight ahead, along +Z, at the glide's speed");
    let start = feet(&sim, actor);
    for _ in 0..30 {
        sim.step(&TickActions::NEUTRAL);
    }
    let went = feet(&sim, actor) - start;
    assert!((went.z - GLIDE_SPEED * 0.5).abs() < 1e-4 && went.x == 0.0, "half a second straight: {went}");

    // East from +Z is a quarter turn: at 120° a second, a half second turns 60° of it, and 45 ticks all of it.
    let yaw = |sim: &Simulation| sim.actor(actor).unwrap().yaw;
    let from = yaw(&sim);
    for _ in 0..30 {
        step(&mut sim, actor, Buttons::NONE, EAST);
    }
    let turned = yaw(&sim) - from;
    assert!((turned - GLIDE_TURN_RATE * 0.5).abs() < 1e-4, "turned {} degrees", turned.to_degrees());
    let heading = velocity(&sim, actor);
    let along = det::direction_from_yaw(yaw(&sim)) * GLIDE_SPEED;
    assert!(
        (heading.x - along.x).abs() < 1e-5 && (heading.z - along.z).abs() < 1e-5,
        "{heading} along {along}"
    );
    for _ in 0..30 {
        step(&mut sim, actor, Buttons::NONE, EAST);
    }
    let east = velocity(&sim, actor);
    assert!((east.x - GLIDE_SPEED).abs() < 1e-5 && east.z.abs() < 1e-5, "heading east: {east}");
    assert!(gliding(&sim, actor));
}

/// A glide that comes down on the floor lands: she's grounded, out of the glide, and the stick walks her. The
/// landing reads as the glide's sink.
#[test]
fn a_glide_lands_out_into_walking() {
    let (mut sim, actor) = heroine(8.0);
    open(&mut sim, actor, 2);
    let mut ticks = 0;
    while gliding(&sim, actor) {
        sim.step(&TickActions::NEUTRAL);
        ticks += 1;
        assert!(ticks < 600, "she came down");
    }
    let state = sim.actor(actor).unwrap().controller.state;
    assert_eq!((state.mode, state.traversal), (GroundMode::Grounded, TraversalMode::None));
    assert!(
        state.events.landed && (state.events.landing_speed - SINK_SPEED).abs() < 1e-5,
        "{:?}",
        state.events
    );
    assert!(feet(&sim, actor).y.abs() < 1e-3, "on the floor: {}", feet(&sim, actor));
    // Some 8 m down at 2 m/s: four seconds.
    assert!((230..245).contains(&ticks), "{ticks} ticks gliding");
    for _ in 0..30 {
        step(&mut sim, actor, Buttons::NONE, EAST);
    }
    let walking = sim.actor(actor).unwrap().controller.state;
    assert_eq!((walking.mode, walking.traversal), (GroundMode::Grounded, TraversalMode::None));
    assert!(walking.velocity.x > 5.0, "running east: {}", walking.velocity);
}

/// A press while gliding drops it: she falls from there under gravity, faster each tick, with the speed across she
/// had. The press opens nothing on the tick it drops one, and a later press high enough opens another.
#[test]
fn a_press_drops_a_glide_into_a_fall() {
    let (mut sim, actor) = heroine(40.0);
    open(&mut sim, actor, 2);
    for _ in 0..30 {
        sim.step(&TickActions::NEUTRAL);
    }
    let (casts, face_casts) = (sim.stats().glide_casts, sim.stats().climb_casts);
    step(&mut sim, actor, Buttons::JUMP, Stick::CENTER);
    assert!(!gliding(&sim, actor), "the press dropped it");
    assert_eq!(sim.stats().glide_casts, casts, "a drop casts no ray of its own");
    assert_eq!(
        sim.stats().climb_casts - face_casts,
        1,
        "her set climbs, so the press first looked for a face"
    );
    let dropped = velocity(&sim, actor);
    for _ in 0..10 {
        sim.step(&TickActions::NEUTRAL);
    }
    let falling = velocity(&sim, actor);
    assert!(falling.y < dropped.y - 3.0, "falling faster under gravity: {dropped} then {falling}");
    assert!(falling.z > 5.0, "still going across: {falling}");
    assert_eq!(sim.stats().glides_started, 1);
    open(&mut sim, actor, 0);
    assert!(gliding(&sim, actor), "a press high enough opens one again");
    assert_eq!(sim.stats().glides_started, 2);
}

/// With a second of stamina a glide drops when it's spent, a second in, into a fall; a spent stamina opens no
/// other until her feet are on the ground, which makes it whole again.
#[test]
fn stamina_ends_a_glide() {
    let (mut sim, actor) = heroine_on(with_stamina(1.0), 60.0);
    open(&mut sim, actor, 2);
    let mut ticks = 1;
    while gliding(&sim, actor) {
        sim.step(&TickActions::NEUTRAL);
        ticks += 1;
        assert!(ticks < 120, "the glide tired");
    }
    assert_eq!(ticks, 60, "a second of glide");
    assert!(feet(&sim, actor).y > 40.0, "dropped in the air: {}", feet(&sim, actor));
    step(&mut sim, actor, Buttons::JUMP, Stick::CENTER);
    assert!(!gliding(&sim, actor), "spent, it opens none");
    assert_eq!(sim.stats().glides_started, 1);
    let mut ticks = 0;
    while sim.actor(actor).unwrap().controller.state.mode != GroundMode::Grounded {
        sim.step(&TickActions::NEUTRAL);
        ticks += 1;
        assert!(ticks < 600, "she landed");
    }
    assert_eq!(
        sim.actor(actor).unwrap().controller.state.stamina_spent.as_subticks(),
        0,
        "whole on the ground"
    );
}

/// A party won't swap its glider out: the press waits in its buffer, and the swap fires once she drops the glide.
#[test]
fn a_party_swaps_nobody_out_of_a_glide() {
    let (mut sim, members) = heroines(heroine_moves(None), 40.0, 2);
    let party = members[0];
    open(&mut sim, party, 5);
    for _ in 0..20 {
        sim.step(&TickActions::NEUTRAL);
    }
    step(&mut sim, party, Buttons::SWAP, Stick::CENTER);
    for _ in 0..3 {
        sim.step(&TickActions::NEUTRAL);
    }
    assert_eq!(sim.stats().swaps, 0, "refused in the glide");
    assert_eq!(sim.party().on_field(party), members[0]);
    assert!(gliding(&sim, members[0]));
    // Drop it with the press still buffered: it fires as she falls.
    step(&mut sim, party, Buttons::JUMP, Stick::CENTER);
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(sim.stats().swaps, 1, "fired once she dropped it");
    let coming = sim.party().on_field(party);
    assert_eq!(coming, members[1]);
    assert_eq!(
        sim.actor(coming).unwrap().controller.state.traversal,
        TraversalMode::None,
        "she came in falling"
    );
}

/// A move taking the stick ends a glide: a light attack pressed in one runs, the glide is over by its first tick,
/// and the attack's drive holds her still across, as any attack does.
#[test]
fn a_move_taking_the_stick_ends_a_glide() {
    let (mut sim, actor) = heroine(40.0);
    open(&mut sim, actor, 2);
    for _ in 0..30 {
        sim.step(&TickActions::NEUTRAL);
    }
    assert!(velocity(&sim, actor).z > 5.0, "flying across: {}", velocity(&sim, actor));
    step(&mut sim, actor, Buttons::LIGHT, Stick::CENTER);
    let mut ticks = 0;
    while sim.actor(actor).unwrap().moves.current.is_none() {
        sim.step(&TickActions::NEUTRAL);
        ticks += 1;
        assert!(ticks < 5, "the attack started");
    }
    sim.step(&TickActions::NEUTRAL);
    assert!(!gliding(&sim, actor), "the attack ended the glide");
    let across = velocity(&sim, actor);
    assert_eq!((across.x, across.z), (0.0, 0.0), "the attack's drive holds her across: {across}");
    assert_eq!(sim.stats().glides_started, 1);
}

/// A glider defeated in her glide drops it, and the swap her defeat forces comes with the newcomer out of any
/// glide. Her glide is tuned low and level, its sink and least height cut, so an opponent standing in its way
/// reaches her.
#[test]
fn a_defeat_ends_a_glide() {
    let text = include_str!("../../../content/combat/player/player.moves.ron");
    let tuned = text
        .replacen("health: 300,", "health: 1,", 1)
        .replacen("glide_min_height: 3.0,", "glide_min_height: 0.5,", 1)
        .replacen("glide_sink_speed: 2.0,", "glide_sink_speed: 0.1,", 1);
    let running = replay::player_moves().unwrap();
    let set = replay::compile_document(&tuned, Some(&running)).unwrap().set;
    let profile = set.movement().copied().unwrap();
    assert_eq!(profile.glide_min_height, 0.5);
    let (mut sim, members) = heroines_among(vec![floor()], set, 1.0, 2);
    let party = members[0];
    let opponent_moves = Some(sim.add_move_set(replay::test_moves().unwrap()));
    let controller = Some(sim.add_controller_profile(&profile).unwrap());
    let opponent = sim
        .spawn(ActorSpawn {
            position: Vec3::new(0.0, 0.0, 6.0),
            yaw: PI,
            moves: opponent_moves,
            controller,
            ..ActorSpawn::default()
        })
        .unwrap();
    open(&mut sim, party, 2);
    assert!(gliding(&sim, party), "a low glide opened");
    let mut defeated = false;
    for tick in 0..200 {
        let buttons = if tick % 10 == 0 { Buttons::LIGHT } else { Buttons::NONE };
        let was_gliding = gliding(&sim, party);
        sim.step(&act(opponent, buttons, Stick::CENTER));
        if sim.actor(party).unwrap().fighter.is_defeated() {
            assert!(was_gliding, "tick {tick}: she was gliding when the blow fell, at {}", feet(&sim, party));
            defeated = true;
            break;
        }
    }
    assert!(defeated, "the blow defeated her, at {}", feet(&sim, party));
    sim.step(&TickActions::NEUTRAL);
    assert!(!gliding(&sim, party), "her defeat dropped the glide");
    for _ in 0..400 {
        if sim.stats().swaps == 1 {
            let coming = sim.party().on_field(party);
            assert_eq!(coming, members[1]);
            assert_eq!(sim.actor(coming).unwrap().controller.state.traversal, TraversalMode::None);
            return;
        }
        sim.step(&TickActions::NEUTRAL);
    }
    panic!("no forced swap");
}

/// A press in a glide before a climbable face grabs it, by A16a's one ray from the hands: she hangs on it with no
/// speed, climbing, and the press neither dropped the glide into a fall nor cast a glide's ray.
#[test]
fn a_press_before_a_climbable_face_grabs_it_from_a_glide() {
    let face_z = 8.0;
    let wall = StaticCollider {
        id: ColliderId(1),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(5.0, 20.0, 1.0) }),
        pose: Pose::at(Vec3::new(0.0, 20.0, face_z + 1.0)),
        memberships: Group::World | Group::Climbable,
    };
    let (mut sim, actors) = heroines_among(vec![floor(), wall], heroine_moves(None), 30.0, 1);
    let actor = actors[0];
    open(&mut sim, actor, 2);
    let mut ticks = 0;
    while feet(&sim, actor).z < face_z - 1.0 || velocity(&sim, actor).z > 0.0 {
        sim.step(&TickActions::NEUTRAL);
        ticks += 1;
        assert!(ticks < 300, "she reached the wall: {} {}", feet(&sim, actor), velocity(&sim, actor));
    }
    assert!(gliding(&sim, actor), "still gliding against the wall");
    let stats = sim.stats();
    step(&mut sim, actor, Buttons::JUMP, Stick::CENTER);
    let state = sim.actor(actor).unwrap().controller.state;
    assert_eq!(state.traversal, TraversalMode::Climbing, "the press grabbed the face");
    assert!(state.climb.is_some());
    assert_eq!(sim.stats().climbs_started - stats.climbs_started, 1);
    assert_eq!(sim.stats().glides_started, stats.glides_started, "no new glide");
    assert_eq!(sim.stats().glide_casts, stats.glide_casts, "no glide's ray");
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(velocity(&sim, actor), Vec3::ZERO, "hanging still on the face");
}
