//! Pillar 5: the camera never clips. The eye's sphere is held free of every collider through walks,
//! look input, pillars, walls and corners, and giving way to them doesn't pump or snap.

#![allow(clippy::print_stdout, reason = "with --nocapture, the measurements the crate's docs quote")]

mod common;

use common::{
    Moment, RATES, arena, combat_director, director, eye_overlaps, ground_with, lock_on_director, run,
    sweeping_look, walk_through,
};
use sapphire_camera::{CameraDirector, CameraPose, CameraTuning, Rig};
use sapphire_math::{Quat, Vec2, Vec3};
use sapphire_physics::{PhysicsWorld, Shape};

const WALK_SPEED: f32 = 3.2;
/// A tour of the committed arena's colliders: around the pillar, past the crate and the ball, and up and
/// back down the ramp.
const WAYPOINTS: [Vec2; 17] = [
    Vec2::new(0.0, 0.0),
    Vec2::new(-2.6, 0.0),
    Vec2::new(-4.0, 1.4),
    Vec2::new(-5.4, 0.0),
    Vec2::new(-4.0, -1.4),
    Vec2::new(-2.6, 0.0),
    Vec2::new(1.0, 1.0),
    Vec2::new(2.0, 3.5),
    Vec2::new(4.0, 3.5),
    Vec2::new(4.5, 1.0),
    Vec2::new(1.5, -3.0),
    Vec2::new(-1.5, -5.0),
    Vec2::new(0.0, -6.8),
    Vec2::new(1.5, -5.5),
    Vec2::new(4.5, -6.0),
    Vec2::new(8.5, -6.0),
    Vec2::new(0.0, 0.0),
];
const TOUR_SECONDS: f32 = 26.0;

/// The player on the tour at `t`, up the ramp where the path meets it.
fn on_tour(t: f32) -> Vec3 {
    let at = walk_through(&WAYPOINTS, WALK_SPEED, t);
    let on_ramp = (5.0..=9.0).contains(&at.x) && (-7.0..=-5.0).contains(&at.y);
    Vec3::new(at.x, if on_ramp { (at.x - 5.0) / 4.0 } else { 0.0 }, at.y)
}

/// The tour with the view swept about, and an enemy nearby if there's a framing rig to frame it.
fn tour(t: f32) -> Moment {
    let player = on_tour(t);
    let angle = 0.9 * t;
    let enemy = player + Vec3::new(4.0 * angle.sin(), 0.0, 4.0 * angle.cos());
    Moment { look_rate: sweeping_look(t), ..Moment::against(player, enemy) }
}

/// Runs the tour and fails on any frame whose eye sphere starts inside a collider.
fn assert_tour_stays_clear(
    mut camera: CameraDirector,
    rate: u32,
    what: &str,
    script: impl Fn(f32) -> Moment,
) {
    let world = arena();
    let radius = camera.tuning().collision.radius;
    run(&mut camera, &world, rate, (0.0, TOUR_SECONDS), script, |time, pose, _| {
        let overlap = eye_overlaps(&world, pose, radius);
        assert!(overlap.is_none(), "{what} at {rate} Hz, {time} s: {:?} in by {overlap:?}", pose.eye);
    });
    let stats = camera.take_stats();
    println!("{what} at {rate} Hz: {stats:?}");
    assert!(
        stats.contacts + stats.pulled_frames + stats.thin_frames > 0,
        "{what}: the tour must meet something"
    );
    assert_eq!(stats.hops, 0, "{what} at {rate} Hz");
}

#[test]
fn a_walk_around_the_arena_never_starts_inside_a_collider() {
    for rate in RATES {
        assert_tour_stays_clear(director(), rate, "explore", tour);
        assert_tour_stays_clear(combat_director(), rate, "combat", tour);
        assert_tour_stays_clear(lock_on_director(), rate, "lock-on", tour);
    }
}

/// A camera looking up hard from below the player's head, so the arm presses into the ground, and a lens
/// of a thousand frames a second wouldn't help it through.
#[test]
fn looking_up_keeps_the_eye_above_the_ground() {
    let script = |t: f32| Moment {
        look_rate: Vec2::new(0.4, if t < 2.0 { -1.0 } else { 0.0 }),
        ..Moment::alone(on_tour(t))
    };
    for rate in RATES {
        assert_tour_stays_clear(director(), rate, "looking up", script);
    }
}

fn pillar_world() -> PhysicsWorld {
    // Between a camera at yaw 0 and a player at the origin, at about the arm's middle.
    ground_with(&[(Shape::Capsule { half_height: 2.0, radius: 0.35 }, Vec3::new(0.0, 2.35, -2.5))])
}

/// The eye's distance from where the arm starts on a player at the origin.
fn arm_length(pose: &CameraPose) -> f32 {
    let anchor = Vec3::Y * 1.7 * CameraTuning::default_tuning().explore.pivot_height;
    pose.eye.distance(anchor)
}

/// How many times a list of lengths changes direction, ignoring a move under `slack`.
fn reversals(lengths: &[f32], slack: f32) -> u32 {
    let (mut count, mut heading, mut anchor) = (0, 0.0_f32, lengths[0]);
    for &length in lengths {
        let moved = length - anchor;
        if moved.abs() > slack {
            if heading != 0.0 && moved.signum() != heading {
                count += 1;
            }
            heading = moved.signum();
            anchor = length;
        }
    }
    count
}

/// The line swinging back and forth over a thin pillar, the worst case for snapping: it stays out behind
/// the pillar the whole time, and doesn't breathe.
#[test]
fn a_thin_pillar_crossing_the_line_doesnt_snap_back_and_forth() {
    let world = pillar_world();
    let script = |t: f32| Moment {
        // Across the pillar and back, about every 2 s: 0.6 rad each way.
        look_rate: Vec2::new(0.96 * (1.6 * t).sin(), 0.0),
        ..Moment::alone(Vec3::ZERO)
    };
    for rate in RATES {
        let mut camera = director();
        camera.look_from(-0.6, 0.2);
        let mut lengths = Vec::new();
        let mut top_step = 0.0_f32;
        let mut last_eye = None;
        run(&mut camera, &world, rate, (0.0, 12.0), script, |_, pose, _| {
            if let Some(last) = last_eye {
                top_step = top_step.max(pose.eye.distance(last) * rate as f32);
            }
            last_eye = Some(pose.eye);
            lengths.push(arm_length(pose));
        });
        let stats = camera.take_stats();
        let (short, long) = lengths[30..].iter().fold((f32::MAX, 0.0_f32), |(a, b), &l| (a.min(l), b.max(l)));
        println!("{rate} Hz: arm {short}..{long}, eye up to {top_step} m/s, {stats:?}");
        assert!(stats.thin_frames > 0, "the line must have crossed the pillar");
        assert_eq!(stats.pulled_frames, 0, "at {rate} Hz it stays out");
        assert!(long - short < 0.02, "at {rate} Hz the arm moved {short}..{long}");
        assert!(top_step < 6.0, "at {rate} Hz the eye reached {top_step} m/s");
    }
}

/// The eye's own orbit runs through the pillar: the arm comes in and goes round it, and no frame jumps.
#[test]
fn an_orbit_through_a_pillar_goes_around_it() {
    let world = pillar_world();
    // The arm is 4.5 m, so standing 2 m past the pillar's centre puts the eye's orbit through it.
    let start = Vec3::new(0.0, 0.0, 2.0);
    let script = move |t: f32| Moment {
        // From 1 rad off to one side, through the pillar, to the other, and back.
        look_rate: Vec2::new(0.5 * (0.5 * t).sin(), 0.0),
        ..Moment::alone(start)
    };
    for rate in RATES {
        let mut camera = director();
        camera.look_from(-1.0, 0.2);
        let radius = camera.tuning().collision.radius;
        let mut top_step = 0.0_f32;
        let mut last_eye = None;
        run(&mut camera, &world, rate, (0.0, 16.0), script, |time, pose, _| {
            assert!(eye_overlaps(&world, pose, radius).is_none(), "{time}: {:?}", pose.eye);
            if let Some(last) = last_eye {
                top_step = top_step.max(pose.eye.distance(last) * rate as f32);
            }
            last_eye = Some(pose.eye);
        });
        let stats = camera.take_stats();
        println!("{rate} Hz: eye up to {top_step} m/s, {stats:?}");
        assert!(stats.pulled_frames > 0, "the orbit must have met the pillar");
        assert_eq!(stats.hops, 0);
        assert!(top_step < 8.0, "at {rate} Hz the eye reached {top_step} m/s");
    }
}

/// A pillar that keeps hiding the player: after its patience the arm comes in front of it, without a hop,
/// and stays there until the line clears, then eases out once.
#[test]
fn a_pillar_that_stays_in_the_way_is_rounded_once() {
    let world = pillar_world();
    let script = |t: f32| Moment {
        // Turn onto the pillar, hold for 6 s, then turn off it and hold.
        look_rate: Vec2::new(if t < 1.0 || (7.0..8.0).contains(&t) { 0.8 } else { 0.0 }, 0.0),
        ..Moment::alone(Vec3::ZERO)
    };
    let mut camera = director();
    camera.look_from(-0.8, 0.2);
    let radius = camera.tuning().collision.radius;
    let mut lengths = Vec::new();
    let mut top_step = 0.0_f32;
    let mut last_eye = None;
    run(&mut camera, &world, 60, (0.0, 14.0), script, |time, pose, _| {
        assert!(eye_overlaps(&world, pose, radius).is_none(), "{time}: {:?}", pose.eye);
        if let Some(last) = last_eye {
            top_step = top_step.max(pose.eye.distance(last) * 60.0);
        }
        last_eye = Some(pose.eye);
        lengths.push(arm_length(pose));
    });
    let stats = camera.take_stats();
    let shortest = lengths.iter().copied().fold(f32::MAX, f32::min);
    let end = lengths[lengths.len() - 1];
    println!("arm down to {shortest}, ending {end}, eye up to {top_step} m/s, {stats:?}");
    assert!(shortest < 2.5, "it came in front of the pillar: {shortest}");
    assert!(end > 4.4, "and went back out: {end}");
    assert_eq!(stats.hops, 0, "rounding it needs no jump");
    assert!(reversals(&lengths, 0.05) <= 1, "in once, out once");
    assert!(top_step < 12.0, "the eye reached {top_step} m/s");
}

/// Turning into a long wall: the arm comes in before the eye gets there, smoothly, and goes back out
/// more slowly once the wall's out of the way.
#[test]
fn a_wall_pulls_the_arm_in_smoothly_and_it_eases_out_slower() {
    // The wall's face is at z = -1.5; a player at the origin has 1.5 m behind them.
    let world =
        ground_with(&[(Shape::Box { half_extents: Vec3::new(30.0, 5.0, 1.0) }, Vec3::new(0.0, 5.0, -2.5))]);
    // Start looking along +X, with the eye clear of the wall, turn to face away from it (the eye at the
    // wall), hold, and turn back.
    let script = |t: f32| Moment {
        look_rate: Vec2::new(
            if t < 1.0 {
                0.0
            } else if t < 2.57 {
                -1.0
            } else if (4.0..5.57).contains(&t) {
                1.0
            } else {
                0.0
            },
            0.0,
        ),
        ..Moment::alone(Vec3::ZERO)
    };
    for rate in RATES {
        let mut camera = director();
        camera.look_from(std::f32::consts::FRAC_PI_2, 0.2);
        let radius = camera.tuning().collision.radius;
        let mut lengths = Vec::new();
        let mut times = Vec::new();
        let mut top_change = 0.0_f32;
        run(&mut camera, &world, rate, (0.0, 12.0), script, |time, pose, _| {
            assert!(eye_overlaps(&world, pose, radius).is_none(), "{time}: {:?}", pose.eye);
            let length = arm_length(pose);
            if let Some(last) = lengths.last() {
                let last: f32 = *last;
                top_change = top_change.max((length - last).abs() * rate as f32);
            }
            lengths.push(length);
            times.push(time);
        });
        let stats = camera.take_stats();
        let full = camera.tuning().explore.distance;
        let shortest = lengths.iter().copied().fold(f32::MAX, f32::min);
        let halfway = (full + shortest) * 0.5;
        // Time from starting to come in to passing halfway, against the same going out.
        let lowest = lengths.iter().position(|&l| l == shortest).expect("it's in there");
        let started_in = lengths.iter().position(|&l| l < full - 0.05).expect("it came in");
        let half_in = lengths.iter().position(|&l| l < halfway).expect("it came in");
        let after_turn = times.iter().position(|&t| t > 4.0).expect("the run is longer");
        let half_out = lowest.max(after_turn)
            + lengths[lowest.max(after_turn)..].iter().position(|&l| l > halfway).expect("it went out");
        let started_out = lowest.max(after_turn)
            + lengths[lowest.max(after_turn)..]
                .iter()
                .position(|&l| l > shortest + 0.05)
                .expect("it went out");
        let (going_in, going_out) =
            (times[half_in] - times[started_in], times[half_out] - times[started_out]);
        println!(
            "{rate} Hz: arm {shortest}..{full}, {top_change} m/s at most, in {going_in} s, out {going_out} s, {stats:?}"
        );
        assert!(shortest < 2.2, "at {rate} Hz the arm stayed at {shortest}");
        assert_eq!(stats.hops, 0, "at {rate} Hz");
        assert!(top_change < camera.tuning().collision.max_pull_speed + 2.0, "at {rate} Hz: {top_change}");
        assert!(lengths[lengths.len() - 1] > full - 0.05, "at {rate} Hz it ends back out");
        assert!(reversals(&lengths, 0.05) <= 1, "at {rate} Hz: in once, out once");
        assert!(going_out > going_in, "at {rate} Hz it went in in {going_in} s and out in {going_out} s");
    }
}

/// Walking past the corner of a thick building: the line meets the corner all at once, nearer the
/// player than the eye. Looking ahead brings the arm in first, so it takes no jump.
#[test]
fn a_corner_is_rounded_without_a_jump() {
    // A block filling x from -21 to -1 and z from -20 to 0.
    let world = ground_with(&[(
        Shape::Box { half_extents: Vec3::new(10.0, 5.0, 10.0) },
        Vec3::new(-11.0, 5.0, -10.0),
    )]);
    // Past the corner on its +X side, along +Z, then left along its +Z side.
    let path = [Vec2::new(0.0, -6.0), Vec2::new(0.0, 2.5), Vec2::new(-9.0, 2.5)];
    for speed in [2.0_f32, 4.0, 8.0] {
        let script = move |t: f32| {
            let at = walk_through(&path, speed, t);
            Moment::alone(Vec3::new(at.x, 0.0, at.y))
        };
        for rate in RATES {
            let mut camera = director();
            let radius = camera.tuning().collision.radius;
            let mut top_step = 0.0_f32;
            let mut last_eye = None;
            let seconds = 2.0 + 17.5 / speed;
            run(&mut camera, &world, rate, (0.0, seconds), script, |time, pose, _| {
                assert!(eye_overlaps(&world, pose, radius).is_none(), "{time}: {:?}", pose.eye);
                if let Some(last) = last_eye {
                    top_step = top_step.max(pose.eye.distance(last) * rate as f32);
                }
                last_eye = Some(pose.eye);
            });
            let stats = camera.take_stats();
            println!("{speed} m/s at {rate} Hz: eye up to {top_step} m/s, {stats:?}");
            assert!(stats.pulled_frames > 0, "the corner must have mattered");
            assert_eq!(stats.hops, 0);
            assert!(top_step < speed + 12.0, "at {speed} m/s, {rate} Hz: eye {top_step} m/s");
        }
    }
}

/// Shut behind a thick wall, with no way round: it counts as stuck, then hops to the player's side. The
/// one jump there is, it's counted and lands clear.
#[test]
fn the_eye_shut_behind_a_wall_hops_to_the_players_side() {
    let world =
        ground_with(&[(Shape::Box { half_extents: Vec3::new(100.0, 8.0, 2.0) }, Vec3::new(0.0, 8.0, 0.0))]);
    // The player crosses the wall in one frame, as a teleport with no cut would.
    let script = |t: f32| Moment::alone(Vec3::new(0.0, 0.0, if t < 1.0 { -6.0 } else { 6.0 }));
    for rate in RATES {
        let mut camera = director();
        let radius = camera.tuning().collision.radius;
        let mut end = None;
        run(&mut camera, &world, rate, (0.0, 4.0), script, |time, pose, _| {
            assert!(eye_overlaps(&world, pose, radius).is_none(), "{time}: {:?}", pose.eye);
            end = Some(*pose);
        });
        let stats = camera.take_stats();
        let end = end.expect("it ran");
        println!("{rate} Hz: ends at {} with {stats:?}", end.eye);
        assert_eq!(stats.hops, 1, "at {rate} Hz");
        assert!(end.eye.z > 2.0, "at {rate} Hz it ends on the player's side: {}", end.eye);
    }
}

/// A cut lands at once where the rig wants it, stopped, or short of what's in the way.
#[test]
fn a_cut_lands_clear_and_at_rest() {
    let wall = (Shape::Box { half_extents: Vec3::new(30.0, 5.0, 1.0) }, Vec3::new(0.0, 5.0, -2.5));
    let world = ground_with(&[wall]);
    let mut camera = director();
    let radius = camera.tuning().collision.radius;
    let far = Moment::alone(Vec3::new(0.0, 0.0, 40.0));
    camera.update(&far.frame(1.0 / 60.0), &world);
    camera.update(&far.frame(1.0 / 60.0), &world);

    // To just in front of the wall, facing away from it.
    let near = Moment::alone(Vec3::ZERO);
    camera.cut();
    let first = camera.update(&near.frame(1.0 / 60.0), &world);
    assert!(eye_overlaps(&world, &first, radius).is_none());
    assert!(first.eye.z > -1.5, "in front of the wall: {}", first.eye);
    for _ in 0..30 {
        let later = camera.update(&near.frame(1.0 / 60.0), &world);
        assert!(later.eye.distance(first.eye) < 1e-3, "{} from {}", later.eye, first.eye);
    }

    // A fresh director's first frame is a cut too, and agrees.
    let mut fresh = director();
    let same = fresh.update(&near.frame(1.0 / 60.0), &world);
    assert!(same.eye.distance(first.eye) < 1e-3, "{} vs {}", same.eye, first.eye);
}

/// An authored shot, its eye at `eye`.
fn shot_from(eye: Vec3) -> CameraPose {
    CameraPose { eye, rotation: Quat::from_rotation_y(0.6) * Quat::from_rotation_x(0.2), fov_y: 0.6 }
}

/// A wall along the view, 2 m to the side of a player at the origin, with the eye on the player's side.
fn wall_beside() -> PhysicsWorld {
    ground_with(&[(Shape::Box { half_extents: Vec3::new(0.2, 5.0, 30.0) }, Vec3::new(2.0, 5.0, 0.0))])
}

/// Past the wall from the eye, and clear.
const BEYOND_THE_WALL: Vec3 = Vec3::new(4.0, 2.0, -3.0);

/// How fast a takeover's eye may move, in metres a second, at most: what a rig really changing is held to
/// (`tests/jerk.rs`).
const TAKEOVER_SPEED: f32 = 40.0;

/// Runs `script` at 60 Hz, failing on any frame with the eye's sphere in a collider, and gives back how
/// many times it hopped and how fast it moved at most, in metres a second, on the other frames.
fn clear_through(
    camera: &mut CameraDirector,
    world: &PhysicsWorld,
    what: &str,
    times: (f32, f32),
    script: impl Fn(f32) -> Moment,
) -> (u32, f32) {
    let radius = camera.tuning().collision.radius;
    let mut steps = Vec::new();
    let mut last = camera.pose().eye;
    run(camera, world, 60, times, script, |time, pose, _| {
        let overlap = eye_overlaps(world, pose, radius);
        assert!(overlap.is_none(), "{what} at {time} s: {:?} in by {overlap:?}", pose.eye);
        steps.push(pose.eye.distance(last));
        last = pose.eye;
    });
    steps.sort_by(f32::total_cmp);
    let hops = camera.take_stats().hops;
    (hops, steps[steps.len() - 1 - hops as usize] * 60.0)
}

/// A shot whose eye stands in a wall is drawn from the nearest place the eye's sphere fits, as its author
/// has it otherwise: a shot's rotation and field of view are its own, and a cut lands there at once.
#[test]
fn a_takeover_inside_a_wall_draws_from_outside_it() {
    let inside = Vec3::new(5.0, 2.0, 0.0);
    let world = ground_with(&[(Shape::Box { half_extents: Vec3::ONE }, inside)]);
    let mut camera = director();
    let radius = camera.tuning().collision.radius;
    let shot = shot_from(inside);
    camera.update(&Moment::alone(Vec3::ZERO).frame(1.0 / 60.0), &world);
    camera.set_takeover(Some(shot));
    camera.cut();
    let first = camera.update(&Moment::alone(Vec3::ZERO).frame(1.0 / 60.0), &world);
    assert!(eye_overlaps(&world, &first, radius).is_none(), "{:?}", first.eye);
    assert!(first.eye.distance(inside) < 1.0 + radius + 0.01, "the nearest way out: {:?}", first.eye);
    assert_eq!((first.rotation, first.fov_y), (shot.rotation, shot.fov_y));
    clear_through(&mut camera, &world, "held in a wall", (0.0, 1.0), |_| Moment::alone(Vec3::ZERO));
    assert_eq!(camera.pose().eye, first.eye, "and it stays there");
}

/// In where two walls meet, one push isn't always out of everything.
#[test]
fn a_takeover_in_a_corner_gets_out_of_both_walls() {
    let world = ground_with(&[
        (Shape::Box { half_extents: Vec3::new(1.0, 5.0, 4.0) }, Vec3::new(5.0, 5.0, 0.0)),
        (Shape::Box { half_extents: Vec3::new(4.0, 5.0, 1.0) }, Vec3::new(2.0, 5.0, 4.5)),
    ]);
    let mut camera = director();
    let radius = camera.tuning().collision.radius;
    camera.set_takeover(Some(shot_from(Vec3::new(4.2, 2.0, 3.7))));
    let pose = camera.update(&Moment::alone(Vec3::ZERO).frame(1.0 / 60.0), &world);
    assert!(eye_overlaps(&world, &pose, radius).is_none(), "{:?}", pose.eye);
}

/// Between walls too close for the eye's sphere, pushing gets nowhere: it stands as far along the way as it gets.
#[test]
fn a_takeover_with_nowhere_to_stand_stops_on_its_way() {
    let wall = Shape::Box { half_extents: Vec3::new(0.5, 5.0, 4.0) };
    let world = ground_with(&[(wall, Vec3::new(3.5, 5.0, 0.0)), (wall, Vec3::new(4.7, 5.0, 0.0))]);
    let mut camera = director();
    let radius = camera.tuning().collision.radius;
    let script = |_: f32| Moment::alone(Vec3::ZERO);
    run(&mut camera, &world, 60, (0.0, 1.0), script, |_, _, _| {});
    camera.set_takeover(Some(shot_from(Vec3::new(4.1, 2.0, 0.0))));
    camera.cut();
    let first = camera.update(&script(0.0).frame(1.0 / 60.0), &world);
    assert!(eye_overlaps(&world, &first, radius).is_none(), "{:?}", first.eye);
    assert!(first.eye.x < 3.0, "on this side of them: {:?}", first.eye);
    let (hops, _) = clear_through(&mut camera, &world, "shut out", (1.0, 5.0), script);
    assert_eq!(hops, 0, "there's nowhere to hop to");
    assert!(first.eye.distance(camera.pose().eye) < 1.0, "{:?}", camera.pose().eye);
}

/// A shot the eye has clear way to is exactly the shot, whatever else is about.
#[test]
fn a_takeover_with_nothing_in_its_way_is_exact() {
    let world = wall_beside();
    let mut camera = director();
    let shot = shot_from(Vec3::new(-3.0, 2.0, -3.0));
    run(&mut camera, &world, 60, (0.0, 1.0), |_| Moment::alone(Vec3::ZERO), |_, _, _| {});
    camera.set_takeover(Some(shot));
    clear_through(&mut camera, &world, "a clear shot", (1.0, 4.0), |_| Moment::alone(Vec3::ZERO));
    assert_eq!(camera.pose(), shot);
}

/// Blending to a shot past a wall, the eye doesn't cross it: it stops and slides, and once that's plainly as far
/// as it gets, hops to the shot, the one jump there is, as it does for the arm. Blending back goes the same way.
#[test]
fn a_takeover_past_a_wall_hops_rather_than_crossing_it() {
    let world = wall_beside();
    let mut camera = director();
    let script = |_: f32| Moment::alone(Vec3::ZERO);
    let shot = shot_from(BEYOND_THE_WALL);
    run(&mut camera, &world, 60, (0.0, 1.0), script, |_, _, _| {});
    let before = camera.pose();
    camera.take_stats();

    camera.set_takeover(Some(shot));
    let (hops, top) = clear_through(&mut camera, &world, "blending in", (1.0, 5.0), script);
    assert_eq!(camera.pose(), shot, "it gets there");
    assert_eq!(hops, 1);
    assert!(top < TAKEOVER_SPEED, "the eye reached {top} m/s");

    camera.set_takeover(None);
    let (hops, top) = clear_through(&mut camera, &world, "blending out", (5.0, 9.0), script);
    assert_eq!(camera.weight(Rig::Takeover), 0.0);
    assert!(camera.pose().eye.distance(before.eye) < 0.01, "back to the rigs: {:?}", camera.pose());
    assert_eq!(hops, 1);
    assert!(top < TAKEOVER_SPEED, "the eye reached {top} m/s");
}

/// Slid along something and clear of it, the eye comes back to its place softly, rather than all at once.
#[test]
fn a_takeover_past_a_short_wall_comes_round_it_softly() {
    // Across the way from the eye to the shot, and ending short of where the shot is along it, so the eye
    // slides off its end.
    let world =
        ground_with(&[(Shape::Box { half_extents: Vec3::new(0.2, 5.0, 1.5) }, Vec3::new(2.0, 5.0, -3.5))]);
    let mut camera = director();
    let script = |_: f32| Moment::alone(Vec3::ZERO);
    run(&mut camera, &world, 60, (0.0, 1.0), script, |_, _, _| {});
    camera.take_stats();
    let shot = shot_from(Vec3::new(4.0, 2.0, -1.0));
    camera.set_takeover(Some(shot));
    let (hops, top) = clear_through(&mut camera, &world, "coming round", (1.0, 5.0), script);
    assert_eq!(hops, 0, "it never had to");
    assert!(top < TAKEOVER_SPEED, "the eye reached {top} m/s");
    assert_eq!(camera.pose(), shot, "and it ends exactly on the shot");
}

/// A shot with its own motion, carried through a wall and out: each frame clear, however it moves.
#[test]
fn a_moving_takeover_stays_out_of_what_it_passes() {
    let world = wall_beside();
    let mut camera = director();
    let script = |_: f32| Moment::alone(Vec3::ZERO);
    run(&mut camera, &world, 60, (0.0, 1.0), script, |_, _, _| {});
    let radius = camera.tuning().collision.radius;
    for frame in 0..600 {
        // From this side of the wall to the far side and back, every 5 s.
        let t = frame as f32 / 60.0;
        camera.set_takeover(Some(shot_from(Vec3::new(2.0 + 3.0 * (t * 1.25).sin(), 2.0, -3.0))));
        let pose = camera.update(&script(t).frame(1.0 / 60.0), &world);
        assert!(eye_overlaps(&world, &pose, radius).is_none(), "at {t} s: {:?}", pose.eye);
    }
    assert!(camera.take_stats().hops >= 1, "it must have been shut out at some point");
}

/// A shake on top of a shot right by a wall stops short of it, as it does on a rig's view.
#[test]
fn a_shaking_takeover_stays_out_of_what_its_beside() {
    let world = wall_beside();
    let mut camera = director();
    let radius = camera.tuning().collision.radius;
    let shake = camera.tuning().shake_id("finish").expect("a shipped preset");
    camera.set_takeover(Some(shot_from(Vec3::new(1.8 - radius - 0.005, 2.0, -3.0))));
    let mut moved = 0.0_f32;
    for frame in 0..300 {
        if frame % 10 == 0 {
            camera.fire_shake(shake, 1.0);
        }
        let pose = camera.update(&Moment::alone(Vec3::ZERO).frame(1.0 / 60.0), &world);
        assert!(eye_overlaps(&world, &pose, radius).is_none(), "frame {frame}: {:?}", pose.eye);
        moved = moved.max(pose.eye.distance(Vec3::new(1.8 - radius - 0.005, 2.0, -3.0)));
    }
    assert!(moved > 0.0, "the shake must have moved it");
}

/// The sphere's radius from the shipped tuning stays well clear of the near plane's corners, which is
/// what stops geometry cutting the view even when the sphere touches something.
#[test]
fn the_sphere_covers_the_near_planes_corners() {
    let tuning = CameraTuning::default_tuning();
    let near = 0.05_f32;
    let half_y = (tuning.combat.framing.fov_y * 0.5).tan();
    let corner = near * (1.0 + half_y * half_y * (1.0 + common::ASPECT * common::ASPECT)).sqrt();
    assert!(tuning.collision.radius > corner * 2.0, "{} vs {corner}", tuning.collision.radius);
}
