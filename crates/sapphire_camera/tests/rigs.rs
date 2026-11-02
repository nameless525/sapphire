//! What each rig does with look input, and what a shake does to the finished pose.

#![allow(clippy::print_stdout, reason = "with --nocapture, the measurements the crate's docs quote")]

mod common;

use common::{Moment, combat_director, director, fight, lock_on_director, open_ground, run};
use sapphire_camera::{CameraDirector, CameraPose, CameraTuning, Rig};
use sapphire_math::{Vec2, Vec3};

/// The view's yaw: from +Z toward +X.
fn yaw_of(pose: &CameraPose) -> f32 {
    let forward = pose.forward();
    forward.x.atan2(forward.z)
}

/// The view's pitch: above zero looks down.
fn pitch_of(pose: &CameraPose) -> f32 {
    (-pose.forward().y).asin()
}

fn wrapped(angle: f32) -> f32 {
    sapphire_math::det::wrap_angle(angle)
}

/// Look input adds to the yaw and pitch, as `CombatControls::look` gives them: a stick pushed right is a
/// negative yaw, which turns the view right.
#[test]
fn look_input_turns_the_yaw_and_pitch() {
    let world = open_ground();
    let mut camera = director();
    run(&mut camera, &world, 60, (0.0, 0.5), |_| Moment::alone(Vec3::ZERO), |_, _, _| {});

    // A second at 0.5 rad/s of yaw, and 0.1 rad/s of pitch, each way.
    for sign in [-1.0, 1.0] {
        let (yaw, pitch) = (yaw_of(&camera.pose()), pitch_of(&camera.pose()));
        let script = |t: f32| Moment {
            look_rate: if t < 1.5 { Vec2::new(0.5, 0.1) * sign } else { Vec2::ZERO },
            ..Moment::alone(Vec3::ZERO)
        };
        run(&mut camera, &world, 60, (0.5, 2.0), script, |_, _, _| {});
        let pose = camera.pose();
        assert!((wrapped(yaw_of(&pose) - yaw) - 0.5 * sign).abs() < 0.01, "{}", yaw_of(&pose) - yaw);
        assert!((pitch_of(&pose) - pitch - 0.1 * sign).abs() < 0.01, "{}", pitch_of(&pose) - pitch);
    }
}

/// The raw stick as a world direction, as `CombatControls::next_tick` turns it (`world_stick` in
/// `sapphire_controls`).
fn world_stick(raw: Vec2, camera_yaw: f32) -> Vec3 {
    sapphire_math::det::rotate_y(Vec3::new(-raw.x, 0.0, raw.y), camera_yaw)
}

/// What the app hands the mapping is the view it draws: forward walks away from the view and right walks
/// to the picture's right, read from the view matrix itself, as I1's `a_push_goes_where_the_view_goes`
/// does for the sandbox's camera. Every rig, with a wall bending the arm, and with a takeover.
#[test]
fn a_push_goes_where_the_view_goes() {
    let world = common::ground_with(&[(
        sapphire_physics::Shape::Box { half_extents: Vec3::new(30.0, 5.0, 1.0) },
        Vec3::new(0.0, 5.0, -3.5),
    )]);
    let moment = Moment::against(Vec3::ZERO, Vec3::new(5.0, 0.0, 3.0));
    let makers: [fn() -> CameraDirector; 3] = [director, combat_director, lock_on_director];
    for make in makers {
        for yaw in [0.0_f32, 0.35, 1.0, std::f32::consts::FRAC_PI_2, 2.5, -2.0, 40.0] {
            let mut camera = make();
            camera.look_from(yaw, 0.2);
            run(&mut camera, &world, 60, (0.0, 3.0), |_| moment, |_, _, _| {});
            assert_push_follows_the_view(&camera, yaw);
        }
    }

    let mut camera = director();
    let shot = CameraPose {
        eye: Vec3::new(3.0, 2.0, -1.0),
        rotation: sapphire_math::Quat::from_rotation_y(2.2) * sapphire_math::Quat::from_rotation_x(0.3),
        fov_y: 0.6,
    };
    camera.set_takeover(Some(shot));
    run(&mut camera, &world, 60, (0.0, 3.0), |_| moment, |_, _, _| {});
    assert_eq!(camera.pose(), shot);
    assert_push_follows_the_view(&camera, 2.2);
}

fn assert_push_follows_the_view(camera: &CameraDirector, yaw: f32) {
    let view = camera.pose().view();
    let right = view.row(0).truncate();
    let forward = -view.row(2).truncate();
    let ground = Vec3::new(forward.x, 0.0, forward.z).normalize();
    for (raw, want) in [(Vec2::Y, ground), (Vec2::X, right), (Vec2::NEG_Y, -ground)] {
        let got = world_stick(raw, camera.heading());
        assert!(got.distance(want) < 0.001, "{raw} from yaw {yaw}: {got}, the view's {want}");
    }
    let forward_yaw = sapphire_math::det::yaw_of_direction(forward);
    assert!(wrapped(camera.heading() - forward_yaw).abs() < 1e-4, "{} vs {forward_yaw}", camera.heading());
}

/// A shake moves the view and not the stick.
#[test]
fn a_shake_doesnt_move_the_heading() {
    let world = open_ground();
    let (mut quiet, mut shaken) = (director(), director());
    let moment = Moment::alone(Vec3::ZERO);
    let id = shaken.tuning().shake_id("finish").expect("a shipped preset");
    shaken.fire_shake(id, 1.0);
    let mut moved = 0.0_f32;
    for _ in 0..12 {
        let (a, b) = (
            quiet.update(&moment.frame(1.0 / 60.0), &world),
            shaken.update(&moment.frame(1.0 / 60.0), &world),
        );
        moved = moved.max(a.rotation.angle_between(b.rotation));
        assert_eq!(quiet.heading(), shaken.heading());
    }
    assert!(moved > 1e-3, "it shook by {moved} rad");
}

/// Zoom builds up as a log factor on the distance, within its limits, as `CombatControls::camera_zoom`
/// gives it: below zero moves closer.
#[test]
fn zoom_scales_the_distance_within_its_limits() {
    let world = open_ground();
    let tuning = CameraTuning::default_tuning();
    let at_rest = |camera: &mut CameraDirector, zoom: f32| {
        let moment = Moment::alone(Vec3::ZERO);
        for frame in 0..240 {
            let zoom = if frame < 60 { zoom / 60.0 } else { 0.0 };
            let frame = sapphire_camera::CameraFrame { zoom, ..moment.frame(1.0 / 60.0) };
            camera.update(&frame, &world);
        }
        camera.pose().eye.distance(Vec3::Y * 1.7 * tuning.explore.pivot_height)
    };
    let mut camera = director();
    assert!((at_rest(&mut camera, 0.0) - tuning.explore.distance).abs() < 0.01);
    let closer = at_rest(&mut camera, -0.2);
    assert!((closer - tuning.explore.distance * (-0.2_f32).exp()).abs() < 0.01, "{closer}");
    let further = at_rest(&mut camera, 0.2);
    assert!((further - tuning.explore.distance).abs() < 0.01, "{further}");

    let least = at_rest(&mut camera, -9.0);
    assert!((least - tuning.explore.distance * tuning.look.zoom_min.exp()).abs() < 0.01, "{least}");
    let most = at_rest(&mut camera, 9.0);
    assert!((most - tuning.explore.distance * tuning.look.zoom_max.exp()).abs() < 0.01, "{most}");
}

/// Zoom moves combat's preferred distance too, and can't bring it closer than fits both fighters.
#[test]
fn zoom_stays_inside_what_fits_both_fighters() {
    let world = open_ground();
    let moment = Moment::against(Vec3::ZERO, Vec3::new(6.0, 0.0, 8.0));
    let mut camera = combat_director();
    let mut worst = Vec2::ZERO;
    for frame in 1..=300 {
        let zoom = if frame < 60 { -1.0 / 60.0 } else { 0.0 };
        let pose = camera.update(&sapphire_camera::CameraFrame { zoom, ..moment.frame(1.0 / 60.0) }, &world);
        worst = common::frame_use(&pose, &moment.player)
            .max(common::frame_use(&pose, &moment.target.expect("it has one")));
    }
    assert!(worst.max_element() < 0.9, "{worst}");
}

#[test]
fn the_pitch_stays_within_its_limits() {
    let world = open_ground();
    let tuning = CameraTuning::default_tuning();
    for (rate, limit) in [(8.0, tuning.look.pitch_min), (-8.0, tuning.look.pitch_max)] {
        let mut camera = director();
        let script = |_| Moment { look_rate: Vec2::new(0.0, -rate), ..Moment::alone(Vec3::ZERO) };
        let mut last = 0.0;
        run(&mut camera, &world, 60, (0.0, 3.0), script, |_, pose, _| last = pitch_of(pose));
        assert!((last - limit).abs() < 0.02, "{last} against {limit}");
    }
}

/// Recentring turns the view behind the player and levels it, and any input ends it.
#[test]
fn recentring_turns_behind_the_player() {
    let world = open_ground();
    let tuning = CameraTuning::default_tuning();
    let facing = 2.0_f32;
    let moment = Moment { player_yaw: facing, ..Moment::alone(Vec3::ZERO) };
    let mut camera = director();
    camera.look_from(-0.5, 0.9);
    camera.update(&moment.frame(1.0 / 60.0), &world);

    camera.recenter();
    run(&mut camera, &world, 60, (0.0, 2.0), |_| moment, |_, _, _| {});
    let pose = camera.pose();
    assert!(wrapped(yaw_of(&pose) - facing).abs() < 0.03, "{}", yaw_of(&pose));
    assert!((pitch_of(&pose) - tuning.explore.pitch).abs() < 0.03, "{}", pitch_of(&pose));

    // Interrupted by input, it stops where it got to.
    camera.look_from(-0.5, 0.9);
    camera.update(&moment.frame(1.0 / 60.0), &world);
    camera.recenter();
    run(&mut camera, &world, 60, (0.0, 0.05), |_| moment, |_, _, _| {});
    let nudge = Moment { look_rate: Vec2::new(0.1, 0.0), ..moment };
    camera.update(&nudge.frame(1.0 / 60.0), &world);
    let stopped = yaw_of(&camera.pose());
    run(&mut camera, &world, 60, (0.0, 1.0), |_| moment, |_, _, _| {});
    assert!(wrapped(yaw_of(&camera.pose()) - stopped).abs() < 0.3, "it stopped turning");
    assert!(wrapped(yaw_of(&camera.pose()) - facing).abs() > 1.0, "well short of behind them");
}

/// With no input for a while, a moving player gets the view back behind them, and a player standing still
/// only gets the pitch back.
#[test]
fn the_view_drifts_back_when_idle() {
    let world = open_ground();
    let tuning = CameraTuning::default_tuning();
    let facing = 1.0_f32;

    let mut camera = director();
    camera.look_from(-1.0, 0.8);
    let walking = move |t: f32| Moment { player_yaw: facing, ..Moment::alone(Vec3::new(3.0 * t, 0.0, 0.0)) };
    let mut last = camera.pose();
    run(&mut camera, &world, 60, (0.0, 12.0), walking, |_, pose, _| last = *pose);
    assert!(wrapped(yaw_of(&last) - facing).abs() < 0.05, "{}", yaw_of(&last));
    assert!((pitch_of(&last) - tuning.explore.pitch).abs() < 0.05, "{}", pitch_of(&last));

    let mut camera = director();
    camera.look_from(-1.0, 0.8);
    let standing = move |_| Moment { player_yaw: facing, ..Moment::alone(Vec3::ZERO) };
    run(&mut camera, &world, 60, (0.0, 12.0), standing, |_, pose, _| last = *pose);
    assert!(wrapped(yaw_of(&last) + 1.0).abs() < 0.01, "standing still keeps the yaw: {}", yaw_of(&last));
    assert!((pitch_of(&last) - tuning.explore.pitch).abs() < 0.05, "{}", pitch_of(&last));
}

/// Combat leaves look input free while the target stays in frame.
#[test]
fn combat_leaves_look_input_free_inside_the_frame() {
    let world = open_ground();
    let mut camera = combat_director();
    let moment = Moment::against(Vec3::ZERO, Vec3::new(0.0, 0.0, 3.0));
    run(&mut camera, &world, 60, (0.0, 1.0), |_| moment, |_, _, _| {});
    let start = yaw_of(&camera.pose());
    let script =
        |t: f32| Moment { look_rate: if t < 1.3 { Vec2::new(1.0, 0.0) } else { Vec2::ZERO }, ..moment };
    run(&mut camera, &world, 60, (1.0, 2.0), script, |_, _, _| {});
    assert!((wrapped(yaw_of(&camera.pose()) - start) - 0.3).abs() < 0.02, "{}", yaw_of(&camera.pose()));
}

/// Pushing past the limit turns the view back to it.
#[test]
fn combat_turns_back_past_its_limit() {
    let world = open_ground();
    let mut camera = combat_director();
    let far = Moment::against(Vec3::ZERO, Vec3::new(0.0, 0.0, 30.0));
    let script = |t: f32| Moment { look_rate: if t < 2.0 { Vec2::new(2.0, 0.0) } else { Vec2::ZERO }, ..far };
    run(&mut camera, &world, 60, (0.0, 5.0), script, |_, _, _| {});
    let off = wrapped(yaw_of(&camera.pose())).abs();
    println!("settled {off} rad off the line");
    assert!(off < 0.75, "{off} rad off a far target's line");
}

/// Lock-on: input only leans the view, within limits, and it returns when input lets go.
#[test]
fn lock_on_input_only_leans() {
    let world = open_ground();
    let tuning = CameraTuning::default_tuning().lock_on;
    let moment = Moment::against(Vec3::ZERO, Vec3::new(0.0, 0.0, 6.0));
    let mut camera = lock_on_director();
    run(&mut camera, &world, 60, (0.0, 2.0), |_| moment, |_, _, _| {});
    let held = camera.pose();
    assert!((wrapped(yaw_of(&held)).abs() - tuning.side_angle).abs() < 0.02, "{}", yaw_of(&held));

    // Pushed hard one way, as far as it goes and no further: the lean's limit.
    let (mut most, mut pushed) = (0.0_f32, 0.0_f32);
    for rate in [5.0_f32, -5.0] {
        let script = |_| Moment { look_rate: Vec2::new(rate, 0.0), ..moment };
        let start = yaw_of(&camera.pose());
        run(&mut camera, &world, 60, (0.0, 2.0), script, |_, pose, _| {
            pushed = pushed.max(wrapped(yaw_of(pose) - start).abs());
        });
        most = most.max(wrapped(yaw_of(&camera.pose()) - yaw_of(&held)).abs());
    }
    println!("leaned {most} rad, pushed {pushed} rad");
    assert!(most <= tuning.lean_yaw + 0.02, "{most}");
    assert!(most > tuning.lean_yaw * 0.8, "{most}");

    // Let go, it returns.
    run(&mut camera, &world, 60, (0.0, 3.0), |_| moment, |_, _, _| {});
    let back = camera.pose();
    assert!(wrapped(yaw_of(&back) - yaw_of(&held)).abs() < 0.02, "{}", yaw_of(&back));
    assert!((pitch_of(&back) - pitch_of(&held)).abs() < 0.02, "{}", pitch_of(&back));
}

/// Lock-on holds its side as the target goes all the way round the player.
#[test]
fn lock_on_keeps_its_side_as_the_target_circles() {
    let world = open_ground();
    let tuning = CameraTuning::default_tuning().lock_on;
    let radius = 4.0_f32;
    let script = move |t: f32| {
        let angle = 0.7 * t;
        Moment::against(Vec3::ZERO, Vec3::new(radius * angle.sin(), 0.0, radius * angle.cos()))
    };
    let mut camera = lock_on_director();
    let mut first_side = None;
    run(&mut camera, &world, 60, (0.0, 12.0), script, |time, pose, moment| {
        if time < 1.0 {
            return;
        }
        let target = moment.target.expect("it has one").position;
        let line = target.x.atan2(target.z);
        let off = wrapped(yaw_of(pose) - line);
        let side = *first_side.get_or_insert(off.signum());
        assert_eq!(off.signum(), side, "at {time}: {off} off the line");
    });
    assert!(tuning.side_angle > 0.0);
}

/// Letting go of a rig leaves the view where it was: it doesn't swing back to an old yaw.
#[test]
fn letting_go_of_a_rig_leaves_the_view_where_it_was() {
    let world = open_ground();
    let moment = Moment::against(Vec3::ZERO, Vec3::new(9.0, 0.0, 0.0));
    let mut camera = lock_on_director();
    camera.look_from(0.0, 0.2);
    run(&mut camera, &world, 60, (0.0, 3.0), |_| moment, |_, _, _| {});
    let held = yaw_of(&camera.pose());
    camera.set_rig(Rig::LockOn, false);
    camera.set_rig(Rig::Combat, false);
    let alone = Moment::alone(Vec3::ZERO);
    run(&mut camera, &world, 60, (0.0, 2.0), |_| alone, |_, _, _| {});
    assert!(wrapped(yaw_of(&camera.pose()) - held).abs() < 0.02, "{} from {held}", yaw_of(&camera.pose()));
}

/// The base rig can't be turned off, and a takeover can't be started by `set_rig`.
#[test]
fn set_rig_only_moves_the_framing_rigs() {
    let world = open_ground();
    let mut camera = director();
    camera.set_rig(Rig::Explore, false);
    camera.set_rig(Rig::Takeover, true);
    run(&mut camera, &world, 60, (0.0, 1.0), |_| Moment::alone(Vec3::ZERO), |_, _, _| {});
    assert_eq!(camera.weight(Rig::Explore), 1.0);
    assert_eq!(camera.weight(Rig::Takeover), 0.0);

    // A framing rig with no one to frame stays off.
    camera.set_rig(Rig::Combat, true);
    run(&mut camera, &world, 60, (1.0, 2.0), |_| Moment::alone(Vec3::ZERO), |_, _, _| {});
    assert_eq!(camera.weight(Rig::Combat), 0.0);
}

/// The whole run with a shake fired against the same run without.
fn with_shake(setting: f32, fire: bool) -> Vec<CameraPose> {
    let world = open_ground();
    let mut camera = combat_director();
    camera.set_shake_setting(setting);
    let id = camera.tuning().shake_id("finish").expect("a shipped preset");
    let mut poses = Vec::new();
    for frame in 1..=240 {
        if frame == 60 && fire {
            camera.fire_shake(id, 1.0);
        }
        poses.push(camera.update(&fight(frame as f32 / 60.0).frame(1.0 / 60.0), &world));
    }
    poses
}

/// A shake is additive and leaves nothing behind: the run after it ends is bit for bit the run without it.
#[test]
fn a_shake_adds_to_the_pose_and_leaves_nothing_behind() {
    let (plain, shaken) = (with_shake(1.0, false), with_shake(1.0, true));
    assert_eq!(&plain[..59], &shaken[..59], "before it fires");
    let most = plain[60..100]
        .iter()
        .zip(&shaken[60..100])
        .map(|(a, b)| a.rotation.angle_between(b.rotation))
        .fold(0.0_f32, f32::max);
    assert!(most > 0.005, "it shook by {most} rad");
    assert!(most < 0.05, "and stayed within the limit: {most} rad");
    assert_eq!(&plain[100..], &shaken[100..], "after it ends");
}

/// A shake setting of 0 is exactly no shake, for players who need that.
#[test]
fn a_setting_of_zero_is_exactly_no_shake() {
    assert_eq!(with_shake(0.0, true), with_shake(1.0, false));
}

/// The same seed and the same fires shake the same, and another seed doesn't.
#[test]
fn shakes_are_deterministic_given_a_seed() {
    let run_with = |seed: u64| {
        let world = open_ground();
        let mut camera = CameraDirector::new(CameraTuning::default_tuning(), seed).expect("valid");
        let id = camera.tuning().shake_id("hit_heavy").expect("a shipped preset");
        let mut poses = Vec::new();
        for frame in 1..=120 {
            if frame % 20 == 0 {
                camera.fire_shake(id, 1.0);
            }
            poses.push(camera.update(&fight(frame as f32 / 60.0).frame(1.0 / 60.0), &world));
        }
        poses
    };
    assert_eq!(run_with(11), run_with(11));
    assert_ne!(run_with(11), run_with(12));
}

/// A shake's offset can't carry the eye into what it's touching.
#[test]
fn a_shake_cant_push_the_eye_into_a_wall() {
    use sapphire_physics::Shape;
    let world = common::ground_with(&[(
        Shape::Box { half_extents: Vec3::new(30.0, 5.0, 1.0) },
        Vec3::new(0.0, 5.0, -2.5),
    )]);
    let mut camera = director();
    let radius = camera.tuning().collision.radius;
    let id = camera.tuning().shake_id("finish").expect("a shipped preset");
    for frame in 1..=600 {
        if frame % 10 == 0 {
            camera.fire_shake(id, 3.0);
        }
        let pose = camera.update(&Moment::alone(Vec3::ZERO).frame(1.0 / 60.0), &world);
        assert!(common::eye_overlaps(&world, &pose, radius).is_none(), "{frame}: {}", pose.eye);
    }
}

/// Stats add up, and taking them resets them.
#[test]
fn stats_count_the_work() {
    let world = open_ground();
    let mut camera = combat_director();
    run(&mut camera, &world, 60, (0.0, 1.0), fight, |_, _, _| {});
    let stats = camera.take_stats();
    assert_eq!(stats.frames, 60);
    assert_eq!(stats.cuts, 1);
    assert!(stats.casts >= 60, "{stats:?}");
    assert_eq!(camera.take_stats().frames, 0);
}
