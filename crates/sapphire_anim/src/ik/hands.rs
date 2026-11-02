//! Hands on a held weapon: a two-bone solve for each arm, each hand's grip point onto its place on
//! the weapon.
//!
//! A weapon is either carried by a joint ([`WeaponPlace::Joint`], O4's blade on `handslot.r`), in which
//! case the main hand is where it is by definition and the off hand reaches for it, or placed by its
//! caller ([`WeaponPlace::Model`], a weapon with an animation of its own), in which case both hands
//! follow it. Weights are the caller's, plain inputs a graph drives: a hand lets go at 0 and leaves the
//! arm exactly as animated.
//!
//! Retargeted clips leave hands at rest under the wrists (N1), so a hand's own turn counts as much as
//! where its wrist goes, and a [`Grip`] sets both.

use sapphire_math::{Affine3A, Quat, Transform, Vec3};

use crate::ik::solve::{SolvePose, TwoBone, TwoBoneGoal, rotation_of, two_bone};

/// An arm's joints.
#[derive(Clone, Debug, PartialEq)]
pub struct ArmBones {
    pub chain: TwoBone,
    /// The upper arm down to the hand, every joint between, parents first.
    pub path: Vec<u16>,
    /// The joint the arm hangs from whose frame an elbow's hint is in, and its rest rotation.
    pub frame: u16,
    pub rest_frame: Quat,
}

/// Where a weapon is this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WeaponPlace {
    /// Placed by its caller, in the model's space.
    Model(Affine3A),
    /// Carried by a joint, with this transform from it: read after the main hand's solve.
    Joint { joint: u16, offset: Affine3A },
}

/// Which hand is which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Left,
    Right,
}

/// One hand's hold on a weapon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grip {
    /// The grip's place on the weapon, in the weapon's space.
    pub on_weapon: Transform,
    /// What holds it, in the hand joint's space: a palm's middle.
    pub in_hand: Transform,
    /// The way the elbow bends, in the model's space. `None` keeps the plane the arm bends in.
    pub pole: Option<Vec3>,
    /// 0 leaves the arm as animated and 1 puts the hand on the grip.
    pub weight: f32,
}

/// What the hands hold. See the `hands` module's docs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeldWeapon {
    pub place: WeaponPlace,
    pub main: Hand,
    /// The main hand's hold. A weapon its hand carries through a joint needs none to follow it, so a
    /// grip here only applies to a placed one.
    pub main_grip: Option<Grip>,
    pub off_grip: Option<Grip>,
}

/// How far a hand reaches before it eases off: see [`TwoBoneGoal::soften`].
const SOFTEN: f32 = 0.99;

/// Puts `arms` (left, then right) on `weapon`. `pose` must be up to date for the arms' paths and their
/// frames. Allocates nothing.
pub(crate) fn hold(pose: &mut SolvePose<'_>, arms: &[ArmBones; 2], weapon: &HeldWeapon) {
    let (main, off) = match weapon.main {
        Hand::Left => (&arms[0], &arms[1]),
        Hand::Right => (&arms[1], &arms[0]),
    };
    if let (WeaponPlace::Model(place), Some(grip)) = (weapon.place, &weapon.main_grip) {
        reach(pose, main, &place, grip);
    }
    let Some(grip) = weapon.off_grip.as_ref().filter(|grip| grip.weight > 0.0) else { return };
    let place = match weapon.place {
        WeaponPlace::Model(place) => place,
        WeaponPlace::Joint { joint, offset } => {
            pose.refresh_chain(usize::from(joint));
            pose.transform(usize::from(joint)) * offset
        }
    };
    reach(pose, off, &place, grip);
}

/// One arm onto its grip on a weapon at `place`.
fn reach(pose: &mut SolvePose<'_>, arm: &ArmBones, place: &Affine3A, grip: &Grip) {
    if grip.weight <= 0.0 {
        return;
    }
    let hand = *place * grip.on_weapon.to_affine3a() * grip.in_hand.to_affine3a().inverse();
    let rotation = pose.rotation(arm.chain.end);
    let frame = pose.rotation(usize::from(arm.frame)) * arm.rest_frame.inverse();
    two_bone(
        pose,
        arm.chain,
        &TwoBoneGoal {
            target: Vec3::from(hand.translation),
            pole: grip.pole,
            hint: frame * Vec3::NEG_Z,
            soften: SOFTEN,
            weight: grip.weight,
        },
    );
    pose.refresh(&arm.path);
    let weight = grip.weight.min(1.0);
    pose.set_rotation(arm.chain.end, rotation.slerp(rotation_of(&hand), weight).normalize());
}
