//! What IK reads of a rig: which joints are which, from its humanoid map and look-at data, worked out
//! once.

use std::sync::Arc;

use sapphire_assets::rig::{ArchivedCharacterRig, ExpressionPreset, HumanBone, LookAtKind};
use sapphire_assets::skeleton::NO_PARENT;
use sapphire_math::{Affine3A, Vec3};

use crate::character::{CharacterPose, JointLayout};
use crate::ik::feet::LegBones;
use crate::ik::hands::ArmBones;
use crate::ik::look::{EyeDrive, EyeRig, LookBones, RangeMap};
use crate::ik::solve::TwoBone;
use crate::pose_math::PoseMath;

/// How a look's turn shares out down the neck: whichever of these the rig has, top down. A rig
/// without an upper chest or a neck, as KayKit's has neither, shares it among what it has.
const LOOK_SHARES: [(&[HumanBone], f32); 3] = [
    (&[HumanBone::UpperChest, HumanBone::Chest], 0.15),
    (&[HumanBone::Neck], 0.3),
    (&[HumanBone::Head], 0.55),
];

/// A rig's bones as IK reads them. See the `rig` module's docs.
#[derive(Clone, Debug, PartialEq)]
pub struct IkRig {
    pub(crate) math: PoseMath,
    pub(crate) layout: Arc<JointLayout>,
    pub(crate) hips: Option<u16>,
    pub(crate) legs: Option<[LegBones; 2]>,
    pub(crate) look: Option<LookBones>,
    pub(crate) arms: Option<[ArmBones; 2]>,
    pub(crate) floor: f32,
    /// How high over the floor the hips stand at rest.
    pub(crate) standing: f32,
    pub(crate) reads: Vec<u16>,
}

impl IkRig {
    /// A rig with `bones` for its humanoid bones, standing on `floor` in the model's space. `eyes` and
    /// `eye_origin`, in the head's space, are its look-at data ([`Self::from_cooked`] reads a cooked
    /// rig's). Allocates; a character keeps one for as long as its skeleton lasts.
    #[must_use]
    pub fn new(
        pose: &CharacterPose,
        bones: &[(HumanBone, usize)],
        eye_origin: Vec3,
        eyes: Option<EyeRig>,
        floor: f32,
    ) -> Self {
        let math = pose.math();
        let count = pose.joint_count();
        let bone = |bone: HumanBone| {
            bones
                .iter()
                .find(|(entry, _)| *entry == bone)
                .map(|&(_, joint)| joint)
                .filter(|&joint| joint < count)
        };
        let rest = rest_model(pose);
        let leg = |upper, lower, foot, toes| {
            let chain = TwoBone { root: bone(upper)?, mid: bone(lower)?, end: bone(foot)? };
            let toes = bone(toes);
            let ankle = rest[chain.end].translation;
            let upper = Vec3::from(rest[chain.mid].translation - rest[chain.root].translation);
            let lower = Vec3::from(ankle - rest[chain.mid].translation);
            let knee_forward = upper.cross(upper.cross(lower)).normalize_or(Vec3::Z);
            // The near-straight bind leg may have a small extension or lateral offset.
            let knee_forward = if knee_forward.z > 0.5 { knee_forward } else { Vec3::Z };
            let ankle_height = ankle.y - floor;
            let heel_back = toes.map_or(ankle_height * 0.5, |toes| {
                math.length_3a((rest[toes].translation - ankle).with_y(0.0)) * 0.55
            });
            Some(LegBones {
                chain,
                toes: toes.map(|toes| toes as u16),
                path: path(pose, chain.root, chain.end),
                rest_foot: pose.rest_rotation(chain.end),
                knee_forward: math.rotate(pose.rest_rotation(chain.root).inverse(), knee_forward),
                ankle_height,
                ball_height: toes.map(|toes| rest[toes].translation.y - floor),
                rest_toes: toes.map(|toes| pose.rest_rotation(toes)),
                heel_back,
            })
        };
        let legs =
            leg(HumanBone::LeftUpperLeg, HumanBone::LeftLowerLeg, HumanBone::LeftFoot, HumanBone::LeftToes)
                .zip(leg(
                    HumanBone::RightUpperLeg,
                    HumanBone::RightLowerLeg,
                    HumanBone::RightFoot,
                    HumanBone::RightToes,
                ))
                .map(|(left, right)| [left, right]);
        let arm = |upper, lower, hand| {
            let chain = TwoBone { root: bone(upper)?, mid: bone(lower)?, end: bone(hand)? };
            let frame = pose.parent(chain.root)?;
            Some(ArmBones {
                chain,
                path: path(pose, chain.root, chain.end),
                frame: frame as u16,
                rest_frame: pose.rest_rotation(frame),
            })
        };
        let arms = arm(HumanBone::LeftUpperArm, HumanBone::LeftLowerArm, HumanBone::LeftHand)
            .zip(arm(HumanBone::RightUpperArm, HumanBone::RightLowerArm, HumanBone::RightHand))
            .map(|(left, right)| [left, right]);
        let look = bone(HumanBone::Head).and_then(|head| look_bones(pose, &bone, head, eye_origin, eyes));
        let hips = bone(HumanBone::Hips).map(|hips| hips as u16);

        let mut reads = Vec::new();
        let mut read = |joint: usize| read_up(pose, joint, &mut reads);
        if let Some(hips) = hips {
            read(usize::from(hips));
        }
        for leg in legs.iter().flatten() {
            read(leg.chain.end);
            if let Some(toes) = leg.toes {
                read(usize::from(toes));
            }
        }
        for arm in arms.iter().flatten() {
            read(arm.chain.end);
        }
        if let Some(look) = &look {
            read(usize::from(look.head));
        }
        reads.sort_unstable();
        reads.dedup();
        let standing = hips.map_or(0.0, |hips| rest[usize::from(hips)].translation.y - floor);
        Self { math, layout: pose.joint_layout(), hips, legs, look, arms, floor, standing, reads }
    }

    /// A rig as its cooked data says, standing on `floor` in the model's space.
    #[must_use]
    pub fn from_cooked(pose: &CharacterPose, rig: &ArchivedCharacterRig, floor: f32) -> Self {
        let bones: Vec<(HumanBone, usize)> =
            rig.humanoid.iter().map(|entry| (entry.bone, usize::from(entry.joint.to_native()))).collect();
        let bone =
            |wanted: HumanBone| bones.iter().find(|(entry, _)| *entry == wanted).map(|&(_, joint)| joint);
        let look_at = rig.look_at.as_ref();
        let origin =
            look_at.map_or(Vec3::ZERO, |look| Vec3::from_array(look.offset_from_head.map(|v| v.to_native())));
        let eyes = look_at.and_then(|look| {
            let range = |range: &sapphire_assets::rig::ArchivedLookAtRange, scale: f32| RangeMap {
                input_max: range.input_max_degrees.to_native().to_radians(),
                output: range.output_scale.to_native() * scale,
            };
            let drive = match look.kind {
                LookAtKind::Bone => {
                    let eye = |wanted: HumanBone| {
                        let joint = bone(wanted)?;
                        let parent = pose.parent(joint)?;
                        Some((joint as u16, pose.rest_rotation(parent)))
                    };
                    let (left, right) = (eye(HumanBone::LeftEye), eye(HumanBone::RightEye));
                    if left.is_none() && right.is_none() {
                        return None;
                    }
                    EyeDrive::Bones { left, right }
                }
                LookAtKind::Expression => {
                    let targets = |preset: ExpressionPreset| -> Vec<(u32, f32)> {
                        rig.expressions
                            .iter()
                            .find(|expression| expression.preset.as_ref() == Some(&preset))
                            .map_or_else(Vec::new, |expression| {
                                expression
                                    .morph_targets
                                    .iter()
                                    .map(|morph| (morph.target.to_native(), morph.weight.to_native()))
                                    .collect()
                            })
                    };
                    EyeDrive::Expressions {
                        up: targets(ExpressionPreset::LookUp),
                        down: targets(ExpressionPreset::LookDown),
                        left: targets(ExpressionPreset::LookLeft),
                        right: targets(ExpressionPreset::LookRight),
                    }
                }
            };
            // Degrees for a bone, and a plain weight for an expression.
            let scale = if look.kind == LookAtKind::Bone { 1f32.to_radians() } else { 1.0 };
            Some(EyeRig {
                inner: range(&look.horizontal_inner, scale),
                outer: range(&look.horizontal_outer, scale),
                down: range(&look.vertical_down, scale),
                up: range(&look.vertical_up, scale),
                drive,
            })
        });
        Self::new(pose, &bones, origin, eyes, floor)
    }
}

fn look_bones(
    pose: &CharacterPose,
    bone: &impl Fn(HumanBone) -> Option<usize>,
    head: usize,
    origin: Vec3,
    eyes: Option<EyeRig>,
) -> Option<LookBones> {
    let present: Vec<(usize, f32)> = LOOK_SHARES
        .iter()
        .filter_map(|&(wanted, share)| Some((wanted.iter().find_map(|&wanted| bone(wanted))?, share)))
        .collect();
    let total: f32 = present.iter().map(|&(_, share)| share).sum();
    let (top, _) = *present.first()?;
    let body = pose.parent(top)?;
    let path = path(pose, top, head);
    let chain = present
        .iter()
        .filter_map(|&(joint, share)| {
            let place = path.iter().position(|&entry| usize::from(entry) == joint)?;
            Some((joint as u16, share / total, place as u16))
        })
        .collect();
    Some(LookBones {
        chain,
        path,
        body: body as u16,
        rest_body: pose.rest_rotation(body),
        neck: bone(HumanBone::Neck).map(|joint| joint as u16),
        head: head as u16,
        rest_head: pose.rest_rotation(head),
        origin,
        eyes,
    })
}

/// `top` down to `bottom`, every joint between, parents first. `bottom` alone if it doesn't hang from
/// `top`.
fn path(pose: &CharacterPose, top: usize, bottom: usize) -> Vec<u16> {
    let mut joints = vec![bottom as u16];
    let mut at = bottom;
    while at != top {
        match pose.parent(at) {
            Some(parent) => {
                joints.push(parent as u16);
                at = parent;
            }
            None => return vec![bottom as u16],
        }
    }
    joints.reverse();
    joints
}

fn read_up(pose: &CharacterPose, joint: usize, reads: &mut Vec<u16>) {
    let mut at = Some(joint);
    while let Some(joint) = at {
        reads.push(joint as u16);
        at = pose.parent(joint);
    }
}

/// The skeleton's rest pose in the model's space: `pose` may already be somewhere else.
fn rest_model(pose: &CharacterPose) -> Vec<Affine3A> {
    let mut model = vec![Affine3A::IDENTITY; pose.joint_count()];
    for (joint, local) in pose.rest().iter().enumerate() {
        let parent = pose.parent(joint).map_or(NO_PARENT, |parent| parent as u16);
        model[joint] = crate::model::model_of(parent, local, &model);
    }
    model
}
