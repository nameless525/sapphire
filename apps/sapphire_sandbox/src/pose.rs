//! A held pose for a skinned humanoid character (M1 X1's look-dev stage): humanoid bones turned
//! from the rest pose, the rig's constraints followed, and the skinning palette that draws it.
//!
//! VRM characters rest in a T-pose, and nothing animates them until `sapphire_anim` (M2). A
//! look-dev frame needs them standing as a character stands, so the stage holds one pose:
//!
//! - **Turns** are rotations of humanoid bones ([`HumanBone`], found through the character's
//!   rig), each given in the model's rest space about the bone's own joint: `leftUpperArm`
//!   turned −70° about +Z lowers the arm whatever the joint's local axes are. A bone under a
//!   turned bone turns with it, then by its own turn.
//! - **Constraints** are the rig's (VRM's `VRMC_node_constraint`): twist and aim helpers that
//!   the skin is weighted to. Without them a lowered arm would leave its sleeve in the T-pose.
//!   Each constrained joint follows its source after the turns, sources first: an aim joint
//!   turns its axis toward the source by the shortest arc from its rest direction, under its
//!   parent's current orientation; a roll joint takes the twist about its axis of the source's
//!   rotation from rest (its swing-twist decomposition), a rotation joint the whole of it, each
//!   by the constraint's weight.
//! - **The palette** is each skin bind's `model_from_joint × inverse_bind`, as compute skinning
//!   reads it (`sapphire_render::SkinnedInstance`).
//!
//! # Later milestones
//!
//! - Animation (`sapphire_anim`, M2) poses characters from clips and evaluates constraints
//!   every frame; this held pose then becomes a look-dev clip of one frame.

use sapphire_assets::mesh::{JointTransform, Skin};
use sapphire_assets::rig::{
    AimAxis, ArchivedCharacterRig, ArchivedConstraintKind, Axis, Constraint, ConstraintKind, HumanBone,
};
use sapphire_math::{Affine3A, Mat4, Quat, Vec3};

/// A skeleton as the pose needs it: each joint's parent and rest transform, and the skin's
/// binds.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Skeleton {
    parents: Vec<Option<usize>>,
    rest: Vec<(Vec3, Quat, Vec3)>,
    /// Per palette entry: its joint and inverse bind matrix.
    binds: Vec<(usize, Affine3A)>,
}

impl Skeleton {
    pub fn new(skin: &Skin) -> Self {
        let rest = |transform: &JointTransform| {
            (
                Vec3::from(transform.translation),
                Quat::from_array(transform.rotation).normalize(),
                Vec3::from(transform.scale),
            )
        };
        Self {
            parents: skin.joints.iter().map(|joint| joint.parent.map(usize::from)).collect(),
            rest: skin.joints.iter().map(|joint| rest(&joint.rest)).collect(),
            binds: skin
                .binds
                .iter()
                .map(|bind| {
                    let matrix = Mat4::from_cols_array_2d(&bind.inverse_bind_matrix);
                    (usize::from(bind.joint), Affine3A::from_mat4(matrix))
                })
                .collect(),
        }
    }

    pub fn joint_count(&self) -> usize {
        self.parents.len()
    }
}

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

    /// The joint that is `bone`.
    pub fn joint(&self, bone: HumanBone) -> Option<usize> {
        self.humanoid.iter().find(|(entry, _)| *entry == bone).map(|&(_, joint)| joint)
    }
}

/// A posed skeleton: every joint's model-space transform, and the palette.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Posed {
    pub model_from_joint: Vec<Affine3A>,
    pub palette: Vec<Affine3A>,
}

/// Poses `skeleton` by `turns` (humanoid bones' rotations in rest model space), following the
/// rig's constraints. Bones the rig doesn't have are skipped.
pub(crate) fn pose(skeleton: &Skeleton, rig: &RigJoints, turns: &[(HumanBone, Quat)]) -> Posed {
    let count = skeleton.joint_count();
    let rest_local: Vec<Quat> = skeleton.rest.iter().map(|&(_, rotation, _)| rotation).collect();
    let rest_world = world_rotations(skeleton, &rest_local);
    // Turns in rest model space, moved into each joint's local frame: the parent's rest frame
    // sees the turn as `parentᵀ · turn · parent`.
    let mut local = rest_local.clone();
    for &(bone, turn) in turns {
        let Some(joint) = rig.joint(bone).filter(|&joint| joint < count) else { continue };
        let parent = skeleton.parents[joint].map_or(Quat::IDENTITY, |parent| rest_world[parent]);
        local[joint] = (parent.inverse() * turn * parent * local[joint]).normalize();
    }
    // Constraints in order: each after every other that moves its source, its source's
    // ancestors or its own ancestors, whose transforms it reads (the rig has no cycles).
    let mut pending: Vec<&Constraint> = rig.constraints.iter().collect();
    let within = |ancestor: usize, mut joint: usize| loop {
        if joint == ancestor {
            return true;
        }
        match skeleton.parents.get(joint).copied().flatten() {
            Some(parent) => joint = parent,
            None => return false,
        }
    };
    let waits = |constraint: &Constraint, pending: &[&Constraint]| {
        let (joint, source) = (usize::from(constraint.joint), usize::from(constraint.source));
        pending.iter().any(|other| {
            let moved = usize::from(other.joint);
            moved != joint && (within(moved, source) || within(moved, joint))
        })
    };
    while !pending.is_empty() {
        let ready = pending.iter().position(|constraint| !waits(constraint, &pending)).unwrap_or(0);
        let constraint = pending.remove(ready);
        let (joint, source) = (usize::from(constraint.joint), usize::from(constraint.source));
        if joint >= count || source >= count {
            continue;
        }
        let weight = constraint.weight.clamp(0.0, 1.0);
        let rest = rest_local[joint];
        // The source's rotation from rest, in its own frame (VRMC_node_constraint 1.0).
        let delta = rest_local[source].inverse() * local[source];
        let target = match constraint.kind {
            ConstraintKind::Roll { axis } => {
                let axis = match axis {
                    Axis::X => Vec3::X,
                    Axis::Y => Vec3::Y,
                    Axis::Z => Vec3::Z,
                };
                // The delta in the source's parent frame, seen from the joint's rest frame.
                let in_joint = rest.inverse() * (local[source] * rest_local[source].inverse()) * rest;
                rest * twist(in_joint, axis)
            }
            ConstraintKind::Rotation => rest * delta,
            ConstraintKind::Aim { axis } => {
                let world = world_rotations(skeleton, &local);
                let model = model_transforms(skeleton, &local);
                let parent = skeleton.parents[joint].map_or(Quat::IDENTITY, |parent| world[parent]);
                let from = (parent * rest * aim_axis(axis)).normalize();
                let to = Vec3::from(model[source].translation - model[joint].translation).normalize_or_zero();
                if to == Vec3::ZERO {
                    rest
                } else {
                    parent.inverse() * Quat::from_rotation_arc(from, to) * parent * rest
                }
            }
        };
        local[joint] = rest.slerp(target.normalize(), weight).normalize();
    }
    let model_from_joint = model_transforms(skeleton, &local);
    let palette =
        skeleton.binds.iter().map(|&(joint, inverse_bind)| model_from_joint[joint] * inverse_bind).collect();
    Posed { model_from_joint, palette }
}

/// The model-space rotation of `joint` in the rest pose.
pub(crate) fn rest_rotation(skeleton: &Skeleton, joint: usize) -> Quat {
    let rest_local: Vec<Quat> = skeleton.rest.iter().map(|&(_, rotation, _)| rotation).collect();
    world_rotations(skeleton, &rest_local).get(joint).copied().unwrap_or(Quat::IDENTITY)
}

/// Model-space rotations of every joint, from local rotations.
fn world_rotations(skeleton: &Skeleton, local: &[Quat]) -> Vec<Quat> {
    let mut world: Vec<Quat> = Vec::with_capacity(local.len());
    for (joint, &rotation) in local.iter().enumerate() {
        let parent = skeleton.parents[joint].map_or(Quat::IDENTITY, |parent| world[parent]);
        world.push(parent * rotation);
    }
    world
}

/// Model-space transforms of every joint, from local rotations and the rest translations and
/// scales.
fn model_transforms(skeleton: &Skeleton, local: &[Quat]) -> Vec<Affine3A> {
    let mut model: Vec<Affine3A> = Vec::with_capacity(local.len());
    for (joint, &rotation) in local.iter().enumerate() {
        let (translation, _, scale) = skeleton.rest[joint];
        let own = Affine3A::from_scale_rotation_translation(scale, rotation, translation);
        let parent = skeleton.parents[joint].map_or(Affine3A::IDENTITY, |parent| model[parent]);
        model.push(parent * own);
    }
    model
}

/// The twist of `rotation` about the unit `axis`: its swing-twist decomposition's twist.
fn twist(rotation: Quat, axis: Vec3) -> Quat {
    let projected = axis * rotation.xyz().dot(axis);
    let twist = Quat::from_xyzw(projected.x, projected.y, projected.z, rotation.w);
    if twist.length_squared() < 1e-12 { Quat::IDENTITY } else { twist.normalize() }
}

fn aim_axis(axis: AimAxis) -> Vec3 {
    match axis {
        AimAxis::PositiveX => Vec3::X,
        AimAxis::NegativeX => Vec3::NEG_X,
        AimAxis::PositiveY => Vec3::Y,
        AimAxis::NegativeY => Vec3::NEG_Y,
        AimAxis::PositiveZ => Vec3::Z,
        AimAxis::NegativeZ => Vec3::NEG_Z,
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use sapphire_assets::mesh::{Influences, Joint, SkinBind};

    use super::*;

    /// A shoulder at the origin with an upper arm along +X, a forearm 0.3 m on, and two helpers
    /// beside the upper arm: one aiming its +X at the forearm, one rolling with it about X.
    fn arm() -> (Skeleton, RigJoints) {
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
        let binds = at
            .iter()
            .enumerate()
            .map(|(joint, &[x, y, z])| SkinBind {
                joint: joint as u16,
                inverse_bind_matrix: Mat4::from_translation(-Vec3::new(x, y, z)).to_cols_array_2d(),
            })
            .collect();
        let skin = Skin { joints, binds, influences: Influences::pack(&[], 5) };
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
        (Skeleton::new(&skin), rig)
    }

    #[test]
    fn the_rest_pose_draws_the_bind_pose() {
        let (skeleton, rig) = arm();
        let posed = pose(&skeleton, &rig, &[]);
        assert!(posed.palette.iter().all(|entry| entry.abs_diff_eq(Affine3A::IDENTITY, 1e-6)));
    }

    #[test]
    fn turns_lower_the_arm_and_helpers_follow() {
        let (skeleton, rig) = arm();
        let down = Quat::from_rotation_z(-70f32.to_radians());
        let twist = Quat::from_rotation_x(40f32.to_radians());
        let posed =
            pose(&skeleton, &rig, &[(HumanBone::LeftUpperArm, down), (HumanBone::LeftLowerArm, twist)]);
        let elbow = posed.model_from_joint[2].translation;
        let expected = Vec3::new(0.1, 1.4, 0.0) + down * Vec3::new(0.3, 0.0, 0.0);
        assert!(Vec3::from(elbow).abs_diff_eq(expected, 1e-5), "{elbow} against {expected}");
        // The aim helper points its +X at the elbow, so a sleeve weighted to it follows the arm.
        let aim = posed.model_from_joint[3];
        let along = aim.transform_vector3(Vec3::X).normalize();
        assert!(along.abs_diff_eq((expected - Vec3::new(0.1, 1.4, 0.0)).normalize(), 1e-4), "{along}");
        // The roll helper takes half the forearm's twist about X.
        let roll = posed.model_from_joint[4].matrix3;
        let upper = posed.model_from_joint[1].matrix3;
        let relative = Quat::from_mat3a(&(upper.inverse() * roll));
        assert!(relative.abs_diff_eq(Quat::from_rotation_x(20f32.to_radians()), 1e-4), "{relative}");
        // Skinning moves a bind-pose point on the forearm with it.
        let wrist = Vec3::new(0.6, 1.4, 0.0);
        let skinned = posed.palette[2].transform_point3(wrist);
        assert!((skinned.distance(Vec3::from(elbow)) - 0.2).abs() < 1e-5, "{skinned}");
    }

    #[test]
    fn a_constraint_waits_for_those_that_move_its_source() {
        let joint = |parent: Option<u16>, translation: [f32; 3]| Joint {
            name: String::new(),
            parent,
            rest: JointTransform { translation, ..JointTransform::IDENTITY },
        };
        // A helper aiming at the end of a chain whose first joint follows a turned bone: listed
        // first, it must still aim where the chain ends up.
        let joints = vec![
            joint(None, [0.0; 3]),
            joint(Some(0), [0.1, 0.0, 0.0]),
            joint(Some(1), [0.3, 0.0, 0.0]),
            joint(Some(0), [0.1, 0.0, 0.0]),
            joint(Some(0), [0.0, 0.1, 0.0]),
        ];
        let skin = Skin { joints, binds: Vec::new(), influences: Influences::pack(&[], 5) };
        let rig = RigJoints {
            humanoid: vec![(HumanBone::Spine, 4)],
            constraints: vec![
                Constraint {
                    joint: 3,
                    source: 2,
                    weight: 1.0,
                    kind: ConstraintKind::Aim { axis: AimAxis::PositiveX },
                },
                Constraint { joint: 1, source: 4, weight: 1.0, kind: ConstraintKind::Rotation },
            ],
        };
        let posed =
            pose(&Skeleton::new(&skin), &rig, &[(HumanBone::Spine, Quat::from_rotation_z(FRAC_PI_2))]);
        let end = Vec3::from(posed.model_from_joint[2].translation);
        assert!(end.abs_diff_eq(Vec3::new(0.1, 0.3, 0.0), 1e-5), "{end}");
        let aim = posed.model_from_joint[3].transform_vector3(Vec3::X).normalize();
        assert!(aim.abs_diff_eq(Vec3::Y, 1e-4), "aims at the chain's end: {aim}");
    }

    #[test]
    fn twist_keeps_only_the_rotation_about_the_axis() {
        let rotation = Quat::from_rotation_x(0.6) * Quat::from_rotation_z(0.0);
        assert!(twist(rotation, Vec3::X).abs_diff_eq(Quat::from_rotation_x(0.6), 1e-6));
        assert!(twist(Quat::from_rotation_z(0.8), Vec3::X).abs_diff_eq(Quat::IDENTITY, 1e-6));
    }
}
