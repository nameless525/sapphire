//! A humanoid for the tests that time and count IK: the bones IK reads, then `extra` joints hanging
//! from the head, as a character's hair and helpers do.

use sapphire_anim::{CharacterIk, CharacterPose, FootSettings, IkRig, Joint, JointTransform, LookSettings, Skeleton};
use sapphire_assets::rig::HumanBone;
use sapphire_math::Vec3;

const SKELETON: [(HumanBone, Option<usize>, [f32; 3]); 21] = [
    (HumanBone::Hips, None, [0.0, 0.9, 0.0]),
    (HumanBone::Spine, Some(0), [0.0, 0.1, 0.0]),
    (HumanBone::Chest, Some(1), [0.0, 0.15, 0.0]),
    (HumanBone::UpperChest, Some(2), [0.0, 0.1, 0.0]),
    (HumanBone::Neck, Some(3), [0.0, 0.15, 0.0]),
    (HumanBone::Head, Some(4), [0.0, 0.1, 0.0]),
    (HumanBone::LeftUpperLeg, Some(0), [0.1, 0.0, 0.0]),
    (HumanBone::LeftLowerLeg, Some(6), [0.0, -0.39, 0.03]),
    (HumanBone::LeftFoot, Some(7), [0.0, -0.39, -0.03]),
    (HumanBone::LeftToes, Some(8), [0.0, -0.06, 0.12]),
    (HumanBone::RightUpperLeg, Some(0), [-0.1, 0.0, 0.0]),
    (HumanBone::RightLowerLeg, Some(10), [0.0, -0.39, 0.03]),
    (HumanBone::RightFoot, Some(11), [0.0, -0.39, -0.03]),
    (HumanBone::RightToes, Some(12), [0.0, -0.06, 0.12]),
    (HumanBone::LeftUpperArm, Some(3), [0.2, 0.1, 0.0]),
    (HumanBone::LeftLowerArm, Some(14), [0.25, 0.0, -0.02]),
    (HumanBone::LeftHand, Some(15), [0.25, 0.0, 0.02]),
    (HumanBone::RightUpperArm, Some(3), [-0.2, 0.1, 0.0]),
    (HumanBone::RightLowerArm, Some(17), [-0.25, 0.0, -0.02]),
    (HumanBone::RightHand, Some(18), [-0.25, 0.0, 0.02]),
    (HumanBone::Jaw, Some(5), [0.0, 0.0, 0.05]),
];

/// The skeleton and which joint is which bone.
pub fn humanoid(extra: usize) -> (Skeleton, Vec<(HumanBone, usize)>) {
    let mut joints: Vec<Joint> = SKELETON
        .iter()
        .map(|&(_, parent, translation)| Joint {
            name: String::new(),
            parent: parent.map(|parent| parent as u16),
            rest: JointTransform { translation, ..JointTransform::IDENTITY },
        })
        .collect();
    for strand in 0..extra {
        joints.push(Joint {
            name: String::new(),
            parent: Some(if strand % 6 == 0 { 5 } else { (joints.len() - 1) as u16 }),
            rest: JointTransform { translation: [0.01, -0.03, 0.0], ..JointTransform::IDENTITY },
        });
    }
    let bones = SKELETON.iter().enumerate().map(|(joint, &(bone, _, _))| (bone, joint)).collect();
    (Skeleton::new(&joints), bones)
}

/// IK for `pose`, a pose of [`humanoid`]'s skeleton.
pub fn ik(pose: &CharacterPose, bones: &[(HumanBone, usize)]) -> CharacterIk {
    let rig = IkRig::new(pose, bones, Vec3::new(0.0, 0.06, 0.06), None, 0.0);
    CharacterIk::new(rig, FootSettings::default(), LookSettings::default())
}
