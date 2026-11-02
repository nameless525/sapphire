//! Retargeting at cook time, by humanoid bone map: one rig's motion onto another's skeleton, with
//! each bone's axes, proportions and bones of its own (spec §5.13).
//!
//! All of it in scalars and `det`, so a move's baked tables can start from what it makes (see [the
//! clip module](super)'s "Byte identity").
//!
//! # A humanoid map
//!
//! [`HumanMap`] says which joint of a skeleton is which [`HumanBone`]. A VRM's comes from its cooked
//! rig. Any other source's comes from its sidecar, `humanoid: { Hips: "hips", … }`, by joint name
//! ([`HumanMap::from_names`]), which needs every [`HumanBone::REQUIRED`] bone, as a VRM has, and
//! fails on the first entry that names no joint. Key-pose documents read the same maps, so they fit
//! any rig that has one. One joint may stand for several bones, for a source with one hand block
//! and a target with fingers.
//!
//! # The method: rest-relative rotation in model space
//!
//! For every bone both rigs map, at every tick:
//!
//! ```text
//! delta  = source_model(bone) · source_rest_model(bone)⁻¹
//! target_model(bone) = delta · target_rest_model(bone)
//! target_local(bone) = target_model(parent)⁻¹ · target_model(bone)
//! ```
//!
//! `delta` is how far the bone has turned from rest, in the one frame the rigs share: model space,
//! +Y up, facing +Z. It has nothing of a bone's local axes in it, so a rig whose bones run along +Y
//! with a half-turned roll drives one whose bones run along ±X. It needs both rigs to rest in
//! nearly the same pose, which they do where both are T-poses; a difference between their rests
//! stays as a fixed difference between their bones' directions, and no more.
//!
//! - **Why model space and not a joint's local rotation:** the rigs' hierarchies differ. A source
//!   without a neck moves the head in one step from the chest, and a target with one takes the head
//!   to the same place in model space: its neck stays at rest under the chest, and the head's local
//!   rotation makes up for what the chest did.
//! - **A bone only one rig maps** moves with its parent: the target's neck, upper chest, shoulders,
//!   fingers, eyes and jaw stay at their local rest, and so does a joint no bone names (hair,
//!   skirts and the twist helpers, which the rig's constraints and springs drive at run time).
//!   A source's unmapped joint is read only through what it does to joints below it.
//! - **A rest pose maps to the rest pose.** Each `delta` is then the identity, so a T-pose clip cooks
//!   to no tracks at all (`clip::cook` drops a track at rest).
//! - **A joint both skeletons have under one name** can follow by that name, as if a bone mapped it:
//!   a sidecar's [`follow`](RetargetSettings::follow). Where the two rigs' rests turn alike, as the
//!   same rig's on other lengths do, that's the source's local rotation. KayKit's hand blocks and
//!   the slots under them have no humanoid bone, so a held blade stays out of the source's grip
//!   without it, and mapping them through a finger would move every clip on a rig that has fingers.
//!
//! # Adjustments
//!
//! Rigs of other builds hold the same motion at other angles. A chibi's arms clear a body twice
//! as thick as the character's, so at the source's angles they hang wide of it. A sidecar's
//! [`adjust`](RetargetSettings::adjust) is a fixed [`Turn`] per bone, in the model's rest space as
//! a key-pose document's is, that goes between a bone and its parent at every tick:
//!
//! ```text
//! bone's turn from rest = parent's turn from rest · adjust(bone) · parent_delta⁻¹ · delta(bone)
//! ```
//!
//! So a bone under a turned bone turns with it, and a turn stays where it was put as the body above
//! it moves. A bone the rig has but the source doesn't (a finger) takes its turn as a fixed pose. A
//! rig that lacks a bone skips its adjustment, as a key-pose document does.
//!
//! [`gain`](RetargetSettings::gain) is how much of a bone's own motion stays: `parent_delta⁻¹ ·
//! delta(bone)` by that share of its angle, 0 at rest. A chibi's short legs take long swings and deep
//! bends to cover ground, so its idle stoops and its walk lunges on long legs. Give a leg's three
//! bones one gain, and a foot flat in the source stays nearly flat, since their angles still sum to
//! what the hips lean by.
//!
//! With nothing to adjust, nothing here runs, so a clip without either keeps its bytes.
//!
//! # Translations
//!
//! Only the hips travel on the target, and root motion: the hips' model-space offset from rest, and
//! the root track's translation, both times the [scale](RetargetOptions::scale). Rotations don't
//! scale. Every other translation or scale channel of the source is left behind (a target's bones
//! keep their own lengths), and its effect on the source's hips isn't.
//!
//! **The scale** is the ratio of the rigs' leg lengths, upper leg to foot ([`leg_length`]): hip height
//! measured along the legs. A turn of a leg moves its foot by the leg's length times the angle, so
//! that's what the hips' and the root's travel must be in for a planted foot to stay put, and a stride
//! to cover the ground it should. A source's hips joint needn't sit where its legs meet: KayKit's is
//! 11 cm below them, so the two heights a rig's hips could be measured by miss: 2.24 by the hips joints
//! and 1.67 by the leg sockets, around the legs' 2.04. Before grounding, the first sinks `Walking_A`'s
//! lowest foot 5.0 cm to the legs' 3.8, and the second lifts it 11.0 to their 9.5, though it's
//! the closest on a still clip (`Idle_A` 0.2 cm under to their 1.0).
//!
//! # Grounding
//!
//! Rotations at the source's angles on legs of other proportions don't land a foot where the
//! source's did, and sockets some rigs slide across the hips (KayKit's, up to 10 cm in a run) have
//! no counterpart. So with [`RetargetOptions::ground`] on, the hips rise or fall a little more at
//! each tick to put the target's lowest foot as high above the floor as the source's lowest is,
//! scaled, and never under it. A foot's height is measured from its height at rest, its toes'
//! likewise where both rigs have them, and the lower of the two counts. A foot that isn't the lowest
//! goes where the rotations take it.
//!
//! # Root motion
//!
//! A root track on `sampled` carries over: its turns as they are, its translation scaled, on the
//! target's hips joint. `root(t) × pose(t)` is then the source's motion in the target's proportions.
//! What root lifting leaves in the source's bones (a rise, a turn about another axis) reaches the
//! target through the hips' offset and `delta`.

use std::collections::BTreeMap;

use glam::{Affine3, Quat, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::rig::{CharacterRig, HumanBone};
use sapphire_assets::skeleton::{NO_PARENT, Skeleton};
use sapphire_math::det;
use serde::{Deserialize, Serialize};

use super::{SampledClip, SampledRoot, rest_model, rest_rotation};
use crate::cache::Stage;
use crate::keypose::Turn;

/// Retargeting and the `.sclip` writer, [`COOKER`](super::COOKER)'s version in the low 16 bits. Bump
/// the high bits whenever retargeting changes output for the same samples and rigs.
///
/// 2: in scalars, for one set of bytes on every architecture (M2 N1).
pub const COOKER: Stage = Stage { name: "sclip retarget", version: (2 << 16) | super::COOKER.version };

/// One entry of a sidecar's `retarget`: which clips of the source to carry onto which rig.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetargetSettings {
    /// The rig: the ID of a source with a skeleton and a humanoid map, as a VRM has.
    pub target: AssetId,
    /// The clips, by name.
    pub names: Vec<String>,
    /// What translations scale by, for a clip the [leg lengths' ratio](leg_length) doesn't suit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f32>,
    /// Whether the hips make up for what rotations alone do to the lowest foot's height: see the
    /// [module docs](self). On unless a clip's own timing needs the source's hips as they are.
    #[serde(default = "grounded", skip_serializing_if = "is_grounded")]
    pub ground: bool,
    /// Fixed turns, by bone, that make up for how the rigs' builds differ: see the
    /// [module docs](self)' "Adjustments".
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub adjust: BTreeMap<HumanBone, Turn>,
    /// How much of a bone's own motion stays, by bone, from 0 (at rest under its parent) up: the
    /// same section.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub gain: BTreeMap<HumanBone, f32>,
    /// Joints that turn as the source's of the same name do, though no humanoid bone names them:
    /// see the [module docs](self). Both skeletons must have each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub follow: Vec<String>,
}

fn grounded() -> bool {
    true
}

fn is_grounded(ground: &bool) -> bool {
    *ground
}

impl RetargetSettings {
    /// Whether its scale, if it has one, and its turns are ones a cook can honor.
    pub(crate) fn is_valid(&self) -> bool {
        self.scale.is_none_or(|scale| scale.is_finite() && scale > 0.0)
            && self.adjust.values().all(|turn| turn.x.is_finite() && turn.y.is_finite() && turn.z.is_finite())
            && self.gain.values().all(|gain| gain.is_finite() && *gain >= 0.0)
    }
}

/// Why a humanoid map can't be built.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HumanMapError {
    #[error("the humanoid map says {bone} is {joint:?}, but the skeleton has no joint of that name")]
    UnknownJoint { bone: &'static str, joint: String },
    #[error("the humanoid map has no {bone} bone")]
    MissingBone { bone: &'static str },
}

/// Why a clip can't be retargeted.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum RetargetError {
    #[error("the target's humanoid map names joint {joint} for both {first} and {second}")]
    SharedTargetJoint { joint: u16, first: &'static str, second: &'static str },
    #[error("a leg of {which} rig has no length, so it has no scale")]
    NoLegLength { which: &'static str },
    #[error("the scale {0} isn't a finite number above 0")]
    Scale(f32),
    #[error("the motion isn't finite at tick {tick}: a scale of zero above the hips?")]
    NonFinite { tick: usize },
    #[error("it should follow joint {name:?}, but {which} rig has no joint of that name")]
    UnknownJoint { which: &'static str, name: String },
    #[error("it should follow joint {name:?}, but the target's humanoid map already says that's {bone}")]
    FollowedBone { name: String, bone: &'static str },
}

/// Which joint of one skeleton is which humanoid bone. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HumanMap {
    joints: BTreeMap<HumanBone, u16>,
}

impl HumanMap {
    /// A cooked rig's map.
    pub fn from_rig(rig: &CharacterRig) -> Self {
        Self { joints: rig.humanoid.iter().map(|entry| (entry.bone, entry.joint)).collect() }
    }

    /// A map already in joints: a VRMA's, whose bones are the file's to name.
    pub fn from_joints(joints: BTreeMap<HumanBone, u16>) -> Self {
        Self { joints }
    }

    /// A sidecar's map: `names` of joints of `skeleton`, by their bones. Every required bone must be
    /// there, and every entry name a joint.
    pub fn from_names(
        names: &BTreeMap<HumanBone, String>,
        skeleton: &Skeleton,
    ) -> Result<Self, HumanMapError> {
        let mut joints = BTreeMap::new();
        for (bone, name) in names {
            let joint = skeleton
                .names
                .iter()
                .position(|candidate| candidate == name)
                .ok_or_else(|| HumanMapError::UnknownJoint { bone: bone.name(), joint: name.clone() })?;
            joints.insert(*bone, joint as u16);
        }
        if let Some(bone) = HumanBone::REQUIRED.iter().find(|bone| !joints.contains_key(bone)) {
            return Err(HumanMapError::MissingBone { bone: bone.name() });
        }
        Ok(Self { joints })
    }

    /// The joint that is `bone`, if the rig has it.
    pub fn joint(&self, bone: HumanBone) -> Option<u16> {
        self.joints.get(&bone).copied()
    }

    /// Every bone and its joint, by bone.
    pub fn iter(&self) -> impl Iterator<Item = (HumanBone, u16)> + '_ {
        self.joints.iter().map(|(bone, joint)| (*bone, *joint))
    }
}

/// A skeleton and which of its joints is which bone.
#[derive(Clone, Copy, Debug)]
pub struct Rig<'a> {
    pub skeleton: &'a Skeleton,
    pub map: &'a HumanMap,
}

/// How a retarget moves the target. See the [module docs](self).
#[derive(Clone, Debug, PartialEq)]
pub struct RetargetOptions {
    /// What translations scale by. `None`: the ratio of the rigs' leg lengths.
    pub scale: Option<f32>,
    /// Whether the hips make up for what rotations alone do to the lowest foot's height.
    pub ground: bool,
    /// Fixed turns, by bone: the [module docs](self)' "Adjustments".
    pub adjust: BTreeMap<HumanBone, Turn>,
    /// How much of a bone's own motion stays, by bone: the same section.
    pub gain: BTreeMap<HumanBone, f32>,
    /// Joints that follow the source's of the same name, as if a bone mapped them.
    pub follow: Vec<String>,
}

impl Default for RetargetOptions {
    fn default() -> Self {
        Self { scale: None, ground: true, adjust: BTreeMap::new(), gain: BTreeMap::new(), follow: Vec::new() }
    }
}

/// The length of a rig's legs at rest, upper leg to foot, both sides averaged: what a retarget's
/// scale is the ratio of. A leg without a required bone counts as none.
pub fn leg_length(rig: Rig<'_>) -> f32 {
    let model = rest_model(rig.skeleton);
    let at = |bone: HumanBone| rig.map.joint(bone).map(|joint| model[usize::from(joint)].translation);
    let leg = |upper, lower, foot| {
        let (upper, lower, foot) = (at(upper), at(lower), at(foot));
        upper
            .zip(lower)
            .zip(foot)
            .map_or(0.0, |((upper, lower), foot)| upper.distance(lower) + lower.distance(foot))
    };
    use HumanBone::{LeftFoot, LeftLowerLeg, LeftUpperLeg, RightFoot, RightLowerLeg, RightUpperLeg};
    (leg(LeftUpperLeg, LeftLowerLeg, LeftFoot) + leg(RightUpperLeg, RightLowerLeg, RightFoot)) * 0.5
}

/// What translations scale by under `options`.
fn scale_of(source: Rig<'_>, target: Rig<'_>, options: &RetargetOptions) -> Result<f32, RetargetError> {
    let scale = match options.scale {
        Some(scale) => scale,
        None => {
            let (from, to) = (leg_length(source), leg_length(target));
            if from <= 0.0 {
                return Err(RetargetError::NoLegLength { which: "the source" });
            }
            if to <= 0.0 {
                return Err(RetargetError::NoLegLength { which: "the target" });
            }
            to / from
        }
    };
    if !scale.is_finite() || scale <= 0.0 {
        return Err(RetargetError::Scale(scale));
    }
    Ok(scale)
}

fn parent_of(skeleton: &Skeleton, joint: usize) -> Option<usize> {
    let parent = skeleton.parents[joint];
    (parent != NO_PARENT).then_some(usize::from(parent))
}

/// Each joint's model-space rest rotation, from normalized local rotations, so a parent's scale
/// doesn't reach a bone's frame (as the held pose builds them, `sapphire_sandbox::pose`).
pub(crate) fn rest_rotations(skeleton: &Skeleton) -> Vec<Quat> {
    let mut model: Vec<Quat> = Vec::with_capacity(skeleton.rest.len());
    for joint in 0..skeleton.rest.len() {
        let local = rest_rotation(skeleton, joint);
        model.push(parent_of(skeleton, joint).map_or(local, |parent| det::quat_mul(model[parent], local)));
    }
    model
}

/// A joint's feet and toes, for measuring a rig's lowest foot.
#[derive(Clone, Copy)]
struct Foot {
    joint: usize,
    rest_height: f32,
}

/// The joints whose heights say where a rig's feet are, and their heights at rest: each foot and,
/// if `toes`, its toes where the rig has them.
fn soles(rig: Rig<'_>, model: &[Affine3], toes: bool) -> Vec<Foot> {
    let bones: &[HumanBone] = if toes {
        &[HumanBone::LeftFoot, HumanBone::RightFoot, HumanBone::LeftToes, HumanBone::RightToes]
    } else {
        &[HumanBone::LeftFoot, HumanBone::RightFoot]
    };
    bones
        .iter()
        .filter_map(|&bone| rig.map.joint(bone))
        .map(usize::from)
        .map(|joint| Foot { joint, rest_height: model[joint].translation.y })
        .collect()
}

/// How high the lowest of `soles` is above where it rests.
fn lowest(soles: &[Foot], model: &[Affine3]) -> f32 {
    let height = |foot: &Foot| model[foot.joint].translation.y - foot.rest_height;
    soles
        .iter()
        .map(height)
        .fold(f32::INFINITY, |lowest, height| if height < lowest { height } else { lowest })
}

/// Points `driver_of` at the source joints `follow` names, for the target joints of those names.
fn follow_by_name(
    follow: &[String],
    source: Rig<'_>,
    target: Rig<'_>,
    bone_of: &[Option<HumanBone>],
    driver_of: &mut [Option<usize>],
) -> Result<(), RetargetError> {
    for name in follow {
        let joint = |rig: Rig<'_>, which| {
            let found = rig.skeleton.names.iter().position(|candidate| candidate == name);
            found.ok_or_else(|| RetargetError::UnknownJoint { which, name: name.clone() })
        };
        let (from, joint) = (joint(source, "the source")?, joint(target, "the target")?);
        if let Some(bone) = bone_of[joint] {
            return Err(RetargetError::FollowedBone { name: name.clone(), bone: bone.name() });
        }
        driver_of[joint] = Some(from);
    }
    Ok(())
}

/// A setting by bone, as one by the target's joints. A rig without the bone skips it.
fn by_target_joint<T, U: Copy>(
    target: Rig<'_>,
    settings: &BTreeMap<HumanBone, T>,
    value: impl Fn(&T) -> U,
) -> Vec<Option<U>> {
    let mut by_joint = vec![None; target.skeleton.parents.len()];
    for (bone, setting) in settings {
        if let Some(joint) = target.map.joint(*bone) {
            by_joint[usize::from(joint)] = Some(value(setting));
        }
    }
    by_joint
}

/// What a gain takes off a bone's own turn, `parent_delta⁻¹ · delta`, as a turn before it.
fn gain_loss(gain: f32, delta: Quat, parent_delta: Quat) -> Quat {
    let own = det::quat_mul(det::quat_conjugate(parent_delta), delta);
    det::quat_mul(det::quat_slerp(Quat::IDENTITY, own, gain), det::quat_conjugate(own))
}

/// What a joint changes about its own motion, in its parent's frame at rest: `turn`, its adjustment,
/// and `gain`, its gain with its `delta`. `None` when it changes nothing.
fn own_change(turn: Option<Quat>, gain: Option<(f32, Quat)>, parent_delta: Quat) -> Option<Quat> {
    if turn.is_none() && gain.is_none() {
        return None;
    }
    let lost = gain.map_or(Quat::IDENTITY, |(gain, delta)| gain_loss(gain, delta, parent_delta));
    Some(det::quat_mul(turn.unwrap_or(Quat::IDENTITY), lost))
}

/// What a joint's `delta` is corrected by: the correction `above` it, then its `change`, moved with
/// the body above it by `parent_delta`.
fn correction_with(above: Option<Quat>, change: Option<Quat>, parent_delta: Quat) -> Option<Quat> {
    change.map_or(above, |change| {
        let moved = det::quat_mul(det::quat_mul(above.unwrap_or(Quat::IDENTITY), parent_delta), change);
        Some(det::quat_mul(moved, det::quat_conjugate(parent_delta)))
    })
}

/// `sampled`, for `target`: see the [module docs](self). Its root motion should already be lifted,
/// and its tracks index `source`'s joints. Morph weights aren't carried: targets have their own.
pub fn retarget(
    sampled: &SampledClip,
    source: Rig<'_>,
    target: Rig<'_>,
    options: &RetargetOptions,
) -> Result<SampledClip, RetargetError> {
    let scale = scale_of(source, target, options)?;
    let source_joints = source.skeleton.parents.len();
    let target_joints = target.skeleton.parents.len();

    // Which target joints each bone writes, and the source joint it reads.
    let mut driver_of: Vec<Option<usize>> = vec![None; target_joints];
    let mut bone_of: Vec<Option<HumanBone>> = vec![None; target_joints];
    for (bone, joint) in target.map.iter() {
        let joint = usize::from(joint);
        if let Some(first) = bone_of[joint] {
            return Err(RetargetError::SharedTargetJoint {
                joint: joint as u16,
                first: first.name(),
                second: bone.name(),
            });
        }
        bone_of[joint] = Some(bone);
        driver_of[joint] = source.map.joint(bone).map(usize::from);
    }
    follow_by_name(&options.follow, source, target, &bone_of, &mut driver_of)?;
    let adjust_of = by_target_joint(target, &options.adjust, |turn| turn.quat());
    let gain_of = by_target_joint(target, &options.gain, |gain| *gain);
    let source_rest_rotation = rest_rotations(source.skeleton);
    let target_rest_rotation = rest_rotations(target.skeleton);
    let source_rest = rest_model(source.skeleton);
    let target_rest = rest_model(target.skeleton);
    let local_rest: Vec<Quat> =
        (0..target_joints).map(|joint| rest_rotation(target.skeleton, joint)).collect();

    // Both rigs measure a foot by what both have.
    let has_toes = |rig: Rig<'_>| {
        rig.map.joint(HumanBone::LeftToes).is_some() && rig.map.joint(HumanBone::RightToes).is_some()
    };
    let with_toes = has_toes(source) && has_toes(target);
    let source_soles = soles(source, &source_rest, with_toes);
    let target_soles = soles(target, &target_rest, with_toes);

    // Only joints on the way to a driven one or a sole need walking: a VRM's hair and skirts don't.
    let mut needed = vec![false; target_joints];
    for foot in &target_soles {
        needed[foot.joint] = true;
    }
    for joint in (0..target_joints).rev() {
        if driver_of[joint].is_some() || adjust_of[joint].is_some() || needed[joint] {
            needed[joint] = true;
            if let Some(parent) = parent_of(target.skeleton, joint) {
                needed[parent] = true;
            }
        }
    }

    let by_joint = |tracks: &[(u16, Vec<Vec3>)]| {
        let mut found: Vec<Option<usize>> = vec![None; source_joints];
        for (index, (joint, _)) in tracks.iter().enumerate() {
            found[usize::from(*joint)] = Some(index);
        }
        found
    };
    let mut rotation_of: Vec<Option<&Vec<Quat>>> = vec![None; source_joints];
    for (joint, samples) in &sampled.rotations {
        rotation_of[usize::from(*joint)] = Some(samples);
    }
    let translation_of = by_joint(&sampled.translations);
    let scale_of_joint = by_joint(&sampled.scales);

    let target_hips = target.map.joint(HumanBone::Hips).map(usize::from);
    let hips_pair = target_hips.zip(source.map.joint(HumanBone::Hips).map(usize::from));
    // A model-space offset of the hips in their parent's space: the parent holds still, as joints
    // above the hips do, since no bone maps them.
    let to_parent = target_hips
        .and_then(|hips| parent_of(target.skeleton, hips))
        .map_or(Affine3::IDENTITY, |parent| target_rest[parent].inverse());

    let count = usize::from(sampled.ticks) + 1;
    let mut rotations: BTreeMap<u16, Vec<Quat>> = BTreeMap::new();
    let mut hips_samples = Vec::with_capacity(count);
    let mut source_rotation = vec![Quat::IDENTITY; source_joints];
    let mut source_model = vec![Affine3::IDENTITY; source_joints];
    let mut target_rotation = vec![Quat::IDENTITY; target_joints];
    let mut target_model = vec![Affine3::IDENTITY; target_joints];
    // Each joint's `delta` this tick, and what its adjustments and those above it add to it.
    let mut delta_of = vec![Quat::IDENTITY; target_joints];
    let mut correction_of: Vec<Option<Quat>> = vec![None; target_joints];
    for tick in 0..count {
        for joint in 0..source_joints {
            let rest = &source.skeleton.rest[joint];
            let local = rotation_of[joint].map_or_else(
                || rest_rotation(source.skeleton, joint),
                |samples| det::quat_normalize(samples[tick]),
            );
            let parent = parent_of(source.skeleton, joint);
            source_rotation[joint] =
                parent.map_or(local, |parent| det::quat_mul(source_rotation[parent], local));
            let vector = |tracks: &[(u16, Vec<Vec3>)], track: Option<usize>, or: [f32; 3]| {
                track.map_or(Vec3::from(or), |track| tracks[track].1[tick])
            };
            let affine = Affine3::from_scale_rotation_translation(
                vector(&sampled.scales, scale_of_joint[joint], rest.scale),
                local,
                vector(&sampled.translations, translation_of[joint], rest.translation),
            );
            source_model[joint] = parent.map_or(affine, |parent| source_model[parent] * affine);
        }

        let hips_rest = |hips: usize| Vec3::from(target.skeleton.rest[hips].translation);
        let mut offset = hips_pair.map_or(Vec3::ZERO, |(_, from)| {
            (source_model[from].translation - source_rest[from].translation) * scale
        });
        for joint in (0..target_joints).filter(|&joint| needed[joint]) {
            let parent = parent_of(target.skeleton, joint);
            let (parent_delta, parent_correction) =
                parent.map_or((Quat::IDENTITY, None), |parent| (delta_of[parent], correction_of[parent]));
            let delta = driver_of[joint].map(|from| {
                det::quat_mul(source_rotation[from], det::quat_conjugate(source_rest_rotation[from]))
            });
            delta_of[joint] = delta.unwrap_or(parent_delta);
            let change = own_change(adjust_of[joint], gain_of[joint].zip(delta), parent_delta);
            let correction = correction_with(parent_correction, change, parent_delta);
            correction_of[joint] = correction;
            let local = if delta.is_some() || adjust_of[joint].is_some() {
                let delta = delta_of[joint];
                let turned = correction.map_or(delta, |correction| det::quat_mul(correction, delta));
                let model = det::quat_normalize(det::quat_mul(turned, target_rest_rotation[joint]));
                target_rotation[joint] = model;
                let local = parent.map_or(model, |parent| {
                    det::quat_mul(det::quat_conjugate(target_rotation[parent]), model)
                });
                let local = det::quat_normalize(local);
                rotations.entry(joint as u16).or_default().push(local);
                local
            } else {
                let local = local_rest[joint];
                target_rotation[joint] =
                    parent.map_or(local, |parent| det::quat_mul(target_rotation[parent], local));
                local
            };
            let translation = if Some(joint) == target_hips {
                hips_rest(joint) + to_parent.transform_vector3(offset)
            } else {
                Vec3::from(target.skeleton.rest[joint].translation)
            };
            let affine = Affine3::from_scale_rotation_translation(
                Vec3::from(target.skeleton.rest[joint].scale),
                local,
                translation,
            );
            target_model[joint] = parent.map_or(affine, |parent| target_model[parent] * affine);
        }

        // Everything below the hips only follows them, so moving them by the shortfall lands the
        // lowest foot where it should be.
        if options.ground && !target_soles.is_empty() && !source_soles.is_empty() {
            let wanted = lowest(&source_soles, &source_model) * scale;
            let wanted = if wanted > 0.0 { wanted } else { 0.0 };
            offset.y += wanted - lowest(&target_soles, &target_model);
        }
        if let Some(hips) = target_hips {
            let at = hips_rest(hips) + to_parent.transform_vector3(offset);
            if !at.is_finite() {
                return Err(RetargetError::NonFinite { tick });
            }
            hips_samples.push(at);
        }
    }

    Ok(SampledClip {
        name: sampled.name.clone(),
        rate: sampled.rate,
        ticks: sampled.ticks,
        rotations: rotations.into_iter().collect(),
        translations: target_hips.map(|hips| (hips as u16, hips_samples)).into_iter().collect(),
        scales: Vec::new(),
        weights: Vec::new(),
        root: match (&sampled.root, target_hips) {
            (Some(root), Some(hips)) => Some(SampledRoot {
                joint: hips as u16,
                translation: root.translation.iter().map(|value| *value * scale).collect(),
                rotation: root.rotation.clone(),
            }),
            _ => None,
        },
    })
}

#[cfg(test)]
pub(crate) mod tests;
