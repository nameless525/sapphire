//! Mirroring a clip left for right, by its rig's humanoid map (M6 A1): a sidecar's
//! `mirror: { "Dodge_Right": "Dodge_Left" }` cooks the first as the second reflected across the
//! model's middle, in place of any animation the file has of that name, and every retarget of it
//! carries the mirror. One side of a dodge or a turn is authored, the other made.
//!
//! # The method: rest-relative, in model space
//!
//! The reflection is `x → −x` in model space (+Y up, facing +Z), and each bone takes its partner's
//! motion: `LeftUpperArm`'s for `RightUpperArm`'s, the hips' own for the hips'. As retargeting does,
//! it works from each bone's turn from rest in model space, which has nothing of a bone's local axes
//! in it, so it holds however a rig rolls its bones:
//!
//! ```text
//! delta              = model(partner) · rest_model(partner)⁻¹
//! mirrored_model(bone) = reflect(delta) · rest_model(bone)
//! ```
//!
//! where `reflect` takes a turn to its mirror image, `(x, y, z, w) → (x, −y, −z, w)`. A bone that
//! moves (the hips) takes its partner's offset from rest, reflected, from its own rest. Root motion
//! reflects the same way: its travel's `x` and its turn change sign. Morph weights stay as they are.
//!
//! - **The rest must be symmetric,** as a T-pose is: a bone's mirror is its partner's motion from
//!   rest laid on its own rest, so an asymmetric rest stays as it was rather than swapping.
//! - **A joint no bone names** keeps its own motion and moves with its parent (a hand's slot stays in
//!   its hand), except joints above the hips, which reflect onto themselves as the hips do.
//! - **Every sided bone needs its partner:** a map naming `LeftToes` and not `RightToes` can't mirror.
//!
//! Mirroring twice gives the clip back, within rounding. In scalars and `det`, as everything on a
//! clip's path is, so a mirrored clip, and a move's tables baked from one, are one set of bytes on
//! every architecture.

use std::collections::BTreeMap;

use glam::{Affine3, Quat, Vec3};
use sapphire_assets::rig::HumanBone;
use sapphire_assets::skeleton::{NO_PARENT, Skeleton};
use sapphire_math::det;

use super::retarget::{HumanMap, Rig, rest_rotations};
use super::{SampledClip, SampledRoot, rest_model, rest_rotation};

/// Why a clip can't be mirrored.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MirrorError {
    #[error("the humanoid map names {bone} but not {partner}, so the clip can't be mirrored")]
    NoPartner { bone: &'static str, partner: &'static str },
    #[error("the humanoid map's joint {joint} stands for bones whose mirrors are different joints")]
    TwoPartners { joint: u16 },
}

/// What a joint's mirror reads.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    /// A bone's partner's motion, reflected: itself, for a bone down the middle.
    Bone(usize),
    /// Its own local motion, under its mirrored parent.
    Keep,
}

/// The bone named `bone`'s mirror image: `leftHand` for `rightHand`, itself down the middle.
pub fn partner(bone: HumanBone) -> HumanBone {
    let name = bone.name();
    let swapped = if let Some(rest) = name.strip_prefix("left") {
        format!("right{rest}")
    } else if let Some(rest) = name.strip_prefix("right") {
        format!("left{rest}")
    } else {
        return bone;
    };
    HumanBone::from_name(&swapped).expect("every sided bone has a partner of the other side")
}

/// What each joint of `rig` reads: see the [module docs](self).
fn sides(rig: Rig<'_>) -> Result<Vec<Side>, MirrorError> {
    let joints = rig.skeleton.parents.len();
    let mut sides = vec![Side::Keep; joints];
    let mut bones: BTreeMap<usize, usize> = BTreeMap::new();
    for (bone, joint) in rig.map.iter() {
        let other = partner(bone);
        let mirror = rig
            .map
            .joint(other)
            .ok_or(MirrorError::NoPartner { bone: bone.name(), partner: other.name() })?;
        let (joint, mirror) = (usize::from(joint), usize::from(mirror));
        if *bones.entry(joint).or_insert(mirror) != mirror {
            return Err(MirrorError::TwoPartners { joint: joint as u16 });
        }
        sides[joint] = Side::Bone(mirror);
    }
    // Above the hips, everything turns the whole body, so it reflects onto itself.
    if let Some(hips) = rig.map.joint(HumanBone::Hips) {
        let mut joint = usize::from(hips);
        while rig.skeleton.parents[joint] != NO_PARENT {
            joint = usize::from(rig.skeleton.parents[joint]);
            if sides[joint] == Side::Keep {
                sides[joint] = Side::Bone(joint);
            }
        }
    }
    Ok(sides)
}

/// A turn's mirror image across the model's middle.
fn reflect(turn: Quat) -> Quat {
    let [x, y, z, w] = turn.to_array();
    Quat::from_xyzw(x, -y, -z, w)
}

/// `sampled` mirrored left for right on `rig`, whose joints its tracks index: see the
/// [module docs](self). Its root motion should already be lifted. It keeps its name: the caller names
/// the mirror.
pub fn mirror(sampled: &SampledClip, rig: Rig<'_>) -> Result<SampledClip, MirrorError> {
    let skeleton = rig.skeleton;
    let sides = sides(rig)?;
    let joints = skeleton.parents.len();
    let parent_of =
        |joint: usize| (skeleton.parents[joint] != NO_PARENT).then(|| usize::from(skeleton.parents[joint]));
    let track_of = |tracks: Vec<u16>| {
        let mut found: Vec<Option<usize>> = vec![None; joints];
        for (index, joint) in tracks.into_iter().enumerate() {
            found[usize::from(joint)] = Some(index);
        }
        found
    };
    let rotation_of = track_of(sampled.rotations.iter().map(|(joint, _)| *joint).collect());
    let translation_of = track_of(sampled.translations.iter().map(|(joint, _)| *joint).collect());
    let scale_of = track_of(sampled.scales.iter().map(|(joint, _)| *joint).collect());
    let rest_turn = rest_rotations(skeleton);
    let rest = rest_model(skeleton);

    // Which joints get a track of each kind: their own, or their partner's.
    let reads = |joint: usize| match sides[joint] {
        Side::Bone(partner) => partner,
        Side::Keep => joint,
    };
    let rotated: Vec<bool> = (0..joints)
        .map(|joint| matches!(sides[joint], Side::Bone(_)) || rotation_of[joint].is_some())
        .collect();
    let moved: Vec<bool> =
        (0..joints).map(|joint| translation_of[joint].or(translation_of[reads(joint)]).is_some()).collect();
    let scaled: Vec<bool> = (0..joints).map(|joint| scale_of[reads(joint)].is_some()).collect();

    let count = usize::from(sampled.ticks) + 1;
    let mut rotations: Vec<Vec<Quat>> = vec![Vec::new(); joints];
    let mut translations: Vec<Vec<Vec3>> = vec![Vec::new(); joints];
    let mut scales: Vec<Vec<Vec3>> = vec![Vec::new(); joints];
    let mut turn = vec![Quat::IDENTITY; joints];
    let mut model = vec![Affine3::IDENTITY; joints];
    let mut mirrored_turn = vec![Quat::IDENTITY; joints];
    let mut mirrored = vec![Affine3::IDENTITY; joints];
    for tick in 0..count {
        let local_turn = |joint: usize| {
            rotation_of[joint].map_or_else(
                || rest_rotation(skeleton, joint),
                |track| det::quat_normalize(sampled.rotations[track].1[tick]),
            )
        };
        let vector = |tracks: &[(u16, Vec<Vec3>)], track: Option<usize>, or: [f32; 3]| {
            track.map_or(Vec3::from(or), |track| tracks[track].1[tick])
        };
        let local_translation = |joint: usize| {
            vector(&sampled.translations, translation_of[joint], skeleton.rest[joint].translation)
        };
        let local_scale = |joint: usize| vector(&sampled.scales, scale_of[joint], skeleton.rest[joint].scale);

        // The clip as authored, in model space.
        for joint in 0..joints {
            let local = local_turn(joint);
            let affine =
                Affine3::from_scale_rotation_translation(local_scale(joint), local, local_translation(joint));
            (turn[joint], model[joint]) = match parent_of(joint) {
                Some(parent) => (det::quat_mul(turn[parent], local), model[parent] * affine),
                None => (local, affine),
            };
        }

        // Its mirror, parents first.
        for joint in 0..joints {
            let parent = parent_of(joint);
            let (local, translation, scale) = match sides[joint] {
                Side::Bone(partner) => {
                    let delta = det::quat_mul(turn[partner], det::quat_conjugate(rest_turn[partner]));
                    let wanted = det::quat_normalize(det::quat_mul(reflect(delta), rest_turn[joint]));
                    mirrored_turn[joint] = wanted;
                    let local = det::quat_normalize(parent.map_or(wanted, |parent| {
                        det::quat_mul(det::quat_conjugate(mirrored_turn[parent]), wanted)
                    }));
                    let translation = if moved[joint] {
                        let offset = model[partner].translation - rest[partner].translation;
                        let at = rest[joint].translation + Vec3::new(-offset.x, offset.y, offset.z);
                        parent.map_or(at, |parent| mirrored[parent].inverse().transform_point3(at))
                    } else {
                        Vec3::from(skeleton.rest[joint].translation)
                    };
                    let scale = scale_of[partner]
                        .map_or(Vec3::from(skeleton.rest[joint].scale), |_| local_scale(partner));
                    (local, translation, scale)
                }
                Side::Keep => {
                    let local = local_turn(joint);
                    mirrored_turn[joint] =
                        parent.map_or(local, |parent| det::quat_mul(mirrored_turn[parent], local));
                    (local, local_translation(joint), local_scale(joint))
                }
            };
            let affine = Affine3::from_scale_rotation_translation(scale, local, translation);
            mirrored[joint] = parent.map_or(affine, |parent| mirrored[parent] * affine);
            if rotated[joint] {
                rotations[joint].push(local);
            }
            if moved[joint] {
                translations[joint].push(translation);
            }
            if scaled[joint] {
                scales[joint].push(scale);
            }
        }
    }

    let tracks = |samples: Vec<Vec<Vec3>>, kept: &[bool]| -> Vec<(u16, Vec<Vec3>)> {
        samples
            .into_iter()
            .enumerate()
            .filter(|(joint, _)| kept[*joint])
            .map(|(joint, s)| (joint as u16, s))
            .collect()
    };
    Ok(SampledClip {
        name: sampled.name.clone(),
        rate: sampled.rate,
        ticks: sampled.ticks,
        rotations: rotations
            .into_iter()
            .enumerate()
            .filter(|(joint, _)| rotated[*joint])
            .map(|(joint, samples)| (joint as u16, samples))
            .collect(),
        translations: tracks(translations, &moved),
        scales: tracks(scales, &scaled),
        weights: sampled.weights.clone(),
        root: sampled.root.as_ref().map(|root| SampledRoot {
            joint: root.joint,
            translation: root.translation.iter().map(|at| Vec3::new(-at.x, at.y, at.z)).collect(),
            rotation: root.rotation.iter().map(|turn| reflect(*turn)).collect(),
        }),
    })
}

/// Whether `skeleton`'s map can mirror at all, for a check before anything cooks.
pub fn check(skeleton: &Skeleton, map: &HumanMap) -> Result<(), MirrorError> {
    sides(Rig { skeleton, map }).map(|_| ())
}

#[cfg(test)]
pub(crate) mod tests;
