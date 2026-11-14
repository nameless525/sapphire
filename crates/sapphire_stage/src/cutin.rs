//! Cut-ins' stands (M5 L10): the character a cut-in shows, standing in a studio of its own, posed by the cut-in's
//! clip on the cut-in's clock, and shot through the renderer's inset path.
//!
//! A stand is an ordinary character entity ([`Scene::stand_cutin`]) with a [`CutInStand`]: a copy of the
//! fighter's mesh, light rig, figure and pose when it's stood, then posed by a clip player of its own
//! ([`Scene::pose_cutin`]), its springs running, apart from the fighter, whose leaving it outlives. It stands at
//! [`stand`], far below the world and apart from showcases' stands, where no view, shadow, probe or local light
//! of the world reaches it: so it receives no shadow, as a showcase doesn't.
//!
//! Each frame the show says what its interface drew of the cut-in ([`SceneShow::cutin_shot`]), and
//! [`Scene::frame_packet`] turns it into an [`InsetView`] numbered [`CUTIN_INSET`]: a camera at the document's
//! offset from the posed joint, rolled and of its field of view, the stand lit by the document's rig and key
//! for that camera, and the backdrop's gradient and lines. The renderer draws it every frame, at the tier's
//! cut-in size. Nothing of this exists while no cut-in plays.
//!
//! [`Scene::stand_cutin`]: crate::Scene::stand_cutin
//! [`Scene::pose_cutin`]: crate::Scene::pose_cutin
//! [`Scene::frame_packet`]: crate::Scene::frame_packet
//! [`SceneShow::cutin_shot`]: crate::SceneShow::cutin_shot

use sapphire_assets::AssetId;
use sapphire_ecs::prelude::*;
use sapphire_math::{Quat, Vec2, Vec3};
use sapphire_render::{
    AmbientLight, CameraData, CharacterLight, DirectionalLight, EffectCoverage, InsetCutIn, InsetView,
    SpeedLines,
};
use sapphire_world::cutin::{CutIn, CutInJoint, CutInLines};

use crate::animate::ClipPlayer;

/// The inset number a cut-in's view and its quads take: past any a screen's document names.
pub const CUTIN_INSET: u16 = u16::MAX;

/// Where a cut-in's stand stands: its feet's point, as far below the world as showcases' and apart from them.
#[must_use]
pub fn stand() -> Vec3 {
    Vec3::new(0.0, -2000.0, 300.0)
}

/// The near plane of a cut-in's camera, in meters: it comes close to a face.
const NEAR: f32 = 0.02;

/// A cut-in's stand: the clip it plays and its player, and where its framed joint is, as of its last pose.
#[derive(Component)]
pub struct CutInStand {
    pub(crate) player: ClipPlayer,
    /// The kind it stands as, whose graph's clips hold its clip.
    pub(crate) kind: AssetId,
    pub(crate) clip: AssetId,
    pub(crate) start: f32,
    pub(crate) rate: f32,
    /// The joints its camera may frame, in [`CutInJoint`]'s order.
    pub(crate) joints: [Option<usize>; 5],
    pub(crate) joint: CutInJoint,
    /// How far its camera stands off the framed joint, in meters: how finely its springs' clearance solves.
    pub(crate) reach: f32,
    /// The framed joint in the world, as of its last pose: the stand's feet before one.
    pub(crate) framed: Vec3,
}

impl CutInStand {
    /// The joint the camera frames, by its place among those a stand knows.
    pub(crate) fn framed_joint(&self) -> Option<usize> {
        let place = match self.joint {
            CutInJoint::Head => 0,
            CutInJoint::Chest => 1,
            CutInJoint::Hips => 2,
            CutInJoint::RightHand => 3,
            CutInJoint::LeftHand => 4,
        };
        self.joints[place].or(self.joints[0])
    }
}

/// What a show's interface drew of its cut-in this frame (M5 L10): what [`crate::Scene::frame_packet`] shoots.
#[derive(Clone, Copy, Debug)]
pub struct CutInShot<'a> {
    pub cutin: &'a CutIn,
    /// Its picture's size in pixels.
    pub size: (u32, u32),
    /// Seconds since it started, on the presentation's clock.
    pub seconds: f32,
    /// Flashes reduced: its backdrop's lines hold one drawing.
    pub still: bool,
}

/// The camera of `shot`, framing the joint at `framed` in the stand's space.
#[must_use]
pub fn camera(shot: &CutInShot<'_>, framed: Vec3) -> CameraData {
    let camera = &shot.cutin.camera;
    let eye = framed + Vec3::from(camera.offset);
    let target = framed + Vec3::from(camera.look);
    let forward = (target - eye).normalize_or(Vec3::NEG_Z);
    // Clockwise in the picture, as a viewer sees it, is a turn about the way the camera looks.
    let up = Quat::from_axis_angle(forward, camera.roll.to_radians()) * Vec3::Y;
    let aspect = shot.size.0 as f32 / shot.size.1.max(1) as f32;
    CameraData::look_at(eye, target, up, camera.fov.to_radians(), aspect, NEAR)
}

/// The view the renderer draws `shot` from, before its instances are named.
#[must_use]
pub fn view(shot: &CutInShot<'_>, camera: CameraData) -> InsetView {
    let CutIn { light, backdrop, .. } = shot.cutin;
    let ambient = AmbientLight {
        sky: light.ambient_sky.into(),
        ground: light.ambient_ground.into(),
        intensity: light.ambient,
    };
    let toward = match backdrop.to {
        Some(_) => {
            let (sin, cos) = backdrop.angle.to_radians().sin_cos();
            Vec2::new(cos, sin) / (cos.abs() + sin.abs()).max(1e-6)
        }
        None => Vec2::ZERO,
    };
    let lines = backdrop.lines.as_ref().map_or(SpeedLines::NONE, |lines| speed_lines(lines, shot));
    let cutin = InsetCutIn { backdrop_far: backdrop.to.unwrap_or(backdrop.color).into(), toward, lines };
    InsetView {
        exposure: light.exposure,
        backdrop: backdrop.color.into(),
        cutin: Some(cutin),
        ..InsetView::new(CUTIN_INSET, camera, shot.size, ambient)
    }
}

/// The backdrop's lines at `shot`'s moment: redrawn `rate` times a second, or one drawing held with flashes
/// reduced.
fn speed_lines(lines: &CutInLines, shot: &CutInShot<'_>) -> SpeedLines {
    let drawing = if shot.still { 0 } else { (shot.seconds.max(0.0) * lines.rate) as u32 };
    SpeedLines {
        opacity: lines.opacity,
        center: Vec2::from(lines.center),
        count: lines.count,
        width: lines.width,
        inner_radius: lines.inner_radius,
        spread: lines.spread,
        color: lines.color.into(),
        seed: drawing.wrapping_mul(0x9E37_79B9).wrapping_add(1),
        coverage: EffectCoverage::WORLD,
    }
}

/// The key light the stand's rig turns, and its character light: the document's, from above the camera's
/// shoulder, as a showcase's.
#[must_use]
pub fn lights(cutin: &CutIn) -> (DirectionalLight, CharacterLight) {
    let light = &cutin.light;
    let color = Vec3::from(light.light);
    let key = DirectionalLight { direction: Vec3::new(0.3, -0.8, -0.5), color, intensity: light.intensity };
    (key, CharacterLight { color, intensity: light.intensity })
}

#[cfg(test)]
mod tests {
    use sapphire_world::cutin::CutInLibrary;

    use super::*;

    fn ultimate() -> CutIn {
        CutInLibrary::default_cutins().find("ultimate", None).expect("shipped").clone()
    }

    #[test]
    fn the_camera_sits_off_the_joint_and_looks_by_the_document() {
        let cutin = ultimate();
        let shot = CutInShot { cutin: &cutin, size: (1920, 700), seconds: 0.0, still: false };
        let framed = stand() + Vec3::new(0.0, 1.6, 0.0);
        let framing = camera(&shot, framed);
        assert!((framing.position - (framed + Vec3::from(cutin.camera.offset))).length() < 1e-4);
        assert!((framing.proj.y_axis.y - 1.0 / (cutin.camera.fov.to_radians() * 0.5).tan()).abs() < 1e-3);
        // Unrolled, the camera's right stays level; rolled, it tips.
        let level = CutIn {
            camera: sapphire_world::cutin::CutInCamera { roll: 0.0, ..cutin.camera },
            ..cutin.clone()
        };
        let level = camera(&CutInShot { cutin: &level, ..shot }, framed);
        assert!(level.view.row(0).y.abs() < 1e-4, "{:?}", level.view);
        assert!(framing.view.row(0).y.abs() > 0.05, "{:?}", framing.view);
    }

    #[test]
    fn the_view_carries_the_backdrop_and_its_lines_hold_still_with_flashes_reduced() {
        let cutin = ultimate();
        let shot = CutInShot { cutin: &cutin, size: (800, 300), seconds: 0.3, still: false };
        let view = view(&shot, camera(&shot, stand()));
        assert_eq!(view.inset, CUTIN_INSET);
        assert_eq!(view.backdrop, Vec3::from(cutin.backdrop.color));
        let look = view.cutin.expect("a cut-in's");
        assert_eq!(look.backdrop_far, Vec3::from(cutin.backdrop.to.expect("a gradient")));
        // A unit direction over its extent across the picture.
        let toward = look.toward;
        assert!(((toward.x.abs() + toward.y.abs()) - 1.0).abs() < 1e-4 && toward.x > 0.0 && toward.y > 0.0);
        let later = CutInShot { seconds: 0.4, ..shot };
        let moved = super::view(&later, camera(&later, stand())).cutin.expect("a cut-in's").lines.seed;
        assert_ne!(look.lines.seed, moved, "redrawn at its rate");
        let still = |seconds| {
            let shot = CutInShot { seconds, still: true, ..shot };
            super::view(&shot, camera(&shot, stand())).cutin.expect("a cut-in's").lines.seed
        };
        assert_eq!(still(0.3), still(0.4));
    }
}
