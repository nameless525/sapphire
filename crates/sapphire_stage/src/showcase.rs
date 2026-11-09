//! Showcases (M3 U6): characters a 3D inset shows, standing in a studio of their own.
//!
//! A showcase is an ordinary character entity ([`Scene::spawn_showcase`]) with a [`Showcased`] naming its
//! inset: the mesh, light rig, figure and pose of the character it shows, which it follows each frame, so
//! an idle the character plays plays in the inset too. It stands at [`stand`], far below the world, where
//! no view, shadow, probe or local light of the world reaches it.
//!
//! Each frame the show names the insets its interface drew ([`SceneShow::inset_shots`]), and
//! [`Scene::frame_packet`] turns each into an [`InsetView`]: a camera orbiting the stand by the inset's
//! turn, from the document's distance, pitch and field of view, and the inset's own light. Its showcases
//! are lit by the document's light rig and key light for that camera, not by the hour, and the
//! renderer draws them in the inset's picture and in no other view. A showcase no inset shows that
//! frame stands in the world at its stand, which no camera sees; a game despawns it with its screen.
//!
//! Turning the inset orbits the camera rather than the showcase: with a portrait rig's key fixed to the
//! camera and the ambient even all round, the two draw the same, and the showcase's transform never
//! changes, so turning extracts nothing.
//!
//! [`Scene::spawn_showcase`]: crate::Scene::spawn_showcase
//! [`Scene::frame_packet`]: crate::Scene::frame_packet
//! [`SceneShow::inset_shots`]: crate::SceneShow::inset_shots

use sapphire_assets::ui::UiInset;
use sapphire_ecs::prelude::*;
use sapphire_math::{Quat, Vec3};
use sapphire_render::{AmbientLight, CameraData, CharacterLight, DirectionalLight, InsetView};

/// Shown in inset `inset`, and in no view of the world, standing for `of`.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Showcased {
    pub inset: u16,
    /// The character it wears the pose of.
    pub of: Entity,
}

/// What a show's interface drew of an inset this frame: what [`crate::Scene::frame_packet`] shoots.
#[derive(Clone, Copy, Debug)]
pub struct InsetShot<'a> {
    pub inset: u16,
    /// Its picture's size in pixels.
    pub size: (u32, u32),
    /// Degrees it's turned from facing the camera.
    pub yaw: f32,
    pub setup: &'a UiInset,
}

/// Where the studio stands: far below anything a world puts at its origin.
const STUDIO: Vec3 = Vec3::new(0.0, -2000.0, 0.0);
/// How far apart two insets' stands are, so neither's showcase stands in the other's picture.
const STAND_SPACING: f32 = 100.0;
/// The near plane of an inset's camera, in meters: a showcase stands metres away.
const NEAR: f32 = 0.05;

/// Where inset `inset`'s showcase stands: its feet's point.
#[must_use]
pub fn stand(inset: u16) -> Vec3 {
    STUDIO + Vec3::X * (f32::from(inset) * STAND_SPACING)
}

/// The camera of `shot`, orbiting `stand` by its turn. Turning it right turns the showcase's front to
/// the picture's right, as a hand on a turntable would.
#[must_use]
pub fn camera(shot: &InsetShot<'_>, stand: Vec3) -> CameraData {
    let setup = shot.setup;
    let target = stand + Vec3::Y * setup.look_at;
    let pitch = setup.pitch.to_radians();
    let back = Vec3::new(0.0, pitch.sin(), pitch.cos());
    let eye = target + Quat::from_rotation_y(-shot.yaw.to_radians()) * back * setup.distance;
    let aspect = shot.size.0 as f32 / shot.size.1.max(1) as f32;
    CameraData::look_at(eye, target, Vec3::Y, setup.fov.to_radians(), aspect, NEAR)
}

/// The view the renderer draws `shot` from, before its instances are named.
#[must_use]
pub fn view(shot: &InsetShot<'_>, camera: CameraData) -> InsetView {
    let setup = shot.setup;
    let ambient = AmbientLight {
        sky: setup.ambient_sky.into(),
        ground: setup.ambient_ground.into(),
        intensity: setup.ambient,
    };
    InsetView {
        exposure: setup.exposure,
        backdrop: setup.backdrop.into(),
        ..InsetView::new(shot.inset, camera, shot.size, ambient)
    }
}

/// The key light a showcase's rig turns, and its character light: the document's, from above the camera's
/// shoulder. A portrait rig fixes the key to the camera and reads only its color and intensity.
#[must_use]
pub fn lights(setup: &UiInset) -> (DirectionalLight, CharacterLight) {
    let color = Vec3::from(setup.light);
    let key = DirectionalLight { direction: Vec3::new(0.3, -0.8, -0.5), color, intensity: setup.intensity };
    (key, CharacterLight { color, intensity: setup.intensity })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(setup: &UiInset, yaw: f32) -> InsetShot<'_> {
        InsetShot { inset: 0, size: (400, 600), yaw, setup }
    }

    #[test]
    fn the_camera_faces_the_front_unturned_and_orbits_by_the_turn() {
        let setup = UiInset { pitch: 0.0, ..UiInset::default() };
        let stand = stand(0);
        let front = camera(&shot(&setup, 0.0), stand);
        let eye = front.position - stand;
        assert!(eye.z > 3.9 && eye.x.abs() < 1e-4, "in front, on +Z: {eye}");
        // A quarter turn right brings its -X side round to the camera.
        let turned = camera(&shot(&setup, 90.0), stand).position - stand;
        assert!(turned.x < -3.9 && turned.z.abs() < 1e-3, "{turned}");
        // A whole turn comes back.
        let whole = camera(&shot(&setup, 360.0), stand).position - stand;
        assert!((whole - eye).length() < 1e-3);
    }

    #[test]
    fn insets_stand_apart_and_far_from_the_world() {
        assert!(stand(0).y < -1000.0);
        assert!(stand(0).distance(stand(1)) >= STAND_SPACING);
    }

    #[test]
    fn the_view_carries_the_document_s_light() {
        let setup = UiInset { exposure: 1.5, backdrop: [0.1, 0.2, 0.3], ..UiInset::default() };
        let shot = shot(&setup, 0.0);
        let view = view(&shot, camera(&shot, stand(0)));
        assert_eq!(view.exposure, 1.5);
        assert_eq!(view.backdrop, Vec3::new(0.1, 0.2, 0.3));
        assert_eq!(view.ambient.intensity, setup.ambient);
        assert_eq!(view.size, (400, 600));
    }
}
