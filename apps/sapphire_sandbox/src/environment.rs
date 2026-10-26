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
//!
//! The courtyard never moves, so its instance is extracted once.
//!
//! [`Content::courtyard`]: crate::content::Content::courtyard

use sapphire_assets::AssetId;

use crate::orbit::OrbitCamera;

/// The courtyard, by its sidecar's ID (`content/samples/environment/courtyard.glb.meta`, the
/// pipeline's `samples::environment::COURTYARD_ID`).
pub(crate) const COURTYARD: AssetId = AssetId::from_text("14ad5875-ec27-4866-bb2e-501408f4e1ad");

/// How much farther than the character's framing the camera stands.
const PULL_BACK: f32 = 1.45;
/// The camera's starting angles in radians: the framing's yaw, a lower pitch.
const YAW: f32 = 0.35;
const PITCH: f32 = 0.1;

/// The camera for a character whose object-space bounds are `bounds`, in the courtyard.
pub(crate) fn camera(bounds: [[f32; 3]; 2]) -> OrbitCamera {
    let mut camera = OrbitCamera::framing(bounds).pulled_back(PULL_BACK);
    camera.look_from(YAW, PITCH);
    camera
}

#[cfg(test)]
mod tests {
    use sapphire_math::Vec3;

    use super::*;

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
}
