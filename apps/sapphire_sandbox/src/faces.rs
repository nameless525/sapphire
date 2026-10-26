//! The face look-dev rig (`--faces`, M1 C3): rows of the face test head, each facing the camera
//! and lit by a key light of its own from an angle in head space, the way an artist checks a face
//! map: a yaw sweep across both sides, top light, and expressions.
//!
//! - **The head** is content: the pipeline's face sample (`samples::face`), loaded by ID with
//!   its face material and maps through the asset server ([`Content::face_head`]).
//! - **Each head is a character**, lit by the character light rig (M1 C2) like the turntable's:
//!   [`Scene::frame_packet`] lists every head with the default preset's lighting resolved for
//!   it, its key light turned to the head's [`FaceRig`] angles in head space, and its face, the
//!   head's pose and the rig's expression ([`character`]). The meshes all face the camera, so the
//!   face map's shapes read as a light moving around still heads, and no nose line shows: it
//!   appears only as a head turns from the camera.
//! - **Rows**, left to right as the camera sees them ([`ROWS`]): the sweep from light behind the
//!   heads' right (lighting the image's left) round to behind their left, then a top light, one
//!   from above and to the side, a blush, and a gloom with a blush.
//!
//! The heads never move, so their instances are extracted once; their characters are listed
//! every frame, as every character's are.
//!
//! [`Content::face_head`]: crate::content::Content::face_head
//! [`Scene::frame_packet`]: crate::scene::Scene::frame_packet

use sapphire_assets::AssetId;
use sapphire_ecs::prelude::*;
use sapphire_math::{Mat3, Mat4, Quat, Vec3};
use sapphire_render::{CharacterFace, CharacterLighting, FaceExpression, HeadBasis};

use crate::orbit::OrbitCamera;

/// The face test head, by its sidecar's ID (`content/samples/face/face_head.glb.meta`, the
/// pipeline's `samples::face::HEAD_ID`).
pub(crate) const HEAD: AssetId = AssetId::from_text("cff436cc-9604-4a9f-8d3a-3bc8b5b6637e");

/// One head of the rig: the key light's angles in head space, in degrees (yaw toward the head's
/// right, negative to its left, and elevation above its horizontal plane), and its expression.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct FaceRig {
    pub yaw: f32,
    pub elevation: f32,
    pub expression: FaceExpression,
}

impl FaceRig {
    /// The unit vector toward the key light for a head facing `forward` with `up` (unit and
    /// perpendicular), whose right is `forward × up`.
    pub fn toward_key(&self, forward: Vec3, up: Vec3) -> Vec3 {
        let (yaw, elevation) = (self.yaw.to_radians(), self.elevation.to_radians());
        let right = forward.cross(up);
        right * (yaw.sin() * elevation.cos()) + up * elevation.sin() + forward * (yaw.cos() * elevation.cos())
    }
}

const fn rig(yaw: f32, elevation: f32, blush: f32, gloom: f32) -> FaceRig {
    FaceRig { yaw, elevation, expression: FaceExpression { blush, gloom } }
}

/// The rig's rows, top to bottom, each left to right as the camera sees it.
pub(crate) const ROWS: [&[FaceRig]; 2] = [
    &[
        rig(135.0, 0.0, 0.0, 0.0),
        rig(90.0, 0.0, 0.0, 0.0),
        rig(45.0, 0.0, 0.0, 0.0),
        rig(-45.0, 0.0, 0.0, 0.0),
        rig(-90.0, 0.0, 0.0, 0.0),
        rig(-135.0, 0.0, 0.0, 0.0),
    ],
    &[
        rig(0.0, 65.0, 0.0, 0.0),
        rig(60.0, 40.0, 0.0, 0.0),
        rig(45.0, 10.0, 1.0, 0.0),
        rig(45.0, 10.0, 0.7, 1.0),
    ],
];

/// Heads in the rig.
pub(crate) const HEADS: usize = ROWS[0].len() + ROWS[1].len();

/// Meters between neighbors in a row, and between rows.
const SPACING: f32 = 0.34;
const ROW_SPACING: f32 = 0.38;
/// Half the head's width and height, in meters, for framing.
const HEAD_HALF_EXTENTS: (f32, f32) = (0.14, 0.16);

/// The rig's center beside a character whose object-space bounds are `bounds`, and half its
/// height: what the stage (`--stage`) shades it to.
pub(crate) fn rig_center(bounds: [[f32; 3]; 2]) -> (Vec3, f32) {
    let half_height = 0.5 * (ROWS.len() as f32 - 1.0) * ROW_SPACING + HEAD_HALF_EXTENTS.1;
    (center(bounds), half_height)
}

/// Where the rig stands beside a character whose object-space bounds are `bounds`: well to its
/// left, at head height, so the camera framing the rig leaves the character out.
fn center(bounds: [[f32; 3]; 2]) -> Vec3 {
    let [min, max] = bounds.map(Vec3::from);
    let height = (max.y - min.y).max(0.1);
    Vec3::new(min.x - 1.6 * height, min.y + 0.55 * height, (min.z + max.z) * 0.5)
}

/// The camera framing the rig.
pub(crate) fn camera(bounds: [[f32; 3]; 2]) -> OrbitCamera {
    let widest = ROWS.iter().map(|row| row.len()).max().unwrap_or(1) as f32;
    let half_width = 0.5 * (widest - 1.0) * SPACING + HEAD_HALF_EXTENTS.0;
    let half_height = 0.5 * (ROWS.len() as f32 - 1.0) * ROW_SPACING + HEAD_HALF_EXTENTS.1;
    OrbitCamera::fitting(center(bounds), (half_width, half_height))
}

/// Every head's rig and placement, in [`ROWS`] order: rows across the camera's view, each head
/// facing the camera, so the rig's angles (which take the camera as straight ahead) hold for
/// heads at the edges of the view too, and turned about that line so its eyes stay level in the
/// image.
pub(crate) fn heads(bounds: [[f32; 3]; 2], camera: &OrbitCamera) -> [(FaceRig, Mat4); HEADS] {
    let view = camera.facing_rotation();
    let (right, up) = (view * Vec3::X, view * Vec3::Y);
    let mut heads = Vec::with_capacity(HEADS);
    for (row, rigs) in ROWS.iter().enumerate() {
        let y = (0.5 * (ROWS.len() as f32 - 1.0) - row as f32) * ROW_SPACING;
        for (column, &rig) in rigs.iter().enumerate() {
            let x = (column as f32 - 0.5 * (rigs.len() as f32 - 1.0)) * SPACING;
            let at = center(bounds) + right * x + up * y;
            let forward = (camera.eye() - at).normalize();
            // A line through the head projects level when it lies in the plane of the line of
            // sight and the camera's right: the head's +X (its left) is that plane's direction
            // across the line of sight.
            let across = (right - forward * right.dot(forward)).normalize();
            let facing = Quat::from_mat3(&Mat3::from_cols(across, forward.cross(across), forward));
            heads.push((rig, Mat4::from_rotation_translation(facing, at)));
        }
    }
    heads.try_into().expect("HEADS counts every row")
}

/// A rig head's character data this frame, from `lighting` resolved for it: the key light turned
/// to the rig's angles in the head's space, and its face. The head is the mesh's, placed by
/// `transform` (its model faces +Z with +Y up, centered on its origin).
pub(crate) fn character(
    rig: &FaceRig,
    transform: Mat4,
    mut lighting: CharacterLighting,
) -> (CharacterLighting, CharacterFace) {
    let head =
        HeadBasis { forward: transform.transform_vector3(Vec3::Z), up: transform.transform_vector3(Vec3::Y) };
    let (forward, up) = head.orthonormalized().unwrap_or((Vec3::Z, Vec3::Y));
    lighting.key.direction = -rig.toward_key(forward, up);
    (lighting, CharacterFace { head, expression: rig.expression })
}

#[cfg(test)]
mod tests {
    use sapphire_render::DirectionalLight;

    use super::*;

    /// `light` in a head's basis: right, up, forward.
    fn in_head(face: CharacterFace, light: Vec3) -> Vec3 {
        let (forward, up) = face.head.orthonormalized().expect("a basis");
        Vec3::new(light.dot(forward.cross(up)), light.dot(up), light.dot(forward))
    }

    #[test]
    fn each_head_is_lit_from_its_angles_in_its_own_space() {
        let bounds = [[-0.5, 0.0, -0.5], [0.5, 2.0, 0.5]];
        let camera = camera(bounds);
        let sun = DirectionalLight { direction: Vec3::new(0.55, -0.7, -0.45), ..DirectionalLight::default() };
        let resolved = CharacterLighting::unstyled(sun);
        for (rig, transform) in heads(bounds, &camera) {
            let (lighting, face) = character(&rig, transform, resolved);
            let (yaw, elevation) = (rig.yaw.to_radians(), rig.elevation.to_radians());
            let expected =
                Vec3::new(yaw.sin() * elevation.cos(), elevation.sin(), yaw.cos() * elevation.cos());
            let seen = in_head(face, -lighting.key.direction);
            assert!(seen.abs_diff_eq(expected, 1e-5), "{rig:?}: {seen}");
            assert_eq!(face.expression, rig.expression);
            assert_eq!(lighting.key.color, sun.color, "only the direction is the rig's");
            let toward_camera = (camera.eye() - transform.transform_point3(Vec3::ZERO)).normalize();
            assert!(face.head.forward.abs_diff_eq(toward_camera, 1e-5), "the head faces the camera");
        }
    }

    #[test]
    fn heads_stand_in_rows_facing_the_camera() {
        let bounds = [[-0.5, 0.0, -0.5], [0.5, 2.0, 0.5]];
        let camera = camera(bounds);
        let heads = heads(bounds, &camera);
        for (_, transform) in &heads {
            let forward = transform.transform_vector3(Vec3::Z);
            let toward_camera = (camera.eye() - transform.transform_point3(Vec3::ZERO)).normalize();
            assert!(forward.abs_diff_eq(toward_camera, 1e-5), "facing the camera: {forward} {toward_camera}");
            let across = transform.transform_vector3(Vec3::X);
            assert!(
                across.dot(camera.eye() - transform.transform_point3(Vec3::ZERO)).abs() < 1e-4,
                "across the line of sight"
            );
            let at = transform.transform_point3(Vec3::ZERO);
            assert!(at.x < -2.5, "clear of the character: {at}");
        }
        // Neighbors don't overlap.
        let first_row: Vec<Vec3> =
            heads[..ROWS[0].len()].iter().map(|(_, t)| t.transform_point3(Vec3::ZERO)).collect();
        assert!(first_row.windows(2).all(|pair| pair[0].distance(pair[1]) > 2.0 * HEAD_HALF_EXTENTS.0));
    }
}
