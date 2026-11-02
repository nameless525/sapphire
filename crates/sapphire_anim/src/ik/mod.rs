//! Inverse kinematics on a pose (M2 N4, spec §5.13): plain scalar solvers that run between sampling
//! and [`CharacterPose::finish`], so the rig's twist helpers follow them, allocating nothing once a
//! character's are made.
//!
//! - **[`two_bone`] and [`aim`]** are the plain functions, over a [`SolvePose`] made from slices, so
//!   a tool can call them with no character in sight.
//! - **[`LookAt`]** (head and eyes), **[`FootPlacement`]** and **[`HeldWeapon`]** hands are what a
//!   character does with them. Their module docs say how each works.
//! - **[`CharacterIk`]** is one character's lot: built once from the rig's humanoid bones
//!   ([`IkRig`]), then each frame:
//!
//!   ```ignore
//!   pose.sample(clip, position);        // or a graph's blend
//!   ik.prepare(&mut pose);              // the joints IK reads, only those
//!   let probes = ik.foot_probes(&mut pose, &world_from_model);
//!   // ...cast through `sapphire_physics` at `Group::World`, from each probe...
//!   ik.solve(&mut pose, &input);
//!   pose.finish();                      // constraints and the whole model pose
//!   ```
//!
//! # No physics edge
//!
//! The ground arrives as data ([`GroundHit`]), so this crate stays callable from the cooker and from
//! [`evaluate_all`](crate::evaluate_all)'s jobs, and a test can hand it any ground it likes.
//! `sapphire_physics::PhysicsWorld` is `Sync` and its queries take `&self`, so a caller can cast inside
//! those jobs, after sampling, which is the only place a foot's place is known.
//!
//! # Time
//!
//! [`IkInput::dt`] is seconds of the owner's clock, so a frozen character's rate limits and fades move
//! nothing. A caller skipping evaluation under a frozen clock should keep skipping it however a look
//! target moves, since nothing would change, but should evaluate when a [`Grip`] moves: a grip isn't
//! smoothed, so it takes effect at once ([`IkInput::frozen_key`]).
//!
//! # What it costs
//!
//! Each stage visits only the joints it names, so its cost doesn't grow with a skeleton's hair and
//! skirts: see `tests/cost.rs` and spec §5.13's measurements.

mod feet;
mod hands;
mod look;
mod rig;
mod solve;

pub use feet::{FeetInput, FootGround, FootPlacement, FootProbe, FootSettings, GroundHit, LegBones};
pub use hands::{ArmBones, Grip, Hand, HeldWeapon, WeaponPlace};
pub use look::{EyeDrive, EyeRig, LookAt, LookBones, LookSettings, RangeMap};
pub use rig::IkRig;
pub use solve::{AimGoal, SolvePose, TwoBone, TwoBoneGoal, aim, two_bone};

use sapphire_math::{Affine3A, Vec3};

use crate::character::CharacterPose;

/// What a look-at takes each frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookInput {
    /// What to look at, in the world. `None` fades out.
    pub target: Option<Vec3>,
    /// 0 to 1: how much of the look to show, reached over [`LookSettings::fade`].
    pub weight: f32,
}

/// What IK takes each frame, all plain data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IkInput {
    /// Seconds of the owner's clock since the last solve. See the [module docs](self).
    pub dt: f32,
    /// The character's transform: upright, with a uniform scale.
    pub world_from_model: Affine3A,
    pub feet: FeetInput,
    pub look: LookInput,
    pub weapon: Option<HeldWeapon>,
}

impl IkInput {
    /// What must be compared to know whether a pose under a frozen clock has to be solved again: what
    /// takes effect with no time passing. See the [module docs](self).
    #[must_use]
    pub fn frozen_key(&self) -> Option<HeldWeapon> {
        self.weapon
    }
}

/// One character's IK. See the [module docs](self).
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterIk {
    reads: Vec<u16>,
    feet: Option<FootPlacement>,
    look: Option<LookAt>,
    arms: Option<[ArmBones; 2]>,
}

impl CharacterIk {
    /// IK for a character with `rig`'s bones. What it hasn't the bones for it just doesn't do.
    #[must_use]
    pub fn new(rig: IkRig, feet: FootSettings, look: LookSettings) -> Self {
        Self {
            feet: rig.legs.zip(rig.hips).map(|(legs, hips)| FootPlacement::new(legs, hips, rig.floor, feet)),
            look: rig.look.map(|bones| LookAt::new(bones, look)),
            arms: rig.arms,
            reads: rig.reads,
        }
    }

    /// Its foot placement, for a rig with feet.
    #[must_use]
    pub fn feet(&self) -> Option<&FootPlacement> {
        self.feet.as_ref()
    }

    /// Its look-at, for a rig with a head.
    #[must_use]
    pub fn look(&self) -> Option<&LookAt> {
        self.look.as_ref()
    }

    /// Whether another solve would move nothing, for the same input.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.feet.as_ref().is_none_or(FootPlacement::is_settled)
            && self.look.as_ref().is_none_or(LookAt::is_settled)
    }

    /// Forgets the ground and the gaze, for a cut or a teleport.
    pub fn reset(&mut self) {
        if let Some(feet) = &mut self.feet {
            feet.reset();
        }
        if let Some(look) = &mut self.look {
            look.reset();
        }
    }

    /// Brings up to date the joints it reads, from a pose just sampled. Allocates nothing.
    pub fn prepare(&self, pose: &mut CharacterPose) {
        pose.solving().refresh(&self.reads);
    }

    /// Where the caller should look for ground under each foot, left then right. `pose` must be
    /// prepared.
    pub fn foot_probes(
        &self,
        pose: &mut CharacterPose,
        world_from_model: &Affine3A,
    ) -> Option<[FootProbe; 2]> {
        let feet = self.feet.as_ref()?;
        Some(feet.probes(&pose.solving(), world_from_model))
    }

    /// Solves everything, feet first since the hips carry the rest, then the look, then the hands,
    /// which follow whatever the look did to the shoulders. `pose` must be prepared, and
    /// [`CharacterPose::finish`] comes after. Allocates nothing.
    pub fn solve(&mut self, pose: &mut CharacterPose, input: &IkInput) {
        let (mut solving, morphs) = pose.solving_with_weights();
        if let Some(feet) = &mut self.feet {
            feet.solve(&mut solving, &self.reads, &input.feet, &input.world_from_model, input.dt);
        }
        if let Some(look) = &mut self.look {
            let target =
                input.look.target.map(|target| input.world_from_model.inverse().transform_point3(target));
            look.solve(&mut solving, morphs, target, input.look.weight, input.dt);
            if look.fade() > 0.0 && input.weapon.is_some() {
                solving.refresh(&self.reads);
            }
        }
        if let (Some(arms), Some(weapon)) = (&self.arms, &input.weapon) {
            hands::hold(&mut solving, arms, weapon);
        }
    }
}

#[cfg(test)]
mod tests;
