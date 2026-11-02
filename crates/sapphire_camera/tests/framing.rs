//! Pillar 5: both fighters stay framed. Each run settles first, then every frame is held to a safe frame
//! wider than the one the rigs solve for, since the springs lag behind it.

#![allow(clippy::print_stdout, reason = "with --nocapture, the measurements the crate's docs quote")]

mod common;

use common::{Moment, RATES, combat_director, fight, frame_use, lock_on_director, open_ground, run};
use sapphire_camera::{CameraDirector, CameraPose, Subject};
use sapphire_math::{Vec2, Vec3};

/// The safe frame, as a share of the screen's half width and height: nothing leaves it.
const SAFE: Vec2 = Vec2::new(0.9, 0.9);
const SETTLE_SECONDS: f32 = 1.0;

/// The worst either fighter comes to the frame's edge, from `from` seconds on.
fn worst_use(
    mut camera: CameraDirector,
    rate: u32,
    seconds: f32,
    from: f32,
    script: impl Fn(f32) -> Moment,
) -> Vec2 {
    let world = open_ground();
    let mut worst = Vec2::ZERO;
    let mut check = |time: f32, pose: &CameraPose, moment: &Moment| {
        if time < from {
            return;
        }
        let target: Subject = moment.target.expect("these scripts have a target");
        worst = worst.max(frame_use(pose, &moment.player)).max(frame_use(pose, &target));
    };
    run(&mut camera, &world, rate, (0.0, seconds), script, &mut check);
    worst
}

fn assert_framed(worst: Vec2, what: &str) {
    assert!(worst.x <= SAFE.x && worst.y <= SAFE.y, "{what}: reached {worst}, outside {SAFE}");
}

#[test]
fn both_fighters_stay_framed_through_a_scripted_fight() {
    for rate in RATES {
        let worst = worst_use(combat_director(), rate, common::FIGHT_SECONDS, SETTLE_SECONDS, fight);
        println!("combat at {rate} Hz: {worst}");
        assert_framed(worst, &format!("combat at {rate} Hz"));

        let worst = worst_use(lock_on_director(), rate, common::FIGHT_SECONDS, SETTLE_SECONDS, fight);
        println!("lock-on at {rate} Hz: {worst}");
        assert_framed(worst, &format!("lock-on at {rate} Hz"));
    }
}

/// A fighter running a circle around the player, at every size: inside the camera's distance, through
/// it and well behind it, so at each lap it passes behind the camera.
#[test]
fn a_fighter_circling_behind_the_camera_stays_framed() {
    for radius in [1.5_f32, 3.0, 5.0, 8.0, 12.0] {
        let speed = 7.0_f32;
        let script = move |t: f32| {
            let angle = t * speed / radius;
            Moment::against(Vec3::ZERO, Vec3::new(radius * angle.sin(), 0.0, radius * angle.cos()))
        };
        for rate in RATES {
            // Enough for two laps of the widest.
            let worst = worst_use(combat_director(), rate, 22.0, SETTLE_SECONDS, script);
            println!("radius {radius} at {rate} Hz: {worst}");
            assert_framed(worst, &format!("circling at {radius} m at {rate} Hz"));

            let worst = worst_use(lock_on_director(), rate, 22.0, SETTLE_SECONDS, script);
            println!("lock-on, radius {radius} at {rate} Hz: {worst}");
            assert_framed(worst, &format!("lock-on, circling at {radius} m at {rate} Hz"));
        }
    }
}

/// The player sweeping the view around as they please: the view gives way, then keeps the target.
#[test]
fn look_input_cant_lose_the_target() {
    let script = |t: f32| Moment {
        look_rate: Vec2::new(2.5 * (0.5 * t).sin(), 0.0),
        ..Moment::against(Vec3::ZERO, Vec3::new(1.0, 0.0, 7.0))
    };
    let worst = worst_use(combat_director(), 60, 20.0, SETTLE_SECONDS, script);
    println!("under look input: {worst}");
    assert_framed(worst, "under look input");
}

/// The narrow screen of a phone held upright keeps both on it too, if not quite inside the safe frame: the
/// solve takes the aspect.
#[test]
fn a_narrow_screen_frames_them_too() {
    let world = open_ground();
    let mut camera = combat_director();
    let aspect = 9.0 / 16.0;
    let mut worst = 0.0_f32;
    for frame in 1..=600 {
        let t = frame as f32 / 60.0;
        let moment = fight(t);
        let pose =
            camera.update(&sapphire_camera::CameraFrame { aspect, ..moment.frame(1.0 / 60.0) }, &world);
        if t < SETTLE_SECONDS {
            continue;
        }
        for subject in [moment.player, moment.target.expect("the fight has a target")] {
            for point in [subject.position, subject.position + Vec3::Y * subject.height] {
                let (x, _, depth) = common::screen_of(&pose, point);
                let half = (pose.fov_y * 0.5).tan();
                worst = worst.max((x * common::ASPECT + subject.radius / (depth * half)).abs() / aspect);
            }
        }
    }
    println!("upright: {worst}");
    assert!(worst <= 1.0, "{worst}");
}
