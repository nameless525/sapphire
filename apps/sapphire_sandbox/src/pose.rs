//! A held pose for a skinned humanoid character (M1 X1's look-dev stage): humanoid bones turned
//! from the rest pose, then the rig's constraints and the palette that draws it, as
//! `sapphire_anim` runs them for a clip.
//!
//! VRM characters rest in a T-pose. A look-dev frame needs them standing as a character stands, so
//! the stage holds one pose: **turns** are rotations of humanoid bones ([`HumanBone`], found through
//! the character's rig), each given in the model's rest space about the bone's own joint:
//! `leftUpperArm` turned −70° about +Z lowers the arm whatever the joint's local axes are. A bone
//! under a turned bone turns with it, then by its own turn. Constraints follow after the turns
//! (`sapphire_anim::Constraints`), and the palette is each skin bind's
//! `model_from_joint × inverse_bind`, as compute skinning reads it
//! (`sapphire_render::SkinnedInstance`).

use sapphire_anim::CharacterPose;
use sapphire_assets::rig::{
    ArchivedCharacterRig, ArchivedConstraintKind, Constraint, ConstraintKind, HumanBone,
};
use sapphire_math::Quat;

/// What the pose reads of a character's rig: its humanoid bones' joints and its constraints.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RigJoints {
    humanoid: Vec<(HumanBone, usize)>,
    constraints: Vec<Constraint>,
}

impl RigJoints {
    pub fn new(rig: &ArchivedCharacterRig) -> Self {
        Self {
            humanoid: rig
                .humanoid
                .iter()
                .map(|entry| (entry.bone, usize::from(entry.joint.to_native())))
                .collect(),
            constraints: rig
                .constraints
                .iter()
                .map(|constraint| Constraint {
                    joint: constraint.joint.to_native(),
                    source: constraint.source.to_native(),
                    weight: constraint.weight.to_native(),
                    kind: match constraint.kind {
                        ArchivedConstraintKind::Roll { axis } => ConstraintKind::Roll { axis },
                        ArchivedConstraintKind::Aim { axis } => ConstraintKind::Aim { axis },
                        ArchivedConstraintKind::Rotation => ConstraintKind::Rotation,
                    },
                })
                .collect(),
        }
    }

    /// A rig of these bones and no constraints: a model with no VRM rig, whose joints were found by
    /// name (M2 O4's enemy).
    pub fn of_bones(humanoid: Vec<(HumanBone, usize)>) -> Self {
        Self { humanoid, constraints: Vec::new() }
    }

    /// The joint that is `bone`.
    pub fn joint(&self, bone: HumanBone) -> Option<usize> {
        self.humanoid.iter().find(|(entry, _)| *entry == bone).map(|&(_, joint)| joint)
    }

    /// The rig's constraints.
    pub fn constraints(&self) -> &[Constraint] {
        &self.constraints
    }
}

/// Holds `pose` by `turns` (humanoid bones' rotations in rest model space) from its rest pose,
/// following its rig's constraints. Bones the rig doesn't have are skipped.
pub(crate) fn hold(pose: &mut CharacterPose, rig: &RigJoints, turns: &[(HumanBone, Quat)]) {
    pose.reset();
    let count = pose.joint_count();
    // Turns in rest model space, moved into each joint's local frame: the parent's rest frame
    // sees the turn as `parentᵀ · turn · parent`.
    for &(bone, turn) in turns {
        let Some(joint) = rig.joint(bone).filter(|&joint| joint < count) else { continue };
        let parent = pose.parent(joint).map_or(Quat::IDENTITY, |parent| pose.rest_rotation(parent));
        let local = &mut pose.locals_mut()[joint];
        local.rotation = (parent.inverse() * turn * parent * local.rotation).normalize();
    }
    pose.finish();
}

#[cfg(test)]
mod tests {
    use sapphire_anim::{Joint, JointTransform, Skeleton};
    use sapphire_assets::mesh::SkinBind;
    use sapphire_assets::rig::{AimAxis, Axis};
    use sapphire_math::{Affine3A, Mat4, Vec3};

    use super::*;

    /// A shoulder at the origin with an upper arm along +X, a forearm 0.3 m on, and two helpers
    /// beside the upper arm: one aiming its +X at the forearm, one rolling with it about X.
    fn arm() -> (CharacterPose, RigJoints) {
        let joint = |name: &str, parent: Option<u16>, translation: [f32; 3]| Joint {
            name: name.to_owned(),
            parent,
            rest: JointTransform { translation, ..JointTransform::IDENTITY },
        };
        let joints = vec![
            joint("shoulder", None, [0.0, 1.4, 0.0]),
            joint("upper", Some(0), [0.1, 0.0, 0.0]),
            joint("lower", Some(1), [0.3, 0.0, 0.0]),
            joint("aim", Some(0), [0.1, 0.0, 0.0]),
            joint("roll", Some(1), [0.3, 0.0, 0.0]),
        ];
        // Inverse binds of the rest pose: each joint's model-space translation undone.
        let at = [[0.0, 1.4, 0.0], [0.1, 1.4, 0.0], [0.4, 1.4, 0.0], [0.1, 1.4, 0.0], [0.4, 1.4, 0.0]];
        let binds: Vec<SkinBind> = at
            .iter()
            .enumerate()
            .map(|(joint, &[x, y, z])| SkinBind {
                joint: joint as u16,
                inverse_bind_matrix: Mat4::from_translation(-Vec3::new(x, y, z)).to_cols_array_2d(),
            })
            .collect();
        let rig = RigJoints {
            humanoid: vec![(HumanBone::LeftUpperArm, 1), (HumanBone::LeftLowerArm, 2)],
            constraints: vec![
                Constraint {
                    joint: 3,
                    source: 2,
                    weight: 1.0,
                    kind: ConstraintKind::Aim { axis: AimAxis::PositiveX },
                },
                Constraint { joint: 4, source: 2, weight: 0.5, kind: ConstraintKind::Roll { axis: Axis::X } },
            ],
        };
        let bytes = Skeleton::new(&joints).to_cooked().expect("cooked");
        let skeleton = Skeleton::read(&bytes).expect("a valid skeleton");
        (CharacterPose::new(skeleton, &binds, rig.constraints(), Vec::new()), rig)
    }

    #[test]
    fn no_turns_draw_the_bind_pose() {
        let (mut pose, rig) = arm();
        hold(&mut pose, &rig, &[]);
        let mut palette = vec![Affine3A::ZERO; pose.palette_len()];
        pose.write_palette(&mut palette);
        assert!(palette.iter().all(|entry| entry.abs_diff_eq(Affine3A::IDENTITY, 1e-6)));
    }

    #[test]
    fn turns_lower_the_arm_and_helpers_follow() {
        let (mut pose, rig) = arm();
        let down = Quat::from_rotation_z(-70f32.to_radians());
        let twist = Quat::from_rotation_x(40f32.to_radians());
        hold(&mut pose, &rig, &[(HumanBone::LeftUpperArm, down), (HumanBone::LeftLowerArm, twist)]);
        let elbow = pose.model()[2].translation;
        let expected = Vec3::new(0.1, 1.4, 0.0) + down * Vec3::new(0.3, 0.0, 0.0);
        assert!(Vec3::from(elbow).abs_diff_eq(expected, 1e-5), "{elbow} against {expected}");
        // The aim helper points its +X at the elbow, so a sleeve weighted to it follows the arm.
        let along = pose.model()[3].transform_vector3(Vec3::X).normalize();
        assert!(along.abs_diff_eq((expected - Vec3::new(0.1, 1.4, 0.0)).normalize(), 1e-4), "{along}");
        // The roll helper takes half the forearm's twist about X.
        let roll = pose.model()[4].matrix3;
        let upper = pose.model()[1].matrix3;
        let relative = Quat::from_mat3a(&(upper.inverse() * roll));
        assert!(relative.abs_diff_eq(Quat::from_rotation_x(20f32.to_radians()), 1e-4), "{relative}");
        // Skinning moves a bind-pose point on the forearm with it.
        let mut palette = vec![Affine3A::IDENTITY; pose.palette_len()];
        pose.write_palette(&mut palette);
        let wrist = Vec3::new(0.6, 1.4, 0.0);
        let skinned = palette[2].transform_point3(wrist);
        assert!((skinned.distance(Vec3::from(elbow)) - 0.2).abs() < 1e-5, "{skinned}");
    }
}
