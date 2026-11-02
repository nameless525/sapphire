//! The enemy on the look-dev stage (`--look-dev enemy`, M2 O4): KayKit's skeleton warrior,
//! re-proportioned and in its anime character materials (`content/combat/enemy/`), squaring up to the
//! character in the town under the stage's day, so the two looks can be judged side by side at combat
//! distance.
//!
//! Nothing plays its clips yet, which cook onto it by retarget (`sapphire_pipeline::combat::warrior`),
//! so it holds one pose, as the character does without `--clip` ([`held`]): a [`CharacterPose`] with
//! the rig's bones turned from its T-pose by [`pose::hold`], found by name rather than through a VRM's
//! rig. The rig has no twist helpers, so no constraints follow.
//!
//! - **Scale:** [`SCALE`]: see there.
//! - **The blade** is a mesh of its own, placed once at the rig's right hand slot ([`HAND_SLOT`]). No
//!   attachment system exists, and a held pose doesn't need one: when clips play, whoever poses the
//!   warrior moves the blade by `place × model_from_joint[hand slot]` each frame. It draws as a
//!   character with no body, so it casts no capsule of its own, and its light is judged at the
//!   warrior's head, so the two are lit alike.
//!
//! Hit volumes and timing are the moves' (`warrior.moves.ron`): nothing here reads them.

use sapphire_anim::{ArchivedSkeleton, CharacterPose};
use sapphire_assets::AssetId;
use sapphire_assets::mesh::Skin;
use sapphire_assets::rig::HumanBone;
use sapphire_math::{Affine3A, Mat4, Quat, Vec3};
use sapphire_render::{FaceExpression, MeshHandle};

use crate::extract::RenderPose;
use crate::pose::{self, RigJoints};
use crate::scene::Figure;

/// The warrior, by its sidecar's ID (`content/combat/enemy/warrior.glb.meta`).
pub(crate) const WARRIOR: AssetId = AssetId::from_text("322aa57e-957d-45f1-8d51-0c1d80f5d1d6");
/// Its blade, by its sidecar's ID (`content/third_party/kaykit_skeletons/assets/Skeleton_Blade.gltf.meta`).
pub(crate) const BLADE: AssetId = AssetId::from_text("1a612690-4615-4346-86f0-c903784d2db5");

/// The joint the blade grips, in the rig's own names.
pub(crate) const HAND_SLOT: &str = "handslot.r";

/// The derived model (`sapphire_pipeline::combat::warrior`) stands 2.33 units to its crown and 2.59
/// to its horns' tips. This puts the crown at 1.87 m and the horns at 2.07 m against the character's
/// 1.62 m: a head taller, as the reference fights' common enemies stand, with a blade whose reach,
/// 1.8 m with the arm straight, is about what its moves author (`warrior.moves.ron`).
pub(crate) const SCALE: f32 = 0.8;

/// Where it stands on the stage, in metres from the character at the origin: to the character's right
/// and a little in front, at the distance its quick cut reaches from, turned toward the character and
/// partly toward the shot, so its eyes show.
const STANDS_AT: Vec3 = Vec3::new(-2.0, 0.0, 0.5);
const FACES: f32 = 52.0;

/// The rig's bones the pose turns, by the rig's own names.
const BONES: [(&str, HumanBone); 13] = [
    ("spine", HumanBone::Spine),
    ("chest", HumanBone::Chest),
    ("head", HumanBone::Head),
    ("upperarm.l", HumanBone::LeftUpperArm),
    ("lowerarm.l", HumanBone::LeftLowerArm),
    ("wrist.l", HumanBone::LeftHand),
    ("upperarm.r", HumanBone::RightUpperArm),
    ("lowerarm.r", HumanBone::RightLowerArm),
    ("wrist.r", HumanBone::RightHand),
    ("upperleg.l", HumanBone::LeftUpperLeg),
    ("upperleg.r", HumanBone::RightUpperLeg),
    ("foot.l", HumanBone::LeftFoot),
    ("foot.r", HumanBone::RightFoot),
];

/// The held pose's turns, in the model's rest space (facing +Z, its left toward +X): hunched forward
/// over a wide stance, the left arm hanging out from its side, and the right held out with the forearm
/// level and the blade raised, which makes the silhouette. In the rest pose the blade points along
/// +Z, so the last turn is a roll of the arm about its bone.
fn turns() -> [(HumanBone, Quat); 12] {
    let degrees = f32::to_radians;
    [
        (HumanBone::Spine, Quat::from_rotation_x(degrees(10.0))),
        (HumanBone::Chest, Quat::from_rotation_x(degrees(8.0)) * Quat::from_rotation_y(degrees(-10.0))),
        (HumanBone::Head, Quat::from_rotation_x(degrees(-4.0)) * Quat::from_rotation_y(degrees(8.0))),
        (
            HumanBone::LeftUpperArm,
            Quat::from_rotation_z(degrees(-62.0)) * Quat::from_rotation_y(degrees(-12.0)),
        ),
        (HumanBone::LeftLowerArm, Quat::from_rotation_y(degrees(-22.0))),
        (HumanBone::LeftHand, Quat::from_rotation_x(degrees(10.0))),
        (
            HumanBone::RightUpperArm,
            Quat::from_rotation_y(degrees(-8.0)) * Quat::from_rotation_z(degrees(52.0)),
        ),
        (
            HumanBone::RightLowerArm,
            Quat::from_rotation_z(degrees(-58.0)) * Quat::from_rotation_y(degrees(22.0)),
        ),
        (HumanBone::RightHand, Quat::from_rotation_x(degrees(-84.0))),
        (HumanBone::LeftUpperLeg, Quat::from_rotation_z(degrees(6.0))),
        (HumanBone::RightUpperLeg, Quat::from_rotation_z(degrees(-6.0))),
        (HumanBone::RightFoot, Quat::from_rotation_y(degrees(-14.0))),
    ]
}

/// The radius of its body capsule (M1 R4), from its feet to its head.
const BODY_RADIUS: f32 = 0.3;
/// From the head joint to the middle of the skull, in the model's units.
const HEAD_CENTER: Vec3 = Vec3::new(0.0, 0.27, 0.03);

/// The enemy's meshes on a renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EnemyMeshes {
    pub warrior: MeshHandle,
    pub blade: MeshHandle,
}

/// The enemy, ready to place: its held pose, the figure it makes, and where the pose puts the blade.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Held {
    pub pose: RenderPose,
    pub figure: Figure,
    /// The blade's mesh in the warrior's model space.
    pub blade: Mat4,
}

/// Where the warrior stands on the stage: the model's space to the world.
pub(crate) fn place() -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::splat(SCALE),
        Quat::from_rotation_y(FACES.to_radians()),
        STANDS_AT,
    )
}

/// The warrior's held pose and what it makes, from its cooked skin and skeleton. Fails if the rig
/// lacks a joint the pose needs.
pub(crate) fn held(skin: &Skin, skeleton: &ArchivedSkeleton) -> Result<Held, String> {
    let find = |name: &str| skeleton.find(name).ok_or_else(|| format!("the warrior's rig has no {name:?}"));
    let bones =
        BONES.iter().map(|&(name, bone)| Ok((bone, find(name)?))).collect::<Result<Vec<_>, String>>()?;
    let hand_slot = find(HAND_SLOT)?;
    let joints = RigJoints::of_bones(bones);
    let head_joint = joints.joint(HumanBone::Head).ok_or("the warrior's rig has no head")?;
    let mut pose = CharacterPose::new(skeleton, &skin.binds, joints.constraints(), Vec::new());
    pose::hold(&mut pose, &joints, &turns());
    let (_, head_rotation, head) = pose.model()[head_joint].to_scale_rotation_translation();
    let figure = Figure {
        head: head + head_rotation * HEAD_CENTER,
        feet: Vec3::new(head.x, 0.0, head.z),
        body_radius: BODY_RADIUS,
        face: Quat::IDENTITY,
        expression: FaceExpression { blush: 0.0, gloom: 0.0 },
    };
    let blade = Mat4::from(pose.model()[hand_slot]);
    let mut palette = vec![Affine3A::IDENTITY; pose.palette_len()];
    pose.write_palette(&mut palette);
    Ok(Held { pose: RenderPose { palette, morph_weights: Vec::new() }, figure, blade })
}

/// The figure the blade draws as, in its own space: no body, so it adds no capsule to the character's
/// shadow or occlusion, and its head where the warrior's is, so the light rig lights it as it does
/// the warrior.
pub(crate) fn blade_figure(held: &Held) -> Figure {
    let head = held.blade.inverse().transform_point3(held.figure.head);
    Figure { head, feet: head, body_radius: 0.0, face: Quat::IDENTITY, expression: held.figure.expression }
}

#[cfg(test)]
mod tests {
    use sapphire_anim::{Joint, JointTransform};
    use sapphire_assets::mesh::{Influences, SkinBind};

    use super::*;

    /// The rig's named joints in a chain, each 0.1 up its parent.
    fn rig() -> Vec<Joint> {
        let names = BONES.iter().map(|&(name, _)| name).chain([HAND_SLOT]);
        names
            .enumerate()
            .map(|(index, name)| Joint {
                name: name.to_owned(),
                parent: index.checked_sub(1).map(|parent| parent as u16),
                rest: JointTransform { translation: [0.0, 0.1, 0.0], ..JointTransform::IDENTITY },
            })
            .collect()
    }

    fn skin_over(count: usize) -> Skin {
        let binds = (0..count)
            .map(|joint| SkinBind {
                joint: joint as u16,
                inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d(),
            })
            .collect();
        Skin { skeleton: None, joint_count: count as u32, binds, influences: Influences::pack(&[], count) }
    }

    #[test]
    fn it_holds_a_pose_with_the_blade_at_the_hand_slot() {
        let bytes = sapphire_anim::Skeleton::new(&rig()).to_cooked().expect("cooked");
        let skeleton = sapphire_anim::Skeleton::read(&bytes).expect("a valid skeleton");
        let held = held(&skin_over(skeleton.len()), skeleton).expect("every joint is there");
        assert_eq!(held.pose.palette.len(), skeleton.len());
        assert_eq!(held.figure.feet.y, 0.0);
        assert!(held.figure.head.y > 0.0);
        // The blade's figure names the warrior's head from the blade's own space.
        let head = held.blade.transform_point3(blade_figure(&held).head);
        assert!(head.distance(held.figure.head) < 1e-3);
        assert_eq!(blade_figure(&held).body_radius, 0.0, "it adds no capsule");
    }

    #[test]
    fn a_rig_without_the_hand_slot_says_so() {
        let joints = [Joint { name: "hips".to_owned(), parent: None, rest: JointTransform::IDENTITY }];
        let bytes = sapphire_anim::Skeleton::new(&joints).to_cooked().expect("cooked");
        let skeleton = sapphire_anim::Skeleton::read(&bytes).expect("a valid skeleton");
        let error = held(&skin_over(1), skeleton).expect_err("no joints");
        assert!(error.contains("no \"spine\""), "{error}");
    }

    /// A head taller than the character, and standing within the reach of its quick cut.
    #[test]
    fn it_stands_a_head_taller_than_the_character_at_combat_distance() {
        let (crown, horns) = (2.33 * SCALE, 2.59 * SCALE);
        assert!((1.75..1.95).contains(&crown) && horns < 2.2, "{crown} and {horns} m against 1.62 m");
        let distance = STANDS_AT.length();
        assert!((1.5..2.6).contains(&distance), "{distance} m from the character");
    }
}
