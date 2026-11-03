//! The outline look-dev scenes (M1 C4, outlines v2):
//!
//! - **The row** (`--outline-row`): more of the character behind it, from 4 m to 90 m further
//!   back, on both sides of the camera's view and none hidden behind another, each a character
//!   lit by the default light rig, seen by a camera lowered to nearly level so all of them stand
//!   near the middle of the image. Their lines follow the width's distance curve: full close up,
//!   thinning with distance, holding their width in the world far away, gone past the cutoff (the
//!   last stands beyond it). With `--fov 20`, which keeps the character framed, a narrow lens
//!   brings the row closer in effective distance, and its lines widen with it (FOV
//!   normalization), the whole row still in view.
//! - **Hard edges** (`--hard-edges`): the pipeline's hard-edged sample (cube, hexagonal and
//!   triangular prisms, a gem; every edge split), loaded by ID from the shared pack and framed by
//!   the camera instead of the character: its cooked outline normals keep every corner's hull
//!   closed.
//!
//! Both never move, so their instances are extracted once.

use sapphire_assets::AssetId;
use sapphire_math::{Mat4, Quat, Vec3};

use crate::orbit::OrbitCamera;

/// The hard-edged sample, by its sidecar's ID (`content/samples/hard_edges.glb.meta`, the
/// pipeline's `samples::hard_edges::ID`).
pub(crate) const HARD_EDGES: AssetId = AssetId::from_text("5b0c2d7e-8f41-4a63-9e1d-3c7a2b6f9d40");

/// Where each of the row stands: how far behind the character, in meters along the camera's
/// view, and how far to the camera's right, as a fraction of that distance plus the camera's
/// from the character (negative: to the left). The nearer stand farther out, so none hides
/// another or behind the character, at 43° or 20° of field of view.
pub(crate) const ROW: [(f32, f32); 5] =
    [(4.0, -0.36), (10.0, 0.34), (22.0, -0.21), (46.0, 0.17), (90.0, 0.235)];

/// The row's camera pitch, radians: nearly level, so the row stands about the image's middle.
const ROW_PITCH: f32 = 0.06;

/// The camera's default angles' horizontal frame for a character with object-space `bounds`:
/// the point on the ground below its center, the direction from the camera into the scene, and
/// the camera's right, all horizontal.
fn ground_frame(bounds: [[f32; 3]; 2]) -> (Vec3, Vec3, Vec3) {
    let camera = OrbitCamera::framing(bounds);
    let [min, max] = bounds.map(Vec3::from);
    let ground = Vec3::new((min.x + max.x) * 0.5, min.y, (min.z + max.z) * 0.5);
    let toward_camera = camera.facing_rotation() * Vec3::Z;
    let back = -Vec3::new(toward_camera.x, 0.0, toward_camera.z).normalize();
    (ground, back, back.cross(Vec3::Y))
}

/// Where each of the row stands, facing the camera, in [`ROW`] order.
pub(crate) fn row(bounds: [[f32; 3]; 2]) -> [Mat4; ROW.len()] {
    let (ground, back, right) = ground_frame(bounds);
    let in_front = row_camera(bounds).eye().distance(ground);
    let facing = Quat::from_rotation_arc(Vec3::Z, -back);
    ROW.map(|(distance, across)| {
        let at = ground + back * distance + right * (across * (distance + in_front));
        Mat4::from_rotation_translation(facing, at)
    })
}

/// The camera for the row: the character's framing from its default angles, lowered to
/// [`ROW_PITCH`].
pub(crate) fn row_camera(bounds: [[f32; 3]; 2]) -> OrbitCamera {
    let mut camera = OrbitCamera::framing(bounds);
    camera.set_pitch(ROW_PITCH);
    camera
}

/// The hard-edged sample's placement: well to the side of the character, facing the camera's
/// default angles, so the camera framing it leaves the character out.
pub(crate) fn hard_edges(bounds: [[f32; 3]; 2]) -> Mat4 {
    let (ground, back, right) = ground_frame(bounds);
    let facing = Quat::from_rotation_arc(Vec3::Z, -back);
    Mat4::from_rotation_translation(facing, ground - right * 6.0)
}

/// The camera framing the hard-edged sample: its row of shapes, 3 m wide and about a meter tall
/// (the sample's `SPACING` and `CENTER_HEIGHT`).
pub(crate) fn hard_edges_camera(bounds: [[f32; 3]; 2]) -> OrbitCamera {
    let center = hard_edges(bounds).transform_point3(Vec3::new(0.0, 0.45, 0.0));
    OrbitCamera::fitting(center, (1.5, 0.45))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_recedes_behind_the_character_on_both_sides_of_the_view() {
        let bounds = [[-0.55, 0.0, -0.55], [0.55, 2.1, 0.55]];
        let eye = row_camera(bounds).eye();
        let target = Vec3::new(0.0, 1.05, 0.0);
        let forward = (target - eye).normalize();
        let mut last = eye.distance(target);
        let mut sides = [0; 2];
        for (index, place) in row(bounds).iter().enumerate() {
            let at = place.transform_point3(Vec3::ZERO);
            let depth = (at - eye).dot(forward);
            assert!(depth > last, "{index}: {depth} after {last}");
            last = depth;
            // Clear of the character, inside the 43° view's 70° across.
            let angle = ((at - eye).normalize().dot(forward)).acos().to_degrees();
            assert!((7.0..22.0).contains(&angle), "{index}: {angle}°");
            sides[usize::from((at - eye).cross(forward).y > 0.0)] += 1;
            // Facing the camera, on the ground.
            let facing = place.transform_vector3(Vec3::Z);
            assert!(facing.dot(-forward) > 0.9, "{index}: {facing}");
            assert!(at.y.abs() < 1e-5);
        }
        assert_eq!(sides, [2, 3], "two on the view's left, three on its right");
    }

    #[test]
    fn the_hard_edged_sample_stands_clear_of_the_character() {
        let bounds = [[-0.55, 0.0, -0.55], [0.55, 2.1, 0.55]];
        let at = hard_edges(bounds).transform_point3(Vec3::ZERO);
        assert!(at.distance(Vec3::ZERO) > 5.0, "{at}");
        let camera = hard_edges_camera(bounds);
        assert!(camera.eye().distance(at) > 2.0);
    }
}
