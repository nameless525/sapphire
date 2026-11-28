//! Skeleton cuts by cook (M6 C8): a VRM sidecar's `joints: (desktop: None, mobile: Some(140))` keeps at
//! most that many joints on that cook, so an authored character meets the phones' row of spec §3.4 with
//! the desktop's skeleton untouched.
//!
//! # What goes
//!
//! Only spring-chain joints, from their tips: each round takes the last joint of every chain that has
//! more than its root, the joints furthest along their chains first, then the deepest in the skeleton,
//! then the latest in joint order, until the count fits, passing again while a pass drops anything (a
//! joint two chains share goes once the joints below it in both have). A round ends every chain one
//! joint shorter before any loses a second, so a chain's tail goes first, and a VRM 1.0 tail is only a
//! point its parent swings toward. A round the cap stops partway takes joints by index, so a cap can
//! cut one side of a mirrored pair and not the other. A joint stays when it is humanoid, holds a
//! collider, is constrained or a constraint's source, is a chain's center, or has a joint below it that
//! stays: so a joint that isn't a spring joint (a helper, a socket) never goes, nor the joints above
//! it. A socket on a spring joint can go: a playable or enemy holding a mesh there is refused at cook.
//! A chain keeps its root, with its own settings, so every chain still swings.
//!
//! # What changes
//!
//! - **The skeleton:** the kept joints, in their order, with their parents renumbered.
//! - **The mesh:** each palette entry on a dropped joint becomes one on the nearest kept joint above
//!   it, in its place, its inverse bind carried up the rest pose. No entry moves, so every vertex's
//!   influences and weights (summing to 1 as imported) are the bytes they were, and the mesh cooks in
//!   the same vertex order.
//! - **The rig:** joint indices renumbered; a cut chain ends in [`SpringTip::Point`] where its first
//!   dropped joint stood, in its last kept joint's space, so that joint swings toward the same point as
//!   before. A cut of tails alone moves nothing but rounding: a tail joint never swings, and its vertices
//!   ride its parent rigidly either way.
//! - **Clips:** a clip read onto a cut skeleton keys only what it has. Key-pose documents and
//!   retargets key humanoid joints, which stay; a source sharing the skeleton (`rig:`) has its keys on
//!   dropped joints ignored ([`crate::clip::source::joint_remap_cut`]).
//!
//! A cap the cut can't reach is an error naming the field. A cook without a cap, or a skeleton within
//! it, is untouched, so the desktop cook's bytes are what they were.
//!
//! All arithmetic is scalar `f32` with no fused operations, so the cut agrees across architectures.

use std::num::NonZeroU16;

use sapphire_assets::rig::{CharacterRig, SpringTip};
use sapphire_assets::skeleton::{Joint, JointTransform};
use serde::{Deserialize, Serialize};

use crate::cook::Cook;
use crate::mesh::ImportedSkin;

/// The most joints each cook keeps: `None` keeps the source's ([module docs](self)).
///
/// Both cooks are written once one is set, as [`MaxSize`](crate::texture::MaxSize)'s are: cache keys
/// don't name fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JointCap {
    pub desktop: Option<NonZeroU16>,
    pub mobile: Option<NonZeroU16>,
}

impl JointCap {
    /// No cap on either cook.
    pub const NONE: Self = Self { desktop: None, mobile: None };

    /// Whether neither cook has a cap.
    pub fn is_none(&self) -> bool {
        *self == Self::NONE
    }

    /// `cook`'s cap, if it has one.
    pub fn of(self, cook: Cook) -> Option<NonZeroU16> {
        match cook {
            Cook::Desktop => self.desktop,
            Cook::Mobile => self.mobile,
        }
    }
}

/// A cap the cut can't reach.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum CutError {
    #[error(
        "`joints.{cook}` is {cap}, but cutting spring chains takes the skeleton's {joints} joints down to \
         {fewest} at the fewest: the others are humanoid, hold a collider, a constraint or a chain's center, \
         aren't spring joints, are a chain's root or hold a joint below them that stays"
    )]
    Unreachable { cook: Cook, cap: u16, joints: usize, fewest: usize },
}

/// Which joints a cut keeps, and where each dropped one goes ([`plan`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cut {
    /// Per joint of the source, its index in the cut skeleton, `None` if it is dropped.
    pub kept: Vec<Option<u16>>,
    /// Per joint of the source, the source index of the joint its vertices follow: itself if it is kept,
    /// else the nearest kept joint above it.
    pub into: Vec<usize>,
}

impl Cut {
    /// How many joints it keeps.
    pub fn kept_count(&self) -> usize {
        self.kept.iter().flatten().count()
    }

    /// The names of the joints it drops, in joint order.
    pub fn dropped<'a>(&self, joints: &'a [Joint]) -> Vec<&'a str> {
        joints
            .iter()
            .zip(&self.kept)
            .filter(|(_, kept)| kept.is_none())
            .map(|(joint, _)| joint.name.as_str())
            .collect()
    }
}

/// Cuts `joints`, their skin and their rig to `cook`'s cap in `cap`, in place; nothing changes without a
/// cap, or within it. Returns the cut made, if any.
pub fn cut(
    joints: &mut Vec<Joint>,
    skin: Option<&mut ImportedSkin>,
    rig: &mut CharacterRig,
    cap: JointCap,
    cook: Cook,
) -> Result<Option<Cut>, CutError> {
    let Some(limit) = cap.of(cook) else { return Ok(None) };
    let Some(cut) = plan(joints, rig, limit.get(), cook)? else { return Ok(None) };
    if let Some(skin) = skin {
        cut_skin(&cut, joints, skin);
    }
    cut_rig(&cut, joints, rig);
    *joints = cut_joints(&cut, joints);
    Ok(Some(cut))
}

/// Cuts a skin that shares a cut skeleton (`skeleton:`, M6 C8): the joints of its own named in
/// `dropped`, the ones the owner's cut dropped, go as the owner's did, each entry and weight on one
/// folding into the nearest kept joint above it. A skin whose kept joint sits under a dropped one, or
/// whose dropped joint has no kept one above it, is left for [`crate::clip::source::joint_remap`] to
/// refuse, since its armature isn't the owner's.
pub fn cut_shared(skin: &mut ImportedSkin, dropped: &[String]) {
    let joints = std::mem::take(&mut skin.joints);
    let gone: Vec<bool> = joints.iter().map(|joint| dropped.contains(&joint.name)).collect();
    let orphan = joints
        .iter()
        .zip(&gone)
        .any(|(joint, &went)| !went && joint.parent.is_some_and(|parent| gone[usize::from(parent)]));
    let mut into = Vec::with_capacity(joints.len());
    for index in 0..joints.len() {
        let mut at = Some(index);
        while let Some(joint) = at.filter(|&joint| gone[joint]) {
            at = joints[joint].parent.map(usize::from);
        }
        into.push(at);
    }
    if !gone.contains(&true) || orphan || into.contains(&None) {
        skin.joints = joints;
        return;
    }
    let mut kept = Vec::with_capacity(joints.len());
    let mut next = 0u16;
    for &gone in &gone {
        kept.push((!gone).then(|| {
            next += 1;
            next - 1
        }));
    }
    let cut = Cut { kept, into: into.into_iter().map(|at| at.expect("checked")).collect() };
    cut_skin(&cut, &joints, skin);
    skin.joints = cut_joints(&cut, &joints);
}

/// The cut that brings `joints` to at most `cap` ([module docs](self)), or `None` if they fit already.
pub fn plan(joints: &[Joint], rig: &CharacterRig, cap: u16, cook: Cook) -> Result<Option<Cut>, CutError> {
    let count = joints.len();
    if count <= usize::from(cap) {
        return Ok(None);
    }
    let mut keep = vec![false; count];
    let mut pin = |joint: u16| {
        if let Some(slot) = keep.get_mut(usize::from(joint)) {
            *slot = true;
        }
    };
    rig.humanoid.iter().for_each(|entry| pin(entry.joint));
    rig.colliders.iter().for_each(|collider| pin(collider.joint));
    for constraint in &rig.constraints {
        pin(constraint.joint);
        pin(constraint.source);
    }
    for spring in &rig.springs {
        spring.center.into_iter().chain(spring.joints.first().map(|entry| entry.joint)).for_each(&mut pin);
    }

    // Each spring joint's place: the first chain it is in, as the springs swing it.
    let mut place: Vec<Option<(usize, usize)>> = vec![None; count];
    for (chain, spring) in rig.springs.iter().enumerate() {
        for (at, entry) in spring.joints.iter().enumerate() {
            if let Some(slot) = place.get_mut(usize::from(entry.joint))
                && slot.is_none()
            {
                *slot = Some((chain, at));
            }
        }
    }
    let depth = depths(joints);
    // Every joint but a chain's root, ordered as rounds from the tips take them.
    let mut order: Vec<(usize, usize, usize, usize)> = place
        .iter()
        .enumerate()
        .filter_map(|(joint, place)| {
            let (chain, at) = (*place)?;
            let from_tip = rig.springs[chain].joints.len() - 1 - at;
            (at > 0).then_some((from_tip, at, depth[joint], joint))
        })
        .collect();
    order.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(b.2.cmp(&a.2)).then(b.3.cmp(&a.3)));

    // Joints with a kept joint below them, counted as children kept.
    let mut kept_children = vec![0usize; count];
    for joint in joints {
        if let Some(parent) = joint.parent {
            kept_children[usize::from(parent)] += 1;
        }
    }
    let mut dropped = vec![false; count];
    let mut left = count;
    let target = usize::from(cap);
    // Passes over the order until the count fits or a pass drops nothing: a joint passed over while a
    // joint below it stayed (one of another chain that shares it) may go once that one has.
    loop {
        let before = left;
        for &(_, _, _, joint) in &order {
            if left <= target {
                break;
            }
            if keep[joint] || dropped[joint] || kept_children[joint] > 0 {
                continue;
            }
            dropped[joint] = true;
            left -= 1;
            if let Some(parent) = joints[joint].parent {
                kept_children[usize::from(parent)] -= 1;
            }
        }
        if left <= target || left == before {
            break;
        }
    }
    if left > target {
        return Err(CutError::Unreachable { cook, cap, joints: count, fewest: left });
    }

    let mut kept = Vec::with_capacity(count);
    let mut next = 0u16;
    for &gone in &dropped {
        kept.push((!gone).then(|| {
            next += 1;
            next - 1
        }));
    }
    let into = (0..count)
        .map(|joint| {
            let mut at = joint;
            while dropped[at] {
                at = usize::from(
                    joints[at].parent.expect("a chain's root stays, so every dropped joint has a parent"),
                );
            }
            at
        })
        .collect();
    Ok(Some(Cut { kept, into }))
}

/// Each joint's depth: how many parents it has.
fn depths(joints: &[Joint]) -> Vec<usize> {
    let mut depth = vec![0; joints.len()];
    for (index, joint) in joints.iter().enumerate() {
        if let Some(parent) = joint.parent {
            depth[index] = depth[usize::from(parent)] + 1;
        }
    }
    depth
}

/// The kept joints, their parents renumbered. A kept joint's parent is kept: only joints with nothing
/// kept below them go.
fn cut_joints(cut: &Cut, joints: &[Joint]) -> Vec<Joint> {
    joints
        .iter()
        .zip(&cut.kept)
        .filter(|(_, kept)| kept.is_some())
        .map(|(joint, _)| Joint {
            name: joint.name.clone(),
            parent: joint
                .parent
                .map(|parent| cut.kept[usize::from(parent)].expect("a kept joint's parent stays")),
            rest: joint.rest,
        })
        .collect()
}

/// The skin on the cut skeleton ([module docs](self)): every palette entry keeps its place, so each
/// vertex's influences and weights are the bytes they were, and a mesh cooks its vertices in the order it
/// did (the heroine's clearance fits samples by that order). An entry on a dropped joint becomes one on
/// the nearest kept joint above it, its inverse bind carried up the rest pose, so its vertices stay where
/// its own skin bound them, whichever skin that was.
fn cut_skin(cut: &Cut, joints: &[Joint], skin: &mut ImportedSkin) {
    for bind in &mut skin.binds {
        let source = usize::from(bind.joint);
        let into = cut.into[source];
        let joint = cut.kept[into].expect("a dropped joint folds into a kept one");
        if into != source {
            // The kept joint's inverse bind: its space to the dropped joint's, then the dropped one's.
            bind.inverse_bind_matrix = multiply(&relative(joints, into, source), &bind.inverse_bind_matrix);
        }
        bind.joint = joint;
    }
}

/// The rig on the cut skeleton ([module docs](self)).
fn cut_rig(cut: &Cut, joints: &[Joint], rig: &mut CharacterRig) {
    let renumber = |joint: u16| cut.kept[usize::from(joint)].expect("the rig's other joints stay");
    rig.joint_count = cut.kept_count() as u32;
    for entry in &mut rig.humanoid {
        entry.joint = renumber(entry.joint);
    }
    for collider in &mut rig.colliders {
        collider.joint = renumber(collider.joint);
    }
    for constraint in &mut rig.constraints {
        constraint.joint = renumber(constraint.joint);
        constraint.source = renumber(constraint.source);
    }
    for spring in &mut rig.springs {
        spring.center = spring.center.map(renumber);
        let Some(first_dropped) =
            spring.joints.iter().position(|entry| cut.kept[usize::from(entry.joint)].is_none())
        else {
            spring.joints.iter_mut().for_each(|entry| entry.joint = renumber(entry.joint));
            continue;
        };
        let last = usize::from(spring.joints[first_dropped - 1].joint);
        let tail = usize::from(spring.joints[first_dropped].joint);
        let to_tail = relative(joints, last, tail);
        spring.tip = SpringTip::Point { offset: [to_tail[3][0], to_tail[3][1], to_tail[3][2]] };
        spring.joints.truncate(first_dropped);
        spring.joints.iter_mut().for_each(|entry| entry.joint = renumber(entry.joint));
    }
}

/// A column-major 4×4 matrix, as glTF's inverse binds are.
type Matrix = [[f32; 4]; 4];

/// `descendant`'s rest transform in `ancestor`'s space: the rest poses from `ancestor`'s child down to
/// `descendant`, multiplied in order.
fn relative(joints: &[Joint], ancestor: usize, descendant: usize) -> Matrix {
    let mut path = Vec::new();
    let mut at = descendant;
    while at != ancestor {
        path.push(at);
        at = usize::from(joints[at].parent.expect("the ancestor is above it"));
    }
    path.iter().rev().fold(IDENTITY, |product, &joint| multiply(&product, &local(&joints[joint].rest)))
}

const IDENTITY: Matrix =
    [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

/// A rest pose as a matrix: scale, then rotation, then translation. Its rotation is normalized first,
/// as everything that reads a skeleton does.
fn local(rest: &JointTransform) -> Matrix {
    let [x, y, z, w] = rest.rotation;
    let length = (x * x + y * y + z * z + w * w).sqrt();
    let (x, y, z, w) = (x / length, y / length, z / length, w / length);
    let [sx, sy, sz] = rest.scale;
    let [tx, ty, tz] = rest.translation;
    [
        [(1.0 - 2.0 * (y * y + z * z)) * sx, 2.0 * (x * y + z * w) * sx, 2.0 * (x * z - y * w) * sx, 0.0],
        [2.0 * (x * y - z * w) * sy, (1.0 - 2.0 * (x * x + z * z)) * sy, 2.0 * (y * z + x * w) * sy, 0.0],
        [2.0 * (x * z + y * w) * sz, 2.0 * (y * z - x * w) * sz, (1.0 - 2.0 * (x * x + y * y)) * sz, 0.0],
        [tx, ty, tz, 1.0],
    ]
}

/// `a × b`, column-major, summed in order.
fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    std::array::from_fn(|column| {
        std::array::from_fn(|row| (0..4).fold(0.0, |sum, k| sum + a[k][row] * b[column][k]))
    })
}

#[cfg(test)]
mod tests;
