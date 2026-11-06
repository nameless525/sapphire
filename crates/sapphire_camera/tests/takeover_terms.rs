//! What a sequencer asks of a takeover beyond the tuning's blend: its own half-lives, an eye out of the
//! collision's rule, a cut between its shots, nobody on stage, and a blend out with a fight underneath.

mod common;

use common::{Moment, director, eye_overlaps, ground_with, open_ground};
use sapphire_camera::{CameraFrame, CameraPose, ShotEye, TakeoverTerms};
use sapphire_math::{Quat, Vec2, Vec3};
use sapphire_physics::Shape;

const DT: f32 = 1.0 / 60.0;
const EXACT: TakeoverTerms = TakeoverTerms { eye: ShotEye::Exact, blend_half_life: 0.1 };

fn shot_at(eye: Vec3) -> CameraPose {
    CameraPose { eye, rotation: Quat::from_rotation_y(0.7), fov_y: 0.6 }
}

fn alone() -> CameraFrame {
    Moment::alone(Vec3::ZERO).frame(DT)
}

/// A wall the shot stands in, and a director settled well away from it.
#[test]
fn an_exact_shot_stands_where_its_author_put_it_even_in_a_wall() {
    let inside = Vec3::new(5.0, 2.0, 0.0);
    let world = ground_with(&[(Shape::Box { half_extents: Vec3::ONE }, inside)]);
    let mut camera = director();
    let radius = camera.tuning().collision.radius;
    camera.update(&alone(), &world);
    camera.set_takeover_terms(EXACT);
    camera.set_takeover(Some(shot_at(inside)));
    for _ in 0..120 {
        camera.update(&alone(), &world);
    }
    let pose = camera.pose();
    assert_eq!(pose.eye, inside);
    assert!(eye_overlaps(&world, &pose, radius).is_some(), "it really is in the wall");

    // The same shot and the same wall, by the collision's rule: out of it.
    let mut clear = director();
    clear.update(&alone(), &world);
    clear.set_takeover_terms(TakeoverTerms { eye: ShotEye::Clear, ..EXACT });
    clear.set_takeover(Some(shot_at(inside)));
    for _ in 0..120 {
        clear.update(&alone(), &world);
    }
    assert!(eye_overlaps(&world, &clear.pose(), radius).is_none());
}

/// An exact shot casts nothing for itself, so it costs nothing beyond the rigs beneath it.
#[test]
fn an_exact_shot_takes_no_extra_casts() {
    let world = open_ground();
    let casts = |terms: Option<TakeoverTerms>| {
        let mut camera = director();
        if let Some(terms) = terms {
            camera.set_takeover_terms(terms);
        }
        camera.set_takeover(Some(shot_at(Vec3::new(3.0, 2.0, 0.0))));
        for _ in 0..60 {
            camera.update(&alone(), &world);
        }
        camera.take_stats().casts
    };
    assert!(casts(Some(EXACT)) < casts(None));
}

#[test]
fn a_shorter_half_life_gets_there_sooner() {
    let world = open_ground();
    let share_after = |half_life: f32, frames: u32| {
        let mut camera = director();
        camera.update(&alone(), &world);
        let before = camera.pose();
        let shot = shot_at(before.eye + Vec3::new(4.0, 0.0, 0.0));
        camera.set_takeover_terms(TakeoverTerms { blend_half_life: half_life, ..EXACT });
        camera.set_takeover(Some(shot));
        for _ in 0..frames {
            camera.update(&alone(), &world);
        }
        (camera.pose().eye.x - before.eye.x) / 4.0
    };
    assert!(share_after(0.05, 6) > share_after(0.5, 6));
    assert!(share_after(0.05, 120) > 0.999);
}

#[test]
fn a_half_life_of_nothing_goes_at_once_both_ways() {
    let world = open_ground();
    let mut camera = director();
    let rigs = camera.update(&alone(), &world);
    let shot = shot_at(Vec3::new(9.0, 2.0, 9.0));
    camera.set_takeover_terms(TakeoverTerms { blend_half_life: 0.0, ..EXACT });
    camera.set_takeover(Some(shot));
    let there = camera.update(&alone(), &world);
    assert_eq!((there.eye, there.rotation, there.fov_y), (shot.eye, shot.rotation, shot.fov_y));
    camera.set_takeover(None);
    let back = camera.update(&alone(), &world);
    assert!(back.eye.distance(rigs.eye) < 1e-3, "{:?} {:?}", back.eye, rigs.eye);
}

#[test]
fn terms_that_arent_numbers_change_nothing() {
    let world = open_ground();
    let run = |half_life: Option<f32>| {
        let mut camera = director();
        camera.set_takeover_terms(EXACT);
        if let Some(blend_half_life) = half_life {
            camera.set_takeover_terms(TakeoverTerms { blend_half_life, ..EXACT });
        }
        camera.set_takeover(Some(shot_at(Vec3::new(4.0, 2.0, 0.0))));
        for _ in 0..5 {
            camera.update(&alone(), &world);
        }
        camera.pose()
    };
    for bad in [f32::NAN, f32::INFINITY, -1.0] {
        assert_eq!(run(Some(bad)), run(None));
    }
}

/// Terms are one takeover's: the next with none gets the tuning's again.
#[test]
fn terms_end_when_the_view_is_the_rigs_again() {
    let inside = Vec3::new(5.0, 2.0, 0.0);
    let world = ground_with(&[(Shape::Box { half_extents: Vec3::ONE }, inside)]);
    let mut camera = director();
    let radius = camera.tuning().collision.radius;
    camera.set_takeover_terms(TakeoverTerms { blend_half_life: 0.0, ..EXACT });
    camera.set_takeover(Some(shot_at(inside)));
    camera.update(&alone(), &world);
    assert_eq!(camera.pose().eye, inside);
    camera.set_takeover(None);
    camera.update(&alone(), &world);
    camera.update(&alone(), &world);
    camera.set_takeover(Some(shot_at(inside)));
    for _ in 0..240 {
        camera.update(&alone(), &world);
    }
    assert!(eye_overlaps(&world, &camera.pose(), radius).is_none());
}

/// A cut between a cinematic's shots lands its eye across a wall it would otherwise stop at, and leaves the
/// rigs' springs running: the view handed back is the same as if no cut had come.
#[test]
fn a_cut_between_shots_lands_the_eye_and_leaves_the_rigs_alone() {
    let wall = (Shape::Box { half_extents: Vec3::new(0.5, 5.0, 60.0) }, Vec3::new(-1.5, 5.0, 0.0));
    let world = ground_with(&[wall]);
    let run = |cut: bool| {
        let mut camera = director();
        let clear = TakeoverTerms { eye: ShotEye::Clear, blend_half_life: 0.05 };
        camera.set_takeover_terms(clear);
        camera.set_takeover(Some(shot_at(Vec3::new(3.0, 2.0, 0.0))));
        // The player runs, so the rigs' springs are mid-move when the shot changes.
        let moving = |t: f32| Moment::alone(Vec3::new(0.0, 0.0, 5.0 * t)).frame(DT);
        for frame in 0..60 {
            camera.update(&moving(frame as f32 * DT), &world);
        }
        let next = shot_at(Vec3::new(-6.0, 2.0, 2.0));
        camera.set_takeover(Some(next));
        if cut {
            camera.cut_takeover();
        }
        let landed = camera.update(&moving(1.0), &world);
        for frame in 61..120 {
            camera.update(&moving(frame as f32 * DT), &world);
        }
        camera.set_takeover_terms(TakeoverTerms { blend_half_life: 0.0, ..clear });
        camera.set_takeover(None);
        for frame in 120..300 {
            camera.update(&moving(frame as f32 * DT), &world);
        }
        (landed.eye, next.eye, camera.pose())
    };
    let (cut_eye, shot_eye, cut_back) = run(true);
    assert_eq!(cut_eye, shot_eye, "landed at once");
    let (slid_eye, shot_eye, slid_back) = run(false);
    assert!(slid_eye.distance(shot_eye) > 1.0, "without a cut it moves there: {slid_eye:?}");
    assert!(cut_back.eye.distance(slid_back.eye) < 1e-3, "and the rigs didn't notice");
}

#[test]
fn look_and_zoom_dont_reach_the_rigs_during_a_takeover() {
    let world = open_ground();
    let handed_back = |look: Vec2, zoom: f32| {
        let mut camera = director();
        camera.set_takeover_terms(EXACT);
        camera.set_takeover(Some(shot_at(Vec3::new(3.0, 2.0, 0.0))));
        for _ in 0..60 {
            camera.update(&CameraFrame { look, zoom, ..alone() }, &world);
        }
        camera.set_takeover(None);
        for _ in 0..240 {
            camera.update(&alone(), &world);
        }
        camera.pose()
    };
    assert_eq!(handed_back(Vec2::new(0.1, 0.05), 0.2), handed_back(Vec2::ZERO, 0.0));

    // And after, they do.
    let mut camera = director();
    camera.update(&alone(), &world);
    let before = camera.heading();
    for _ in 0..60 {
        camera.update(&CameraFrame { look: Vec2::new(0.05, 0.0), ..alone() }, &world);
    }
    assert!((camera.heading() - before).abs() > 0.5);
}

#[test]
fn nobody_on_stage_is_a_takeover_like_any_other() {
    let world = open_ground();
    let nobody = |dt| CameraFrame { player: None, target: None, ..Moment::alone(Vec3::ZERO).frame(dt) };
    let mut camera = director();
    let shot = shot_at(Vec3::new(3.0, 2.0, 1.0));
    camera.set_takeover_terms(TakeoverTerms { blend_half_life: 0.0, ..EXACT });
    camera.set_takeover(Some(shot));
    let pose = camera.update(&nobody(DT), &world);
    assert_eq!(pose, shot, "from the first frame there's ever been");

    // Someone walked off: the rigs hold on where they were, rather than where nobody isn't.
    let mut camera = director();
    let there = Moment::alone(Vec3::new(10.0, 0.0, 10.0));
    camera.cut();
    let held = camera.update(&there.frame(DT), &world);
    for _ in 0..30 {
        let pose = camera.update(&nobody(DT), &world);
        assert!(pose.eye.is_finite() && pose.eye.distance(held.eye) < 1e-2, "{:?}", pose.eye);
    }
}

/// A blend out with a fight underneath: the rigs have followed it all along, so what the view lands on is
/// exactly what it would have been with no takeover, mid-run and all.
#[test]
fn a_blend_out_lands_exactly_on_a_rig_that_kept_up_with_the_fight() {
    let world = open_ground();
    let moving = |frame: u32| {
        let t = frame as f32 * DT;
        Moment::against(Vec3::new(0.0, 0.0, 4.0 * t), Vec3::new(3.0, 0.0, 4.0 * t + 4.0)).frame(DT)
    };
    let mut plain = common::combat_director();
    let mut cinematic = common::combat_director();
    let mut landed = None;
    for frame in 0..600 {
        let start = frame == 60;
        if start {
            cinematic.set_takeover_terms(EXACT);
            cinematic.set_takeover(Some(shot_at(Vec3::new(-3.0, 3.0, 1.0))));
        }
        if frame == 240 {
            cinematic.set_takeover_terms(TakeoverTerms { blend_half_life: 0.1, ..EXACT });
            cinematic.set_takeover(None);
        }
        let plain_pose = plain.update(&moving(frame), &world);
        let pose = cinematic.update(&moving(frame), &world);
        assert!(pose.eye.is_finite() && pose.rotation.is_finite());
        if frame >= 340 {
            landed = Some((pose, plain_pose));
            assert!(
                pose.eye.distance(plain_pose.eye) < 0.02,
                "at {frame}: {:?} {:?}",
                pose.eye,
                plain_pose.eye
            );
            assert!((pose.fov_y - plain_pose.fov_y).abs() < 1e-3);
        }
    }
    assert!(landed.is_some());
}

/// A shake moves an exact shot without asking the world, and a full-weight shot without one is the pose.
#[test]
fn a_shake_moves_an_exact_shot_without_casting() {
    let inside = Vec3::new(5.0, 2.0, 0.0);
    let world = ground_with(&[(Shape::Box { half_extents: Vec3::ONE }, inside)]);
    let mut camera = director();
    camera.set_takeover_terms(TakeoverTerms { blend_half_life: 0.0, ..EXACT });
    camera.set_takeover(Some(shot_at(inside)));
    assert_eq!(camera.update(&alone(), &world).eye, inside);
    let presets = camera.tuning().shakes.presets.len();
    assert!(presets > 0);
    camera.fire_shake(sapphire_camera::ShakeId(0), 1.0);
    let mut moved = false;
    for _ in 0..20 {
        let pose = camera.update(&alone(), &world);
        assert!(pose.eye.distance(inside) < 1.0, "it stays by the shot, in the wall: {:?}", pose.eye);
        moved |= pose.eye != inside;
    }
    assert!(moved, "the shake reached it");
}

#[test]
fn simultaneous_cuts_are_consumed_on_the_same_frame() {
    let wall = (Shape::Box { half_extents: Vec3::new(0.5, 5.0, 60.0) }, Vec3::new(-1.5, 5.0, 0.0));
    let world = ground_with(&[wall]);
    let mut camera = director();
    camera.set_takeover_terms(TakeoverTerms { eye: ShotEye::Clear, blend_half_life: 0.0 });
    camera.set_takeover(Some(shot_at(Vec3::new(3.0, 2.0, 0.0))));
    camera.cut();
    camera.cut_takeover();
    camera.update(&alone(), &world);
    let next = shot_at(Vec3::new(-6.0, 2.0, 2.0));
    camera.set_takeover(Some(next));
    let moved = camera.update(&alone(), &world);
    assert!(moved.eye.distance(next.eye) > 1.0, "no second cut: {moved:?}");
    camera.cut_takeover();
    let paused = camera.update(&CameraFrame { dt: 0.0, ..alone() }, &world);
    assert_eq!(paused.eye, next.eye, "an explicit cut also works while paused");
}
