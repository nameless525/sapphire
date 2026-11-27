//! Mantles and vaults (M6 A15), on the heroine's moves: a press of jump in front of a ledge of 0.5 to 2 m, within
//! 45° of the facing, starts a mantle that stands the character on its top; at a run in front of a low obstacle
//! with ground beyond, a vault over it. No room, too steep an approach, too tall or too low a ledge, and the press
//! jumps. The same ledge reached at half and twice the clock's speed ends in the same place.

use sapphire_core::Name;
use sapphire_gameplay::traversal::{self, LedgeKind, Refused};
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, GroundMode, SimConfig, Simulation, Stick, TickActions,
    WarpTarget, actor_body, replay,
};
use sapphire_math::{Vec3, det};
use sapphire_moves::ControllerProfile;
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};
use sapphire_time::ScaleSource;

/// Where the ledge's face is: a block from z 1 on, across x −3 to 3, facing −Z toward the character.
const FACE_Z: f32 = 1.0;
/// How far from the face the character's axis stands before it presses: 0.15 m clear of its capsule.
const STANDS_OFF: f32 = 0.45;

fn block(id: u32, min: Vec3, max: Vec3) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Convex(Shape::Box { half_extents: (max - min) * 0.5 }),
        pose: Pose::at((min + max) * 0.5),
        memberships: Group::World.mask(),
    }
}

fn floor() -> StaticCollider {
    block(0, Vec3::new(-50.0, -1.0, -50.0), Vec3::new(50.0, 0.0, 50.0))
}

/// A ledge `height` tall and 2 m deep, its face at [`FACE_Z`].
fn ledge(height: f32) -> StaticCollider {
    block(1, Vec3::new(-3.0, 0.0, FACE_Z), Vec3::new(3.0, height, FACE_Z + 2.0))
}

/// The heroine on her moves and movement, at `at` facing `yaw`, on `statics`.
fn heroine(statics: Vec<StaticCollider>, at: Vec3, yaw: f32) -> (Simulation, ActorId) {
    let mut sim = Simulation::new(SimConfig::default());
    sim.set_physics_world(PhysicsWorld::new(statics, sim.body_capacity()).unwrap()).unwrap();
    let set = replay::player_moves().unwrap();
    let profile = set.movement().copied().unwrap_or_default();
    let controller = Some(sim.add_controller_profile(&profile).unwrap());
    let moves = Some(sim.add_move_set(set));
    let id = sim.spawn(ActorSpawn { position: at, yaw, moves, controller, ..ActorSpawn::default() }).unwrap();
    // A spawn starts in the air: its first ticks find the floor.
    for _ in 0..10 {
        sim.step(&TickActions::NEUTRAL);
    }
    (sim, id)
}

fn act(actor: ActorId, buttons: Buttons, stick: Stick) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    actions.set(actor, ActorActions { buttons, stick }).unwrap();
    actions
}

/// Presses jump standing still, then lets 90 ticks go by: long enough for either mantle and a landing.
fn press_and_wait(sim: &mut Simulation, actor: ActorId) {
    sim.step(&act(actor, Buttons::JUMP, Stick::CENTER));
    for _ in 0..90 {
        sim.step(&TickActions::NEUTRAL);
    }
}

/// Stands the heroine before a ledge at an approach of `degrees` off its face's normal, the ray along her facing
/// meeting the face at x 0.
fn before(height: f32, degrees: f32) -> (Simulation, ActorId) {
    let yaw = degrees.to_radians();
    let back = det::direction_from_yaw(yaw) * (STANDS_OFF / det::cos(yaw));
    heroine(vec![floor(), ledge(height)], Vec3::new(0.0, 0.0, FACE_Z) - back, yaw)
}

fn feet(sim: &Simulation, actor: ActorId) -> Vec3 {
    sim.actor(actor).unwrap().position
}

/// Every ledge from 0.5 to 2 m, a station on each limit included, at every approach within 45° of the face's
/// normal either way: the press mantles, and she stands on the top, past its edge.
#[test]
fn a_ledge_of_half_a_metre_to_two_is_mantled_within_45_degrees() {
    for height in [0.5, 0.75, 1.0, 1.25, 1.3, 1.5, 1.75, 2.0] {
        for degrees in [0.0, 15.0, -30.0, 45.0, -45.0] {
            let (mut sim, actor) = before(height, degrees);
            press_and_wait(&mut sim, actor);
            let at = feet(&sim, actor);
            let view = sim.actor(actor).unwrap();
            let what = format!("{height} m at {degrees}°: feet at {at}");
            assert_eq!(sim.stats().mantles_started, 1, "{what}");
            assert!((at.y - height).abs() < 0.01, "on the top: {what}");
            assert!(at.z > FACE_Z + 0.3, "past the edge: {what}");
            assert_eq!(view.controller.state.mode, GroundMode::Grounded, "{what}");
            assert!(sim.stats().ledge_casts <= 4, "a mantle's probe is four casts: {what}");
        }
    }
}

/// Past 45°, under the lowest ledge, over the highest or with no room above the top, the press jumps: she
/// comes back down to the floor in front of it, or onto the step.
#[test]
fn a_press_with_nothing_to_mantle_jumps() {
    let slab = |height: f32| {
        block(2, Vec3::new(-3.0, height + 1.2, FACE_Z), Vec3::new(3.0, height + 1.5, FACE_Z + 2.0))
    };
    let cases: [(&str, Vec<StaticCollider>, f32); 5] = [
        ("too steep an approach", vec![floor(), ledge(1.0)], 50.0),
        ("a step, which the feet take", vec![floor(), ledge(0.3)], 0.0),
        ("a ledge over the highest", vec![floor(), ledge(2.3)], 0.0),
        ("no room over the top", vec![floor(), ledge(1.0), slab(1.0)], 0.0),
        ("nothing in reach", vec![floor()], 0.0),
    ];
    for (what, statics, degrees) in cases {
        let yaw = f32::to_radians(degrees);
        let back = det::direction_from_yaw(yaw) * (STANDS_OFF / det::cos(yaw));
        let (mut sim, actor) = heroine(statics, Vec3::new(0.0, 0.0, FACE_Z) - back, yaw);
        sim.step(&act(actor, Buttons::JUMP, Stick::CENTER));
        assert!(sim.actor(actor).unwrap().controller.state.events.jumped, "{what}: it jumped");
        for _ in 0..90 {
            sim.step(&TickActions::NEUTRAL);
        }
        assert_eq!((sim.stats().mantles_started, sim.stats().ledges_refused), (0, 1), "{what}");
        assert!(feet(&sim, actor).y < 0.31, "{what}: {}", feet(&sim, actor));
    }
}

/// How high the heroine's vault lifts her root: what the probe asks room for over her at a run.
const VAULT_APEX: f32 = 1.3;
/// A run along the facing, past half way from walking to running speed.
const RUNNING: f32 = 6.0;

/// Asks the probe about `statics` with the heroine before them at `degrees`, moving at `speed` along her facing,
/// on `profile`: what it found and how many casts it made.
fn probe_before(
    statics: Vec<StaticCollider>,
    degrees: f32,
    speed: f32,
    profile: &ControllerProfile,
) -> (Result<traversal::Ledge, Refused>, u32) {
    let resolved = sapphire_gameplay::ResolvedProfile::new(profile).unwrap();
    let yaw = f32::to_radians(degrees);
    let back = det::direction_from_yaw(yaw) * (STANDS_OFF / det::cos(yaw));
    let (sim, actor) = heroine(statics, Vec3::new(0.0, 0.0, FACE_Z) - back, yaw);
    let view = sim.actor(actor).unwrap();
    let mut casts = 0;
    let found = traversal::probe(
        sim.physics(),
        actor_body(actor),
        view.position,
        view.yaw,
        det::direction_from_yaw(yaw) * speed,
        &resolved,
        Some(VAULT_APEX),
        &mut casts,
    );
    (found, casts)
}

/// A slab over everything, its underside `height` up: a ceiling over the character as much as the top.
fn ceiling(height: f32) -> StaticCollider {
    block(3, Vec3::new(-10.0, height, -10.0), Vec3::new(10.0, height + 0.3, 10.0))
}

/// A wall `height` tall and `depth` deep, its face at [`FACE_Z`], with the floor beyond it.
fn wall(height: f32, depth: f32) -> StaticCollider {
    block(1, Vec3::new(-3.0, 0.0, FACE_Z), Vec3::new(3.0, height, FACE_Z + depth))
}

/// A probe case: what it is, the scenery, the approach in degrees or the speed, what the probe says and its casts.
type ProbeCase = (&'static str, Vec<StaticCollider>, f32, Result<LedgeKind, Refused>, u32);

/// The probe says why it refused: no room to go in is the slab over the top, no room to rise a ceiling over
/// the character (refused at the third cast, before any crossing), and the others what they are.
#[test]
fn the_probe_says_why() {
    let profile = ControllerProfile::default();
    let slab = block(2, Vec3::new(-3.0, 2.2, FACE_Z), Vec3::new(3.0, 2.5, FACE_Z + 2.0));
    let cases: [ProbeCase; 6] = [
        ("a ledge", vec![floor(), ledge(1.0)], 0.0, Ok(LedgeKind::Mantle), 4),
        ("a slab over the top", vec![floor(), ledge(1.0), slab], 0.0, Err(Refused::NoRoom), 4),
        ("a ceiling over her", vec![floor(), ledge(1.0), ceiling(2.4)], 0.0, Err(Refused::NoRoom), 3),
        ("too steep an approach", vec![floor(), ledge(1.0)], 50.0, Err(Refused::Approach), 1),
        ("too tall", vec![floor(), ledge(2.5)], 0.0, Err(Refused::Height), 2),
        ("nothing", vec![floor()], 0.0, Err(Refused::NoFace), 1),
    ];
    for (what, statics, degrees, expected, casts) in cases {
        let (found, made) = probe_before(statics, degrees, 0.0, &profile);
        assert_eq!((found.map(|ledge| ledge.kind), made), (expected, casts), "{what}");
    }
}

/// At a run, an obstacle a vault's depth deep is vaulted and one deeper is mantled, the far ray looking where
/// the capsule's near side would land; under a ceiling too low for the vault's arc the run is refused for
/// want of room, though a walk up to the same wall mantles it.
#[test]
fn a_vault_asks_for_the_ground_past_its_depth_and_room_over_its_arc() {
    let profile = ControllerProfile::default();
    let low = || vec![floor(), wall(0.5, 0.4), ceiling(2.4)];
    let cases: [ProbeCase; 5] = [
        ("0.4 m deep at a run", vec![floor(), wall(1.0, 0.4)], RUNNING, Ok(LedgeKind::Vault), 5),
        ("0.8 m deep at a run", vec![floor(), wall(1.0, 0.8)], RUNNING, Ok(LedgeKind::Vault), 5),
        ("1 m deep at a run", vec![floor(), wall(1.0, 1.0)], RUNNING, Ok(LedgeKind::Mantle), 5),
        ("under a ceiling at a run", low(), RUNNING, Err(Refused::NoRoom), 3),
        ("under a ceiling at a walk", low(), 0.0, Ok(LedgeKind::Mantle), 4),
    ];
    for (what, statics, speed, expected, casts) in cases {
        let (found, made) = probe_before(statics, 0.0, speed, &profile);
        assert_eq!((found.map(|ledge| ledge.kind), made), (expected, casts), "{what}");
        if let Some(ledge) = found.ok().filter(|ledge| ledge.kind == LedgeKind::Vault) {
            assert!(ledge.target.y.abs() < 1e-3, "{what}: lands on the floor, at {}", ledge.target);
        }
    }
}

/// A profile whose step is higher than its lowest mantle: a ledge within the step, the slack included, is
/// stepped and never mantled, and one just past it is mantled (A14's rule, held on the slack's side).
#[test]
fn a_ledge_the_feet_step_onto_is_never_mantled() {
    let profile = ControllerProfile { step_height: 0.6, ..ControllerProfile::default() };
    assert!(profile.step_height >= profile.mantle_min_height);
    let cases = [(0.6, Err(Refused::Height)), (0.605, Err(Refused::Height)), (0.65, Ok(LedgeKind::Mantle))];
    for (height, expected) in cases {
        let (found, _) = probe_before(vec![floor(), ledge(height)], 0.0, 0.0, &profile);
        assert_eq!(found.map(|ledge| ledge.kind), expected, "{height} m");
    }
}

/// An attack's turn and travel from where the heroine stands: 40 ticks after a press of light.
fn attack(sim: &mut Simulation, actor: ActorId) -> (Vec3, f32) {
    let (at, yaw) = (feet(sim, actor), sim.actor(actor).unwrap().yaw);
    sim.step(&act(actor, Buttons::LIGHT, Stick::CENTER));
    for _ in 0..40 {
        sim.step(&TickActions::NEUTRAL);
    }
    let view = sim.actor(actor).unwrap();
    (view.position - at, view.yaw - yaw)
}

/// A mantle's aim is its own: after one, with no soft lock, an attack turns and moves as it would with no
/// mantle before it, nothing pulling it back toward the ledge.
#[test]
fn an_attack_after_a_mantle_warps_toward_nothing() {
    let (mut plain, actor) = heroine(vec![floor()], Vec3::ZERO, std::f32::consts::FRAC_PI_2);
    let (expected, unturned) = attack(&mut plain, actor);
    assert!(unturned.abs() < 1e-4);

    let (mut sim, actor) = before(1.0, 0.0);
    press_and_wait(&mut sim, actor);
    assert_eq!(sim.stats().mantles_started, 1);
    // Off east along the top, away from the point the mantle aimed at, then still.
    for _ in 0..12 {
        sim.step(&act(actor, Buttons::NONE, Stick::new(127, 0)));
    }
    for _ in 0..30 {
        sim.step(&TickActions::NEUTRAL);
    }
    let yaw = sim.actor(actor).unwrap().yaw;
    assert!(det::wrap_angle(yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-3, "she faces east: {yaw}");
    let (moved, turned) = attack(&mut sim, actor);
    assert!(turned.abs() < 1e-4, "the attack turned {turned} rad");
    assert!(
        (moved - expected).length() < 1e-3,
        "the attack moved {moved}, against {expected} with no mantle"
    );
}

/// A warp window's rise stops at its `max_rise`: `mantle_low` aimed 5 m over its start rises by its table
/// and the window's 0.6 m, no more.
#[test]
fn a_warp_rises_no_further_than_its_max_rise() {
    let (mut sim, actor) = heroine(vec![floor()], Vec3::ZERO, 0.0);
    assert!(sim.set_warp_target(actor, WarpTarget::Point(Vec3::new(0.0, 5.0, 0.6))));
    assert!(sim.force_move(actor, Name::new("mantle_low")));
    let mut highest = 0.0_f32;
    for _ in 0..40 {
        sim.step(&TickActions::NEUTRAL);
        highest = highest.max(feet(&sim, actor).y);
    }
    // The table's own rise is 1.05 m.
    assert!((highest - 1.65).abs() < 1e-3, "it rose to {highest}");
}

/// The same ledge at half, normal and twice the clock's speed: each reaches the same place, to the millimetre.
#[test]
fn a_mantle_reaches_the_same_place_at_half_and_twice_the_speed() {
    for height in [0.5, 1.0, 1.5, 2.0] {
        let ends = [500, 1000, 2000].map(|per_mille| {
            let (mut sim, actor) = before(height, 20.0);
            sim.clock_mut(actor).unwrap().0.push_scale(ScaleSource::Debug, per_mille).unwrap();
            // A slower clock plays the same mantle over more steps.
            sim.step(&act(actor, Buttons::JUMP, Stick::CENTER));
            for _ in 0..200 {
                sim.step(&TickActions::NEUTRAL);
            }
            assert_eq!(sim.stats().mantles_started, 1, "{height} m at {per_mille}‰");
            feet(&sim, actor)
        });
        for (end, per_mille) in ends.iter().zip([500, 2000]) {
            let apart = (*end - ends[1]).length();
            assert!(apart < 1e-3, "{height} m at {per_mille}‰: {end} against {} at 1000‰", ends[1]);
        }
    }
}

/// At a run, a 1 m wall 0.4 m deep with the floor beyond is vaulted, onto the floor on its far side; walking up
/// to it, the press mantles onto it instead.
#[test]
fn a_run_vaults_a_low_wall_and_a_walk_mantles_it() {
    let wall = || block(1, Vec3::new(-3.0, 0.0, FACE_Z), Vec3::new(3.0, 1.0, FACE_Z + 0.4));
    for (stick, vaults) in [(Stick::new(0, 127), true), (Stick::new(0, 64), false)] {
        let (mut sim, actor) = heroine(vec![floor(), wall()], Vec3::new(0.0, 0.0, FACE_Z - 8.0), 0.0);
        let mut pressed = false;
        for _ in 0..400 {
            let near = feet(&sim, actor).z > FACE_Z - 0.75;
            let buttons = if near && !pressed { Buttons::JUMP } else { Buttons::NONE };
            pressed |= near;
            sim.step(&act(actor, buttons, if pressed { Stick::CENTER } else { stick }));
        }
        let at = feet(&sim, actor);
        let stats = sim.stats();
        if vaults {
            assert_eq!((stats.vaults_started, stats.mantles_started), (1, 0), "{stats:?}");
            // The whole capsule past the wall's back, not on its edge.
            assert!(at.z > FACE_Z + 0.4 + 0.3 && at.y.abs() < 1e-3, "over it, on the floor: {at}");
            assert!(stats.ledge_casts <= 5, "a vault's probe is five casts");
        } else {
            assert_eq!((stats.vaults_started, stats.mantles_started), (0, 1), "{stats:?}");
        }
    }
}
