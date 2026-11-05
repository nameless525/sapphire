//! Independent controls for the measurements shared by camera scenarios.

mod common;

use sapphire_camera::{CameraPose, Subject};
use sapphire_math::{Quat, Vec3};

fn pose() -> CameraPose {
    CameraPose { eye: Vec3::ZERO, rotation: Quat::IDENTITY, fov_y: std::f32::consts::FRAC_PI_2 }
}

#[test]
fn the_left_and_right_subject_edges_have_the_same_extent() {
    let subject = |x| Subject { position: Vec3::new(x, 0.0, 1.0), height: 0.1, radius: 0.1 };
    let left = common::frame_use(&pose(), &subject(-0.95 * common::ASPECT));
    let right = common::frame_use(&pose(), &subject(0.95 * common::ASPECT));
    assert!(left.x > 1.0, "the radius crosses the left edge: {left}");
    assert_eq!(left, right);
}

#[test]
#[should_panic]
fn a_nan_eye_cannot_look_clear() {
    let mut invalid = pose();
    invalid.eye.x = f32::NAN;
    common::eye_overlaps(&common::open_ground(), &invalid, 0.18);
}

#[test]
#[should_panic]
fn a_nan_rotation_cannot_disappear_from_motion_maxima() {
    let mut invalid = pose();
    invalid.rotation = Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0);
    common::Motion::default().add(&invalid, 1.0 / 60.0);
}
