//! The camera's clock and smoothing: the same motion at any frame rate, no overshoot, huge frames that
//! don't explode, and what hitstop, slow motion and a pause each do.

#![allow(clippy::print_stdout, reason = "with --nocapture, the measurements the crate's docs quote")]

mod common;

use common::{
    ASPECT, Moment, RATES, combat_director, director, fight, lock_on_director, open_ground, run,
    sweeping_look,
};
use sapphire_camera::{CameraFrame, CameraPose};
use sapphire_math::{Quat, Vec2, Vec3};

/// The run's poses at whole 1/30 s, which all these rates land on.
fn poses(
    mut camera: sapphire_camera::CameraDirector,
    rate: u32,
    seconds: f32,
    script: impl Fn(f32) -> Moment,
) -> Vec<CameraPose> {
    let world = open_ground();
    let mut out = Vec::new();
    let step = rate / 30;
    let mut frame = 0;
    run(&mut camera, &world, rate, (0.0, seconds), script, |_, pose, _| {
        frame += 1;
        if frame % step == 0 {
            out.push(*pose);
        }
    });
    out
}

fn angle_between(a: Quat, b: Quat) -> f32 {
    2.0 * a.dot(b).abs().min(1.0).acos()
}

/// How far two runs drift apart, at most anywhere along the run and at its end.
fn compare(a: &[CameraPose], b: &[CameraPose]) -> ((f32, f32), (f32, f32)) {
    assert_eq!(a.len(), b.len());
    let mut most = (0.0_f32, 0.0_f32);
    for (a, b) in a.iter().zip(b) {
        most.0 = most.0.max(a.eye.distance(b.eye));
        most.1 = most.1.max(angle_between(a.rotation, b.rotation));
    }
    let (end_a, end_b) = (a[a.len() - 1], b[b.len() - 1]);
    (most, (end_a.eye.distance(end_b.eye), angle_between(end_a.rotation, end_b.rotation)))
}

/// How far a run at 30 or 60 Hz may be from one at 120 Hz, in metres and radians, anywhere along the run.
#[derive(Clone, Copy)]
struct Along {
    eye: f32,
    turn: f32,
}

/// Where a run ends, settled, every rig agrees to within these.
const END_EYE: f32 = 0.02;
const END_TURN: f32 = 0.004;

fn assert_rates_agree(
    what: &str,
    make: impl Fn() -> sapphire_camera::CameraDirector,
    seconds: f32,
    allowed: Along,
    script: impl Fn(f32) -> Moment + Copy,
) {
    let reference = poses(make(), 120, seconds, script);
    for rate in [30, 60] {
        let (along, end) = compare(&poses(make(), rate, seconds, script), &reference);
        println!("{what}, {rate} against 120 Hz: along the run {along:?}, at the end {end:?}");
        assert!(
            along.0 <= allowed.eye && along.1 <= allowed.turn,
            "{what} at {rate} Hz, along the run: {along:?}"
        );
        assert!(end.0 <= END_EYE && end.1 <= END_TURN, "{what} at {rate} Hz, at the end: {end:?}");
    }
}

/// The fight, then fighters at rest, so the end is settled and the middle is the worst of it.
fn fight_then_rest(t: f32) -> Moment {
    fight(t.min(common::FIGHT_SECONDS - 12.0))
}

/// 30, 60 and 120 Hz runs of the same motion end together, and take the same path. Lock-on's path allows the
/// most, about half of a 30 Hz frame's turn: while the view runs at its speed limit after an enemy sprinting
/// round the player, the moment it comes off the limit falls on a frame's edge.
#[test]
fn thirty_sixty_and_120_hz_runs_end_together() {
    let seconds = common::FIGHT_SECONDS - 8.0;
    assert_rates_agree("explore", director, seconds, Along { eye: 0.07, turn: 0.012 }, fight_then_rest);
    assert_rates_agree("combat", combat_director, seconds, Along { eye: 0.12, turn: 0.004 }, fight_then_rest);
    assert_rates_agree(
        "lock-on",
        lock_on_director,
        seconds,
        Along { eye: 0.45, turn: 0.055 },
        fight_then_rest,
    );
}

/// Look input as a rate gives the same turn at every frame rate.
#[test]
fn look_input_turns_the_same_at_any_rate() {
    // The rate holds for each 1/30 s, so every frame rate adds up the same input.
    let script = |t: f32| {
        let held = (t * 30.0 - 1e-3).ceil() / 30.0;
        Moment {
            look_rate: if held < 6.0 { sweeping_look(held) } else { Vec2::ZERO },
            ..Moment::alone(Vec3::new(0.0, 0.0, 2.0))
        }
    };
    assert_rates_agree("look input", director, 7.0, Along { eye: 0.04, turn: 0.008 }, script);
}

/// A player running at a steady speed through uneven frames: the view holds one offset from them, where
/// a spring that doesn't know the target's moving would make it wander with each frame's length.
#[test]
fn uneven_frames_dont_judder() {
    let world = open_ground();
    let mut camera = director();
    let pattern = [1.0_f32 / 240.0, 1.0 / 30.0, 1.0 / 60.0, 1.0 / 20.0, 1.0 / 120.0, 1.0 / 60.0];
    let mut now = 0.0_f32;
    let (mut low, mut high) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let speed = Vec3::new(1.5, 0.0, 5.0);
    for frame in 0..600 {
        let dt = pattern[frame % pattern.len()];
        now += dt;
        let player = speed * now;
        let pose = camera.update(&Moment::alone(player).frame(dt), &world);
        if now > 2.0 {
            let offset = pose.eye - player;
            low = low.min(offset);
            high = high.max(offset);
        }
    }
    let wander = (high - low).length();
    println!("wandered {wander} m");
    assert!(wander < 0.002, "{low} to {high}");
}

/// A player stopping dead: the eye arrives without passing its final place.
#[test]
fn stopping_doesnt_overshoot() {
    let world = open_ground();
    for rate in RATES {
        let mut camera = director();
        let stop = 3.0_f32;
        let script = |t: f32| Moment::alone(Vec3::new(0.0, 0.0, 6.0 * t.min(stop)));
        let mut end = 0.0;
        // Find the final place first.
        run(&mut camera, &world, rate, (0.0, 8.0), script, |_, pose, _| end = pose.eye.z);

        let mut camera = director();
        let mut last = f32::MIN;
        run(&mut camera, &world, rate, (0.0, 8.0), script, |time, pose, _| {
            assert!(pose.eye.z <= end + 1e-3, "at {rate} Hz, {time}: {} overshot {end}", pose.eye.z);
            assert!(pose.eye.z >= last - 1e-3, "at {rate} Hz, {time}: {} fell back from {last}", pose.eye.z);
            last = pose.eye.z;
        });
    }
}

/// A framing rig blending in to a shot further out: the eye backs off to its goal, and overshoots it by no
/// more than the 5 cm that looking ahead for a moving pivot can add.
#[test]
fn a_rig_blending_in_doesnt_overshoot() {
    let world = open_ground();
    let mut camera = director();
    let far = Moment::against(Vec3::ZERO, Vec3::new(0.0, 0.0, 14.0));
    let mut end = 0.0;
    camera.set_rig(sapphire_camera::Rig::Combat, true);
    run(&mut camera, &world, 60, (0.0, 8.0), |_| far, |_, pose, _| end = pose.eye.z);

    let mut camera = director();
    run(&mut camera, &world, 60, (0.0, 1.0), |_| far, |_, _, _| {});
    camera.set_rig(sapphire_camera::Rig::Combat, true);
    let mut settled_from = camera.pose().eye.z;
    run(
        &mut camera,
        &world,
        60,
        (1.0, 9.0),
        |_| far,
        |time, pose, _| {
            assert!(pose.eye.z >= end - 0.05, "{time}: {} passed {end}", pose.eye.z);
            settled_from = pose.eye.z;
        },
    );
    assert!((settled_from - end).abs() < 0.01, "{settled_from} vs {end}");
}

/// Frames of a second, an hour and more, and vanishing ones, in a fight: nothing explodes, and a long
/// frame lands where a settled camera stands.
#[test]
fn huge_frame_times_dont_explode() {
    let world = open_ground();
    for step in [1e-6_f32, 0.25, 1.0, 1e3, 1e9, f32::MAX] {
        let mut camera = combat_director();
        run(&mut camera, &world, 60, (0.0, 2.0), fight, |_, _, _| {});
        let moment = fight(2.5);
        for _ in 0..3 {
            let pose = camera.update(&moment.frame(step), &world);
            assert!(
                pose.eye.is_finite() && pose.rotation.is_finite() && pose.fov_y.is_finite(),
                "{step}: {pose:?}"
            );
            assert!(pose.rotation.is_normalized(), "{step}: {pose:?}");
            assert!(pose.eye.distance(moment.player.position) < 30.0, "{step}: {pose:?}");
        }
        // And on the next ordinary frame it's where it should be, with no kick.
        let before = camera.pose();
        let after = camera.update(&moment.frame(1.0 / 60.0), &world);
        if step >= 1.0 {
            assert!(
                after.eye.distance(before.eye) < 0.01,
                "{step}: kicked from {} to {}",
                before.eye,
                after.eye
            );
        }
    }
    // Nonsense in, the same out.
    let mut camera = combat_director();
    let good = fight(0.0).frame(1.0 / 60.0);
    let held = camera.update(&good, &world);
    let bad =
        CameraFrame { dt: f32::NAN, look: Vec2::splat(f32::NAN), player_yaw: f32::NAN, aspect: 0.0, ..good };
    assert_eq!(camera.update(&bad, &world), held, "a NaN step holds");
    let bad = CameraFrame {
        dt: 1.0 / 60.0,
        player: common::player_at(Vec3::NAN),
        target: Some(common::enemy_at(Vec3::NAN)),
        ..bad
    };
    let pose = camera.update(&bad, &world);
    assert!(pose.eye.is_finite() && pose.rotation.is_finite(), "{pose:?}");
}

/// The script both clock tests watch: a player running, who stops at 2 s.
fn run_and_stop(t: f32) -> Moment {
    Moment::alone(Vec3::new(0.0, 0.0, 5.0 * t.min(2.0)))
}

/// Hitstop: the fighters' clocks stop and the frame's dt doesn't. The view keeps settling and shakes keep
/// playing through the freeze, which is what makes it read alive.
#[test]
fn a_hitstop_doesnt_stop_the_cameras_smoothing() {
    let world = open_ground();
    let mut camera = director();
    run(&mut camera, &world, 60, (0.0, 2.0), run_and_stop, |_, _, _| {});
    let frozen = run_and_stop(2.0);
    let before = camera.pose();
    let mut moved = 0.0;
    for _ in 0..6 {
        let pose = camera.update(&frozen.frame(1.0 / 60.0), &world);
        moved = pose.eye.distance(before.eye);
    }
    assert!(moved > 0.05, "the view settled by {moved} m in a 6-frame freeze");

    let id = camera.tuning().shake_id("hit_heavy").expect("a shipped preset");
    camera.fire_shake(id, 1.0);
    let quiet = camera.pose();
    let mut shaken = 0.0_f32;
    for _ in 0..6 {
        let pose = camera.update(&frozen.frame(1.0 / 60.0), &world);
        shaken = shaken.max(angle_between(pose.rotation, quiet.rotation));
    }
    assert!(shaken > 1e-3, "a shake plays through a freeze: {shaken} rad");
}

/// Slow motion scales the fighters and not the camera: the same run at a fifth speed trails by a fifth.
#[test]
fn slow_motion_doesnt_slow_the_camera() {
    let world = open_ground();
    let lag_at = |slowed: f32| {
        let mut camera = director();
        let script = move |t: f32| Moment::alone(Vec3::new(0.0, 0.0, 5.0 * slowed * t));
        let mut lag = 0.0;
        run(&mut camera, &world, 60, (0.0, 4.0), script, |time, pose, _| {
            lag = 5.0 * slowed * time - (pose.eye.z + camera_distance());
        });
        lag
    };
    let (full, slowed) = (lag_at(1.0), lag_at(0.2));
    println!("lag {full} m at full speed, {slowed} m at a fifth");
    assert!((slowed - full * 0.2).abs() < 0.01, "{full} and {slowed}");
}

/// How far back the shipped explore rig stands across the ground.
fn camera_distance() -> f32 {
    let tuning = sapphire_camera::CameraTuning::default_tuning();
    tuning.explore.distance * tuning.explore.pitch.cos()
}

/// A pause holds the pose exactly, shakes included, and when it ends the run carries on as if it had never
/// happened.
#[test]
fn a_pause_holds_everything() {
    let world = open_ground();
    let id = sapphire_camera::CameraTuning::default_tuning().shake_id("hit_heavy").expect("a shipped preset");
    let script = |t: f32| Moment { look_rate: sweeping_look(t), ..fight(t) };
    let frames = 120;

    let mut straight = combat_director();
    let mut paused = combat_director();
    for frame in 1..=frames {
        let t = frame as f32 / 60.0;
        let moment = script(t);
        if frame == 30 {
            straight.fire_shake(id, 1.0);
            paused.fire_shake(id, 1.0);
        }
        if frame == 40 {
            let held = paused.pose();
            for _ in 0..50 {
                // Look input and a moving world don't get through a pause either.
                let frozen = Moment { look_rate: Vec2::splat(3.0), ..script(t + 0.5) };
                assert_eq!(paused.update(&frozen.frame(0.0), &world), held);
            }
        }
        let frame = moment.frame(1.0 / 60.0);
        let (a, b) = (straight.update(&frame, &world), paused.update(&frame, &world));
        assert_eq!(a, b, "at {t} s");
    }
}

/// The aspect is what the framing solves for: with the fighters side by side, a square window stands
/// further back than a wide one.
#[test]
fn the_aspect_reaches_the_framing() {
    let world = open_ground();
    let distance_at = |aspect: f32| {
        let mut camera = combat_director();
        camera.look_from(1.2, 0.2);
        let moment = Moment::against(Vec3::ZERO, Vec3::new(0.0, 0.0, 6.0));
        let mut eye = Vec3::ZERO;
        for _ in 0..300 {
            eye = camera.update(&CameraFrame { aspect, ..moment.frame(1.0 / 60.0) }, &world).eye;
        }
        eye.distance(Vec3::new(0.0, 1.4, 2.1))
    };
    let (square, wide) = (distance_at(1.0), distance_at(ASPECT));
    println!("square {square}, wide {wide}");
    assert!(square > wide + 0.5, "{square} vs {wide}");
}
