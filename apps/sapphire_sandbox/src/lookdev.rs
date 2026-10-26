//! The look-dev town (`--look-dev <shot>`, M1 X1): a town square in the anime-aesthetic look, a
//! character standing in it, and authored cameras to judge the look by. Every landed feature is
//! on: time of day and its sky, fog and fog cards, shadows, the environment family, post's
//! compositing layers, the outlines, and the character prepass's readers (the rim, the hair's
//! highlight, the bangs' shadow and eyes through the bangs).
//!
//! - **The town** is content: `content/lookdev/town/town.assembly.ron`, an assembly of Kenney's
//!   Fantasy Town Kit (`sapphire_pipeline::assembly`) cooked into one static mesh whose slots its
//!   sidecar binds to the town's environment materials ([`TOWN`]). It never moves, so it is
//!   extracted once.
//! - **The character** is pixiv's VRM sample (A6, [`CHARACTER`]) in place of the mannequin. VRM
//!   characters rest in a T-pose, so the stage holds a standing pose ([`held_pose`]): humanoid
//!   bones turned from rest, the rig's twist and aim constraints followed ([`crate::pose`]), and a
//!   soft expression. Its figure (head, feet and body capsule, its face's basis) comes from the
//!   posed skeleton. Its look-dev materials (`content/lookdev/character/`, bound by the VRM's
//!   sidecar) say which parts are face, hair and eyes, as MToon doesn't, so the character
//!   prepass's IDs tell them apart.
//! - **Shots** ([`Shot`]): a wide establishing shot across the square, a three-quarter hero shot
//!   of the character and a low angle up past her toward the sky and the rooftops. `--view`,
//!   `--fov` and `--zoom` adjust them as they do the turntable's framing.
//! - **Fog cards**, where frames draw fog: the courtyard's mist sheets and haze cards
//!   ([`crate::environment::FOG_CARDS`]), placed across the valley beyond the town and down the
//!   street out of the square.
//! - **Lamps**: a warm point light at each lantern's glass, its intensity following time of day's
//!   night ([`LAMPS`]), so the square lights up at dusk.
//!
//! The town's lanterns are placed in its assembly; [`LAMPS`] repeats their places, and a test
//! here checks the two against each other.

use sapphire_assets::AssetId;
use sapphire_assets::mesh::Mesh;
use sapphire_assets::rig::{ArchivedCharacterRig, HumanBone};
use sapphire_math::{Quat, Vec3};
use sapphire_platform::AppInitError;
use sapphire_render::{LightKind, LocalLight, Renderer};
use sapphire_rhi::GpuContext;

use crate::content::Content;
use crate::environment::{CardPlace, HAZE, MIST};
use crate::extract::RenderPose;
use crate::orbit::OrbitCamera;
use crate::pose::{self, RigJoints, Skeleton};
use crate::scene::Figure;

/// The town, by its sidecar's ID (`content/lookdev/town/town.assembly.ron.meta`).
pub(crate) const TOWN: AssetId = AssetId::from_text("1ff3290e-5fa9-4f61-b891-2ca6d76b285f");
/// The character: pixiv's `VRM1_Constraint_Twist_Sample`, by its sidecar's ID
/// (`content/third_party/vrm1_constraint_twist_sample/VRM1_Constraint_Twist_Sample.vrm.meta`).
pub(crate) const CHARACTER: AssetId = AssetId::from_text("6f8a81b6-29c4-485f-b161-847b001d561f");
/// The character's rig: the VRM's `rig` sub-asset, whose ID the pipeline derives from the VRM's
/// (`sapphire_pipeline::meta::sub_asset_id`; its look-dev tests pin this value).
pub(crate) const RIG: AssetId = AssetId::from_text("4b4073fd-44c5-8437-a712-1cccad486da3");

/// A look-dev camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shot {
    /// Across the square from its open side: the fountain, the market, the rows of houses, the
    /// gate and the tower over the roofs, the character small in the middle ground.
    Wide,
    /// The character in three-quarter view from her right at chest height on a long lens, the
    /// market behind her; at golden hour the sun comes from behind her left shoulder.
    Hero,
    /// Low and in front of the character, looking up past her at the rooftops and the sky.
    Sky,
}

impl Shot {
    pub const ALL: [Self; 3] = [Self::Wide, Self::Hero, Self::Sky];

    pub fn name(self) -> &'static str {
        match self {
            Self::Wide => "wide",
            Self::Hero => "hero",
            Self::Sky => "sky",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|shot| shot.name() == name)
    }

    /// The shot's camera: its target, yaw and pitch (radians; yaw orbits right from in front of
    /// the character, negative pitch looks up), distance in meters and vertical field of view.
    pub fn camera(self) -> OrbitCamera {
        let (target, yaw, pitch, distance, fov): (Vec3, f32, f32, f32, f32) = match self {
            Self::Wide => (Vec3::new(-1.0, 2.8, -9.0), 14.0, 1.5, 20.0, 50.0),
            Self::Hero => (Vec3::new(0.0, 1.2, 0.0), -38.0, 6.0, 3.0, 32.0),
            Self::Sky => (Vec3::new(0.0, 1.45, 0.0), -28.0, -18.0, 3.2, 58.0),
        };
        OrbitCamera::shot(target, yaw.to_radians(), pitch.to_radians(), distance, fov.to_radians())
    }
}

/// The lanterns' glass, where their lights are: each lantern's place in the town (x, z) at the
/// glass's height. The same places as the `@lantern` placements in the town's assembly.
pub(crate) const LAMPS: [[f32; 2]; 7] =
    [[-8.0, -2.0], [-1.0, -13.0], [8.0, -2.5], [-14.0, -10.0], [14.5, -14.0], [0.6, -28.0], [5.8, -38.0]];
/// The glass's height: 1.39 of the lantern's 1.556 at its scale of 1.6.
const LAMP_HEIGHT: f32 = 2.22;
/// A lamp's light at full night: a warm candle color, its reach and intensity.
const LAMP_COLOR: Vec3 = Vec3::new(1.0, 0.62, 0.3);
const LAMP_RANGE: f32 = 7.0;
pub(crate) const LAMP_INTENSITY: f32 = 2.5;

/// The lamps' lights at full intensity, by night: scale them by time of day's `day_night`.
pub(crate) fn lamps() -> impl Iterator<Item = LocalLight> {
    LAMPS.iter().map(|&[x, z]| LocalLight {
        position: Vec3::new(x, LAMP_HEIGHT, z),
        color: LAMP_COLOR,
        intensity: LAMP_INTENSITY,
        range: LAMP_RANGE,
        kind: LightKind::Point,
    })
}

/// The town's fog cards: mist sheets over the fields and hills beyond the town, and haze down
/// the street out of the square and among the rooftops behind the back row.
pub(crate) const CARDS: [CardPlace; 7] = [
    CardPlace { mesh: MIST, center: [10.0, 4.0, -85.0], size: [90.0, 8.0], yaw: 0.05 },
    CardPlace { mesh: MIST, center: [-40.0, 6.0, -120.0], size: [110.0, 12.0], yaw: 0.2 },
    CardPlace { mesh: MIST, center: [50.0, 10.0, -170.0], size: [160.0, 18.0], yaw: -0.15 },
    CardPlace { mesh: MIST, center: [-20.0, 16.0, -240.0], size: [260.0, 28.0], yaw: 0.0 },
    CardPlace { mesh: HAZE, center: [3.2, 2.2, -44.0], size: [10.0, 4.0], yaw: 0.0 },
    CardPlace { mesh: HAZE, center: [-4.0, 9.0, -36.0], size: [22.0, 6.0], yaw: 0.0 },
    CardPlace { mesh: HAZE, center: [12.0, 8.0, -40.0], size: [20.0, 5.0], yaw: 0.0 },
];

/// The held pose's turns, in the model's rest space (VRM: facing +Z, the character's left
/// toward +X): arms down at her sides and a little forward, elbows softly bent, the head turned a
/// little toward the hero shot, on her right.
fn turns() -> [(HumanBone, Quat); 7] {
    let degrees = f32::to_radians;
    [
        (
            HumanBone::LeftUpperArm,
            Quat::from_rotation_z(degrees(-76.0)) * Quat::from_rotation_y(degrees(-4.0)),
        ),
        (
            HumanBone::RightUpperArm,
            Quat::from_rotation_z(degrees(76.0)) * Quat::from_rotation_y(degrees(4.0)),
        ),
        (HumanBone::LeftLowerArm, Quat::from_rotation_y(degrees(-10.0))),
        (HumanBone::RightLowerArm, Quat::from_rotation_y(degrees(10.0))),
        (HumanBone::LeftHand, Quat::from_rotation_x(degrees(10.0))),
        (HumanBone::RightHand, Quat::from_rotation_x(degrees(10.0))),
        (HumanBone::Head, Quat::from_rotation_y(degrees(-10.0)) * Quat::from_rotation_x(degrees(4.0))),
    ]
}

/// The expression the character holds, and its weight: a soft, relaxed face.
const EXPRESSION: (&str, f32) = ("relaxed", 0.35);

/// The radius of the character's body capsule (M1 R4), from her feet to her head.
const BODY_RADIUS: f32 = 0.16;
/// From the head joint (the top of the neck) to the middle of the head, in meters.
const HEAD_CENTER: Vec3 = Vec3::new(0.0, 0.09, 0.01);

/// Loads the character's rig and holds the character in its pose ([`held_pose`]), from the copy
/// of its mesh `content` kept ([`Content::keep_character_mesh`]).
pub(crate) fn hold(
    content: &mut Content,
    renderer: &mut Renderer,
    gpu: &GpuContext,
) -> Result<(RenderPose, Figure), AppInitError> {
    content.load_rig(RIG, renderer, gpu)?;
    let mesh =
        content.character_mesh().ok_or_else(|| AppInitError::msg("the look-dev character isn't loaded"))?;
    let rig = content.rig().ok_or_else(|| AppInitError::msg("the look-dev character's rig isn't loaded"))?;
    held_pose(mesh, rig).map_err(AppInitError::msg)
}

/// The character's held pose and the figure it makes: the skinning palette and morph weights
/// for its mesh, and its head, feet, body and face basis in its own space. Fails if the mesh has
/// no skin or the rig no head.
pub(crate) fn held_pose(mesh: &Mesh, rig: &ArchivedCharacterRig) -> Result<(RenderPose, Figure), String> {
    check_counts(rig, mesh)?;
    let skin = mesh.skin.as_ref().ok_or("the look-dev character has no skin")?;
    let skeleton = Skeleton::new(skin);
    let joints = RigJoints::new(rig);
    let posed = pose::pose(&skeleton, &joints, &turns());
    let head_joint = joints.joint(HumanBone::Head).ok_or("the look-dev character's rig has no head")?;
    let head = posed.model_from_joint[head_joint];
    let (_, head_rotation, head_position) = head.to_scale_rotation_translation();
    // The head's basis turned by the held pose, from its rest orientation.
    let rest_head = pose::rest_rotation(&skeleton, head_joint);
    let face = (head_rotation * rest_head.inverse()).normalize();
    let feet = Vec3::new(head_position.x, mesh.streams.bounds.min[1].max(0.0), head_position.z);
    let figure = Figure { head: head_position + face * HEAD_CENTER, feet, body_radius: BODY_RADIUS, face };
    let mut morph_weights = vec![0.0; mesh.morph_targets.len()];
    let (name, weight) = EXPRESSION;
    if let Some(expression) = rig.expressions.iter().find(|expression| expression.name.as_str() == name) {
        for target in expression.morph_targets.iter() {
            if let Some(slot) = morph_weights.get_mut(target.target.to_native() as usize) {
                *slot += weight * target.weight.to_native();
            }
        }
    }
    Ok((RenderPose { palette: posed.palette, morph_weights }, figure))
}

/// Whether `rig` was cooked with `mesh`: the same joints and morph targets, which the pose
/// indexes one through the other.
fn check_counts(rig: &ArchivedCharacterRig, mesh: &Mesh) -> Result<(), String> {
    let joints = mesh.skin.as_ref().map_or(0, |skin| skin.joints.len());
    let counts = (rig.joint_count.to_native() as usize, rig.morph_target_count.to_native() as usize);
    if counts != (joints, mesh.morph_targets.len()) {
        return Err(format!(
            "the look-dev character's rig has {counts:?} joints and morph targets, its mesh {:?}",
            (joints, mesh.morph_targets.len())
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shots_parse_by_name() {
        for shot in Shot::ALL {
            assert_eq!(Shot::from_name(shot.name()), Some(shot));
        }
        assert_eq!(Shot::from_name("close"), None);
    }

    #[test]
    fn shots_see_the_character_from_the_front_and_the_sky_shot_looks_up() {
        for shot in Shot::ALL {
            let camera = shot.camera();
            let eye = camera.eye();
            assert!(eye.z > 0.0, "{shot:?} in front of her: {eye}");
            assert!(eye.x.atan2(eye.z).abs() < std::f32::consts::FRAC_PI_2, "{shot:?} sees her face");
        }
        assert!(Shot::Sky.camera().eye().y < 1.45, "below her chest, looking up");
        assert!(Shot::Wide.camera().eye().distance(Vec3::ZERO) > 10.0, "from across the square");
    }

    #[test]
    fn lamps_light_the_town_s_lanterns() {
        let town = include_str!("../../../content/lookdev/town/town.assembly.ron");
        let lanterns: Vec<[f32; 2]> = town
            .lines()
            .filter(|line| line.contains(r#"(piece: "@lantern""#))
            .map(|line| {
                let at = line.split("at: (").nth(1).expect("placed").split(')').next().expect("closed");
                let numbers: Vec<f32> = at.split(',').map(|n| n.trim().parse().expect("a number")).collect();
                [numbers[0], numbers[2]]
            })
            .collect();
        assert_eq!(lanterns, LAMPS);
        assert_eq!(lamps().count(), LAMPS.len());
    }
}
