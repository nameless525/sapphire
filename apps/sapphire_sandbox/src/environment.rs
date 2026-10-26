//! The environment scene (`--environment`, M1 W4): the courtyard from the pipeline's environment
//! samples around the character, drawn by the environment shader beside the character's anime
//! character materials, under the same time of day.
//!
//! - **The courtyard** is content: one mesh in world space (ground, boulders, a colonnade, a wall
//!   and paper lanterns), its slots bound to environment materials with a terminator band and a
//!   painted ground map, loaded by ID through the asset server ([`Content::courtyard`]). The
//!   character's pedestal stands on its flat middle; the mannequin and its pedestal keep their
//!   anime character materials, so every existing scene draws as before.
//! - **The camera** pulls back from the character's framing and lowers its view a little, so
//!   the colonnade, the wall and the hills fill the frame behind the character, which still reads
//!   first: the relation between the world and the characters the look is about.
//! - **Fog cards** (M1 W3), where frames draw fog (not with `--fog off`): sheets of mist across
//!   the valley, placed in the world and turned toward the camera's side, and upright haze among
//!   the colonnade, from the pipeline's fog card samples ([`Content::fog_cards`]). Each is an
//!   instance of a unit card scaled to its size, so the renderer sorts them one by one; the
//!   sheets dip a little into the rolling ground, where the depth fade softens them (and on
//!   phones, which have none, the map's soft lower edge).
//!
//! - **The arcade and the probe grid** (`--probes`, M1 W5): a stone gateway through the far wall
//!   and a loggia with a terracotta roof along it, in world space too, and the ambient probe grid
//!   the cooker baked for the courtyard with them ([`Content::probes`]). The grid lights the
//!   world in place of the ambient hemisphere: colored bounce in the shade, the sky seen past the
//!   arcade's roof and arches, and the characters' ambient from around them; time of day picks
//!   the keys its bounce blends from the hour. The camera turns to it through a longer lens, the
//!   gateway and the loggia large either side behind the character.
//!
//! The courtyard, its cards and the arcade never move, so their instances are extracted once.
//!
//! [`Content::courtyard`]: crate::content::Content::courtyard
//! [`Content::fog_cards`]: crate::content::Content::fog_cards
//! [`Content::probes`]: crate::content::Content::probes

use sapphire_assets::AssetId;
use sapphire_math::{Mat4, Quat, Vec3};

use crate::orbit::OrbitCamera;

/// The courtyard, by its sidecar's ID (`content/samples/environment/courtyard.glb.meta`, the
/// pipeline's `samples::environment::COURTYARD_ID`).
pub(crate) const COURTYARD: AssetId = AssetId::from_text("14ad5875-ec27-4866-bb2e-501408f4e1ad");

/// The fog cards' meshes, by their sidecars' IDs (`content/samples/environment/*.glb.meta`): the
/// mist sheet, placed in the world, and the haze card, upright toward the camera.
pub(crate) const FOG_CARDS: [AssetId; 2] = [
    AssetId::from_text("4b217f2d-6046-4760-a4ee-cadc08d243ae"),
    AssetId::from_text("046a80c3-c224-44ec-bacf-36e1ccf62f63"),
];
const MIST: usize = 0;
const HAZE: usize = 1;

/// A fog card in the courtyard: which of [`FOG_CARDS`] it draws, its center and its width and
/// height in meters, and its turn about the up axis in radians (a mist sheet's; haze turns toward
/// the camera itself).
struct CardPlace {
    mesh: usize,
    center: [f32; 3],
    size: [f32; 2],
    yaw: f32,
}

/// The courtyard's fog cards: mist sheets from 30 m out to the foot of the hills, and haze
/// beyond the wall and among the far pillars, their lower edges a little into the ground.
const CARDS: [CardPlace; 7] = [
    CardPlace { mesh: MIST, center: [-8.0, 3.5, -30.0], size: [34.0, 5.0], yaw: 0.35 },
    CardPlace { mesh: MIST, center: [-20.0, 1.5, -36.0], size: [20.0, 4.0], yaw: 0.3 },
    CardPlace { mesh: MIST, center: [-16.0, 4.0, -48.0], size: [50.0, 8.0], yaw: 0.3 },
    CardPlace { mesh: MIST, center: [6.0, 7.0, -62.0], size: [60.0, 10.0], yaw: 0.4 },
    CardPlace { mesh: HAZE, center: [-5.5, 1.5, -13.0], size: [9.0, 3.5], yaw: 0.0 },
    CardPlace { mesh: HAZE, center: [4.0, 1.3, -15.0], size: [10.0, 3.0], yaw: 0.0 },
    CardPlace { mesh: HAZE, center: [-11.0, 4.2, -24.0], size: [14.0, 4.5], yaw: 0.0 },
];

/// The courtyard's fog cards: each one's mesh, an index into [`FOG_CARDS`], and its transform.
pub(crate) fn fog_cards() -> impl Iterator<Item = (usize, Mat4)> {
    CARDS.iter().map(|card| {
        let scale = Vec3::new(card.size[0], card.size[1], 1.0);
        let transform = Mat4::from_scale_rotation_translation(
            scale,
            Quat::from_rotation_y(card.yaw),
            Vec3::from(card.center),
        );
        (card.mesh, transform)
    })
}

/// The arcade (`content/samples/environment/arcade.glb.meta`, the pipeline's
/// `samples::environment::ARCADE_ID`).
pub(crate) const ARCADE: AssetId = AssetId::from_text("29f2acee-efb7-4abb-8df6-f9b80a877df8");

/// The courtyard's ambient probe grid (`content/samples/environment/courtyard.probes.ron.meta`,
/// the pipeline's `samples::environment::PROBES_ID`).
pub(crate) const PROBES: AssetId = AssetId::from_text("2b0e5d95-2b11-4042-8435-7de3da07ba18");

/// How much farther than the character's framing the camera stands.
const PULL_BACK: f32 = 1.45;
/// The camera's starting angles in radians: the framing's yaw, a lower pitch.
const YAW: f32 = 0.35;
const PITCH: f32 = 0.1;

/// The arcade camera's angles in radians (−48° and 3°) and vertical field of view (20°).
const ARCADE_YAW: f32 = -0.837_758;
const ARCADE_PITCH: f32 = 0.052_359_88;
const ARCADE_FOV: f32 = 0.349_065_8;

/// The camera for a character whose object-space bounds are `bounds`, in the courtyard.
pub(crate) fn camera(bounds: [[f32; 3]; 2]) -> OrbitCamera {
    let mut camera = OrbitCamera::framing(bounds).pulled_back(PULL_BACK);
    camera.look_from(YAW, PITCH);
    camera
}

/// The courtyard's camera turned toward the arcade (`--probes`), its lens narrowed with the
/// character's framing kept.
pub(crate) fn arcade_camera(bounds: [[f32; 3]; 2]) -> OrbitCamera {
    let mut camera = camera(bounds);
    camera.look_from(ARCADE_YAW, ARCADE_PITCH);
    camera.set_fov(ARCADE_FOV);
    camera
}

#[cfg(test)]
mod tests {
    use sapphire_math::Vec3;

    use super::*;

    #[test]
    fn fog_cards_stand_behind_the_character_and_face_the_camera() {
        let bounds = [[-0.55, 0.0, -0.55], [0.55, 2.2, 0.55]];
        let eye = camera(bounds).eye();
        let mut kinds = [0; FOG_CARDS.len()];
        for (mesh, transform) in fog_cards() {
            kinds[mesh] += 1;
            let center = transform.transform_point3(Vec3::ZERO);
            assert!(center.z < -10.0, "behind the wall: {center}");
            // The card's front, +Z in its object space, toward the camera.
            let front = transform.transform_vector3(Vec3::Z);
            assert!(front.dot(eye - center) > 0.0, "facing the camera: {center}");
        }
        assert!(kinds.iter().all(|&count| count > 0), "both kinds: {kinds:?}");
    }

    #[test]
    fn the_camera_frames_the_character_from_farther_and_lower() {
        let bounds = [[-0.55, 0.0, -0.55], [0.55, 2.2, 0.55]];
        let (framing, wide) = (OrbitCamera::framing(bounds), camera(bounds));
        let target = Vec3::new(0.0, 1.1, 0.0);
        let ratio = wide.eye().distance(target) / framing.eye().distance(target);
        assert!((ratio - PULL_BACK).abs() < 1e-4, "{ratio}");
        assert!(wide.eye().y < framing.eye().y, "lower: {} against {}", wide.eye(), framing.eye());
        assert!(wide.eye().z > 0.0, "still in front of the character");
    }

    #[test]
    fn the_arcade_camera_looks_past_the_character_at_the_arcade() {
        let bounds = [[-0.55, 0.0, -0.55], [0.55, 2.2, 0.55]];
        let camera = arcade_camera(bounds);
        let target = Vec3::new(0.0, 1.1, 0.0);
        // The gateway's middle and the loggia's (the pipeline's `samples::environment` arcade),
        // either side of the character, both well within the 16:9 frame's 17° to either side.
        let forward = (target - camera.eye()).normalize();
        for (name, middle) in
            [("gateway", Vec3::new(6.25, 2.0, -9.5)), ("loggia", Vec3::new(11.8, 2.0, -7.2))]
        {
            let toward = (middle - camera.eye()).normalize();
            assert!(forward.dot(toward) > 12f32.to_radians().cos(), "the {name} in view");
            assert!(forward.dot(toward) < 3f32.to_radians().cos(), "the {name} beside the character");
        }
        assert!(camera.eye().distance(target) > 2.0 * environment_distance(bounds), "a longer lens");
    }

    fn environment_distance(bounds: [[f32; 3]; 2]) -> f32 {
        camera(bounds).eye().distance(Vec3::new(0.0, 1.1, 0.0))
    }
}
