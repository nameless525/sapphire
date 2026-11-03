//! Pillar 5: it never jerks. The bound is on how fast the view moves and how fast that changes from frame
//! to frame, per second so it reads the same at any frame rate, and nothing a rig does may break it.

#![allow(clippy::print_stdout, reason = "with --nocapture, the measurements the crate's docs quote")]

mod common;

use common::{
    Moment, Motion, RATES, combat_director, director, fight, lock_on_director, open_ground, run,
    without_clearing,
};
use sapphire_camera::{CameraDirector, CameraTuning, Rig};
use sapphire_math::{Quat, Vec3};

/// A bound on how a view may move.
#[derive(Clone, Copy, Debug)]
struct Bound {
    /// The eye's speed, in metres a second.
    speed: f32,
    /// The view's turn, in radians a second.
    turn: f32,
    /// The eye's change of velocity, in metres a second squared.
    acceleration: f32,
    /// The view's change of turning, in radians a second squared.
    turn_acceleration: f32,
}

/// Through a fight. The script's 15 m/s dash reaches 150 m/s² by itself, so the eye's acceleration is the
/// fighters' own, smoothed. Lock-on takes most of this: the view holding the line to an enemy sprinting
/// round the player at 1.8 m. Combat stays within 17 m/s, 1.6 rad/s, 90 m/s² and 6 rad/s². That's with
/// clearing off: a turn it chooses is a view really changing, so it's held to [`SWING`].
const FIGHT: Bound = Bound { speed: 30.0, turn: 3.2, acceleration: 190.0, turn_acceleration: 22.0 };

/// A rig really changing, with its target directly behind the camera and far off: the view swings round
/// at `max_turn_rate`, 3 rad/s, on an arm of up to 12 m.
const SWING: Bound = Bound { speed: 40.0, turn: 3.2, acceleration: 220.0, turn_acceleration: 30.0 };

fn assert_within(bound: Bound, what: &str, motion: &Motion) {
    println!(
        "{what}: {:.2} m/s, {:.2} rad/s, {:.1} m/s², {:.1} rad/s²",
        motion.top_speed, motion.top_turn, motion.top_acceleration, motion.top_turn_acceleration
    );
    assert!(motion.top_speed <= bound.speed, "{what}: {} m/s", motion.top_speed);
    assert!(motion.top_turn <= bound.turn, "{what}: {} rad/s", motion.top_turn);
    assert!(motion.top_acceleration <= bound.acceleration, "{what}: {} m/s²", motion.top_acceleration);
    assert!(
        motion.top_turn_acceleration <= bound.turn_acceleration,
        "{what}: {} rad/s²",
        motion.top_turn_acceleration
    );
}

/// The motion of a run after its first frame's cut, with `each` called before every frame.
fn measure(
    mut camera: CameraDirector,
    rate: u32,
    seconds: f32,
    script: impl Fn(f32) -> Moment,
    mut each: impl FnMut(u32, &mut CameraDirector),
) -> Motion {
    let world = open_ground();
    let mut motion = Motion::default();
    let dt = 1.0 / rate as f32;
    let frames = (seconds * rate as f32) as u32;
    for frame in 1..=frames {
        each(frame, &mut camera);
        let pose = camera.update(&script(frame as f32 * dt).frame(dt), &world);
        motion.add(&pose, dt);
    }
    motion
}

#[test]
fn a_scripted_fight_stays_within_the_bound() {
    for rate in RATES {
        let motion = measure(director(), rate, common::FIGHT_SECONDS, fight, |_, _| {});
        assert_within(FIGHT, &format!("explore at {rate} Hz"), &motion);
        let camera = without_clearing(combat_director());
        let motion = measure(camera, rate, common::FIGHT_SECONDS, fight, |_, _| {});
        assert_within(FIGHT, &format!("combat at {rate} Hz"), &motion);
        let camera = without_clearing(lock_on_director());
        let motion = measure(camera, rate, common::FIGHT_SECONDS, fight, |_, _| {});
        assert_within(FIGHT, &format!("lock-on at {rate} Hz"), &motion);

        let motion = measure(combat_director(), rate, common::FIGHT_SECONDS, fight, |_, _| {});
        assert_within(SWING, &format!("combat, choosing, at {rate} Hz"), &motion);
        let motion = measure(lock_on_director(), rate, common::FIGHT_SECONDS, fight, |_, _| {});
        assert_within(SWING, &format!("lock-on, choosing, at {rate} Hz"), &motion);
    }
}

/// A rig popped on and off in one frame, every half second through the fight: the bound is the run without
/// the pops, and they add next to nothing to it.
#[test]
fn a_rig_popping_on_and_off_in_one_frame_keeps_the_bound() {
    for rate in RATES {
        let plain = measure(director(), rate, common::FIGHT_SECONDS, fight, |_, _| {});
        let bound = Bound {
            speed: plain.top_speed + 0.5,
            turn: plain.top_turn + 0.1,
            acceleration: plain.top_acceleration + 5.0,
            turn_acceleration: plain.top_turn_acceleration + 2.0,
        };
        for rig in [Rig::Combat, Rig::LockOn] {
            let period = rate / 2;
            let pop = |frame: u32, camera: &mut CameraDirector| {
                camera.set_rig(rig, frame.is_multiple_of(period));
            };
            let motion = measure(director(), rate, common::FIGHT_SECONDS, fight, pop);
            assert_within(bound, &format!("{rig:?} popping at {rate} Hz"), &motion);
        }
    }
}

/// One such pop, from rest: what it does at most, with the rig's goal as far from the view as it can be.
#[test]
fn one_pop_barely_moves_the_view() {
    let world = open_ground();
    let behind = Moment::against(Vec3::ZERO, Vec3::new(0.0, 0.0, -12.0));
    let mut camera = director();
    run(&mut camera, &world, 60, (0.0, 2.0), |_| behind, |_, _, _| {});
    let rest = camera.pose();

    camera.set_rig(Rig::LockOn, true);
    camera.update(&behind.frame(1.0 / 60.0), &world);
    camera.set_rig(Rig::LockOn, false);
    let (mut far, mut turned) = (0.0_f32, 0.0_f32);
    let mut motion = Motion::default();
    run(
        &mut camera,
        &world,
        60,
        (2.0, 6.0),
        |_| behind,
        |_, pose, _| {
            far = far.max(pose.eye.distance(rest.eye));
            turned = turned.max(2.0 * pose.rotation.dot(rest.rotation).abs().min(1.0).acos());
            motion.add(pose, 1.0 / 60.0);
        },
    );
    println!("one pop: {far} m, {turned} rad");
    assert!(far < 0.25, "{far} m");
    assert!(turned < 0.05, "{turned} rad");
    assert_within(FIGHT, "one pop", &motion);
}

/// Every change of rig the stack has, at each's worst: the target directly behind the camera, far off.
#[test]
fn a_rig_changing_at_the_worst_moment_keeps_the_bound() {
    let behind = Moment::against(Vec3::ZERO, Vec3::new(0.0, 0.0, -12.0));
    for rate in RATES {
        for (from, to) in [(None, Rig::Combat), (None, Rig::LockOn), (Some(Rig::Combat), Rig::LockOn)] {
            let change = |frame: u32, camera: &mut CameraDirector| {
                if frame == rate {
                    camera.set_rig(to, true);
                }
                if frame == rate * 5 {
                    camera.set_rig(to, false);
                }
            };
            let mut camera = director();
            if let Some(from) = from {
                camera.set_rig(from, true);
            }
            // A cut's first frame, then a second to rest, then the change.
            let motion = measure(camera, rate, 9.0, |_| behind, change);
            assert_within(SWING, &format!("{from:?} to {to:?} at {rate} Hz"), &motion);
        }
    }
}

/// A soft lock switching between enemies on opposite sides: the framed position follows, with no jump.
#[test]
fn a_target_switch_keeps_the_bound() {
    let script = |t: f32| {
        let side = if ((t * 0.5) as u32).is_multiple_of(2) { 8.0 } else { -8.0 };
        Moment::against(Vec3::ZERO, Vec3::new(side, 0.0, 6.0))
    };
    for rate in RATES {
        let motion = measure(combat_director(), rate, 16.0, script, |_, _| {});
        assert_within(SWING, &format!("combat, switching at {rate} Hz"), &motion);
        let motion = measure(lock_on_director(), rate, 16.0, script, |_, _| {});
        assert_within(SWING, &format!("lock-on, switching at {rate} Hz"), &motion);
    }
}

/// The target going away with no warning, and coming back.
#[test]
fn a_target_coming_and_going_keeps_the_bound() {
    let script = |t: f32| {
        if ((t * 0.5) as u32).is_multiple_of(2) {
            Moment::alone(Vec3::ZERO)
        } else {
            Moment::against(Vec3::ZERO, Vec3::new(4.0, 0.0, -9.0))
        }
    };
    for rate in RATES {
        let motion = measure(combat_director(), rate, 16.0, script, |_, _| {});
        assert_within(SWING, &format!("combat, coming and going at {rate} Hz"), &motion);
    }
}

/// A takeover blends in and out within the bound for an ordinary shot, and at full weight is exactly the
/// authored pose.
#[test]
fn a_takeover_blends_and_lands_exactly() {
    let world = open_ground();
    let authored = sapphire_camera::CameraPose {
        eye: Vec3::new(3.0, 2.0, -4.0),
        rotation: Quat::from_rotation_y(0.6) * Quat::from_rotation_x(0.2),
        fov_y: 0.6,
    };
    let script = |_| Moment::against(Vec3::ZERO, Vec3::new(1.0, 0.0, 6.0));
    let mut camera = combat_director();
    run(&mut camera, &world, 60, (0.0, 2.0), script, |_, _, _| {});
    let before = camera.pose();

    camera.set_takeover(Some(authored));
    let mut motion = Motion::default();
    run(&mut camera, &world, 60, (2.0, 6.0), script, |_, pose, _| motion.add(pose, 1.0 / 60.0));
    assert_eq!(camera.weight(Rig::Takeover), 1.0);
    assert_eq!(camera.pose(), authored);

    camera.set_takeover(None);
    run(&mut camera, &world, 60, (6.0, 10.0), script, |_, pose, _| motion.add(pose, 1.0 / 60.0));
    assert_eq!(camera.weight(Rig::Takeover), 0.0);
    assert!(camera.pose().eye.distance(before.eye) < 0.01, "back to the rigs: {:?}", camera.pose());
    assert_within(SWING, "takeover", &motion);
}

/// Reloading tuning with the camera running only moves goals.
#[test]
fn a_reload_keeps_the_bound() {
    let world = open_ground();
    let mut wide = CameraTuning::default_tuning();
    wide.combat.framing.distance = 9.0;
    wide.combat.framing.fov_y = 1.2;
    wide.combat.framing.pivot_bias = 0.1;
    let shipped = CameraTuning::default_tuning();

    let mut camera = combat_director();
    let mut motion = Motion::default();
    let dt = 1.0 / 60.0;
    for frame in 1..=(60 * 8) {
        if frame % 120 == 0 {
            let tuning = if (frame / 120) % 2 == 1 { wide.clone() } else { shipped.clone() };
            camera.set_tuning(tuning).expect("both are valid");
        }
        let pose = camera.update(&fight(frame as f32 * dt).frame(dt), &world);
        motion.add(&pose, dt);
    }
    // A reload's goals can move any distance in a frame, so this holds speed and turn, and a reload is a
    // tool's, in any case.
    let reload = Bound { acceleration: f32::MAX, turn_acceleration: f32::MAX, ..SWING };
    assert_within(reload, "reloading", &motion);

    let mut bad = CameraTuning::default_tuning();
    bad.collision.radius = f32::NAN;
    let error = camera.set_tuning(bad).expect_err("a NaN radius");
    assert!(error.to_string().contains("collision.radius"), "{error}");
    assert_eq!(camera.tuning().collision.radius, shipped.collision.radius, "the old tuning stays");
}
