//! Sampling: what a clip says a skeleton's joints and morph targets hold at a position.
//!
//! Plain functions over the cooked formats, with no state between calls: the same arguments give
//! the same bits, and a jump costs what a step costs (the [`clip`](sapphire_assets::clip) module's
//! docs say why there is no cursor). A move's per-tick tables don't come through here: see the
//! [crate docs](crate).

use sapphire_assets::clip::ArchivedClip;
use sapphire_assets::skeleton::ArchivedSkeleton;
use sapphire_math::{Quat, Transform, Vec3};

/// `skeleton`'s rest pose: each joint's local transform, rotations normalized. Allocates, so a
/// caller keeps it: [`sample`] starts from it every time.
#[must_use]
pub fn rest_pose(skeleton: &ArchivedSkeleton) -> Vec<Transform> {
    skeleton
        .rest
        .iter()
        .map(|joint| {
            let joint = joint.to_native();
            Transform {
                translation: Vec3::from(joint.translation),
                rotation: Quat::from_array(joint.rotation).normalize(),
                scale: Vec3::from(joint.scale),
            }
        })
        .collect()
}

/// A position in a clip that key lookups can trust: NaN and anything before the start land on the
/// first tick. A position past the end holds the last key where a track reads it.
#[inline]
fn guarded(position: f32) -> f32 {
    if position >= 0.0 { position } else { 0.0 }
}

/// `clip` at `position` over `rest` ([`rest_pose`]): every joint and channel the clip has no track
/// for stays at rest. `position` is a tick and a fraction ([`ArchivedClip::position`] makes one from
/// seconds); past the clip's end it holds the last keys. Allocates nothing.
///
/// # Panics
///
/// If `pose` and `rest` differ in length.
pub fn sample(clip: &ArchivedClip, position: f32, rest: &[Transform], pose: &mut [Transform]) {
    pose.copy_from_slice(rest);
    overlay(clip, position, pose);
}

/// `clip`'s tracks at `position`, written over `pose`: what [`sample`] does after copying the rest
/// pose, for a caller starting from something else. A track past the end of `pose` is skipped, so a
/// clip cooked for another skeleton can't read out of bounds, but such a pairing draws nonsense:
/// check it with [`ArchivedClip::check_skeleton`]. Allocates nothing.
pub fn overlay(clip: &ArchivedClip, position: f32, pose: &mut [Transform]) {
    let position = guarded(position);
    for track in clip.rotations.iter() {
        if let Some(joint) = pose.get_mut(usize::from(track.joint.to_native())) {
            joint.rotation = Quat::from_array(clip.rotation(track, position));
        }
    }
    for track in clip.translations.iter() {
        if let Some(joint) = pose.get_mut(usize::from(track.joint.to_native())) {
            joint.translation = Vec3::from(clip.vector(track, position));
        }
    }
    for track in clip.scales.iter() {
        if let Some(joint) = pose.get_mut(usize::from(track.joint.to_native())) {
            joint.scale = Vec3::from(clip.vector(track, position));
        }
    }
}

/// `clip`'s morph weights at `position`, written over `weights`: a target with no track keeps what
/// it had, so a caller starts from its rest weights. A track past the end of `weights` is skipped.
/// Allocates nothing.
pub fn overlay_weights(clip: &ArchivedClip, position: f32, weights: &mut [f32]) {
    let position = guarded(position);
    for track in clip.weights.iter() {
        if let Some(weight) = weights.get_mut(usize::from(track.target.to_native())) {
            *weight = clip.weight(track, position);
        }
    }
}

/// `clip`'s root motion at `position`, a transform from its first tick: `root(t) × pose(t)` is the
/// motion as authored. `None` for a clip without any. Allocates nothing.
#[must_use]
pub fn root_motion(clip: &ArchivedClip, position: f32) -> Option<Transform> {
    let root = clip.root.as_ref()?;
    let position = guarded(position);
    Some(Transform::from_translation_rotation(
        Vec3::from(clip.vector(&root.translation, position)),
        Quat::from_array(clip.rotation(&root.rotation, position)),
    ))
}
