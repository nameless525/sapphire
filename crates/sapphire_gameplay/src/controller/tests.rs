//! The controller inside a simulation: its clock, its checksum, and the seam a move system writes to.

#![allow(
    clippy::print_stdout,
    reason = "these report what they measured, and CI's log fills the checksum in"
)]

use sapphire_math::Vec3;
use sapphire_moves::{MoveSetDocument, PathSample};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};
use sapphire_time::ScaleSource;

use super::*;
use crate::actions::{ActorActions, Stick, TickActions};
use crate::actors::{ActorSpawn, ActorTable};
use crate::hits::{HurtboxId, MAX_HURTBOXES};
use crate::moves::MoveOutput;
use crate::simulation::{SimConfig, Simulation};

fn solid(id: u32, min: Vec3, max: Vec3) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Convex(Shape::Box { half_extents: (max - min) / 2.0 }),
        pose: Pose::at((min + max) / 2.0),
        memberships: Group::World.mask(),
    }
}

fn floor(id: u32) -> StaticCollider {
    solid(id, Vec3::new(-100.0, -1.0, -100.0), Vec3::new(100.0, 0.0, 100.0))
}

/// A simulation with these colliders, and a character from each of these starts.
fn sim_with(statics: Vec<StaticCollider>, starts: &[Vec3]) -> (Simulation, Vec<ActorId>) {
    let mut sim = Simulation::new(SimConfig { max_actors: 8, ..SimConfig::default() });
    sim.set_physics_world(PhysicsWorld::new(statics, sim.body_capacity()).unwrap()).unwrap();
    let profile = sim.add_controller_profile(&ControllerProfile::default()).unwrap();
    let actors = starts
        .iter()
        .map(|&position| {
            sim.spawn(ActorSpawn { position, controller: Some(profile), ..ActorSpawn::default() }).unwrap()
        })
        .collect();
    (sim, actors)
}

fn tick_of(actor: ActorId, stick: Stick, buttons: Buttons) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    actions.set(actor, ActorActions { buttons, stick }).unwrap();
    actions
}

fn run_right(actor: ActorId) -> TickActions {
    tick_of(actor, Stick::from_axes(1.0, 0.0), Buttons::NONE)
}

fn entity(sim: &Simulation, id: ActorId) -> Entity {
    sim.world().resource::<ActorTable>().entity(id).unwrap()
}

fn apply_hitstop(sim: &mut Simulation, id: ActorId, ticks: u16) {
    let entity = entity(sim, id);
    sim.ecs_mut().world_mut().get_mut::<EntityClock>(entity).unwrap().0.apply_hitstop(ticks);
}

fn local_scale(sim: &mut Simulation, id: ActorId, per_mille: u32) {
    let entity = entity(sim, id);
    sim.ecs_mut()
        .world_mut()
        .get_mut::<EntityClock>(entity)
        .unwrap()
        .0
        .push_scale(ScaleSource::Gameplay, per_mille)
        .unwrap();
}

/// A view by its exact bits.
fn exact(sim: &Simulation, id: ActorId) -> ([u32; 3], [u32; 3], [u32; 3], u64) {
    let view = sim.actor(id).unwrap();
    let state = view.controller.state;
    (
        view.position.to_array().map(f32::to_bits),
        state.velocity.to_array().map(f32::to_bits),
        state.ground_normal.to_array().map(f32::to_bits),
        state.coyote.as_subticks(),
    )
}

// --- Hitstop and time scale ---

/// Frozen, in each of the states a character can be in, nothing moves and nothing accumulates: the
/// stick held and gravity pulling through six ticks of hitstop change no bit, and afterward it goes on
/// exactly as a twin that never froze.
#[test]
fn a_frozen_owner_doesnt_move_and_doesnt_accumulate() {
    let plateau = solid(2, Vec3::new(-50.0, -1.0, -20.0), Vec3::new(0.0, 1.0, 20.0));
    let statics = || vec![floor(1), plateau.clone()];
    // (a start, ticks to run first, whether it jumps at the end of them)
    let rows = [
        ("running", Vec3::new(-20.0, 1.0, 0.0), 10, false),
        ("mid-run-start", Vec3::new(-20.0, 1.0, 0.0), 2, false),
        ("rising", Vec3::new(-20.0, 1.0, 0.0), 10, true),
        ("falling", Vec3::new(5.0, 6.0, 0.0), 12, false),
        ("on coyote time", Vec3::new(-0.5, 1.0, 0.0), 8, false),
    ];
    for (name, start, ticks, jumps) in rows {
        let (mut frozen, actors) = sim_with(statics(), &[start]);
        let (mut twin, _) = sim_with(statics(), &[start]);
        let id = actors[0];
        for _ in 0..ticks {
            frozen.step(&run_right(id));
            twin.step(&run_right(id));
        }
        if jumps {
            let jump = tick_of(id, Stick::from_axes(1.0, 0.0), Buttons::JUMP);
            frozen.step(&jump);
            twin.step(&jump);
        }
        assert_eq!(exact(&frozen, id), exact(&twin, id), "{name}: the twins agree so far");

        apply_hitstop(&mut frozen, id, 6);
        let held = exact(&frozen, id);
        let body = frozen.physics().body_pose(actor_body(id));
        for tick in 0..6 {
            // Hard the other way: if anything accumulated, this would show in the velocity after.
            frozen.step(&tick_of(id, Stick::from_axes(-1.0, 1.0), Buttons::NONE));
            assert_eq!(exact(&frozen, id), held, "{name}, frozen tick {tick}");
            assert_eq!(frozen.physics().body_pose(actor_body(id)), body);
        }
        for tick in 0..40 {
            frozen.step(&run_right(id));
            twin.step(&run_right(id));
            assert_eq!(exact(&frozen, id), exact(&twin, id), "{name}, tick {tick} after the freeze");
        }
    }
}

/// Coyote time is the owner's: a frozen owner's doesn't run out, and at half speed it lasts twice the
/// ticks.
#[test]
fn coyote_time_runs_on_the_owners_clock() {
    let plateau = solid(2, Vec3::new(-50.0, -1.0, -20.0), Vec3::new(0.0, 1.0, 20.0));
    let (mut sim, actors) = sim_with(vec![floor(1), plateau], &[Vec3::new(-0.5, 1.0, 0.0)]);
    let id = actors[0];
    local_scale(&mut sim, id, 500);
    sim.step(&TickActions::NEUTRAL);
    for _ in 0..600 {
        if sim.actor(id).unwrap().controller.state.mode != GroundMode::Grounded {
            break;
        }
        sim.step(&run_right(id));
    }
    let start = sim.actor(id).unwrap().controller.state.coyote;
    assert_eq!(start, SimDuration::from_ticks(5));
    // 5 ticks of its own time at 500‰ is 10 real ticks.
    for _ in 0..9 {
        sim.step(&run_right(id));
    }
    let left = sim.actor(id).unwrap().controller.state.coyote;
    assert_eq!(left, SimDuration::from_subticks(500), "half a tick of its time left");
    // Hitstop now would let it wait as long as it likes.
    apply_hitstop(&mut sim, id, 30);
    for _ in 0..30 {
        sim.step(&run_right(id));
    }
    assert_eq!(sim.actor(id).unwrap().controller.state.coyote, left);
    sim.step(&tick_of(id, Stick::from_axes(1.0, 0.0), Buttons::JUMP));
    assert!(sim.actor(id).unwrap().controller.state.events.jumped);
}

/// Under a time scale the same motion takes more ticks and covers the same distance: a jump reaches
/// the same height at 500‰ and at 333‰, whose ticks alternate 333 and 334 sub-ticks, and a run start
/// covers the same ground in its own time.
#[test]
fn a_time_scale_stretches_time_and_not_distance() {
    let apex = |per_mille: Option<u32>| {
        let (mut sim, actors) = sim_with(vec![floor(1)], &[Vec3::ZERO]);
        let id = actors[0];
        sim.step(&TickActions::NEUTRAL);
        if let Some(per_mille) = per_mille {
            local_scale(&mut sim, id, per_mille);
            sim.step(&TickActions::NEUTRAL);
        }
        sim.step(&tick_of(id, Stick::CENTER, Buttons::JUMP));
        let mut highest = 0.0_f32;
        let mut ticks = 0;
        loop {
            highest = highest.max(sim.actor(id).unwrap().position.y);
            sim.step(&TickActions::NEUTRAL);
            ticks += 1;
            if sim.actor(id).unwrap().controller.state.mode == GroundMode::Grounded || ticks > 1000 {
                return (highest, ticks);
            }
        }
    };
    let (base, base_ticks) = apex(None);
    let hand = 7.0 * 7.0 / (2.0 * 22.0);
    for (per_mille, stretch) in [(500, 2.0_f32), (333, 3.003), (2000, 0.5)] {
        let (highest, ticks) = apex(Some(per_mille));
        println!(
            "jump at {per_mille}‰: {highest:.5} m in {ticks} ticks, at full speed {base:.5} m in {base_ticks}"
        );
        // The arc's top falls between two ticks, by at most g dt² / 8 of the tick's own length.
        let dt = per_mille as f32 / 1000.0 / 60.0;
        let sampled = 22.0 * dt * dt / 8.0 + 1e-4;
        assert!((highest - hand).abs() < sampled, "{per_mille}‰ reached {highest}, not {hand} ± {sampled}");
        let expected = base_ticks as f32 * stretch;
        assert!(
            (ticks as f32 - expected).abs() <= stretch.max(1.0) + 1.0,
            "{per_mille}‰ took {ticks}, not {expected}"
        );
    }

    // Tick 2k at 500‰ is tick k of its own time: the run start's numbers, at half the ticks' pace.
    let (mut sim, actors) = sim_with(vec![floor(1)], &[Vec3::ZERO]);
    let id = actors[0];
    sim.step(&TickActions::NEUTRAL);
    local_scale(&mut sim, id, 500);
    sim.step(&TickActions::NEUTRAL);
    for tick in 1..=8 {
        sim.step(&run_right(id));
        let own = tick as f32 / 2.0;
        let expected = 0.0125 * own * own;
        let x = sim.actor(id).unwrap().position.x;
        assert!((x - expected).abs() < 1e-5, "tick {tick} at 500‰: {x} is not {expected}");
    }
}

/// The world's scale reaches the controller through the same clock.
#[test]
fn the_worlds_time_scale_reaches_it_too() {
    let (mut sim, actors) = sim_with(vec![floor(1)], &[Vec3::ZERO]);
    let id = actors[0];
    sim.step(&TickActions::NEUTRAL);
    sim.ecs_mut()
        .world_mut()
        .resource_mut::<WorldClock>()
        .0
        .push_scale(ScaleSource::PerfectDodge, 200)
        .unwrap();
    sim.step(&TickActions::NEUTRAL);
    for _ in 0..5 {
        sim.step(&run_right(id));
    }
    // One tick of its own time: 1.25 cm.
    let x = sim.actor(id).unwrap().position.x;
    assert!((x - 0.0125).abs() < 1e-5, "{x}");
}

// --- The checksum ---

/// Every field of the controller's state moves the checksum, for either of two actors that differ.
/// Each row flips one, and the checksum must differ from the unflipped one, and come back when it's
/// flipped back. Because the writes are destructured, a new field can't compile unwritten, and it gets
/// a row here.
#[test]
fn every_state_field_is_in_the_checksum() {
    type Flip = fn(&mut CharacterController);
    let rows: &[(&str, Flip)] = &[
        ("mode", |c| c.state.mode = GroundMode::Sliding),
        ("velocity.x", |c| c.state.velocity.x += 1e-5),
        ("velocity.y", |c| c.state.velocity.y += 1e-5),
        ("velocity.z", |c| c.state.velocity.z += 1e-5),
        ("ground_normal.x", |c| c.state.ground_normal.x += 1e-5),
        ("ground_normal.y", |c| c.state.ground_normal.y -= 1e-5),
        ("ground_normal.z", |c| c.state.ground_normal.z += 1e-5),
        ("coyote", |c| c.state.coyote += SimDuration::from_subticks(1)),
        ("landed", |c| c.state.events.landed = !c.state.events.landed),
        ("landing_speed", |c| c.state.events.landing_speed += 1e-5),
        ("jumped", |c| c.state.events.jumped = !c.state.events.jumped),
        ("left_ground", |c| c.state.events.left_ground = !c.state.events.left_ground),
        ("stepped_up", |c| c.state.events.stepped_up = !c.state.events.stepped_up),
        ("height_pop", |c| c.state.events.height_pop += 1e-5),
        ("ledge_held", |c| c.state.events.ledge_held = !c.state.events.ledge_held),
        ("pushed_out", |c| c.state.events.pushed_out = !c.state.events.pushed_out),
        ("drive.root_motion.x", |c| c.drive.root_motion.x += 1e-5),
        ("drive.root_motion.y", |c| c.drive.root_motion.y += 1e-5),
        ("drive.root_motion.z", |c| c.drive.root_motion.z += 1e-5),
        ("drive.locomotion", |c| c.drive.locomotion = !c.drive.locomotion),
        ("drive.gravity", |c| c.drive.gravity = !c.drive.gravity),
        ("drive.mode", |c| c.drive.mode = MovementMode::Combat),
        ("drive.jump", |c| c.drive.jump = !c.drive.jump),
        ("profile", |c| c.profile = Some(ProfileId(c.profile.map_or(0, |p| p.0 + 1)))),
    ];
    let (mut sim, actors) = sim_with(vec![floor(1)], &[Vec3::ZERO, Vec3::new(3.0, 2.0, 1.0)]);
    for tick in 0..10 {
        let mut input = TickActions::NEUTRAL;
        input
            .set(actors[0], ActorActions { buttons: Buttons::NONE, stick: Stick::from_axes(1.0, 0.0) })
            .unwrap();
        if tick == 5 {
            input
                .set(actors[1], ActorActions { buttons: Buttons::JUMP, stick: Stick::from_axes(0.0, 1.0) })
                .unwrap();
        }
        sim.step(&input);
    }
    let base = sim.checksum();
    for &id in &actors {
        let entity = entity(&sim, id);
        for (name, flip) in rows {
            let world = sim.ecs_mut().world_mut();
            let original = *world.get::<CharacterController>(entity).unwrap();
            flip(&mut world.get_mut::<CharacterController>(entity).unwrap());
            assert_ne!(sim.checksum(), base, "{name} isn't in the checksum, for actor {}", id.raw());
            *sim.ecs_mut().world_mut().get_mut::<CharacterController>(entity).unwrap() = original;
            assert_eq!(sim.checksum(), base, "{name} didn't restore");
        }
    }
    // The capsule's own pose goes through the world's canonical state.
    let id = actors[1];
    let pose = sim.physics().body_pose(actor_body(id)).unwrap();
    let mut moved = pose;
    moved.position.x = f32::from_bits(moved.position.x.to_bits() + 1);
    let world = sim.ecs_mut().world_mut();
    world.resource_mut::<CharacterWorld>().physics.set_body_pose(actor_body(id), moved).unwrap();
    assert_ne!(sim.checksum(), base, "the world's bodies are in the checksum");
}

/// A static collider moves the checksum too, since a replay in another arena isn't this one.
#[test]
fn the_arena_is_in_the_checksum() {
    let (a, _) = sim_with(vec![floor(1)], &[Vec3::ZERO]);
    let (b, _) = sim_with(vec![floor(1), solid(2, Vec3::X, Vec3::ONE * 2.0)], &[Vec3::ZERO]);
    assert_ne!(a.checksum(), b.checksum());
}

/// Tuning isn't state, and a replay records it apart: the hash sees every field, in order.
#[test]
fn every_profile_field_is_in_its_hash() {
    let good = ControllerProfile::default();
    let rows = [
        ControllerProfile { height: good.height + 0.1, ..good },
        ControllerProfile { radius: good.radius + 0.01, ..good },
        ControllerProfile { skin: good.skin + 0.001, ..good },
        ControllerProfile { walk_speed: good.walk_speed + 0.1, ..good },
        ControllerProfile { run_speed: good.run_speed + 0.1, ..good },
        ControllerProfile { walk_stick: good.walk_stick + 0.01, ..good },
        ControllerProfile { run_stick: good.run_stick + 0.01, ..good },
        ControllerProfile { acceleration: good.acceleration + 1.0, ..good },
        ControllerProfile { deceleration: good.deceleration + 1.0, ..good },
        ControllerProfile { turn_rate_degrees: good.turn_rate_degrees + 1.0, ..good },
        ControllerProfile { gravity: good.gravity + 1.0, ..good },
        ControllerProfile { jump_speed: good.jump_speed + 1.0, ..good },
        ControllerProfile { air_acceleration: good.air_acceleration + 1.0, ..good },
        ControllerProfile { max_fall_speed: good.max_fall_speed + 1.0, ..good },
        ControllerProfile { coyote_ticks: good.coyote_ticks + 1, ..good },
        ControllerProfile { slope_limit_degrees: good.slope_limit_degrees + 1.0, ..good },
        ControllerProfile { step_height: good.step_height + 0.01, ..good },
        ControllerProfile { ground_snap: good.ground_snap + 0.01, ..good },
        ControllerProfile { stance_radius: good.stance_radius + 0.01, ..good },
        ControllerProfile { max_slide_speed: good.max_slide_speed + 1.0, ..good },
    ];
    let hash = |profile: &ControllerProfile| {
        let mut sim = Simulation::new(SimConfig::default());
        sim.add_controller_profile(profile).unwrap();
        sim.controller_profiles_hash()
    };
    let base = hash(&good);
    assert_eq!(hash(&good), base);
    for (index, row) in rows.iter().enumerate() {
        assert_ne!(hash(row), base, "row {index}");
    }
}

// --- The seam, and the actor's life ---

/// 27.5 m along +Z over 55 ticks, turning a radian, after 5 ticks of startup standing still.
const LUNGE: &str = r#"(
    type: "sapphire::moves::MoveSetDocument",
    version: 1,
    data: (
        intents: [(name: "light", button: Light, buffer: 12, priority: 1)],
        neutral: [(intent: "light", move: "lunge")],
        moves: [
            (
                name: "lunge",
                startup: 5,
                active: 5,
                recovery: 50,
                motion: (root: [(at: 5), (at: 60, offset: (0.0, 0.0, 27.5), yaw: 1.0)]),
            ),
        ],
    ),
)"#;

/// A move's root motion reaches the controller and goes through collision, where an actor with no
/// controller passes through as it always did. This is `apply_root_motion` writing the drive: the seam.
/// A held stick does nothing for the whole move, its still startup included, and hits' two ends of a
/// tick are its start and the collided position. Yaw goes straight on the transform, and the capsule
/// turns to match.
#[test]
fn a_moves_root_motion_goes_through_the_controller() {
    let wall = solid(2, Vec3::new(-2.0, -1.0, 4.0), Vec3::new(2.0, 6.0, 5.0));
    let (mut sim, _) = sim_with(vec![floor(1), wall], &[]);
    let moves = sim.add_move_set(MoveSetDocument::parse(LUNGE).unwrap().compile().unwrap());
    let profile = sim.add_controller_profile(&ControllerProfile::default()).unwrap();
    let actor = |x: f32, controller| ActorSpawn {
        position: Vec3::new(x, 0.0, 0.0),
        moves: Some(moves),
        controller,
        ..ActorSpawn::default()
    };
    let blocked = sim.spawn(actor(0.0, Some(profile))).unwrap();
    let free = sim.spawn(actor(10.0, Some(profile))).unwrap();
    let ghost = sim.spawn(actor(-10.0, None)).unwrap();
    sim.step(&TickActions::NEUTRAL);

    let stick = Stick::from_axes(1.0, 0.0);
    let held = |buttons| {
        let mut input = TickActions::NEUTRAL;
        for id in [blocked, free, ghost] {
            input.set(id, ActorActions { buttons, stick }).unwrap();
        }
        input
    };
    sim.step(&held(Buttons::LIGHT));
    let start = sim.actor(blocked).unwrap();
    let mut last = ActorTransform { position: start.position, yaw: start.yaw };
    for tick in 0..100 {
        sim.step(&held(Buttons::NONE));
        let view = sim.actor(blocked).unwrap();
        let output = sim.world().get::<MoveOutput>(entity(&sim, blocked)).unwrap();
        assert_eq!(output.previous, last, "tick {tick}: where this tick began");
        last = ActorTransform { position: view.position, yaw: view.yaw };
        assert_eq!(view.position.x, 0.0, "tick {tick}: the stick is off");
        assert_eq!(sim.actor(free).unwrap().position.x, 10.0, "tick {tick}: the stick is off");
        if view.moves.current.is_none() {
            break;
        }
    }
    let capsule = sim.physics().body_pose(actor_body(blocked)).unwrap();
    let (blocked, free, ghost) =
        (sim.actor(blocked).unwrap(), sim.actor(free).unwrap(), sim.actor(ghost).unwrap());
    assert!((blocked.yaw - 1.0).abs() < 1e-3, "the table's yaw: {}", blocked.yaw);
    assert_eq!(capsule.rotation, sapphire_math::det::quat_from_rotation_y(blocked.yaw));
    println!(
        "a move of 27.5 m at a wall: stopped at z = {:.4}, in the open {:.4}",
        blocked.position.z, free.position.z
    );
    assert!(blocked.moves.current.is_none(), "the move still ran out");
    assert!((blocked.position.z - (4.0 - 0.31)).abs() < 1e-3, "{}", blocked.position);
    assert_eq!(blocked.controller.state.mode, GroundMode::Grounded);
    assert_eq!(blocked.controller.drive, ControllerDrive::NONE, "a drive lasts a tick");
    assert!((free.position.z - 27.5).abs() < 1e-3, "the table's whole distance: {}", free.position);
    assert!((ghost.position.z - 27.5).abs() < 1e-3, "no controller, no collision: {}", ghost.position);
    assert_eq!(ghost.position.x, -10.0, "and no stick");
}

/// A strike ignores a press of jump, and a move can still ask for one: the seam a buffered intent uses.
#[test]
fn a_strike_takes_no_jump_but_a_drive_can_ask_for_one() {
    let (mut sim, actors) = sim_with(vec![floor(1)], &[Vec3::ZERO]);
    let id = actors[0];
    sim.step(&TickActions::NEUTRAL);
    let entity = entity(&sim, id);
    let drive = |sim: &mut Simulation, drive: ControllerDrive| {
        sim.ecs_mut().world_mut().get_mut::<CharacterController>(entity).unwrap().drive = drive;
    };

    drive(&mut sim, ControllerDrive::strike(Vec3::ZERO));
    sim.step(&tick_of(id, Stick::CENTER, Buttons::JUMP));
    assert!(!sim.actor(id).unwrap().controller.state.events.jumped);

    drive(&mut sim, ControllerDrive { jump: true, ..ControllerDrive::strike(Vec3::ZERO) });
    sim.step(&TickActions::NEUTRAL);
    assert!(sim.actor(id).unwrap().controller.state.events.jumped);
}

/// A stick quantized at a full diagonal, whose axes round on their own and come out a little past 1
/// (I1's contract), runs at the run speed and not past it.
#[test]
fn a_rounded_up_stick_doesnt_pass_the_run_speed() {
    let (mut sim, actors) = sim_with(vec![floor(1)], &[Vec3::ZERO]);
    let id = actors[0];
    let stick =
        Stick::from_axes(std::f32::consts::FRAC_1_SQRT_2 + 0.002, std::f32::consts::FRAC_1_SQRT_2 + 0.002);
    assert!(stick.to_vec2().length() > 1.0, "{}", stick.to_vec2());
    for _ in 0..20 {
        sim.step(&tick_of(id, stick, Buttons::NONE));
    }
    let speed = sim.actor(id).unwrap().controller.state.velocity.length();
    assert!(speed <= 6.0 + 1e-5, "{speed}");
    assert!(speed > 6.0 - 1e-3, "{speed}");
}

/// Yaw from a table goes straight on the transform, and the body turns to match at the tick's end.
#[test]
fn a_yaw_set_from_outside_reaches_the_body() {
    let (mut sim, actors) = sim_with(vec![floor(1)], &[Vec3::ZERO]);
    let id = actors[0];
    sim.step(&TickActions::NEUTRAL);
    let entity = entity(&sim, id);
    sim.ecs_mut().world_mut().get_mut::<ActorTransform>(entity).unwrap().yaw = 1.25;
    sim.step(&TickActions::NEUTRAL);
    let rotation = sim.physics().body_pose(actor_body(id)).unwrap().rotation;
    assert_eq!(rotation, sapphire_math::det::quat_from_rotation_y(1.25));
    assert_eq!(sim.actor(id).unwrap().yaw, 1.25);
}

/// A controller's feet and its body stay together, and what's written to the transform is the feet.
#[test]
fn the_transform_is_the_bodys_feet() {
    let (mut sim, actors) = sim_with(vec![floor(1)], &[Vec3::new(1.0, 0.0, 2.0)]);
    let id = actors[0];
    for _ in 0..30 {
        sim.step(&run_right(id));
        let view = sim.actor(id).unwrap();
        let pose = sim.physics().body_pose(actor_body(id)).unwrap();
        assert_eq!(pose.position, view.position + Vec3::Y * 0.81);
    }
}

/// A despawn takes the capsule out of the world, and a later spawn's ID gets its own.
#[test]
fn a_body_comes_and_goes_with_its_actor() {
    let (mut sim, actors) = sim_with(vec![floor(1)], &[Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)]);
    assert_eq!(sim.physics().body_count(), 2);
    assert!(sim.despawn(actors[0]));
    assert_eq!(sim.physics().body_count(), 1);
    assert!(sim.physics().body_pose(actor_body(actors[0])).is_none());
    let profile = sim.actor(actors[1]).unwrap().controller.profile();
    let new = sim.spawn(ActorSpawn { controller: profile, ..ActorSpawn::default() }).unwrap();
    assert_eq!(sim.physics().body_count(), 2);
    assert!(sim.physics().body_pose(actor_body(new)).is_some());
    // No controller, no body.
    let plain = sim.spawn(ActorSpawn::default()).unwrap();
    assert!(!sim.actor(plain).unwrap().controller.is_active());
    assert_eq!(sim.physics().body_count(), 2);
}

/// A spawn that can't have a controller lives on without one, and the stats say so: an unknown profile,
/// or a world with no room, from a spawn inside a step as much as outside one.
#[test]
fn a_controller_that_cant_be_had_is_counted() {
    let (mut sim, _) = sim_with(vec![floor(1)], &[]);
    let id = sim.spawn(ActorSpawn { controller: Some(ProfileId(99)), ..ActorSpawn::default() }).unwrap();
    assert!(!sim.actor(id).unwrap().controller.is_active());
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(sim.stats().controllers_refused, 1);

    // A non-finite spawn has no valid pose.
    let profile = sim.add_controller_profile(&ControllerProfile::default()).unwrap();
    let bad = ActorSpawn {
        position: Vec3::new(f32::NAN, 0.0, 0.0),
        controller: Some(profile),
        ..ActorSpawn::default()
    };
    let id = sim.spawn(bad).unwrap();
    assert!(!sim.actor(id).unwrap().controller.is_active());
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(sim.stats().controllers_refused, 2);
}

/// A collision world goes in before bodies do, with room for the pool.
#[test]
fn a_physics_world_must_fit() {
    let mut sim = Simulation::new(SimConfig { max_actors: 4, ..SimConfig::default() });
    let needed = sim.body_capacity();
    assert_eq!(needed, 4 * (1 + MAX_HURTBOXES as usize));
    assert_eq!(sim.physics().body_capacity(), needed, "a new simulation's world has room too");
    let small = PhysicsWorld::new(vec![], needed - 1).unwrap();
    assert_eq!(
        sim.set_physics_world(small).unwrap_err(),
        PhysicsWorldError::TooSmall { capacity: needed - 1, needed }
    );
    sim.set_physics_world(PhysicsWorld::new(vec![floor(1)], needed).unwrap()).unwrap();
    assert_eq!(sim.physics().static_count(), 1);
    let profile = sim.add_controller_profile(&ControllerProfile::default()).unwrap();
    sim.spawn(ActorSpawn { controller: Some(profile), ..ActorSpawn::default() }).unwrap();
    assert_eq!(
        sim.set_physics_world(PhysicsWorld::new(vec![], needed).unwrap()).unwrap_err(),
        PhysicsWorldError::BodiesExist
    );
}

/// The sum: a full pool of characters with every hurtbox set fills a world of [`Simulation::body_capacity`]
/// exactly, refused by nothing, in a new simulation's world and a set one alike. The characters still
/// step, and a despawn gives all of an actor's bodies back, because their IDs never come round again.
#[test]
fn a_full_cast_with_every_hurtbox_is_refused_by_nothing() {
    for set_a_world in [false, true] {
        let mut sim = Simulation::new(SimConfig { max_actors: 4, ..SimConfig::default() });
        if set_a_world {
            sim.set_physics_world(PhysicsWorld::new(vec![floor(1)], sim.body_capacity()).unwrap()).unwrap();
        }
        let profile = sim.add_controller_profile(&ControllerProfile::default()).unwrap();
        let cast: Vec<ActorId> = (0..4)
            .map(|index| {
                let position = Vec3::new(index as f32 * 2.0, 0.0, 0.0);
                sim.spawn(ActorSpawn { position, controller: Some(profile), ..ActorSpawn::default() })
                    .unwrap()
            })
            .collect();
        let shape = Shape::Sphere { radius: 0.1 };
        for &actor in &cast {
            let root = ActorTransform { position: sim.actor(actor).unwrap().position, yaw: 0.0 };
            let world = sim.ecs_mut().world_mut();
            let mut characters = world.resource_mut::<CharacterWorld>();
            for index in 0..MAX_HURTBOXES {
                let hurtbox = HurtboxId::new(actor, index).unwrap();
                hurtbox.set(characters.physics_mut(), Some(shape), root, PathSample::IDENTITY).unwrap();
            }
        }
        let full = sim.body_capacity();
        assert_eq!(sim.physics().body_count(), full, "world set: {set_a_world}");
        assert_eq!(sim.physics().body_capacity(), full, "world set: {set_a_world}");

        for _ in 0..3 {
            sim.step(&run_right(cast[1]));
        }
        assert_eq!(sim.stats().controllers_refused, 0);
        assert!(sim.actor(cast[1]).unwrap().position.x != 2.0, "it still moves");

        assert!(sim.despawn(cast[0]));
        sim.step(&TickActions::NEUTRAL);
        assert_eq!(sim.physics().body_count(), full - (1 + MAX_HURTBOXES as usize));
    }
}

/// R9: the stats name what the step did.
#[test]
fn the_stats_count_the_controller() {
    let (mut sim, actors) = sim_with(vec![floor(1)], &[Vec3::ZERO, Vec3::new(5.0, 0.0, 0.0)]);
    sim.step(&run_right(actors[0]));
    sim.step(&run_right(actors[0]));
    let stats = sim.stats();
    assert_eq!(stats.physics.bodies, 2);
    assert_eq!(stats.physics.statics, 1);
    assert!(stats.physics.moves >= 1, "{:?}", stats.physics);
    assert!(stats.physics.shape_casts >= 2, "one ground cast each: {:?}", stats.physics);
    assert!(stats.controllers_last <= stats.controllers_longest);
    assert_eq!(stats.controllers_refused, 0);
}

/// A second character's cast finds where the first moved to this same tick: ID order, with no step.
#[test]
fn later_actors_see_earlier_actors_moves_in_the_same_tick() {
    let (mut sim, actors) = sim_with(vec![floor(1)], &[Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0)]);
    sim.step(&TickActions::NEUTRAL);
    // Toward each other at a run, with 0.39 m between their skins. Each covers 1.25, 3.75 and 6.25 cm
    // on the first three ticks, then the first takes its whole 8.75 cm of the fourth, and the second
    // only what that leaves: 20 cm against 19. In the other order they'd swap.
    let mut input = TickActions::NEUTRAL;
    input
        .set(actors[0], ActorActions { stick: Stick::from_axes(1.0, 0.0), ..ActorActions::default() })
        .unwrap();
    input
        .set(actors[1], ActorActions { stick: Stick::from_axes(-1.0, 0.0), ..ActorActions::default() })
        .unwrap();
    for _ in 0..30 {
        sim.step(&input);
    }
    let a = sim.actor(actors[0]).unwrap().position.x;
    let b = sim.actor(actors[1]).unwrap().position.x;
    assert!((a - 0.2).abs() < 2e-3, "the first: {a}");
    assert!((1.0 - b - 0.19).abs() < 2e-3, "the second: {b}");
}

#[test]
fn a_stick_is_world_space() {
    let (mut sim, actors) = sim_with(vec![floor(1)], &[Vec3::ZERO]);
    let id = actors[0];
    for _ in 0..10 {
        sim.step(&tick_of(id, Stick::from_axes(0.0, 1.0), Buttons::NONE));
    }
    let view = sim.actor(id).unwrap();
    assert!(view.position.z > 0.5 && view.position.x.abs() < 1e-4, "{}", view.position);
}

// --- One checksum for one set of inputs ---

#[path = "../../tests/support/mesh_arena.rs"]
mod mesh_arena;

/// arm64: an Apple M5. CI's macOS runners must agree.
const CHECKSUM_AARCH64: u64 = 0xef94_6904_a64c_c2ef;

/// x86-64: Linux and Windows must both reach this one value. **Not pinned yet:** the first CI run fails,
/// printing the value to put here. Linux and Windows failing with different values is what this exists
/// to find: something on that path reaches a platform's C math library.
const CHECKSUM_X86_64: Option<u64> = Some(0xa2e2_e22e_0a96_c2d2);

const EXPECTED: Option<u64> = if cfg!(target_arch = "aarch64") {
    Some(CHECKSUM_AARCH64)
} else if cfg!(target_arch = "x86_64") {
    CHECKSUM_X86_64
} else {
    None
};

/// Six characters on the mesh arena, none alike: three profiles, each with its own stick and starts, and
/// hitstop, a local scale and a world scale dropping in on their own rhythm. Returns every tick's
/// checksum.
fn scripted_run(ticks: u64) -> Vec<u64> {
    let mut sim = Simulation::new(SimConfig { max_actors: 16, seed: 3, ..SimConfig::default() });
    sim.set_physics_world(PhysicsWorld::new(mesh_arena::statics(), sim.body_capacity()).unwrap()).unwrap();
    let profiles = mesh_arena::profiles().map(|profile| sim.add_controller_profile(&profile).unwrap());
    let actors: Vec<ActorId> = (0..6)
        .map(|index| {
            let (position, profile) = mesh_arena::start(index);
            let spawn = ActorSpawn {
                position,
                yaw: index as f32 * 0.4,
                controller: Some(profiles[profile]),
                ..ActorSpawn::default()
            };
            sim.spawn(spawn).unwrap()
        })
        .collect();
    (0..ticks)
        .map(|tick| {
            sim.step(&mesh_arena::tick_actions(tick, &actors));
            if tick % 211 == 40 {
                apply_hitstop(&mut sim, actors[(tick % 6) as usize], 5);
            }
            if tick % 307 == 70 {
                let id = actors[(tick % 6) as usize];
                let entity = entity(&sim, id);
                let world = sim.ecs_mut().world_mut();
                let mut clock = world.get_mut::<EntityClock>(entity).unwrap();
                if clock.0.remove_source(ScaleSource::Gameplay) == 0 {
                    clock.0.push_scale(ScaleSource::Gameplay, 333).unwrap();
                }
            }
            if tick % 509 == 100 {
                let world = sim.ecs_mut().world_mut();
                let mut clock = world.resource_mut::<WorldClock>();
                if clock.0.remove_source(ScaleSource::PerfectDodge) == 0 {
                    clock.0.push_scale(ScaleSource::PerfectDodge, 700).unwrap();
                }
            }
            sim.checksum()
        })
        .collect()
}

#[test]
fn the_same_inputs_give_the_same_checksums() {
    assert_eq!(scripted_run(1500), scripted_run(1500));
}

/// The run does what it claims: everyone moves and leaves the ground, and nobody's stuck in one place.
#[test]
fn the_scripted_run_exercises_the_controller() {
    let mut sim = Simulation::new(SimConfig { max_actors: 16, ..SimConfig::default() });
    sim.set_physics_world(PhysicsWorld::new(mesh_arena::statics(), sim.body_capacity()).unwrap()).unwrap();
    let profiles = mesh_arena::profiles().map(|profile| sim.add_controller_profile(&profile).unwrap());
    let actors: Vec<ActorId> = (0..6)
        .map(|index| {
            let (position, profile) = mesh_arena::start(index);
            let spawn = ActorSpawn { position, controller: Some(profiles[profile]), ..ActorSpawn::default() };
            sim.spawn(spawn).unwrap()
        })
        .collect();
    let (mut landed, mut jumped, mut stepped, mut moves, mut casts) = (0, 0, 0, 0, 0);
    for tick in 0..3000 {
        sim.step(&mesh_arena::tick_actions(tick, &actors));
        for view in sim.actors() {
            let events = view.controller.state.events;
            landed += u32::from(events.landed);
            jumped += u32::from(events.jumped);
            stepped += u32::from(events.stepped_up);
            assert!(view.position.is_finite(), "{}", view.position);
            assert!(view.position.y > -0.2, "under the ground: {}", view.position);
        }
        let stats = sim.stats().physics;
        moves += stats.moves;
        casts += stats.shape_casts;
    }
    println!(
        "3000 ticks of 6 characters: {landed} landings, {jumped} jumps, {stepped} step-ups, {moves} moves, {casts} casts"
    );
    assert!(landed > 60 && jumped > 60 && stepped > 2, "{landed} {jumped} {stepped}");
    // Sliding along things, but mostly a move and a cast a tick.
    assert!(moves >= 9000, "{moves}");
}

/// Set for the child process of the test below.
const CHILD: &str = "SAPPHIRE_CONTROLLER_DETERMINISM_CHILD";

/// Prints the run's checksums when another run of this test binary asks for them.
#[test]
fn prints_checksums_for_a_parent_process() {
    use std::io::Write as _;
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let line = format!("checksums {:x?}\n", scripted_run(600));
    std::io::stdout().write_all(line.as_bytes()).unwrap();
}

/// A new process has new addresses and hash seeds, and threads that start otherwise.
#[test]
fn the_same_inputs_give_the_same_checksums_across_processes() {
    let expected = format!("checksums {:x?}", scripted_run(600));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "controller::tests::prints_checksums_for_a_parent_process"])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().find(|line| line.contains("checksums [")).expect("the child's checksums");
    assert_eq!(&line[line.find("checksums [").unwrap()..], expected);
}

/// What the run reaches on this architecture (spec §9, gate 2). A change here means the controller's or
/// the collision world's numbers moved: if that's intended, update the constant, and expect replays to
/// re-record.
#[test]
fn the_scripted_run_matches_this_architectures_checksum() {
    let checksums = scripted_run(1500);
    let computed = *checksums.last().unwrap();
    let arch = std::env::consts::ARCH;
    println!("sapphire_gameplay controller checksum on {arch}: {computed:#018x}");
    match EXPECTED {
        Some(expected) => assert_eq!(
            computed, expected,
            "the checksum on {arch} moved: computed {computed:#018x}, pinned {expected:#018x}"
        ),
        None => panic!(
            "no checksum is pinned for {arch}: it computed {computed:#018x}. Set CHECKSUM_X86_64 to this once \
             Linux and Windows print the same value (controller/tests.rs)"
        ),
    }
}
