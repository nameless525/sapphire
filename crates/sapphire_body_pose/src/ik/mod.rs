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
//! The ground arrives as data ([`GroundHit`]), so the caller can solve in a job or supply test ground.
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
//! # A body lying down
//!
//! Feet and a look are what a standing body does. As the hips fall from half their rest height over
//! the floor to three tenths of it, both fade to nothing, whoever's asking, so a death or a knockdown
//! needs no controls of its own, and a heel on the floor isn't planted as a sole. A clip with its hips
//! above that is left exactly as it was.
//!
//! # What it costs
//!
//! Each stage visits only the joints it names, so its cost doesn't grow with a skeleton's hair and
//! skirts: see `tests/cost.rs` and spec §5.13's measurements.

mod authored_support;
mod feet;
mod footwork;
mod gather;
mod hands;
mod look;
mod rig;
mod solve;

pub use authored_support::AuthoredFootMotion;
pub use feet::{
    FeetInput, FootContact, FootGround, FootPlacement, FootProbe, FootSettings, GroundHit, LegBones,
};
pub use hands::{ArmBones, Grip, Hand, HeldWeapon, WeaponPlace};
pub use look::{EyeDrive, EyeRig, LookAt, LookBones, LookSettings, RangeMap};
pub use rig::IkRig;
pub use solve::{AimGoal, SolvePose, TwoBone, TwoBoneGoal, aim, two_bone};

use std::sync::Arc;

use sapphire_math::ease::smoothstep;
use sapphire_math::{Affine3A, Vec3};

use crate::character::{CharacterPose, JointLayout};
use crate::pose_math::PoseMath;

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
    /// Seconds of the owner's clock since the last solve. See the `ik` module's docs.
    pub dt: f32,
    /// The character's transform: upright, with a uniform scale.
    pub world_from_model: Affine3A,
    pub feet: FeetInput,
    pub look: LookInput,
    pub weapon: Option<HeldWeapon>,
}

impl IkInput {
    /// What must be compared to know whether a pose under a frozen clock has to be solved again: what
    /// takes effect with no time passing. See the `ik` module's docs.
    #[must_use]
    pub fn frozen_key(&self) -> Option<HeldWeapon> {
        self.weapon
    }
}

/// The share of their rest height over the floor at which the hips stop being a standing body's.
const STANDING: f32 = 0.5;
/// The share at which they're a body lying down.
const LYING: f32 = 0.3;

/// One character's IK. See the `ik` module's docs.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterIk {
    math: PoseMath,
    layout: Arc<JointLayout>,
    /// The hips, the floor under them and how high they stand on it at rest.
    stance: Option<(u16, f32, f32)>,
    reads: Vec<u16>,
    feet: Option<FootPlacement>,
    look: Option<LookAt>,
    arms: Option<[ArmBones; 2]>,
}

impl CharacterIk {
    pub(crate) fn reset_body_history(&mut self) {
        if let Some(feet) = &mut self.feet {
            feet.reset_body_history();
        }
        if let Some(look) = &mut self.look {
            look.reset_body_history();
        }
    }

    pub(crate) fn write_canonical(
        &self,
        out: &mut crate::graph::canonical::Canonical<'_, impl FnMut(&[u8])>,
    ) {
        out.optional(self.feet.as_ref(), |out, feet| feet.write_canonical(out));
        out.optional(self.look.as_ref(), |out, look| look.write_canonical(out));
    }
}
impl CharacterIk {
    /// IK for a character with `rig`'s bones. What it hasn't the bones for it just doesn't do.
    #[must_use]
    pub fn new(rig: IkRig, feet: FootSettings, look: LookSettings) -> Self {
        Self {
            math: rig.math,
            layout: rig.layout,
            stance: rig.hips.filter(|_| rig.standing > 0.0).map(|hips| (hips, rig.floor, rig.standing)),
            feet: rig.legs.zip(rig.hips).map(|(legs, hips)| FootPlacement::new(legs, hips, rig.floor, feet)),
            look: rig.look.map(|bones| LookAt::new(bones, look)),
            arms: rig.arms,
            reads: rig.reads,
        }
    }

    pub(crate) fn matches_body(&self, pose: &CharacterPose) -> bool {
        self.math == pose.math() && Arc::ptr_eq(&self.layout, &pose.joint_layout())
    }

    /// Its foot placement, for a rig with feet.
    #[must_use]
    pub fn feet(&self) -> Option<&FootPlacement> {
        self.feet.as_ref()
    }

    /// Identifies the released feet of a prepared jump while terrain correction fades.
    pub fn set_prepared_release(&mut self, active: bool) {
        if let Some(feet) = &mut self.feet {
            feet.set_prepared_release(active);
        }
    }

    /// Loads or rolls through the active support of a traveling jump.
    pub fn set_moving_takeoff(&mut self, phase: Option<f32>) {
        if let Some(feet) = &mut self.feet {
            feet.set_moving_takeoff(phase);
        }
    }

    /// Chooses how the feet follow the ground for the motion being sampled.
    pub fn set_foot_contact(&mut self, contact: FootContact) {
        if let Some(feet) = &mut self.feet {
            feet.set_contact(contact);
        }
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

    /// Prepares, probes supplied ground and solves in the legacy body order. The caller owns time and
    /// chooses which physics scene or deterministic replay probe supplies ground.
    pub fn solve_on_ground(
        &mut self,
        pose: &mut CharacterPose,
        mut input: IkInput,
        contact: FootContact,
        head_weight: f32,
        ground_under: impl Fn(Vec3, &FootSettings) -> Option<GroundHit>,
    ) {
        self.set_foot_contact(contact);
        self.prepare(pose);
        if let (Some(placement), Some(probes)) =
            (self.feet(), self.foot_probes(pose, &input.world_from_model))
            && (input.feet.weight > 0.0 || placement.fade() > 0.0)
        {
            let settings = placement.settings();
            let cast = |at| ground_under(at, settings);
            for (ground, probe) in input.feet.ground.iter_mut().zip(probes) {
                *ground = FootGround { ankle: cast(probe.ankle), ball: probe.ball.and_then(cast) };
            }
        }
        self.solve_with_head_weight(pose, &input, head_weight);
    }

    /// Solves everything, feet first since the hips carry the rest, then the look, then the hands,
    /// which follow whatever the look did to the shoulders. `pose` must be prepared, and
    /// [`CharacterPose::finish`] comes after. Allocates nothing.
    pub fn solve(&mut self, pose: &mut CharacterPose, input: &IkInput) {
        self.solve_with_head_weight(pose, input, 1.0);
    }

    /// Solves IK with an extra 0-to-1 share of the procedural neck and head turn. The gaze state
    /// and eye tracking keep their normal weight and follow the resulting head orientation.
    pub fn solve_with_head_weight(&mut self, pose: &mut CharacterPose, input: &IkInput, head_weight: f32) {
        let (mut solving, morphs) = pose.solving_with_weights();
        let upright = self.stance.map_or(1.0, |(hips, floor, standing)| {
            let height = (solving.position(usize::from(hips)).y - floor) / standing;
            smoothstep(LYING, STANDING, height)
        });
        if let Some(feet) = &mut self.feet {
            let wanted = FeetInput { weight: input.feet.weight * upright, ..input.feet };
            feet.solve(&mut solving, &self.reads, &wanted, &input.world_from_model, input.dt);
        }
        if let Some(look) = &mut self.look {
            let target =
                input.look.target.map(|target| input.world_from_model.inverse().transform_point3(target));
            look.solve_with_head_weight(
                &mut solving,
                morphs,
                target,
                input.look.weight * upright,
                input.dt,
                head_weight,
            );
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
#[allow(clippy::disallowed_methods, reason = "reference fixtures also exercise the legacy math policy")]
mod tests;
