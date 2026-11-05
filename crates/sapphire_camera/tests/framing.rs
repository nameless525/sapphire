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

/// The solve accepts a narrow aspect when its authored distance budget leaves room for it. The
/// shipped game is landscape-only; retain its horizontal reach for this portrait stress fixture.
#[test]
fn a_narrow_screen_frames_them_too() {
    let world = open_ground();
    let aspect = 9.0 / 16.0;
    let mut camera = combat_director();
    let mut tuning = camera.tuning().clone();
    // A fixed vertical FOV narrows horizontal coverage in proportion to aspect. Keep the
    // landscape fixture's horizontal pullback budget rather than silently requiring it to
    // exceed its authored hard cap; every original frame and the full-screen bound remain.
    tuning.combat.framing.max_distance *= common::ASPECT / aspect;
    camera.set_tuning(tuning).expect("the portrait fixture's pullback range is valid");
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
                assert!(depth > 0.05, "subject behind the eye: {pose:?}");
                worst = worst.max((x.abs() * common::ASPECT + subject.radius / (depth * half)) / aspect);
            }
        }
    }
    println!("upright: {worst}");
    assert!(worst <= 1.0, "{worst}");
}

/// Fitting both fighters is subordinate to the authored hard pullback cap. A budget too small
/// to fit even the player must not make the solver zoom out forever.
#[test]
fn framing_respects_the_authored_cap_when_both_subjects_cannot_fit() {
    let world = open_ground();
    let mut camera = combat_director();
    let mut tuning = camera.tuning().clone();
    tuning.clearing.reach = 0.0;
    tuning.combat.framing.pivot_bias = 0.0;
    tuning.combat.framing.distance = 1.0;
    tuning.combat.framing.max_distance = 1.0;
    camera.set_tuning(tuning.clone()).unwrap();
    let moment = Moment::against(Vec3::ZERO, Vec3::Z * 20.0);
    let mut pose = camera.pose();
    run(&mut camera, &world, 60, (0.0, 3.0), |_| moment, |_, current, _| pose = *current);
    let anchor = moment.player.position + Vec3::Y * (moment.player.height * tuning.explore.pivot_height);
    let pivot = anchor; // With zero bias, the authored pivot is exactly the player's anchor.
    assert!((pose.eye.distance(pivot) - 1.0).abs() < 1e-3, "{pose:?}, pivot {pivot}");
    assert!(
        frame_use(&pose, &moment.player).max_element() > 1.0,
        "this capped fixture cannot frame the player"
    );
}
