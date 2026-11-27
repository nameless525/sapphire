//! Climbing (M6 A16a), on the heroine's moves and numbers, one case a test: a press of jump in front of a face in
//! `Group::Climbable` grabs it; the stick climbs up and sideways at the profile's `climb_speed`, round an outside
//! corner and an inside one; a climb to the top tops out by a mantle; a press lets go; a face that isn't tagged is
//! never grabbed; stamina's end lets go, and stamina off never does; a party won't swap off a face. Each case says
//! what its ticks cast: the probes for one climber, by count.

use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, GroundMode, Lip, PartyTuning, SimConfig, Simulation, Stick,
    TickActions, TraversalMode, replay,
};
use sapphire_math::{Vec3, det};
use sapphire_moves::MoveSet;
use sapphire_physics::{
    ColliderId, Group, GroupMask, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape,
};

/// Where the wall's face is: a block from z 1 on, across x −3 to 3, facing −Z toward the heroine.
const FACE_Z: f32 = 1.0;
/// How far from the face her axis stands before she presses: 0.15 m clear of her capsule, within her reach.
const STANDS_OFF: f32 = 0.45;
/// Where her axis holds a face: her radius and the hold's gap off it.
const HOLDS_AT: f32 = 0.3 + 0.02;
/// The heroine's climbing speed, the profile's default.
const CLIMB_SPEED: f32 = 1.5;

fn block(id: u32, min: Vec3, max: Vec3, memberships: GroupMask) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Convex(Shape::Box { half_extents: (max - min) * 0.5 }),
        pose: Pose::at((min + max) * 0.5),
        memberships,
    }
}

fn climbable() -> GroupMask {
    Group::World | Group::Climbable
}

fn floor() -> StaticCollider {
    block(0, Vec3::new(-50.0, -1.0, -50.0), Vec3::new(50.0, 0.0, 50.0), Group::World.mask())
}

/// A wall `height` tall and 2 m deep, its face at [`FACE_Z`], in `memberships`.
fn wall(height: f32, memberships: GroupMask) -> StaticCollider {
    block(1, Vec3::new(-3.0, 0.0, FACE_Z), Vec3::new(3.0, height, FACE_Z + 2.0), memberships)
}

/// A climbable block standing out from the wall's face at its east end, toward her: its west side meets the face
/// in an inside corner at x 3.
fn wing() -> StaticCollider {
    block(2, Vec3::new(3.0, 0.0, FACE_Z - 2.0), Vec3::new(5.0, 6.0, FACE_Z), climbable())
}

/// The heroine's moves, her document's movement given `stamina` seconds if it's `Some`.
fn heroine_moves(stamina: Option<f32>) -> MoveSet {
    let running = replay::player_moves().unwrap();
    let Some(seconds) = stamina else { return running };
    let text = include_str!("../../../content/combat/player/player.moves.ron");
    let tuned = text.replacen(
        "swim_speed: 2.5,\n",
        &format!("swim_speed: 2.5,\n                stamina: Some({seconds:?}),\n"),
        1,
    );
    assert_ne!(tuned, text, "the document's movement still ends at its swimming speed");
    let set = replay::compile_document(&tuned, Some(&running)).unwrap().set;
    assert_eq!(set.movement().unwrap().stamina, Some(seconds));
    set
}

/// The heroine on her moves, with `stamina`, at `x` before the wall's face facing it, on `statics`.
fn heroine_with(statics: Vec<StaticCollider>, x: f32, stamina: Option<f32>) -> (Simulation, ActorId) {
    let mut sim = Simulation::new(SimConfig::default());
    sim.set_physics_world(PhysicsWorld::new(statics, sim.body_capacity()).unwrap()).unwrap();
    let set = heroine_moves(stamina);
    let profile = set.movement().copied().unwrap_or_default();
    let controller = Some(sim.add_controller_profile(&profile).unwrap());
    let moves = Some(sim.add_move_set(set));
    let at = Vec3::new(x, 0.0, FACE_Z - STANDS_OFF);
    let id =
        sim.spawn(ActorSpawn { position: at, yaw: 0.0, moves, controller, ..ActorSpawn::default() }).unwrap();
    // A spawn starts in the air: its first ticks find the floor.
    for _ in 0..10 {
        sim.step(&TickActions::NEUTRAL);
    }
    (sim, id)
}

fn heroine(statics: Vec<StaticCollider>, x: f32) -> (Simulation, ActorId) {
    heroine_with(statics, x, None)
}

fn act(actor: ActorId, buttons: Buttons, stick: Stick) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    actions.set(actor, ActorActions { buttons, stick }).unwrap();
    actions
}

/// A full stick toward +Z (toward the face), +X or the other ways.
const TOWARD: Stick = Stick::new(0, 127);
const EAST: Stick = Stick::new(127, 0);
const AWAY: Stick = Stick::new(0, -127);

fn feet(sim: &Simulation, actor: ActorId) -> Vec3 {
    sim.actor(actor).unwrap().position
}

fn climbing(sim: &Simulation, actor: ActorId) -> bool {
    let state = sim.actor(actor).unwrap().controller.state;
    assert_eq!(
        state.traversal == TraversalMode::Climbing,
        state.climb.is_some(),
        "the mode and the hold agree"
    );
    state.climb.is_some()
}

fn normal(sim: &Simulation, actor: ActorId) -> Vec3 {
    sim.actor(actor).unwrap().controller.state.climb.expect("climbing").normal
}

/// Steps once with `stick` and `buttons`, returning what the step's climbing cast.
fn step(sim: &mut Simulation, actor: ActorId, buttons: Buttons, stick: Stick) -> u64 {
    let before = sim.stats().climb_casts;
    sim.step(&act(actor, buttons, stick));
    sim.stats().climb_casts - before
}

/// Grabs the face in front of her, and settles onto it with a tick hanging still.
fn grab(sim: &mut Simulation, actor: ActorId) {
    // The press looks for a face with one ray, and the same step's climb holds it to the gap: the hold's ray and
    // the sweep in.
    assert_eq!(step(sim, actor, Buttons::JUMP, Stick::CENTER), 3, "a grab and its first hold");
    assert!(climbing(sim, actor), "it grabbed the face");
    assert_eq!(step(sim, actor, Buttons::NONE, Stick::CENTER), 1, "then the hold's ray alone");
}

/// A press in front of a climbable face grabs it: she hangs at the hold's gap off it, facing it, with no fall,
/// and a tick hanging still casts only the hold's ray.
#[test]
fn a_press_before_a_climbable_face_grabs_it() {
    let (mut sim, actor) = heroine(vec![floor(), wall(6.0, climbable())], 0.0);
    grab(&mut sim, actor);
    let held = feet(&sim, actor);
    for _ in 0..60 {
        assert_eq!(step(&mut sim, actor, Buttons::NONE, Stick::CENTER), 1, "hanging still: the hold's ray");
    }
    let view = sim.actor(actor).unwrap();
    assert!(climbing(&sim, actor));
    assert!((view.position - held).length() < 1e-5, "no fall: {} from {held}", view.position);
    assert!((view.position.z - (FACE_Z - HOLDS_AT)).abs() < 1e-3, "at the gap: {}", view.position);
    assert!((normal(&sim, actor) - Vec3::NEG_Z).length() < 1e-5);
    assert!(det::wrap_angle(view.yaw).abs() < 1e-5, "facing the face: {}", view.yaw);
    assert_eq!(view.controller.state.mode, GroundMode::Airborne);
    assert_eq!(sim.stats().climbs_started, 1);
    assert!(!view.controller.state.events.jumped, "the press grabbed, and didn't jump");
}

/// The stick toward the face climbs straight up it at the climbing speed, each tick a sweep and the hold's ray.
#[test]
fn the_stick_toward_the_face_climbs_up_it() {
    let (mut sim, actor) = heroine(vec![floor(), wall(6.0, climbable())], 0.0);
    grab(&mut sim, actor);
    let from = feet(&sim, actor);
    for _ in 0..60 {
        assert_eq!(step(&mut sim, actor, Buttons::NONE, TOWARD), 2, "a sweep and the hold");
    }
    let to = feet(&sim, actor);
    let rose = to.y - from.y;
    // A full stick is 127 of 127: a second at the climbing speed.
    assert!((rose - CLIMB_SPEED).abs() < 0.01, "rose {rose}");
    assert!((to.x - from.x).abs() < 1e-4 && (to.z - from.z).abs() < 1e-4, "straight up: {from} to {to}");
    assert!(climbing(&sim, actor));
}

/// The stick along the face moves along it, level, at the climbing speed.
#[test]
fn the_stick_along_the_face_climbs_sideways() {
    let (mut sim, actor) = heroine(vec![floor(), wall(6.0, climbable())], -2.0);
    grab(&mut sim, actor);
    for _ in 0..20 {
        step(&mut sim, actor, Buttons::NONE, TOWARD);
    }
    let from = feet(&sim, actor);
    for _ in 0..60 {
        assert_eq!(step(&mut sim, actor, Buttons::NONE, EAST), 2, "a sweep and the hold");
    }
    let to = feet(&sim, actor);
    assert!((to.x - from.x - CLIMB_SPEED).abs() < 0.01, "east by a second's climb: {from} to {to}");
    assert!(
        (to.y - from.y).abs() < 1e-4 && (to.z - from.z).abs() < 1e-4,
        "level, on the face: {from} to {to}"
    );
}

/// Past the face's east end she goes round its outside corner onto the wall's east side, facing it, and holds it
/// at the gap.
#[test]
fn climbing_sideways_rounds_an_outside_corner() {
    let (mut sim, actor) = heroine(vec![floor(), wall(6.0, climbable())], 2.0);
    grab(&mut sim, actor);
    for _ in 0..20 {
        step(&mut sim, actor, Buttons::NONE, TOWARD);
    }
    let height = feet(&sim, actor).y;
    let mut corner = None;
    for tick in 0..120 {
        let casts = step(&mut sim, actor, Buttons::NONE, EAST);
        if normal(&sim, actor).x > 0.5 {
            corner = Some((tick, casts));
            break;
        }
    }
    let (_, casts) = corner.expect("it turned the corner");
    assert!(casts <= 6, "a corner's tick: {casts}");
    let view = sim.actor(actor).unwrap();
    assert!((normal(&sim, actor) - Vec3::X).length() < 1e-5);
    assert!(
        (view.position.x - (3.0 + HOLDS_AT)).abs() < 1e-3,
        "at the gap off the east side: {}",
        view.position
    );
    assert!(view.position.z > FACE_Z, "round the corner: {}", view.position);
    assert!((view.position.y - height).abs() < 1e-4, "level: {}", view.position);
    let facing = det::direction_from_yaw(view.yaw);
    assert!((facing - Vec3::NEG_X).length() < 1e-4, "facing the east side: {facing}");
    // The stick held east keeps the meaning it had on the face: on round the corner along the east side, level,
    // never down it.
    let z = view.position.z;
    for _ in 0..20 {
        step(&mut sim, actor, Buttons::NONE, EAST);
    }
    let on = feet(&sim, actor);
    assert!(on.z > z + 0.4 && (on.y - height).abs() < 1e-4, "carried on along the east side: {on}");
    assert!((normal(&sim, actor) - Vec3::X).length() < 1e-5, "still on it");
    // Released and pushed east again, it reads against the east side: away from it, down.
    step(&mut sim, actor, Buttons::NONE, Stick::CENTER);
    step(&mut sim, actor, Buttons::NONE, EAST);
    assert!(feet(&sim, actor).y < height, "{}", feet(&sim, actor));
}

/// Climbing east along the face into the wing standing out from it, she turns onto the wing's side, facing it.
#[test]
fn climbing_sideways_turns_an_inside_corner() {
    let (mut sim, actor) = heroine(vec![floor(), wall(6.0, climbable()), wing()], 1.5);
    grab(&mut sim, actor);
    for _ in 0..20 {
        step(&mut sim, actor, Buttons::NONE, TOWARD);
    }
    let height = feet(&sim, actor).y;
    let mut corner = None;
    for tick in 0..120 {
        let casts = step(&mut sim, actor, Buttons::NONE, EAST);
        if normal(&sim, actor).x < -0.5 {
            corner = Some((tick, casts));
            break;
        }
    }
    let (_, casts) = corner.expect("it turned the corner");
    assert!(casts <= 3, "an inside corner's tick: {casts}");
    let view = sim.actor(actor).unwrap();
    assert!((normal(&sim, actor) - Vec3::NEG_X).length() < 1e-5);
    assert!((view.position.x - (3.0 - HOLDS_AT)).abs() < 1e-3, "at the gap off the wing: {}", view.position);
    assert!(view.position.z < FACE_Z - 0.3, "beside the face: {}", view.position);
    assert!((view.position.y - height).abs() < 1e-4, "level: {}", view.position);
    // The stick held east keeps the meaning it had on the face: on along the wing's side, away from the face,
    // level.
    let z = view.position.z;
    for _ in 0..20 {
        step(&mut sim, actor, Buttons::NONE, EAST);
    }
    let on = feet(&sim, actor);
    assert!(on.z < z - 0.4 && (on.y - height).abs() < 1e-4, "on along the wing: {on}");
    // Released and pushed east again, it reads against the wing's side: toward it, up.
    step(&mut sim, actor, Buttons::NONE, Stick::CENTER);
    step(&mut sim, actor, Buttons::NONE, EAST);
    assert!(feet(&sim, actor).y > height, "{}", feet(&sim, actor));
}

/// A diagonal stick into an inside corner, up and toward the wing, turns onto the wing once and stays there:
/// the stick keeps the meaning it had on the face, up and along away from it, so it never pushes her back.
#[test]
fn a_diagonal_into_an_inside_corner_turns_once() {
    let (mut sim, actor) = heroine(vec![floor(), wall(6.0, climbable()), wing()], 1.5);
    grab(&mut sim, actor);
    let start = feet(&sim, actor);
    let mut normals = vec![normal(&sim, actor)];
    // About 70 ticks to the corner, and 60 held into it.
    for _ in 0..130 {
        step(&mut sim, actor, Buttons::NONE, Stick::new(127, 127));
        let now = normal(&sim, actor);
        if (now - *normals.last().unwrap()).length() > 1e-3 {
            normals.push(now);
        }
    }
    assert_eq!(normals.len(), 2, "turned once: {normals:?}");
    assert!((normals[1] - Vec3::NEG_X).length() < 1e-5, "onto the wing: {normals:?}");
    let end = feet(&sim, actor);
    assert!(end.y > start.y + 0.5, "it climbed: {end}");
    assert!(end.z < FACE_Z - HOLDS_AT - 0.2, "along the wing, away from the face: {end}");
}

/// A tagged 2 m panel set into an untagged wall: a diagonal stick up and toward its side end brushes past the
/// end and still climbs, its lip still below, and the stick straight up climbs on from there.
#[test]
fn a_diagonal_at_a_side_end_still_climbs() {
    let panel = block(2, Vec3::new(-1.0, 0.0, FACE_Z), Vec3::new(1.0, 6.0, FACE_Z + 2.0), climbable());
    let west =
        block(3, Vec3::new(-3.0, 0.0, FACE_Z), Vec3::new(-1.0, 6.0, FACE_Z + 2.0), Group::World.mask());
    let east = block(4, Vec3::new(1.0, 0.0, FACE_Z), Vec3::new(3.0, 6.0, FACE_Z + 2.0), Group::World.mask());
    let (mut sim, actor) = heroine(vec![floor(), panel, west, east], 0.0);
    grab(&mut sim, actor);
    let mut last = feet(&sim, actor);
    for tick in 0..90 {
        step(&mut sim, actor, Buttons::NONE, Stick::new(-127, 127));
        let now = feet(&sim, actor);
        assert!(now.y > last.y, "tick {tick}: it climbs at the end too: {now}");
        last = now;
    }
    assert!(last.x < -0.9, "it reached the side end: {last}");
    let hold = sim.actor(actor).unwrap().controller.state.climb.expect("climbing");
    assert_eq!(hold.lip, Lip::Below, "nowhere near the top");
    for _ in 0..20 {
        step(&mut sim, actor, Buttons::NONE, TOWARD);
    }
    assert!(feet(&sim, actor).y > last.y + 0.4, "straight up climbs on: {}", feet(&sim, actor));
}

/// A climb to the top of a 3 m wall: her hands reach its lip, and the stick still up tops out with the high
/// mantle onto its top, which stands her there, past the edge, out of the climb.
#[test]
fn a_climb_to_the_top_tops_out_by_a_mantle() {
    let (mut sim, actor) = heroine(vec![floor(), wall(3.0, climbable())], 0.0);
    grab(&mut sim, actor);
    let mut reached = false;
    let mut top_out = None;
    for _ in 0..200 {
        let before = sim.stats().climb_casts;
        // Up until the top-out, and then still, or the stick would run her off the top's far side.
        let stick = if top_out.is_none() { TOWARD } else { Stick::CENTER };
        sim.step(&act(actor, Buttons::NONE, stick));
        let state = sim.actor(actor).unwrap().controller.state;
        reached |= state.climb.is_some_and(|hold| hold.lip == Lip::Reached);
        if top_out.is_none() && sim.stats().climb_top_outs == 1 {
            top_out = Some(sim.stats().climb_casts - before);
        }
    }
    let view = sim.actor(actor).unwrap();
    assert!(reached, "its hands reached the lip");
    assert_eq!(top_out, Some(4), "the top-out's probe is a mantle's four casts");
    assert_eq!(sim.stats().mantles_started, 0, "a top-out isn't a press's mantle");
    assert!(!climbing(&sim, actor));
    assert_eq!(view.controller.state.mode, GroundMode::Grounded);
    assert!((view.position.y - 3.0).abs() < 0.01, "on the top: {}", view.position);
    assert!(view.position.z > FACE_Z + 0.3, "past the edge: {}", view.position);
}

/// A press of jump while climbing lets go: she falls back to the floor, and stands there.
#[test]
fn a_press_lets_go_of_the_face() {
    let (mut sim, actor) = heroine(vec![floor(), wall(6.0, climbable())], 0.0);
    grab(&mut sim, actor);
    for _ in 0..60 {
        step(&mut sim, actor, Buttons::NONE, TOWARD);
    }
    assert!(feet(&sim, actor).y > 1.4);
    assert_eq!(step(&mut sim, actor, Buttons::JUMP, Stick::CENTER), 0, "a let-go casts nothing of a climb's");
    assert!(!climbing(&sim, actor), "it let go");
    assert!(!sim.actor(actor).unwrap().controller.state.events.jumped, "and didn't jump");
    for _ in 0..60 {
        sim.step(&TickActions::NEUTRAL);
    }
    let view = sim.actor(actor).unwrap();
    assert_eq!(view.controller.state.mode, GroundMode::Grounded);
    assert!(view.position.y.abs() < 1e-3, "on the floor: {}", view.position);
}

/// Climbing down onto the floor lets go there: her feet take it.
#[test]
fn climbing_down_to_the_floor_stands_on_it() {
    let (mut sim, actor) = heroine(vec![floor(), wall(6.0, climbable())], 0.0);
    grab(&mut sim, actor);
    for _ in 0..30 {
        step(&mut sim, actor, Buttons::NONE, TOWARD);
    }
    for _ in 0..60 {
        step(&mut sim, actor, Buttons::NONE, AWAY);
    }
    let view = sim.actor(actor).unwrap();
    assert!(!climbing(&sim, actor));
    assert_eq!(view.controller.state.mode, GroundMode::Grounded);
    assert!(view.position.y.abs() < 1e-3, "on the floor: {}", view.position);
}

/// The same wall in `World` alone, not tagged: the press looks and grabs nothing, and jumps.
#[test]
fn a_face_that_isnt_tagged_is_never_grabbed() {
    let (mut sim, actor) = heroine(vec![floor(), wall(6.0, Group::World.mask())], 0.0);
    assert_eq!(step(&mut sim, actor, Buttons::JUMP, Stick::CENTER), 1, "the grab's ray, which finds nothing");
    let view = sim.actor(actor).unwrap();
    assert!(!climbing(&sim, actor));
    assert!(view.controller.state.events.jumped, "it jumped");
    // Pressed again at the top of the jump, beside the face, still nothing.
    for _ in 0..15 {
        sim.step(&TickActions::NEUTRAL);
    }
    step(&mut sim, actor, Buttons::JUMP, Stick::CENTER);
    assert!(!climbing(&sim, actor));
    assert_eq!(sim.stats().climbs_started, 0);
}

/// With half a second of stamina she lets go when it's spent and falls; she can't grab again in the air, but on
/// the floor it comes back whole and she can. With stamina off she hangs for ten seconds and holds on.
#[test]
fn staminas_end_lets_go_and_stamina_off_never_does() {
    let (mut sim, actor) = heroine_with(vec![floor(), wall(6.0, climbable())], 0.0, Some(0.5));
    grab(&mut sim, actor);
    for _ in 0..20 {
        step(&mut sim, actor, Buttons::NONE, TOWARD);
    }
    let mut held = 2 + 20;
    while climbing(&sim, actor) && held < 120 {
        step(&mut sim, actor, Buttons::NONE, Stick::CENTER);
        held += 1;
    }
    assert_eq!(held, 30, "half a second, then it let go");
    let spent = sim.actor(actor).unwrap().controller.state.stamina_spent;
    assert_eq!(spent.whole_ticks(), 30);
    step(&mut sim, actor, Buttons::JUMP, Stick::CENTER);
    assert!(!climbing(&sim, actor), "spent: no grab in the air");
    for _ in 0..60 {
        sim.step(&TickActions::NEUTRAL);
    }
    let view = sim.actor(actor).unwrap();
    assert_eq!(view.controller.state.mode, GroundMode::Grounded);
    assert_eq!(view.controller.state.stamina_spent.as_subticks(), 0, "back whole on the floor");
    step(&mut sim, actor, Buttons::JUMP, Stick::CENTER);
    assert!(climbing(&sim, actor), "and it grabs again");

    let (mut sim, actor) = heroine(vec![floor(), wall(6.0, climbable())], 0.0);
    grab(&mut sim, actor);
    for _ in 0..600 {
        step(&mut sim, actor, Buttons::NONE, Stick::CENTER);
    }
    assert!(climbing(&sim, actor), "stamina off: it never tires");
    assert_eq!(sim.actor(actor).unwrap().controller.state.stamina_spent.as_subticks(), 0);
}

/// A party won't swap its climber off a face: the press waits in its buffer, and the swap fires once she lets go.
#[test]
fn a_party_swaps_nobody_off_a_face() {
    let mut sim = Simulation::new(SimConfig::default());
    sim.set_physics_world(
        PhysicsWorld::new(vec![floor(), wall(6.0, climbable())], sim.body_capacity()).unwrap(),
    )
    .unwrap();
    let set = heroine_moves(Some(5.0));
    let profile = set.movement().copied().unwrap_or_default();
    let controller = Some(sim.add_controller_profile(&profile).unwrap());
    let moves = Some(sim.add_move_set(set));
    let at = Vec3::new(0.0, 0.0, FACE_Z - STANDS_OFF);
    let members = [0, 1].map(|_| {
        sim.spawn(ActorSpawn { position: at, yaw: 0.0, moves, controller, ..ActorSpawn::default() }).unwrap()
    });
    sim.form_party(&members, PartyTuning { swap_cooldown: 10, rest: 10 }).unwrap();
    for _ in 0..30 {
        sim.step(&TickActions::NEUTRAL);
    }
    let party = members[0];
    grab(&mut sim, party);
    for _ in 0..20 {
        step(&mut sim, party, Buttons::NONE, TOWARD);
    }
    step(&mut sim, party, Buttons::SWAP, TOWARD);
    for _ in 0..3 {
        step(&mut sim, party, Buttons::NONE, TOWARD);
    }
    assert_eq!(sim.stats().swaps, 0, "refused on the face");
    assert_eq!(sim.party().on_field(party), members[0]);
    assert!(climbing(&sim, members[0]));
    // Let go with the press still buffered: it fires as she falls.
    step(&mut sim, party, Buttons::JUMP, Stick::CENTER);
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(sim.stats().swaps, 1, "fired once she let go");
    let coming = sim.party().on_field(party);
    assert_eq!(coming, members[1]);
    let state = sim.actor(coming).unwrap().controller.state;
    assert_eq!((state.traversal, state.climb), (TraversalMode::None, None), "she came in off the face");
    // The stamina spent is each one's own: the one coming in hasn't climbed, and the one who did keeps hers.
    assert_eq!(state.stamina_spent.as_subticks(), 0, "the newcomer's own stamina");
    let left = sim.actor(members[0]).unwrap().controller.state.stamina_spent;
    assert!(left.whole_ticks() >= 20, "the climber's spent stays hers: {left:?}");
}

/// A swap pressed as the climber tops out waits for the top-out's mantle to end, and the mantle stands her on
/// the top, rather than the newcomer coming in where she hung and falling.
#[test]
fn a_swap_waits_for_a_top_out() {
    let mut sim = Simulation::new(SimConfig::default());
    sim.set_physics_world(
        PhysicsWorld::new(vec![floor(), wall(3.0, climbable())], sim.body_capacity()).unwrap(),
    )
    .unwrap();
    let set = replay::player_moves().unwrap();
    let profile = set.movement().copied().unwrap_or_default();
    let controller = Some(sim.add_controller_profile(&profile).unwrap());
    let moves = Some(sim.add_move_set(set));
    let at = Vec3::new(0.0, 0.0, FACE_Z - STANDS_OFF);
    let members = [0, 1].map(|_| {
        sim.spawn(ActorSpawn { position: at, yaw: 0.0, moves, controller, ..ActorSpawn::default() }).unwrap()
    });
    sim.form_party(&members, PartyTuning { swap_cooldown: 10, rest: 10 }).unwrap();
    for _ in 0..30 {
        sim.step(&TickActions::NEUTRAL);
    }
    let party = members[0];
    grab(&mut sim, party);
    for _ in 0..200 {
        step(&mut sim, party, Buttons::NONE, TOWARD);
        if sim.stats().climb_top_outs == 1 {
            break;
        }
    }
    assert_eq!(sim.stats().climb_top_outs, 1, "it topped out");
    // The press lands on the first tick of the mantle, with the climb let go of.
    step(&mut sim, party, Buttons::SWAP, Stick::CENTER);
    for _ in 0..80 {
        sim.step(&TickActions::NEUTRAL);
    }
    assert_eq!(sim.stats().swaps, 0, "the press waited out the mantle, and its buffer ran out");
    assert_eq!(sim.party().on_field(party), members[0]);
    let view = sim.actor(members[0]).unwrap();
    assert!((view.position.y - 3.0).abs() < 0.01, "on the top: {}", view.position);
    assert_eq!(view.controller.state.mode, GroundMode::Grounded);
}

/// A climber defeated on the face lets go as her defeat plays out, and the swap the defeat forces comes once
/// that has played out, no later than a tick after, with the newcomer off the face.
#[test]
fn a_defeat_on_a_face_forces_its_swap_off_the_face() {
    let mut sim = Simulation::new(SimConfig { max_actors: 8, ..SimConfig::default() });
    sim.set_physics_world(
        PhysicsWorld::new(vec![floor(), wall(6.0, climbable())], sim.body_capacity()).unwrap(),
    )
    .unwrap();
    let text = include_str!("../../../content/combat/player/player.moves.ron");
    let frail = text.replacen("health: 300,", "health: 1,", 1);
    assert_ne!(frail, text, "the heroine's health is where it was");
    let running = replay::player_moves().unwrap();
    let set = replay::compile_document(&frail, Some(&running)).unwrap().set;
    let profile = set.movement().copied().unwrap_or_default();
    let controller = Some(sim.add_controller_profile(&profile).unwrap());
    let moves = Some(sim.add_move_set(set));
    let at = Vec3::new(0.0, 0.0, FACE_Z - STANDS_OFF);
    let members = [0, 1].map(|_| {
        sim.spawn(ActorSpawn { position: at, yaw: 0.0, moves, controller, ..ActorSpawn::default() }).unwrap()
    });
    let behind = Vec3::new(0.0, 0.0, FACE_Z - HOLDS_AT - 1.2);
    let opponent_moves = Some(sim.add_move_set(replay::test_moves().unwrap()));
    let opponent = sim
        .spawn(ActorSpawn {
            position: behind,
            yaw: 0.0,
            moves: opponent_moves,
            controller,
            ..ActorSpawn::default()
        })
        .unwrap();
    sim.form_party(&members, PartyTuning { swap_cooldown: 10, rest: 10 }).unwrap();
    for _ in 0..30 {
        sim.step(&TickActions::NEUTRAL);
    }
    let party = members[0];
    grab(&mut sim, party);
    let mut defeated = None;
    for tick in 0..200 {
        let buttons = if tick % 20 == 0 { Buttons::LIGHT } else { Buttons::NONE };
        let on_face = climbing(&sim, party);
        sim.step(&act(opponent, buttons, Stick::CENTER));
        if sim.actor(party).unwrap().fighter.is_defeated() {
            assert!(on_face, "she was on the face when the blow fell");
            defeated = Some(tick);
            break;
        }
    }
    defeated.expect("the blow defeated her");
    // Her defeat's reaction is a move, which starts on the next step: it plays out when it has run and ended.
    let (mut ran, mut played_out) = (false, None);
    for tick in 0..400 {
        let view = sim.actor(party).unwrap();
        if sim.stats().swaps == 1 {
            let played_out = played_out.expect("its defeat played out first");
            assert!(tick <= played_out + 1, "the swap came at {tick}, its defeat out at {played_out}");
            let coming = sim.party().on_field(party);
            assert_eq!(coming, members[1]);
            let state = sim.actor(coming).unwrap().controller.state;
            assert_eq!((state.traversal, state.climb), (TraversalMode::None, None), "off the face");
            return;
        }
        ran |= view.moves.current.is_some();
        if ran && played_out.is_none() && view.moves.current.is_none() {
            assert_eq!(view.controller.state.climb, None, "her defeat let go of the face");
            played_out = Some(tick);
        }
        if ran {
            assert_eq!(view.controller.state.climb, None, "a move lets go of the face");
        }
        sim.step(&TickActions::NEUTRAL);
    }
    panic!("no forced swap");
}
