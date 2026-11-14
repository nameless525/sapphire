//! The characters' shadows on each other (M5 L12, `--shadow-party`): two more of the character
//! stand toward the sun from it, one behind the other, so the nearer shadows the character and the
//! farther shadows both, seen from the sun's side, where the cast shadows lie on lit surfaces.
//! Placed at the scene's hour (so after it's set), each a character lit by the default light rig
//! and facing the camera, as the outline row's are; they never move, so their instances are
//! extracted once.

use sapphire_math::{Mat4, Quat, Vec3};

use crate::orbit::OrbitCamera;

/// Where each of the others stands from the character, in its heights: how far toward the sun
/// and how far across the sun's line, toward the camera's side.
pub(crate) const OTHERS: [(f32, f32); 2] = [(0.6, 0.08), (1.25, 0.2)];

/// The camera's angle off the sun's line, radians, toward the side the others step to: enough
/// that each stands clear of the next on screen.
const CAMERA_ANGLE: f32 = 1.05;

/// The camera's pitch, radians: a little above them.
const CAMERA_PITCH: f32 = 0.12;

/// The sun's line and the camera's side on the ground, for a light from `toward_sun`: the
/// horizontal directions toward the sun and across it (toward +Z where the sun leaves a choice,
/// the character's front).
fn ground_frame(toward_sun: Vec3) -> (Vec3, Vec3) {
    let sun = Vec3::new(toward_sun.x, 0.0, toward_sun.z).try_normalize().unwrap_or(Vec3::Z);
    let across = sun.cross(Vec3::Y);
    (sun, if across.z < 0.0 { -across } else { across })
}

/// The camera's horizontal direction from the party toward it.
fn camera_side(toward_sun: Vec3) -> Vec3 {
    let (sun, across) = ground_frame(toward_sun);
    sun * CAMERA_ANGLE.cos() + across * CAMERA_ANGLE.sin()
}

/// Where each of the others stands, facing the camera, in [`OTHERS`] order, for the character's
/// object-space `bounds` (standing at the origin) under a light from `toward_sun`.
pub(crate) fn places(bounds: [[f32; 3]; 2], toward_sun: Vec3) -> [Mat4; OTHERS.len()] {
    let height = bounds[1][1] - bounds[0][1];
    let (sun, across) = ground_frame(toward_sun);
    let facing = Quat::from_rotation_arc(Vec3::Z, camera_side(toward_sun));
    OTHERS.map(|(toward, aside)| {
        Mat4::from_rotation_translation(facing, (sun * toward + across * aside) * height)
    })
}

/// The camera on the three, from the sun's side.
pub(crate) fn camera(bounds: [[f32; 3]; 2], toward_sun: Vec3) -> OrbitCamera {
    let [min, max] = bounds.map(Vec3::from);
    let height = max.y - min.y;
    let side = camera_side(toward_sun);
    let right = Vec3::Y.cross(side).normalize();
    let mut stands: Vec<Vec3> =
        places(bounds, toward_sun).iter().map(|place| place.w_axis.truncate()).collect();
    stands.push(Vec3::ZERO);
    let across = stands.iter().map(|stand| stand.dot(right));
    let (left, right_most) = across.fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
    let middle = stands.iter().copied().sum::<Vec3>() / stands.len() as f32;
    let width = (right_most - left) * 0.5 + (max.x - min.x).max(max.z - min.z) * 0.6;
    let mut camera = OrbitCamera::fitting(middle + Vec3::Y * (min.y + height * 0.5), (width, height * 0.6));
    camera.look_from(side.x.atan2(side.z), CAMERA_PITCH);
    camera
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: [[f32; 3]; 2] = [[-0.55, 0.0, -0.55], [0.55, 1.7, 0.55]];

    #[test]
    fn the_others_stand_toward_the_sun_each_clear_of_the_next_on_screen() {
        // A low sun from the front right, as at golden hour, and from behind.
        for toward_sun in [Vec3::new(0.85, 0.3, 0.45), Vec3::new(-0.3, 0.4, -0.9), Vec3::Y] {
            let (sun, _) = ground_frame(toward_sun);
            let stands: Vec<Vec3> =
                places(BOUNDS, toward_sun).iter().map(|place| place.w_axis.truncate()).collect();
            let mut last = 0.0;
            for stand in &stands {
                let toward = stand.dot(sun);
                assert!(toward > last + 0.9, "{toward_sun}: each farther toward the sun: {toward}");
                last = toward;
                assert!(stand.y.abs() < 1e-5, "on the ground");
            }
            let camera = camera(BOUNDS, toward_sun);
            let eye = camera.eye();
            let forward =
                (camera.camera_data(16.0 / 9.0).view.inverse().transform_vector3(Vec3::NEG_Z)).normalize();
            let right = forward.cross(Vec3::Y).normalize();
            let mut across: Vec<f32> = [Vec3::ZERO, stands[0], stands[1]]
                .iter()
                .map(|stand| (*stand + Vec3::Y * 0.85 - eye).normalize().dot(right))
                .collect();
            across.sort_by(f32::total_cmp);
            for pair in across.windows(2) {
                assert!(pair[1] - pair[0] > 0.12, "{toward_sun}: apart on screen: {across:?}");
            }
            assert!(eye.dot(sun) > 0.0, "{toward_sun}: on the sun's side");
        }
    }
}
