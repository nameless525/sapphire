//! Blending poses: one toward another with a weight, per-joint masks, and additive layers.
//!
//! Each function changes a pose in place, so a layered caller folds its layers into one buffer
//! and nothing here needs one of its own: none of them allocates. Rotations blend along the short
//! way round, whichever hemisphere each quaternion is written in, since `q` and `−q` are one
//! rotation and a decoded key's sign is whatever made its largest component positive
//! ([`PackedRotation`](sapphire_assets::clip::PackedRotation)).
//!
//! The edges are exact, so a layer that fades out leaves nothing behind: a weight of 0 changes no
//! bit of the pose, and a full [`blend`] leaves the other pose's bits.

use sapphire_assets::skeleton::{ArchivedSkeleton, NO_PARENT};
use sapphire_math::{Quat, Transform, Vec3};

/// How much a joint takes part in a [`blend`] or [`add`], 0 to 1, for an upper-body override, say.
/// A joint past the mask's end takes no part.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JointMask {
    weights: Vec<f32>,
}

impl JointMask {
    /// A mask of `joints` joints that takes none of them.
    #[must_use]
    pub fn nothing(joints: usize) -> Self {
        Self { weights: vec![0.0; joints] }
    }

    /// A mask of `joints` joints that takes all of them.
    #[must_use]
    pub fn everything(joints: usize) -> Self {
        Self { weights: vec![1.0; joints] }
    }

    /// Sets one joint's weight, held to 0 to 1. A joint past the end is ignored.
    pub fn set(&mut self, joint: usize, weight: f32) {
        if let Some(entry) = self.weights.get_mut(joint) {
            *entry = unit(weight);
        }
    }

    /// Sets `root` and every joint under it in `skeleton` to `weight`, held to 0 to 1.
    pub fn set_subtree(&mut self, skeleton: &ArchivedSkeleton, root: usize, weight: f32) {
        // Parents come first, so one pass finds every descendant.
        let mut under = vec![false; skeleton.len()];
        for joint in 0..skeleton.len() {
            let parent = skeleton.parents[joint].to_native();
            under[joint] = joint == root || (parent != NO_PARENT && under[usize::from(parent)]);
            if under[joint] {
                self.set(joint, weight);
            }
        }
    }

    /// One joint's weight: 0 past the end.
    #[must_use]
    pub fn weight(&self, joint: usize) -> f32 {
        self.weights.get(joint).copied().unwrap_or(0.0)
    }

    /// The weights by joint.
    #[must_use]
    pub fn weights(&self) -> &[f32] {
        &self.weights
    }
}

/// `weight` held to 0 to 1, with NaN as 0.
#[inline]
fn unit(weight: f32) -> f32 {
    if weight >= 0.0 { weight.min(1.0) } else { 0.0 }
}

/// Blends `pose` toward `other` by `weight`, times `mask`'s weight for each joint: linearly for
/// translation and scale, and for rotation along the short way, renormalized. At 0 a joint keeps
/// its bits and at 1 it takes `other`'s. A joint past the end of either pose or the mask is left
/// alone. Allocates nothing.
pub fn blend(pose: &mut [Transform], other: &[Transform], weight: f32, mask: Option<&JointMask>) {
    let weight = unit(weight);
    if weight == 0.0 {
        return;
    }
    match mask {
        None => {
            for (joint, other) in pose.iter_mut().zip(other) {
                *joint = mixed(joint, other, weight);
            }
        }
        Some(mask) => {
            for ((joint, other), &share) in pose.iter_mut().zip(other).zip(mask.weights()) {
                *joint = mixed(joint, other, weight * share);
            }
        }
    }
}

#[inline]
fn mixed(from: &Transform, to: &Transform, weight: f32) -> Transform {
    if weight <= 0.0 {
        *from
    } else if weight >= 1.0 {
        *to
    } else {
        Transform {
            translation: from.translation.lerp(to.translation, weight),
            // glam's lerp takes the short way and renormalizes.
            rotation: from.rotation.lerp(to.rotation, weight),
            scale: from.scale.lerp(to.scale, weight),
        }
    }
}

/// `delta`, what [`add`] adds to reach `pose` from `reference`: per joint, the translation's
/// difference, the rotation that takes `reference`'s to `pose`'s in the parent's frame, and the
/// scale's ratio (1 where `reference`'s axis is zero). A joint past the end of any of them is left
/// alone. Allocates nothing.
pub fn make_delta(pose: &[Transform], reference: &[Transform], delta: &mut [Transform]) {
    let ratio = |value: f32, of: f32| if of == 0.0 { 1.0 } else { value / of };
    for ((delta, pose), reference) in delta.iter_mut().zip(pose).zip(reference) {
        *delta = Transform {
            translation: pose.translation - reference.translation,
            rotation: (pose.rotation * reference.rotation.inverse()).normalize(),
            scale: Vec3::new(
                ratio(pose.scale.x, reference.scale.x),
                ratio(pose.scale.y, reference.scale.y),
                ratio(pose.scale.z, reference.scale.z),
            ),
        };
    }
}

/// Adds `delta` ([`make_delta`]) on top of `pose` by `weight`, times `mask`'s weight for each joint:
/// what a hit reaction or breathing layer does. The rotation is applied in the parent's frame,
/// after the pose's. At 0 a joint keeps its bits. A joint past the end of either pose or the mask
/// is left alone. Allocates nothing.
pub fn add(pose: &mut [Transform], delta: &[Transform], weight: f32, mask: Option<&JointMask>) {
    let weight = unit(weight);
    if weight == 0.0 {
        return;
    }
    match mask {
        None => {
            for (joint, delta) in pose.iter_mut().zip(delta) {
                added(joint, delta, weight);
            }
        }
        Some(mask) => {
            for ((joint, delta), &share) in pose.iter_mut().zip(delta).zip(mask.weights()) {
                added(joint, delta, weight * share);
            }
        }
    }
}

#[inline]
fn added(joint: &mut Transform, delta: &Transform, weight: f32) {
    if weight <= 0.0 {
        return;
    }
    joint.translation += delta.translation * weight;
    // From no turn toward the delta's by the short way: lerp handles the delta's sign.
    joint.rotation = (Quat::IDENTITY.lerp(delta.rotation, weight) * joint.rotation).normalize();
    joint.scale *= Vec3::ONE.lerp(delta.scale, weight);
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use sapphire_assets::clip::{Clip, DEFAULT_RATE, PackedRotation, RotationTrack};
    use sapphire_assets::skeleton::{Joint, JointTransform, Skeleton};

    use super::*;
    use crate::{overlay, sample};

    /// A pose as a snapshot reads it: one line per joint, to 5 decimals (so a backend's rounding
    /// in the last bit doesn't move it), with rotations in the hemisphere whose largest component
    /// is positive, since `q` and `−q` are one rotation.
    fn snapshot(pose: &[Transform]) -> String {
        let number = |value: f32| {
            let text = format!("{value:.5}");
            if text.trim_start_matches('-').bytes().all(|byte| byte == b'0' || byte == b'.') {
                text.trim_start_matches('-').to_owned()
            } else {
                text
            }
        };
        let mut lines = String::new();
        for (joint, transform) in pose.iter().enumerate() {
            let q = transform.rotation.to_array();
            let largest =
                q.iter().fold(0.0_f32, |largest, &c| if c.abs() > largest.abs() { c } else { largest });
            let sign = if largest < 0.0 { -1.0 } else { 1.0 };
            let [tx, ty, tz] = transform.translation.to_array();
            let [sx, sy, sz] = transform.scale.to_array();
            writeln!(
                lines,
                "{joint}: t {} {} {}  r {} {} {} {}  s {} {} {}",
                number(tx),
                number(ty),
                number(tz),
                number(q[0] * sign),
                number(q[1] * sign),
                number(q[2] * sign),
                number(q[3] * sign),
                number(sx),
                number(sy),
                number(sz),
            )
            .expect("writing to a string");
        }
        lines
    }

    fn bits(pose: &[Transform]) -> Vec<u32> {
        pose.iter()
            .flat_map(|joint| {
                let [tx, ty, tz] = joint.translation.to_array();
                let [rx, ry, rz, rw] = joint.rotation.to_array();
                let [sx, sy, sz] = joint.scale.to_array();
                [tx, ty, tz, rx, ry, rz, rw, sx, sy, sz].map(f32::to_bits)
            })
            .collect()
    }

    /// Three joints with a little of everything: an upper arm, a forearm, a hand.
    fn from() -> [Transform; 3] {
        [
            Transform::from_translation(Vec3::new(0.0, 1.0, 0.0)),
            Transform::from_rotation(Quat::from_rotation_z(0.5)),
            Transform { scale: Vec3::splat(2.0), ..Transform::from_rotation(Quat::from_rotation_x(-0.25)) },
        ]
    }

    fn to() -> [Transform; 3] {
        [
            Transform::from_translation(Vec3::new(1.0, 3.0, -2.0)),
            Transform::from_rotation(Quat::from_rotation_z(1.5)),
            Transform {
                scale: Vec3::new(1.0, 4.0, 1.0),
                ..Transform::from_rotation(Quat::from_rotation_x(0.75))
            },
        ]
    }

    #[test]
    fn a_weight_of_zero_changes_no_bit_and_a_full_one_takes_the_other_pose() {
        let (from, to) = (from(), to());
        let mut pose = from;
        blend(&mut pose, &to, 0.0, None);
        assert_eq!(bits(&pose), bits(&from));
        blend(&mut pose, &to, f32::NAN, None);
        assert_eq!(bits(&pose), bits(&from), "NaN is no weight");
        blend(&mut pose, &to, -1.0, None);
        assert_eq!(bits(&pose), bits(&from));

        blend(&mut pose, &to, 1.0, None);
        assert_eq!(bits(&pose), bits(&to));
        let mut pose = from;
        blend(&mut pose, &to, 7.0, None);
        assert_eq!(bits(&pose), bits(&to), "held to 1");
        let mut pose = from;
        blend(&mut pose, &to, 1.0, Some(&JointMask::everything(3)));
        assert_eq!(bits(&pose), bits(&to));
    }

    #[test]
    fn a_mask_of_nothing_changes_no_bit() {
        let (from, to) = (from(), to());
        let mut pose = from;
        blend(&mut pose, &to, 1.0, Some(&JointMask::nothing(3)));
        assert_eq!(bits(&pose), bits(&from));
        add(&mut pose, &to, 1.0, Some(&JointMask::nothing(3)));
        assert_eq!(bits(&pose), bits(&from));
        // A mask shorter than the pose takes nothing past its end.
        let mut short = JointMask::everything(1);
        short.set(5, 1.0);
        blend(&mut pose, &to, 1.0, Some(&short));
        assert_eq!(bits(&pose[..1]), bits(&to[..1]));
        assert_eq!(bits(&pose[1..]), bits(&from[1..]));
    }

    #[test]
    fn a_blend_with_a_mask_snapshot() {
        let (from, to) = (from(), to());
        let mut mask = JointMask::nothing(3);
        mask.set(1, 0.5);
        mask.set(2, 1.0);
        let mut pose = from;
        blend(&mut pose, &to, 0.5, Some(&mask));
        insta::assert_snapshot!(snapshot(&pose));
    }

    /// Blending across the double cover: the same two rotations in any hemispheres are the same
    /// blend, along the short way, and never pass through zero length.
    #[test]
    fn a_blend_takes_the_short_way_in_any_hemisphere() {
        let a = Quat::from_rotation_y(0.2);
        let b = Quat::from_rotation_y(0.6);
        let halfway = Quat::from_rotation_y(0.4);
        let joint = |rotation| [Transform::from_rotation(rotation)];
        for (from, to) in [(a, b), (-a, b), (a, -b), (-a, -b)] {
            let mut pose = joint(from);
            blend(&mut pose, &joint(to), 0.5, None);
            let blended = pose[0].rotation;
            assert!(blended.is_normalized(), "{blended}");
            assert!(blended.dot(halfway).abs() > 1.0 - 1e-5, "{from} to {to} gave {blended}");
        }
        // A rotation and its own negative are one rotation: blending moves nothing.
        let mut pose = joint(a);
        blend(&mut pose, &joint(-a), 0.5, None);
        assert!(pose[0].rotation.dot(a).abs() > 1.0 - 1e-6, "{}", pose[0].rotation);
    }

    #[test]
    fn blends_along_the_way_snapshot() {
        let (from, to) = (from(), to());
        let mut lines = String::new();
        for weight in [0.25, 0.5, 0.75] {
            let mut pose = from;
            blend(&mut pose, &to, weight, None);
            writeln!(lines, "weight {weight}\n{}", snapshot(&pose)).expect("writing to a string");
        }
        insta::assert_snapshot!(lines);
    }

    /// The other side of the double cover, snapshotted: the result must be the same as above's.
    #[test]
    fn a_negated_pose_blends_to_the_same_rotations() {
        let (from, to) = (from(), to());
        let negated = to.map(|joint| Transform { rotation: -joint.rotation, ..joint });
        for weight in [0.25, 0.5, 0.75] {
            let (mut a, mut b) = (from, from);
            blend(&mut a, &to, weight, None);
            blend(&mut b, &negated, weight, None);
            assert_eq!(snapshot(&a), snapshot(&b), "weight {weight}");
        }
    }

    #[test]
    fn additive_delta_round_trips_and_weighs() {
        let (from, to) = (from(), to());
        let mut delta = [Transform::IDENTITY; 3];
        make_delta(&to, &from, &mut delta);

        let mut pose = from;
        add(&mut pose, &delta, 1.0, None);
        for (joint, expected) in pose.iter().zip(&to) {
            assert!(joint.translation.abs_diff_eq(expected.translation, 1e-5), "{joint:?}");
            assert!(joint.rotation.dot(expected.rotation).abs() > 1.0 - 1e-5, "{joint:?}");
            assert!(joint.scale.abs_diff_eq(expected.scale, 1e-5), "{joint:?}");
        }

        let mut pose = from;
        add(&mut pose, &delta, 0.0, None);
        assert_eq!(bits(&pose), bits(&from), "no weight, no change");

        // A delta at rest is nothing.
        let mut identity = [Transform::IDENTITY; 3];
        make_delta(&from, &from, &mut identity);
        let mut pose = from;
        add(&mut pose, &identity, 1.0, None);
        for (joint, expected) in pose.iter().zip(&from) {
            assert!(joint.translation.abs_diff_eq(expected.translation, 1e-6), "{joint:?}");
            assert!(joint.rotation.dot(expected.rotation).abs() > 1.0 - 1e-6, "{joint:?}");
            assert!(joint.scale.abs_diff_eq(expected.scale, 1e-6), "{joint:?}");
        }
    }

    /// A delta written in the far hemisphere still adds its short way: half of a small turn, not
    /// nearly a whole one.
    #[test]
    fn an_additive_turn_takes_the_short_way() {
        let turn = Quat::from_rotation_x(0.4);
        let mut pose = [Transform::IDENTITY];
        add(&mut pose, &[Transform::from_rotation(-turn)], 0.5, None);
        let expected = Quat::from_rotation_x(0.2);
        assert!(pose[0].rotation.dot(expected).abs() > 1.0 - 1e-5, "{}", pose[0].rotation);
    }

    #[test]
    fn a_weighted_additive_layer_snapshot() {
        let (from, to) = (from(), to());
        let mut delta = [Transform::IDENTITY; 3];
        make_delta(&to, &from, &mut delta);
        let mut mask = JointMask::everything(3);
        mask.set(0, 0.0);
        let mut pose = from;
        add(&mut pose, &delta, 0.5, Some(&mask));
        insta::assert_snapshot!(snapshot(&pose));
    }

    #[test]
    fn a_subtree_mask_takes_a_joint_and_what_hangs_from_it() {
        let joints: Vec<Joint> = [None, Some(0), Some(1), Some(0), Some(3), Some(4)]
            .into_iter()
            .map(|parent| Joint { name: String::new(), parent, rest: JointTransform::IDENTITY })
            .collect();
        let bytes = Skeleton::new(&joints).to_cooked().expect("cooked");
        let skeleton = Skeleton::read(&bytes).expect("a valid skeleton");
        let mut mask = JointMask::nothing(6);
        mask.set_subtree(skeleton, 3, 2.0);
        assert_eq!(mask.weights(), [0.0, 0.0, 0.0, 1.0, 1.0, 1.0], "held to 1");
        assert_eq!(mask.weight(99), 0.0);
    }

    /// One rotation track on joint 0, from no turn to a quarter turn over `ticks`.
    fn turning(ticks: u16) -> Clip {
        let half = core::f32::consts::FRAC_1_SQRT_2;
        Clip {
            name: "turn".into(),
            skeleton: Some(sapphire_assets::AssetId::from_bytes([2; 16])),
            joint_count: 2,
            morph_target_count: 0,
            rate: DEFAULT_RATE,
            ticks,
            rotations: vec![RotationTrack { joint: 0, count: 2, first: 0 }],
            rotation_ticks: vec![0, ticks],
            rotation_keys: vec![
                PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
                PackedRotation::encode([half, 0.0, 0.0, half]),
            ],
            ..Clip::default()
        }
    }

    /// A position past a clip's end holds its last key, and NaN and negatives hold its first.
    #[test]
    fn a_clip_shorter_than_its_position_holds_its_end() {
        let bytes = turning(10).to_cooked().expect("cooked");
        let clip = Clip::read(&bytes).expect("a valid clip");
        let rest = [Transform::IDENTITY; 2];
        let mut pose = rest;
        let mut at = |position: f32| {
            sample(clip, position, &rest, &mut pose);
            pose[0].rotation
        };
        let end = at(10.0);
        assert!(end.dot(Quat::from_rotation_x(core::f32::consts::FRAC_PI_2)).abs() > 1.0 - 1e-5);
        assert_eq!(at(10.5).to_array().map(f32::to_bits), end.to_array().map(f32::to_bits));
        assert_eq!(at(1e6).to_array().map(f32::to_bits), end.to_array().map(f32::to_bits));
        let start = at(0.0);
        assert_eq!(at(-3.0).to_array().map(f32::to_bits), start.to_array().map(f32::to_bits));
        assert_eq!(at(f32::NAN).to_array().map(f32::to_bits), start.to_array().map(f32::to_bits));
    }

    /// Overlaying skips what it can't reach rather than reading out of bounds.
    #[test]
    fn a_track_past_a_short_pose_is_skipped() {
        let mut clip = turning(10);
        clip.rotations[0].joint = 1;
        let bytes = clip.to_cooked().expect("cooked");
        let clip = Clip::read(&bytes).expect("a valid clip");
        let mut pose = [Transform::IDENTITY];
        overlay(clip, 5.0, &mut pose);
        assert_eq!(pose[0], Transform::IDENTITY);
    }
}
