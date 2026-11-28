//! Eyes that move on their own (M6 A22): a graph's [`Eyes`] bound to a character's look-at.
//!
//! **A pure function of the clock and the seed.** Time falls into cells a mean hold long; each cell's jump
//! starts up to the hold's spread late, so the time from one jump's start to the next's stays within the
//! document's `hold`, as the blink's starts do. Each jump goes to a gaze of its own, from the cell and the
//! seed, nearer the middle of the reach more often than out at its edge, and takes as long as its length
//! at the document's speed, eased in and out. So the gaze at a tick needs only the cell's jump and the
//! one before: nothing carries from frame to frame, a frozen clock holds the eyes, a jump lands where it
//! should, and two seeds look about apart.
//!
//! **Through the rig's look-at,** VRM 1.0's: the gaze goes through its range maps
//! ([`EyeRig::response`], the IK's look shares them) to eye bones turned in rest model space, or to the
//! `lookUp`, `lookDown`, `lookLeft` and `lookRight` expressions by weight.
//!
//! **Overrides.** An expression that overrides the look turns the expressions down in the face layer, as it
//! does the blink, since the face layer scales the look family's morph targets as the pose made them. Bones
//! aren't morphs, so for a look-at of bones the face layer's share comes back in through
//! [`GraphPlayer::set_eyes_share`](super::GraphPlayer::set_eyes_share).

use sapphire_assets::graph::Eyes;
use sapphire_assets::rig::{ArchivedCharacterRig, HumanBone};
use sapphire_assets::skeleton::ArchivedSkeleton;
use sapphire_math::{Quat, det};

use super::GraphBindError;
use crate::character::CharacterPose;
use crate::ik::{EyeDrive, EyeRig};
use crate::pose_math::PoseMath;

/// The streams a cell's numbers come from, so a jump's time, its gaze and its blink don't move together.
const START: u64 = 1;
const YAW: [u64; 2] = [2, 3];
const PITCH: [u64; 2] = [4, 5];
const BLINK: u64 = 6;

/// A graph's eyes, resolved for one character's rig.
#[derive(Clone, Debug)]
pub(super) struct BoundEyes {
    /// Radians of gaze either way: yaw, pitch.
    reach: [f32; 2],
    period: f64,
    jitter: f64,
    /// Radians a tick.
    speed: f32,
    blink_chance: f32,
    rig: EyeRig,
}

/// One jump: when it starts, from where to where, and how long it takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Jump {
    pub(super) cell: i64,
    pub(super) start: f64,
    pub(super) from: [f32; 2],
    pub(super) to: [f32; 2],
    pub(super) length: f32,
}

pub(super) fn bind_eyes(
    eyes: &Eyes,
    skeleton: &ArchivedSkeleton,
    rig: Option<&ArchivedCharacterRig>,
    morph_targets: usize,
) -> Result<BoundEyes, GraphBindError> {
    let rig = rig.ok_or(GraphBindError::NoLookAt)?;
    let rest = crate::sample::rest_pose(skeleton);
    // A joint's rest rotation in the model's space, as a pose works it out.
    let rest_rotation = |joint: usize| {
        let mut chain = vec![joint];
        while let Some(parent) = skeleton.parent(*chain.last().expect("never empty")) {
            chain.push(parent);
        }
        chain
            .iter()
            .rev()
            .fold(Quat::IDENTITY, |above, &joint| PoseMath::Legacy.multiply(above, rest[joint].rotation))
    };
    let eye = |wanted: HumanBone| {
        let joint = rig
            .humanoid
            .iter()
            .find(|entry| entry.bone == wanted)
            .map(|entry| usize::from(entry.joint.to_native()))
            .filter(|&joint| joint < skeleton.len())?;
        let parent = skeleton.parent(joint)?;
        Some((joint as u16, rest_rotation(parent)))
    };
    let mut rig = EyeRig::from_cooked(rig, eye).ok_or(GraphBindError::NoLookAt)?;
    if let EyeDrive::Expressions { up, down, left, right } = &mut rig.drive {
        for targets in [&mut *up, &mut *down, &mut *left, &mut *right] {
            targets.retain(|&(target, _)| (target as usize) < morph_targets);
        }
        if [up, down, left, right].iter().all(|targets| targets.is_empty()) {
            return Err(GraphBindError::NoLookAt);
        }
    }
    let [least, most] = eyes.hold.map(f64::from);
    Ok(BoundEyes {
        reach: eyes.reach.map(f32::to_radians),
        period: (least + most) * 0.5,
        jitter: (most - least) * 0.5,
        speed: eyes.speed.to_radians(),
        blink_chance: eyes.blink_chance,
        rig,
    })
}

impl BoundEyes {
    /// Whether its look-at turns bones, which the face layer's overrides can't reach by the morphs.
    pub(super) fn turns_bones(&self) -> bool {
        matches!(self.rig.drive, EyeDrive::Bones { .. })
    }

    /// Where the gaze is at `ticks`, radians from rest: yaw toward the character's left, pitch up.
    pub(super) fn gaze(&self, ticks: f64, seed: u32) -> [f32; 2] {
        let jump = self.jump(ticks, seed);
        let through = ((ticks - jump.start) as f32 / jump.length).clamp(0.0, 1.0);
        let eased = through * through * (3.0 - 2.0 * through);
        std::array::from_fn(|axis| jump.from[axis] + (jump.to[axis] - jump.from[axis]) * eased)
    }

    /// When the jumps that come with a blink started, of the one under way at `ticks` and the one before:
    /// a blink lasts no longer than the least hold, so no earlier jump's can still show.
    pub(super) fn blinks(&self, ticks: f64, seed: u32) -> [Option<f64>; 2] {
        if self.blink_chance <= 0.0 {
            return [None; 2];
        }
        let now = self.jump(ticks, seed);
        [now, self.jump_of(now.cell - 1, seed)].map(|jump| {
            let corner = distance([0.0; 2], self.reach);
            let length = distance(jump.from, jump.to);
            let chance = f64::from(self.blink_chance * (length / corner).min(1.0));
            (unit(jump.cell, seed, BLINK) < chance).then_some(jump.start)
        })
    }

    /// Turns the eyes in `pose` to `gaze` ([`Self::gaze`]), `share` of the way (what the face's overrides
    /// leave of a bone look-at): eye bones on whatever the layers left, expressions added to the morph
    /// weights and held within 1, as the blink is.
    pub(super) fn apply(&self, gaze: [f32; 2], share: f32, pose: &mut CharacterPose, math: PoseMath) {
        let [left_yaw, right_yaw, lift] = self.rig.response(gaze[0], gaze[1]);
        match &self.rig.drive {
            EyeDrive::Bones { left, right } => {
                let locals = pose.locals_mut();
                for (eye, yaw) in [(left, left_yaw), (right, right_yaw)] {
                    if let Some((joint, parent_rest)) = eye {
                        // By the deterministic trigonometry, so the eyes turn the same on every platform's
                        // goldens; the IK's look keeps the pose's own maths.
                        let turn = math.multiply(
                            det::quat_from_rotation_y(yaw * share),
                            det::quat_from_rotation_x(-lift * share),
                        );
                        let local = &mut locals[usize::from(*joint)];
                        let turn = math.multiply(math.multiply(parent_rest.inverse(), turn), *parent_rest);
                        local.rotation = math.normalize(math.multiply(turn, local.rotation));
                    }
                }
            }
            EyeDrive::Expressions { up, down, left, right } => {
                let weights = pose.weights_mut();
                let mut add = |targets: &[(u32, f32)], amount: f32| {
                    if amount > 0.0 {
                        for &(target, share) in targets {
                            let weight = &mut weights[target as usize];
                            *weight = (*weight + amount * share).min(1.0);
                        }
                    }
                };
                add(up, lift);
                add(down, -lift);
                add(left, left_yaw);
                add(right, -left_yaw);
            }
        }
    }

    /// Turns eye bones over a pose the fixed step published ([`crate::BodySample::publish_to`]) and
    /// refreshes their model transforms: only bones, since the published morphs are the simulation's to
    /// match, and only joints the published body leaves to presentation. Whether it turned them.
    pub(super) fn apply_published(&self, gaze: [f32; 2], share: f32, pose: &mut CharacterPose) -> bool {
        let EyeDrive::Bones { left, right } = &self.rig.drive else { return false };
        let joints = [left, right].map(|eye| eye.map(|(joint, _)| usize::from(joint)));
        if joints.iter().flatten().any(|&joint| pose.protects(joint)) {
            return false;
        }
        let math = pose.math();
        self.apply(gaze, share, pose, math);
        for joint in joints.into_iter().flatten() {
            pose.refresh_model_below(joint);
        }
        true
    }

    /// The jump under way at `ticks`, or the last one done: the cell's, if it's started, else the one
    /// before's, which started before the cell did since a jump starts less than a mean hold late.
    pub(super) fn jump(&self, ticks: f64, seed: u32) -> Jump {
        let cell = (ticks / self.period).floor() as i64;
        let jump = self.jump_of(cell, seed);
        if ticks >= jump.start { jump } else { self.jump_of(cell - 1, seed) }
    }

    pub(super) fn jump_of(&self, cell: i64, seed: u32) -> Jump {
        let from = self.target(cell - 1, seed);
        let to = self.target(cell, seed);
        let length = (distance(from, to) / self.speed).max(1.0);
        Jump {
            cell,
            start: cell as f64 * self.period + self.jitter * unit(cell, seed, START),
            from,
            to,
            length,
        }
    }

    /// Where cell `cell`'s jump goes: within the reach, the middle likelier than the edge.
    fn target(&self, cell: i64, seed: u32) -> [f32; 2] {
        let spread = |[a, b]: [u64; 2]| (unit(cell, seed, a) - unit(cell, seed, b)) as f32;
        [spread(YAW) * self.reach[0], spread(PITCH) * self.reach[1]]
    }
}

/// How far apart two gazes are, in radians: by the square root, which is exact everywhere, where the
/// standard library's hypotenuse isn't pinned.
pub(super) fn distance(from: [f32; 2], to: [f32; 2]) -> f32 {
    let [x, y] = [to[0] - from[0], to[1] - from[1]];
    (x * x + y * y).sqrt()
}

/// A number from 0 to 1 for this cell of this character, from stream `stream`: stateless, so nothing about
/// the eyes depends on when anyone looked.
fn unit(cell: i64, seed: u32, stream: u64) -> f64 {
    let mut value = (cell as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (u64::from(seed) << 32 | u64::from(seed))
        ^ stream.wrapping_mul(0xd6e8_feb8_6659_fd93);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    (value >> 11) as f64 / (1_u64 << 53) as f64
}
