//! Foot placement: both feet on the ground under them, with the hips dropped so the lower one reaches.
//!
//! A clip's feet stand on a flat floor at the model's feet. The ground under each one is somewhere
//! else: a slope falls away, a step rises, and the character controller's round foot hovers over any
//! slope by `radius × (1/cos θ − 1)`, 4.6 cm at 30° (M2 G1). So:
//!
//! - **The ground comes in as data.** The caller casts where [`FootProbe`]s say and hands over what
//!   it hit, in the world, so this crate has no physics edge and runs in `evaluate_all`'s jobs.
//!   Under each foot it chooses between what's under the ankle and what's under the ball of the foot
//!   ([`FootGround`]): whichever plane the other's point isn't above, so a foot facing a step stands
//!   on it, and one overhanging a step's edge stays on top.
//! - **That plane is smoothed, in the world.** A plane holds still however a foot crosses it, so a
//!   swinging foot over a constant slope doesn't lag it, and a planted foot stays put when the root
//!   pops up a step. Only a change of ground eases in.
//! - **Each ankle goes where the clip had it over a flat floor, moved with the ground.** What the clip
//!   lifted a foot by, it keeps. A planted sole tilts to the ground about the ankle, which then has to
//!   stand `ankle / cos θ` above the plane, not `ankle`; a foot lifting off lets the tilt go by
//!   [`FootSettings::plant_height`].
//! - **The hips drop by the lower foot's move,** and rise only as far as [`FootSettings::max_raise`]
//!   when both feet want up, and drop further if a leg still can't reach. Their height eases in the
//!   model's space, but a root that jumps ([`FootSettings::pop_speed`]) is taken back out of it, so
//!   a step-up shows on screen as the feet alone. Then two-bone solves
//!   ([`two_bone`]) keep each knee's plane from the clip.
//! - **A weight fades it all,** [`FeetInput::weight`]: a state or move says 0 in the air or while it
//!   wants its feet as animated, and it eases off over [`FootSettings::fade`]. With no ground under a
//!   foot, that foot alone eases back to its animation.
//! - **Arena contacts** ([`FootContact`]) can recover a sinking sole and stabilize small amounts
//!   of locomotion slip. For combat warps, track the clip's heel and ball in its unwarped root
//!   space and hold genuine contacts in the world until the step or the leg's reach releases them.
//!
//! The model's +Y is taken to be the world's up, with the character upright and scaled uniformly.

use sapphire_math::{Affine3A, Quat, Vec3};

use crate::ik::authored_support::{
    AuthoredFootMotion, AuthoredSupport, SoleMotion, SolePoints, reachable_share,
};
use crate::ik::footwork::{DisplayedShoe, Footwork, ShoeTurns};
use crate::ik::gather::{DepartureSpace, Gather, SoleHistory, TURN_CATCH_LIFT, TURN_RELEASE_CLEARANCE};
use crate::ik::look::approach;
use crate::ik::solve::{SolvePose, TwoBone, TwoBoneGoal, rotation_of, two_bone, weighted};
use crate::pose_math::PoseMath;

mod takeoff;

use takeoff::{TakeoffCarry, TakeoffSupport};

/// How much an animated foot should follow its support.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum FootContact {
    /// Adapt to terrain while keeping the signed lift of the animation.
    #[default]
    Terrain,
    /// Correct moderate sole penetration while letting the feet travel as animated.
    Sole,
    /// Also stabilize a stance foot over small horizontal discrepancies.
    Stance,
    /// Stabilize the loaded heel or ball while a shoe rolls through a traveling step.
    RollingStance,
    /// Hold source-space plants in the world while a combat move's root follows gameplay.
    Authored(AuthoredFootMotion),
    /// Hold source-space landing plants, then carry the grounded soles into the next gait.
    Landing(AuthoredFootMotion),
    /// An authored catch whose lifting shoe carries its last solved offset into swing.
    LandingCatch { motion: AuthoredFootMotion, release: f32 },
    /// Transfer the landing's weight to one shoe while the other begins the first gait step.
    LandingStep {
        /// Supporting side, left (0) or right (1).
        side: usize,
    },
    /// Keep the loaded sole in place until the other catches, then recover an overextended stance.
    Gather {
        /// Progress of the graph's settling pose, from 0 to 1.
        phase: f32,
    },
    /// Keep the previous support through a turning step and then transfer weight to the new gait.
    TurnStep {
        /// Progress of the step (0–1), followed by its support release.
        phase: f32,
    },
    /// The last running contact, receiving heel and closing step of a grounded stop.
    Brake {
        /// Owner ticks since the release.
        elapsed: f32,
        /// The shoe holding the first braking load.
        support: usize,
        /// Predicted remaining root travel from the first braking sample, in metres.
        remaining: f32,
        /// Speed before release, in metres per second.
        speed: f32,
    },
    /// A driven first step that holds its push and receiving contacts into the gait.
    Launch {
        /// Owner ticks since the movement input resumed.
        elapsed: f32,
        /// The shoe driving the launch while the other foot steps through.
        support: usize,
        /// Current controller speed in metres per second.
        speed: f32,
        /// Phase of the synced gait measured from the receiving heel strike.
        gait: f32,
        /// Phase of toe-off in the current gait mix.
        toe_off: f32,
    },
}

type History<'a, F> = crate::graph::canonical::Canonical<'a, F>;

impl FootContact {
    pub(crate) fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        let ordinal = match self {
            Self::Terrain => 0,
            Self::Sole => 1,
            Self::Stance => 2,
            Self::RollingStance => 3,
            Self::Authored(_) => 4,
            Self::Landing(_) => 5,
            Self::LandingCatch { .. } => 6,
            Self::LandingStep { .. } => 7,
            Self::Gather { .. } => 8,
            Self::TurnStep { .. } => 9,
            Self::Brake { .. } => 10,
            Self::Launch { .. } => 11,
        };
        out.u16(ordinal);
        match self {
            Self::Authored(motion) | Self::Landing(motion) => motion.write_canonical(out),
            Self::LandingCatch { motion, release } => {
                motion.write_canonical(out);
                out.f32(release);
            }
            Self::LandingStep { side } => out.len(side),
            Self::Gather { phase } | Self::TurnStep { phase } => out.f32(phase),
            Self::Brake { elapsed, support, remaining, speed } => {
                out.len(support);
                out.floats(&[elapsed, remaining, speed]);
            }
            Self::Launch { elapsed, support, speed, gait, toe_off } => {
                out.len(support);
                out.floats(&[elapsed, speed, gait, toe_off]);
            }
            Self::Terrain | Self::Sole | Self::Stance | Self::RollingStance => {}
        }
    }
}

impl Plane {
    fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        out.vec3(self.normal);
        out.f32(self.offset);
    }
}

impl FootState {
    fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        self.plane.write_canonical(out);
        out.f32(self.presence);
        out.optional(self.goal, |out, plane| plane.write_canonical(out));
        out.optional(self.anchor, |out, anchor| {
            out.vec3(anchor.point);
            out.len(anchor.pivot);
            out.flag(anchor.rolling);
        });
        out.flag(self.can_plant);
        out.vec3(self.pin);
        self.authored.write_canonical(out);
        out.optional(self.last_sole, |out, sole| sole.write_canonical(out));
        out.optional(self.last_turns, |out, turns| turns.write_canonical(out));
    }
}

impl FootPlacement {
    #[cfg(test)]
    pub(crate) fn prepared_release(&self) -> bool {
        self.prepared_release
    }

    #[cfg(test)]
    pub(crate) fn has_landing_plant(&self) -> bool {
        self.feet.iter().any(|foot| foot.authored.holds_landing_plant())
    }

    pub(super) fn reset_body_history(&mut self) {
        self.reset();
        self.drop = 0.0;
        self.drop_goal = 0.0;
        self.root_height = 0.0;
        self.fade = 0.0;
        self.fade_goal = 0.0;
        self.contact = FootContact::Terrain;
        self.prepared_release = false;
        for foot in &mut self.feet {
            foot.plane = FLAT;
            foot.presence = 0.0;
            foot.goal = None;
        }
    }

    pub(super) fn write_canonical(&self, out: &mut History<'_, impl FnMut(&[u8])>) {
        for foot in self.feet {
            foot.write_canonical(out);
        }
        out.floats(&[
            self.drop,
            self.drop_goal,
            self.root_height,
            self.fade,
            self.fade_goal,
            self.sole_fade,
            self.pin_fade,
        ]);
        self.contact.write_canonical(out);
        out.optional(self.gather, |out, gather| gather.write_canonical(out));
        out.optional(self.footwork, |out, step| step.write_canonical(out));
        out.flag(self.started);
        out.flag(self.prepared_release);
        out.optional(self.takeoff_phase, History::f32);
        out.optional(self.takeoff_support, |out, support| support.write_canonical(out));
        out.optional(self.takeoff_carry, |out, carry| carry.write_canonical(out));
    }
}
/// How foot placement behaves. Lengths in metres, times in seconds.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FootSettings {
    /// How far above and below an ankle the caller should look for ground.
    pub reach_up: f32,
    pub reach_down: f32,
    /// Ground steeper than this isn't ground: its normal's Y is at least this.
    pub min_normal_y: f32,
    /// How far a sole tilts to the ground, radians.
    pub max_tilt: f32,
    /// How far above its place on the floor a clip's ankle has lifted when the sole stops following
    /// the ground's tilt.
    pub plant_height: f32,
    pub max_drop: f32,
    pub max_raise: f32,
    /// How quickly a foot takes up a change of ground, and the hips a change of height.
    pub ground_half_life: f32,
    pub hips_half_life: f32,
    /// How long a weight takes to go from 0 to 1.
    pub fade: f32,
    /// A root rising or falling faster than this, metres a second, has jumped rather than moved, as
    /// the controller's step-up does, and the hips stay where they were on screen. Over what a run up
    /// the steepest slope can make.
    pub pop_speed: f32,
    /// See [`TwoBoneGoal::soften`].
    pub soften: f32,
}

impl Default for FootSettings {
    fn default() -> Self {
        Self {
            reach_up: 0.5,
            reach_down: 0.4,
            min_normal_y: 0.5,
            max_tilt: 40f32.to_radians(),
            plant_height: 0.12,
            max_drop: 0.3,
            max_raise: 0.08,
            ground_half_life: 0.04,
            hips_half_life: 0.06,
            fade: 0.12,
            pop_speed: 5.5,
            soften: 0.985,
        }
    }
}

/// A leg's joints.
#[derive(Clone, Debug, PartialEq)]
pub struct LegBones {
    pub chain: TwoBone,
    pub toes: Option<u16>,
    /// The upper leg's place down to the foot, every joint between, parents first.
    pub path: Vec<u16>,
    /// The foot's rest rotation in the model's space, which its forward turns from.
    pub rest_foot: Quat,
    /// The knee's forward direction in the thigh's local frame.
    pub knee_forward: Vec3,
    /// How far the ankle stands above the floor at rest.
    pub ankle_height: f32,
    /// How far the ball of the foot stands above the floor at rest, for a rig with toes.
    pub ball_height: Option<f32>,
    /// The rest rotation of the ball joint, if there is one.
    pub rest_toes: Option<Quat>,
    /// Estimated heel position behind the ankle, based on the rig's foot length.
    pub heel_back: f32,
}

impl LegBones {
    fn shoe_turns(&self, pose: &SolvePose<'_>, world_turn: Quat) -> ShoeTurns {
        let math = pose.math();
        let world = |joint, rest: Quat| {
            math.multiply(world_turn, math.multiply(pose.rotation(joint), rest.inverse()))
        };
        let shoe = world(self.chain.end, self.rest_foot);
        let toes =
            self.toes.zip(self.rest_toes).map_or(shoe, |(joint, rest)| world(usize::from(joint), rest));
        ShoeTurns { shoe, toes }
    }

    fn set_shoe_turns(&self, pose: &mut SolvePose<'_>, turns: ShoeTurns, model_turn: Quat) {
        let math = pose.math();
        let model = |world, rest| math.multiply(math.multiply(model_turn, world), rest);
        pose.set_rotation(self.chain.end, model(turns.shoe, self.rest_foot));
        if let Some((toes, rest)) = self.toes.zip(self.rest_toes) {
            pose.refresh(&[toes]);
            pose.set_rotation(usize::from(toes), model(turns.toes, rest));
        }
    }

    fn sole(&self, pose: &SolvePose<'_>, rotate_ball: bool) -> [Vec3; 2] {
        let math = pose.math();
        let ankle = pose.position(self.chain.end);
        let turn = math.multiply(pose.rotation(self.chain.end), self.rest_foot.inverse());
        let heel = ankle + math.rotate(turn, Vec3::new(0.0, -self.ankle_height, -self.heel_back));
        let ball =
            self.toes.zip(self.ball_height).zip(self.rest_toes).map_or(heel, |((toes, height), rest)| {
                let toes = usize::from(toes);
                let rotation = if rotate_ball {
                    math.multiply(pose.rotation(toes), rest.inverse())
                } else {
                    Quat::IDENTITY
                };
                pose.position(toes) - math.rotate(rotation, Vec3::Y) * height
            });
        [heel, ball]
    }
}

/// Where to look for ground under a foot, in the world: from each of these, look from
/// [`FootSettings::reach_up`] above to [`FootSettings::reach_down`] below.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FootProbe {
    pub ankle: Vec3,
    /// The ball of the foot, for a rig with toes.
    pub ball: Option<Vec3>,
}

/// A place on the ground, in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundHit {
    pub point: Vec3,
    /// Up out of the ground, unit length.
    pub normal: Vec3,
}

/// What a caller found under one foot's [`FootProbe`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FootGround {
    pub ankle: Option<GroundHit>,
    pub ball: Option<GroundHit>,
}

/// What foot placement takes each frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FeetInput {
    /// 0 lets the feet go as animated, 1 grounds them. See the `feet` module's docs.
    pub weight: f32,
    /// Left, then right.
    pub ground: [FootGround; 2],
}

/// A plane of the world: where `normal · p = offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Plane {
    normal: Vec3,
    offset: f32,
}

impl Plane {
    fn of(hit: GroundHit) -> Self {
        Self { normal: hit.normal, offset: hit.normal.dot(hit.point) }
    }

    /// The plane's height over `at`'s place on the ground.
    fn height_at(self, at: Vec3) -> f32 {
        (self.offset - self.normal.x * at.x - self.normal.z * at.z) / self.normal.y
    }
}

/// The plane a foot stands on: see the [module docs](self).
fn supporting(ground: &FootGround, probe: &FootProbe, min_normal_y: f32) -> Option<Plane> {
    let usable = |hit: Option<GroundHit>| hit.filter(|hit| hit.normal.y >= min_normal_y).map(Plane::of);
    match (usable(ground.ankle), usable(ground.ball), probe.ball) {
        (Some(ankle), Some(ball), Some(place)) => {
            // A hair of slack, so one plane seen twice is never told apart by rounding.
            if ankle.height_at(place) >= ball.height_at(place) - 1e-3 { Some(ankle) } else { Some(ball) }
        }
        (Some(ankle), _, _) => Some(ankle),
        (None, ball, _) => ball,
    }
}

/// One foot's state.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FootState {
    plane: Plane,
    /// How much of the ground this foot follows: it eases out where nothing is under it.
    presence: f32,
    /// What it's closing on, while there is ground.
    goal: Option<Plane>,
    anchor: Option<PinAnchor>,
    can_plant: bool,
    pin: Vec3,
    authored: AuthoredSupport,
    last_sole: Option<SoleHistory>,
    last_turns: Option<ShoeTurns>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PinAnchor {
    point: Vec3,
    pivot: usize,
    rolling: bool,
}

impl FootState {
    fn displayed(self) -> Option<DisplayedShoe> {
        Some(DisplayedShoe { sole: self.last_sole?, turns: self.last_turns? })
    }

    fn update_pin(&mut self, requested: bool, fade: f32, points: [Vec3; 2], lifts: [f32; 2], rolling: bool) {
        const FREE_SLIP: f32 = 0.025;
        const RELEASE_SLIP: f32 = 0.10;

        let (plant_height, release_height) = if rolling { (0.016, 0.046) } else { (0.025, 0.065) };
        let low = usize::from(lifts[1] < lifts[0]);
        if self.anchor.is_some_and(|anchor| anchor.rolling != rolling) {
            self.anchor = None;
            self.can_plant = true;
        }
        if lifts[low] >= release_height || (!requested && fade == 0.0) || self.presence == 0.0 {
            self.anchor = None;
            self.can_plant = true;
        } else if requested && self.can_plant && self.goal.is_some() && lifts[low] <= plant_height {
            self.anchor = Some(PinAnchor { point: points[low], pivot: low, rolling });
            self.can_plant = false;
        } else if rolling
            && let Some(anchor) = &mut self.anchor
            && anchor.pivot != low
            && lifts[anchor.pivot] > plant_height + 0.006
            && lifts[low] <= plant_height
        {
            // Transfer the last displayed translation. Re-solving the old pivot would oppose
            // the heel's authored lift and pull a still-loaded ball across the ground.
            anchor.point = points[low] + self.pin;
            anchor.pivot = low;
        }
        self.pin = Vec3::ZERO;
        if let Some(anchor) = self.anchor {
            let lift = lifts[anchor.pivot];
            let slip = (anchor.point - points[anchor.pivot]).with_y(0.0);
            let distance = slip.length();
            if distance >= RELEASE_SLIP {
                // After a fast step or pivot, wait for toe-off instead of catching again mid-step.
                self.anchor = None;
            } else {
                let lifted = smoothstep((lift - plant_height) / (release_height - plant_height));
                let slipped = smoothstep((distance - FREE_SLIP) / (RELEASE_SLIP - FREE_SLIP));
                self.pin = slip * ((1.0 - lifted) * (1.0 - slipped) * fade);
            }
        }
    }
}

const FLAT: Plane = Plane { normal: Vec3::Y, offset: 0.0 };

/// One character's foot placement. See the `feet` module's docs.
#[derive(Clone, Debug, PartialEq)]
pub struct FootPlacement {
    legs: [LegBones; 2],
    hips: u16,
    /// The floor a clip's feet stand on, in the model's space.
    floor: f32,
    settings: FootSettings,
    feet: [FootState; 2],
    /// What's changed the hips' height, in the model's space.
    drop: f32,
    drop_goal: f32,
    /// The root's height last frame, in the world, to tell a root that jumped.
    root_height: f32,
    fade: f32,
    fade_goal: f32,
    contact: FootContact,
    sole_fade: f32,
    pin_fade: f32,
    gather: Option<Gather>,
    footwork: Option<Footwork>,
    prepared_release: bool,
    takeoff_phase: Option<f32>,
    takeoff_support: Option<TakeoffSupport>,
    takeoff_carry: Option<TakeoffCarry>,
    /// Whether anything has been seen since a [`Self::reset`]: the first ground is taken at once.
    started: bool,
}

impl FootPlacement {
    #[must_use]
    pub fn new(legs: [LegBones; 2], hips: u16, floor: f32, settings: FootSettings) -> Self {
        let foot = FootState {
            plane: FLAT,
            presence: 0.0,
            goal: None,
            anchor: None,
            can_plant: true,
            pin: Vec3::ZERO,
            authored: AuthoredSupport::default(),
            last_sole: None,
            last_turns: None,
        };
        Self {
            legs,
            hips,
            floor,
            settings,
            feet: [foot; 2],
            drop: 0.0,
            drop_goal: 0.0,
            root_height: 0.0,
            fade: 0.0,
            fade_goal: 0.0,
            contact: FootContact::Terrain,
            sole_fade: 0.0,
            pin_fade: 0.0,
            gather: None,
            footwork: None,
            prepared_release: false,
            takeoff_phase: None,
            takeoff_support: None,
            takeoff_carry: None,
            started: false,
        }
    }

    pub(super) fn set_prepared_release(&mut self, active: bool) {
        self.prepared_release = active;
    }

    pub(super) fn set_moving_takeoff(&mut self, phase: Option<f32>) {
        self.takeoff_phase = phase.filter(|phase| phase.is_finite()).map(|phase| phase.clamp(0.0, 1.0));
    }

    /// Runs by `settings` from now on, wherever the feet have got to.
    pub fn set_settings(&mut self, settings: FootSettings) {
        self.settings = settings;
    }

    /// The settings it runs by, for whoever casts for it.
    #[must_use]
    pub fn settings(&self) -> &FootSettings {
        &self.settings
    }

    /// The share of its placement the character is showing, 0 to 1.
    #[must_use]
    pub fn fade(&self) -> f32 {
        self.fade
    }

    /// Chooses the support policy for the motion being sampled.
    pub fn set_contact(&mut self, contact: FootContact) {
        self.contact = contact;
    }

    /// Whether another step would move nothing, for the same ground.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.fade == self.fade_goal
            && self.sole_fade == self.sole_goal()
            && self.pin_fade == self.pin_goal()
            && self
                .feet
                .iter()
                .enumerate()
                .all(|(side, foot)| foot.authored.settled_in(self.sole_motion(side)))
            && (matches!(self.contact, FootContact::Gather { .. }) || self.gather.is_none())
            && self.footwork.is_none_or(Footwork::settled)
            && if self.fade == 0.0 {
                self.drop == 0.0
            } else {
                self.drop == self.drop_goal
                    && self.feet.iter().all(|foot| match foot.goal {
                        Some(goal) => foot.presence == 1.0 && foot.plane == goal,
                        None => foot.presence == 0.0,
                    })
            }
    }

    /// Forgets the ground, for a cut or a teleport: what's under the feet next is taken at once.
    pub fn reset(&mut self) {
        self.started = false;
        self.clear_contacts();
    }

    /// The heel or forefoot carrying the current turn, as chosen from the solved sole history.
    pub fn turn_support(&self) -> Option<usize> {
        self.gather.as_ref()?.turn_support()
    }

    /// Picks the load-bearing side from the previously solved heel and ball geometry.
    pub fn ground_support(&self) -> Option<usize> {
        Footwork::support_side(self.feet.map(FootState::displayed))
    }

    fn clear_contacts(&mut self) {
        self.takeoff_phase = None;
        self.takeoff_support = None;
        self.takeoff_carry = None;
        self.sole_fade = 0.0;
        self.pin_fade = 0.0;
        self.gather = None;
        self.footwork = None;
        for foot in &mut self.feet {
            foot.anchor = None;
            foot.can_plant = true;
            foot.pin = Vec3::ZERO;
            foot.authored = AuthoredSupport::default();
            foot.last_sole = None;
            foot.last_turns = None;
        }
    }

    fn sole_goal(&self) -> f32 {
        f32::from(self.contact != FootContact::Terrain && self.fade_goal > 0.0)
    }

    fn pin_goal(&self) -> f32 {
        f32::from(
            matches!(
                self.contact,
                FootContact::Stance
                    | FootContact::RollingStance
                    | FootContact::LandingStep { .. }
                    | FootContact::Launch { .. }
            ) && self.fade_goal > 0.0,
        )
    }

    fn sole_motion(&self, side: usize) -> SoleMotion {
        match self.contact {
            FootContact::Authored(motion) => SoleMotion::Authored(motion),
            FootContact::Landing(motion) => SoleMotion::Landing(motion),
            FootContact::LandingCatch { motion, release } => SoleMotion::LandingCatch(motion, release),
            FootContact::Stance | FootContact::RollingStance | FootContact::Launch { .. }
                if self.fade_goal > 0.0 =>
            {
                SoleMotion::Gait
            }
            FootContact::LandingStep { side: support } if self.fade_goal > 0.0 => {
                if side == support {
                    SoleMotion::Gait
                } else {
                    SoleMotion::FreeStep
                }
            }
            _ => SoleMotion::Inactive,
        }
    }

    /// Where a caller should look for ground under each foot. `pose` must be up to date for the feet
    /// and toes.
    #[must_use]
    pub fn probes(&self, pose: &SolvePose<'_>, world_from_model: &Affine3A) -> [FootProbe; 2] {
        let probe = |leg: &LegBones| FootProbe {
            ankle: world_from_model.transform_point3(pose.position(leg.chain.end)),
            ball: leg.toes.map(|toes| world_from_model.transform_point3(pose.position(usize::from(toes)))),
        };
        [probe(&self.legs[0]), probe(&self.legs[1])]
    }

    /// Grounds the feet, by `dt` seconds of the owner's clock. `pose` must be up to date for the hips
    /// and legs; `reads` is every joint the solvers read, parents first, which it refreshes if the hips
    /// move. Allocates nothing.
    pub fn solve(
        &mut self,
        pose: &mut SolvePose<'_>,
        reads: &[u16],
        input: &FeetInput,
        world_from_model: &Affine3A,
        dt: f32,
    ) {
        self.fade_goal = input.weight.clamp(0.0, 1.0);
        // The hips ease more slowly than the fade goes, so they outlast it: off means they're home too.
        if self.fade == 0.0 && self.fade_goal == 0.0 {
            if !self.started {
                self.drop = 0.0;
            }
            if self.drop == 0.0 {
                if !self.started || dt > 0.0 {
                    self.clear_contacts();
                }
                return;
            }
        }
        // A character just started stands on the ground at once. One fading back in has been off, so
        // it takes what's there now rather than easing from wherever it left the ground, while the
        // fade itself still eases.
        let fresh = !self.started;
        let snap = fresh || (self.fade == 0.0 && self.fade_goal > 0.0);
        let advance = fresh || dt != 0.0;
        self.started = true;
        // On a frozen solve, reapply the held offsets to the sampled pose. Only a fresh solve
        // initializes them without time passing.
        if fresh {
            self.fade = self.fade_goal;
        } else if advance {
            self.fade = approach(self.fade, self.fade_goal, dt / self.settings.fade.max(1e-3));
        }
        let probes = self.probes(pose, world_from_model);
        let math = pose.math();
        let scale = math.length_3a(world_from_model.matrix3.y_axis).max(1e-6);
        let world_turn = rotation_of(world_from_model, math);
        let model_turn = world_turn.inverse();
        let ease = 1.0 - math.powf(0.5, dt / self.settings.ground_half_life.max(1e-4));

        if advance {
            if self.fade_goal > 0.0 {
                self.takeoff_carry = None;
            } else if self.takeoff_support.is_some() || self.footwork.is_some() {
                self.takeoff_carry = TakeoffCarry::begin(self.feet.map(FootState::displayed));
            }
            if self.takeoff_carry.as_mut().is_some_and(|carry| !carry.advance(dt)) {
                self.takeoff_carry = None;
            }
        }
        // Where each foot wants its ankle, as a move up or down from where the clip has it, and how much
        // its sole tilts.
        let sole_goal = self.sole_goal();
        let pin_goal = self.pin_goal();
        let gather_phase = match self.contact {
            FootContact::Gather { phase } | FootContact::TurnStep { phase } => Some(phase),
            _ => None,
        };
        let turning = matches!(self.contact, FootContact::TurnStep { .. });
        let grounded_gait =
            matches!(self.contact, FootContact::Stance | FootContact::RollingStance) && self.fade_goal > 0.0;
        if advance {
            self.sole_fade = if fresh { sole_goal } else { approach(self.sole_fade, sole_goal, dt / 0.07) };
            self.pin_fade = if fresh { pin_goal } else { approach(self.pin_fade, pin_goal, dt / 0.07) };
            if let FootContact::Brake { support, .. } | FootContact::Launch { support, .. } = self.contact {
                self.gather = None;
                let shoes = self.feet.map(FootState::displayed);
                if self.footwork.is_none_or(|step| !step.matches(self.contact)) {
                    self.footwork = self.legs.get(support).and_then(|leg| {
                        let chain = leg.chain;
                        let knee = pose.position(chain.mid);
                        let span = knee.distance(pose.position(chain.root))
                            + knee.distance(pose.position(chain.end));
                        Footwork::begin(self.contact, shoes, world_from_model, world_turn, scale, span)
                    });
                }
                if let Some(step) = &mut self.footwork {
                    step.advance(self.contact, shoes, world_from_model);
                }
            } else {
                self.footwork = None;
            }
            if let Some(step) = &mut self.gather {
                step.retain_turn_catch(grounded_gait, dt);
            }
            if let Some(phase) = gather_phase
                && self.gather.is_none_or(|step| step.is_returning() || step.is_turning() != turning)
            {
                let soles = self.feet.map(|foot| foot.last_sole);
                self.gather = if turning {
                    Gather::begin_turn(soles, phase, self.gather)
                } else {
                    Gather::begin(soles, phase)
                };
            } else if let Some(gather) = &mut self.gather
                && !gather.advance(gather_phase, dt)
            {
                self.gather = None;
            }
            if turning && let Some(step) = &mut self.gather {
                step.catch_turn_step(self.feet.map(|foot| foot.last_sole));
            }
            if let Some(step) = &mut self.gather
                && let Some(side) = step.guard_support_to_check()
            {
                let guard =
                    self.legs[side].sole(pose, true).map(|point| world_from_model.transform_point3(point));
                step.begin_guard_step(guard, self.feet.map(|foot| foot.last_sole), scale);
            }
        }
        if advance {
            self.update_takeoff_support(pose, input, &probes, world_from_model);
        }
        let mut moves = [Vec3::ZERO; 2];
        let mut terrain_moves = [0.0f32; 2];
        let mut tilts = [Quat::IDENTITY; 2];
        let displayed_soles = self.feet.map(|foot| foot.last_sole);
        let mut pelvis_shift = self.footwork.map_or(Vec3::ZERO, |step| {
            step.receiving_shift(pose.position(usize::from(self.hips)), world_from_model, displayed_soles)
        });
        for (side, leg) in self.legs.iter().enumerate() {
            let sole_motion = self.sole_motion(side);
            let foot = &mut self.feet[side];
            foot.goal = supporting(&input.ground[side], &probes[side], self.settings.min_normal_y);
            if advance
                && (!turning && !grounded_gait || foot.goal.is_none())
                && let Some(step) = &mut self.gather
            {
                step.stop_departing_shoe(side);
            }
            if advance {
                if let Some(goal) = foot.goal {
                    foot.plane = if snap || foot.presence == 0.0 {
                        goal
                    } else {
                        ease_plane(foot.plane, goal, ease, probes[side].ankle)
                    };
                    foot.presence = if snap { 1.0 } else { ease_to(foot.presence, 1.0, ease) };
                } else {
                    foot.presence = if snap { 0.0 } else { ease_to(foot.presence, 0.0, ease) };
                }
            }
            let use_authored =
                sole_motion.is_clip() || sole_motion == SoleMotion::FreeStep || foot.authored.active();
            if advance
                && foot.goal.is_none()
                && let Some(step) = &mut self.gather
                && step.holds_turn_catch(side)
            {
                step.release_turn_catch();
            }
            let uses_gather = self.gather.is_some_and(|step| step.affects(side));
            let uses_footwork = self.footwork.is_some_and(|step| step.controls(side)) && foot.goal.is_some();
            if (use_authored || gather_phase.is_some() || uses_gather || uses_footwork) && advance {
                foot.anchor = None;
                foot.pin = Vec3::ZERO;
                foot.can_plant = true;
            } else if advance && (self.pin_fade > 0.0 || foot.anchor.is_some() || !foot.can_plant) {
                let rolling = matches!(self.contact, FootContact::RollingStance | FootContact::Launch { .. });
                let (points, lifts) = if rolling {
                    let soles = leg.sole(pose, true).map(|point| world_from_model.transform_point3(point));
                    (soles, soles.map(|point| point.y - foot.plane.height_at(point)))
                } else {
                    let lift = (pose.position(leg.chain.end).y - self.floor - leg.ankle_height) * scale;
                    ([probes[side].ankle; 2], [lift; 2])
                };
                foot.update_pin(pin_goal > 0.0, self.pin_fade, points, lifts, rolling);
            }
            if foot.presence == 0.0 || self.fade == 0.0 {
                if advance {
                    foot.authored = AuthoredSupport::default();
                }
                continue;
            }
            let ankle = pose.position(leg.chain.end);
            let floor_height = world_from_model.translation.y + self.floor * scale;
            let ground = (foot.plane.height_at(probes[side].ankle) - floor_height) / scale + self.floor;
            let lift = ankle.y - self.floor - leg.ankle_height;
            let planted = 1.0 - smoothstep(lift / self.settings.plant_height.max(1e-3));
            let cosine = foot.plane.normal.y.max(math.cos(self.settings.max_tilt)).max(1e-3);
            let wanted = ground + leg.ankle_height + lift + leg.ankle_height * (1.0 / cosine - 1.0) * planted;
            let share = foot.presence * self.fade;
            terrain_moves[side] = (wanted - ankle.y) * share;
            moves[side].y = terrain_moves[side];
            if let Some(step) = self.footwork.as_mut().filter(|_| uses_footwork) {
                let normal = foot.plane;
                let in_world = |point| world_from_model.transform_point3(point);
                let raw = leg.sole(pose, true).map(in_world);
                let raw = DisplayedShoe {
                    sole: SoleHistory::new(raw, raw.map(|point| normal.height_at(point)), None, dt),
                    turns: leg.shoe_turns(pose, world_turn),
                };
                let goal = step.goal(side, raw, foot.displayed(), math, advance);
                leg.set_shoe_turns(pose, goal.turns, model_turn);
                let world = leg.sole(pose, true).map(in_world);
                let planar = (goal.point - world[goal.pivot]).with_y(0.0);
                let below = |point: Vec3| normal.height_at(point + planar) - point.y;
                let height = below(world[0]).max(below(world[1])) + goal.clearance.max(0.0);
                moves[side] = math.rotate(model_turn, planar.with_y(height)) * (share / scale);
                continue;
            }
            if turning
                && let Some(gather) = self.gather
                && let Some((toes, rest)) = leg.toes.zip(leg.rest_toes)
            {
                let planted = gather.forefoot_weight(side);
                if planted > 0.0 {
                    // While the heel rises around a held ball, let the toe stay along the floor.
                    let rotation = pose.rotation(usize::from(toes));
                    let forward = math.rotate(math.multiply(rotation, rest.inverse()), Vec3::Z);
                    let normal = math.rotate(model_turn, foot.plane.normal);
                    let tangent = (forward - normal * forward.dot(normal)).normalize_or(forward);
                    let pitch = weighted(math.rotation_arc(forward, tangent), planted * share, math);
                    pose.set_rotation(usize::from(toes), math.multiply(pitch, rotation));
                    pose.refresh(&[toes]);
                }
            }
            if self.sole_fade > 0.0 || use_authored || uses_gather {
                let model = leg.sole(pose, use_authored || uses_gather);
                let sole_lift = model[0].y.min(model[1].y) - self.floor;
                // Keep the authored lift above the floor. Ease into correction through the first
                // centimetre, and let a deep pose keep most of its shape.
                let sink = (-sole_lift).clamp(0.0, 0.08);
                let sole_correction = sink * smoothstep(sink / 0.01);
                let sole_move = sole_correction * share * self.sole_fade;
                moves[side].y += sole_move;
                if use_authored || uses_gather {
                    let world = model.map(|point| world_from_model.transform_point3(point));
                    let hip = pose.position(leg.chain.root);
                    let knee = pose.position(leg.chain.mid);
                    let length = hip.distance(knee) + knee.distance(ankle);
                    let base = ankle + moves[side] - hip;
                    if let Some(gather) = self.gather.as_mut().filter(|_| uses_gather) {
                        if advance {
                            gather.rebase_turn_interrupt(side, world, foot.last_sole);
                        }
                        if turning && advance {
                            gather.begin_turn_departure(side, world, sole_lift * scale, foot.last_sole);
                        }
                        let ground_shift = moves[side].y * scale;
                        let other = 1 - side;
                        let opposite_ball = displayed_soles[other].map(|sole| {
                            if other < side {
                                let ball = self.legs[other].sole(pose, true)[1] + moves[other];
                                world_from_model.transform_point3(ball)
                            } else {
                                sole.ball()
                            }
                        });
                        let mut offset = gather
                            .departing_correction(
                                side,
                                world,
                                ground_shift,
                                foot.last_sole,
                                DepartureSpace {
                                    outward: math.rotate(
                                        model_turn.inverse(),
                                        if side == 0 { Vec3::X } else { -Vec3::X },
                                    ),
                                    opposite_ball,
                                    scale,
                                    dt,
                                },
                            )
                            .unwrap_or_else(|| {
                                gather.correction(side, world, ground_shift, sole_lift * scale, math)
                            });
                        if turning {
                            // Keep the lower end of a rolling sole above the support plane while
                            // its planted pivot carries the horizontal part of the turn.
                            let below = world.map(|point| foot.plane.height_at(point) - point.y);
                            offset.y = offset.y.max(below[0].max(below[1]) - moves[side].y * scale);
                        }
                        let mut offset = math.rotate(model_turn, offset) / scale;
                        // Permit a little weight acceptance over the supporting foot. The hips
                        // reach check below only uses as much of this room as the leg needs.
                        let drop_room = gather.pelvis_room(side) / scale;
                        let base = base + Vec3::Y * drop_room;
                        let reach = (length * 0.9995).max(base.length());
                        if let Some(shift) = gather.reach_release_start_shift(side) {
                            pelvis_shift = shift;
                        } else if drop_room > 0.0 {
                            let shift = gather.pelvis_shift(side, base + offset, reach, scale);
                            let other = self.legs[1 - side].chain;
                            let (hip, knee, ankle) = (
                                pose.position(other.root),
                                pose.position(other.mid),
                                pose.position(other.end),
                            );
                            let other_base = ankle - hip + Vec3::Y * drop_room;
                            let other_reach = hip.distance(knee) + knee.distance(ankle);
                            pelvis_shift = shift * reachable_share(other_base, -shift, other_reach);
                        }
                        let mut reachable = reachable_share(base - pelvis_shift, offset, reach);
                        if advance && gather.holds_turn_catch(side) {
                            let lift = sole_lift * scale;
                            if lift >= TURN_CATCH_LIFT {
                                gather.release_turn_catch();
                            } else if reachable < 0.995 {
                                // A caught forefoot at full leg reach must leave the floor. Give
                                // the first toe-off its clearance, then decay the achieved offset
                                // as the shoe follows its authored swing.
                                offset.y += (TURN_RELEASE_CLEARANCE - lift.max(0.0)).max(0.0) / scale;
                                reachable = reachable_share(base - pelvis_shift, offset, reach);
                                gather.begin_reach_release(
                                    math.rotate(model_turn.inverse(), offset * reachable * scale),
                                    pelvis_shift,
                                );
                            } else {
                                gather.remember_catch_offset(
                                    math.rotate(model_turn.inverse(), offset * reachable * share * scale),
                                );
                            }
                        }
                        moves[side] += offset * reachable * share;
                    } else {
                        let normal = foot.plane.normal;
                        let along_ground = |world: Vec3| {
                            let world = world.with_y(-normal.dot(world) / normal.y);
                            math.rotate(model_turn, world) / scale
                        };
                        let reachable = |world, lift: f32| {
                            // Leave a little flexion for the support to release before the leg
                            // reaches its hard limit. The lift is authored whenever possible.
                            let base = base + Vec3::Y * (lift / scale);
                            let reach = (length * self.settings.soften).max(base.length());
                            reachable_share(base, along_ground(world), reach)
                        };
                        if advance {
                            foot.authored.update(
                                sole_motion,
                                SolePoints { model, world, solved: foot.last_sole },
                                self.floor,
                                foot.goal.is_some(),
                                dt,
                                |world| reachable(world, 0.0),
                            );
                        }
                        if matches!(self.contact, FootContact::LandingStep { side: support } if support == side)
                            && foot.goal.is_some()
                            && foot.authored.holds_landing_plant()
                        {
                            // The loaded sole already has ground contact through the handoff.
                            moves[side].y += sole_correction * foot.presence - sole_move;
                        }
                        let offset = foot.authored.correction(world, sole_lift * scale, reachable, math);
                        let contact_share = if foot.authored.landing_handoff(sole_motion)
                            || sole_motion == SoleMotion::FreeStep
                        {
                            foot.presence
                        } else {
                            share
                        };
                        let landing_contact =
                            sole_motion.is_landing() && foot.authored.active() && foot.goal.is_some();
                        // A caught landing pivot already meets the floor. Only its horizontal
                        // correction acts at once, while the terrain solve still fades in.
                        let planar = along_ground(offset.with_y(0.0));
                        moves[side] += (planar + Vec3::Y * (offset.y / scale)) * contact_share;
                        if landing_contact {
                            moves[side] += planar * (foot.presence - contact_share);
                        }
                    }
                }
            }
            if foot.pin != Vec3::ZERO {
                // Follow the support plane when shifting the stance joint or sole pivot.
                let normal = foot.plane.normal;
                let pin = foot.pin.with_y(-normal.dot(foot.pin) / normal.y);
                moves[side] += math.rotate(model_turn, pin) * (share / scale);
            }
            tilts[side] = weighted(
                tilt_to(math.rotate(model_turn, foot.plane.normal), self.settings.max_tilt, math),
                planted * share,
                math,
            );
        }

        let mut takeoff_load = 0.0;
        if let Some((support, phase)) = self.takeoff_support.zip(self.takeoff_phase)
            && let Some(plane) = self.feet[support.side].goal
            && !self.footwork.is_some_and(|step| step.controls(support.side))
        {
            let up = math.rotate(model_turn, plane.normal);
            if let Some((offset, load)) = support.push(self, pose, world_from_model, plane, phase, up) {
                moves[support.side] = math.rotate(model_turn, offset) / scale;
                takeoff_load = load;
                // The support rocker already follows the plane through the selected pivot.
                tilts[support.side] = Quat::IDENTITY;
            }
        }

        if let Some(carry) = self.takeoff_carry {
            for (side, leg) in self.legs.iter().enumerate() {
                moves[side] =
                    carry.ankle_offset(side, leg, pose, world_from_model, world_turn, scale, moves[side]);
                tilts[side] = Quat::IDENTITY;
            }
        }

        // The hips go down with the lower foot, easing, and at once as far as a leg can't otherwise
        // reach: a foot short of the ground shows more than a hip that moved a little quicker.
        let lowest = -self.settings.max_drop;
        let mut reachable = f32::MAX;
        for (side, leg) in self.legs.iter().enumerate() {
            let hip = pose.position(leg.chain.root);
            let target = pose.position(leg.chain.end) + moves[side];
            let length = pose.position(leg.chain.mid).distance(hip)
                + pose.position(leg.chain.end).distance(pose.position(leg.chain.mid));
            let hip = hip + pelvis_shift;
            let across = math.hypot(hip.x - target.x, hip.z - target.z);
            let room = (length * length - across * across).max(0.0).sqrt();
            // A hair of slack: a leg the clip has straight, on flat ground, is no reason to move.
            reachable = reachable.min(room + target.y - hip.y + 1e-4);
        }
        // Fixing a sole on flat ground should leave the upper body's authored height alone.
        self.drop_goal = terrain_moves[0]
            .min(terrain_moves[1])
            .clamp(-self.settings.max_drop, self.settings.max_raise)
            .min(reachable)
            .max(lowest);
        // A grounded root step should move the feet without popping the hips. In flight, let
        // the old hip adjustment ease away without counteracting the actor's vertical travel.
        let root_height = world_from_model.translation.y;
        let rise = root_height - self.root_height;
        self.root_height = root_height;
        if advance {
            if self.fade_goal > 0.0 && !snap && dt > 0.0 && rise.abs() > self.settings.pop_speed * dt {
                self.drop =
                    (self.drop - rise / scale).clamp(-self.settings.max_drop, self.settings.max_raise);
            }
            self.drop = if fresh || (snap && self.drop == 0.0) {
                self.drop_goal
            } else {
                let ease = 1.0 - math.powf(0.5, dt / self.settings.hips_half_life.max(1e-4));
                let eased = self.drop + (self.drop_goal - self.drop) * ease;
                let eased = if (self.drop_goal - eased).abs() < 1e-4 { self.drop_goal } else { eased };
                eased.min(reachable).max(lowest)
            };
        }
        let loaded_drop = if takeoff_load == 0.0 {
            self.drop
        } else {
            (self.drop + takeoff_load).min(reachable).max(lowest)
        };
        let hips_move = pelvis_shift.with_y(loaded_drop);
        if hips_move != Vec3::ZERO {
            pose.translate(usize::from(self.hips), hips_move);
            pose.refresh(reads);
        }

        for (side, leg) in self.legs.iter().enumerate() {
            let mut left = moves[side] - hips_move;
            if self.prepared_release && self.fade_goal == 0.0 && self.fade > 0.0 && left.y < 0.0 {
                // The shoes have released. Keep part of the authored flexion as the
                // outgoing terrain correction fades under the rising controller root.
                let hip = pose.position(leg.chain.root);
                let knee = pose.position(leg.chain.mid);
                let ankle = pose.position(leg.chain.end);
                let authored = ankle - hip;
                let span = hip.distance(knee) + knee.distance(ankle);
                let reach = authored.length().midpoint(span);
                left *= reachable_share(authored, left, reach);
            }
            let tilted = tilts[side] != Quat::IDENTITY;
            if left.length_squared() < 1e-10 && !tilted {
                continue;
            }
            let rotation = pose.rotation(leg.chain.end);
            let goal = TwoBoneGoal {
                target: pose.position(leg.chain.end) + left,
                pole: None,
                hint: math.rotate(pose.rotation(leg.chain.root), leg.knee_forward),
                soften: if self.gather.is_some_and(|step| step.affects(side))
                    || self.footwork.is_some_and(|step| step.controls(side))
                {
                    1.0
                } else {
                    self.settings.soften
                },
                weight: 1.0,
            };
            two_bone(pose, leg.chain, &goal);
            pose.refresh(&leg.path);
            pose.set_rotation(leg.chain.end, math.multiply(tilts[side], rotation));
        }
        if advance {
            let track = matches!(
                self.contact,
                FootContact::Stance
                    | FootContact::RollingStance
                    | FootContact::Landing(_)
                    | FootContact::LandingCatch { .. }
                    | FootContact::LandingStep { .. }
                    | FootContact::Gather { .. }
                    | FootContact::TurnStep { .. }
                    | FootContact::Brake { .. }
                    | FootContact::Launch { .. }
            );
            if track {
                pose.refresh(reads);
            }
            for (foot, leg) in self.feet.iter_mut().zip(&self.legs) {
                foot.last_sole = if track && foot.goal.is_some() {
                    let points = leg.sole(pose, true).map(|point| world_from_model.transform_point3(point));
                    let ground = points.map(|point| foot.plane.height_at(point));
                    Some(SoleHistory::new(points, ground, foot.last_sole, dt))
                } else {
                    None
                };
                foot.last_turns = foot.last_sole.map(|_| leg.shoe_turns(pose, world_turn));
            }
        }
    }
}

/// `value` eased toward `goal`, arriving at a hair's breadth so a settled state is exactly settled.
fn ease_to(value: f32, goal: f32, ease: f32) -> f32 {
    let eased = value + (goal - value) * ease;
    if (goal - eased).abs() < 1e-3 { goal } else { eased }
}

fn ease_plane(plane: Plane, goal: Plane, ease: f32, probe: Vec3) -> Plane {
    // The height difference beneath the foot is translation-invariant, even when the normals differ.
    if plane == goal
        || (plane.normal.abs_diff_eq(goal.normal, 1e-4)
            && (plane.height_at(probe) - goal.height_at(probe)).abs() < 1e-4)
    {
        return goal;
    }
    let normal = plane.normal.lerp(goal.normal, ease);
    let length = normal.length();
    if !length.is_finite() || length <= f32::EPSILON {
        return goal;
    }
    // A plane's normal and offset are homogeneous coefficients: normalize both together.
    Plane { normal: normal / length, offset: (plane.offset + (goal.offset - plane.offset) * ease) / length }
}

/// The turn from up to `normal`, no more than `max` radians of it.
fn tilt_to(normal: Vec3, max: f32, math: PoseMath) -> Quat {
    let turn = math.rotation_arc(Vec3::Y, normal);
    let (axis, angle) = math.to_axis_angle(turn);
    if angle < 1e-5 { Quat::IDENTITY } else { math.axis_angle(axis, angle.min(max)) }
}

/// 0 at or under 0, 1 at or over 1, smoothly between.
fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forefoot_remains_planted_as_the_heel_rolls_and_rises() {
        let mut foot = FootState {
            plane: FLAT,
            presence: 1.0,
            goal: Some(FLAT),
            anchor: None,
            can_plant: true,
            pin: Vec3::ZERO,
            authored: AuthoredSupport::default(),
            last_sole: None,
            last_turns: None,
        };
        let heel = Vec3::ZERO;
        let ball = Vec3::Z * 0.16;
        foot.update_pin(true, 1.0, [heel, ball + Vec3::Y * 0.03], [0.0, 0.03], true);
        assert_eq!(foot.anchor.unwrap().pivot, 0);
        foot.update_pin(true, 1.0, [heel + Vec3::Y * 0.025, ball], [0.025, 0.0], true);
        assert_eq!(foot.anchor.unwrap().pivot, 1);
        let drifted = ball + Vec3::Z * 0.009;
        foot.update_pin(true, 1.0, [heel + Vec3::Y * 0.12, drifted], [0.12, 0.0], true);
        assert!((drifted + foot.pin).abs_diff_eq(ball, 1e-6));
        foot.update_pin(true, 1.0, [heel + Vec3::Y * 0.16, ball + Vec3::Y * 0.05], [0.16, 0.05], true);
        assert!(foot.anchor.is_none(), "the rolling contact releases once the toe clears");
        assert!(foot.pin.abs_diff_eq(Vec3::ZERO, 1e-6));
    }

    fn hit(point: Vec3, normal: Vec3) -> Option<GroundHit> {
        Some(GroundHit { point, normal: normal.normalize() })
    }

    #[test]
    fn a_planes_height_is_its_height() {
        let slope = Plane::of(GroundHit {
            point: Vec3::new(1.0, 2.0, 3.0),
            normal: Vec3::new(-0.5, 1.0, 0.0).normalize(),
        });
        assert!((slope.height_at(Vec3::new(1.0, 99.0, -4.0)) - 2.0).abs() < 1e-5);
        assert!((slope.height_at(Vec3::new(2.0, 0.0, 3.0)) - 2.5).abs() < 1e-5);
    }

    #[test]
    fn nearly_equal_ground_planes_snap_at_the_same_height_after_translation() {
        let probe = Vec3::new(0.1, 0.12, 0.12);
        let normal = Vec3::new(0.00005, 1.0, 0.0).normalize();
        for translation in [Vec3::ZERO, Vec3::new(12.0, 100.0, -25.0), Vec3::new(-100.0, -50.0, 80.0)] {
            let plane = Plane::of(GroundHit { point: translation, normal });
            let goal = Plane::of(GroundHit { point: translation, normal: Vec3::Y });
            assert_eq!(ease_plane(plane, goal, 0.5, probe + translation), goal);
        }
        // Equal heights at the probe must not skip a visible change of tilt.
        let plane = Plane::of(GroundHit { point: Vec3::ZERO, normal: Vec3::new(0.1, 1.0, 0.0).normalize() });
        let goal = Plane::of(GroundHit { point: Vec3::ZERO, normal: Vec3::Y });
        assert_ne!(ease_plane(plane, goal, 0.5, Vec3::ZERO), goal);
    }

    #[test]
    fn changing_ground_planes_are_translation_invariant() {
        let normals = [Vec3::Y, Vec3::new(-0.3, 1.0, 0.2).normalize()];
        for translation in [Vec3::ZERO, Vec3::new(12.0, 100.0, -25.0), Vec3::new(-100.0, -50.0, 80.0)] {
            for ease in [0.0, 0.1, 0.5, 1.0] {
                let planes = normals.map(|normal| Plane::of(GroundHit { point: translation, normal }));
                let eased = ease_plane(planes[0], planes[1], ease, translation);
                assert!(
                    (eased.height_at(translation) - translation.y).abs() < 2e-5,
                    "{translation:?} {ease}: {eased:?}"
                );
                assert!((eased.normal.length() - 1.0).abs() < 1e-6);
            }
        }
    }

    /// A foot facing a step stands on it, a foot overhanging an edge stays on top, and a slope seen
    /// from both ends of a foot is one slope.
    #[test]
    fn a_foot_stands_on_the_plane_it_cant_go_through() {
        let probe = FootProbe { ankle: Vec3::ZERO, ball: Some(Vec3::new(0.0, 0.0, 0.2)) };
        let low = hit(Vec3::ZERO, Vec3::Y);
        let high = hit(Vec3::new(0.0, 0.15, 0.2), Vec3::Y);
        let up_a_step = supporting(&FootGround { ankle: low, ball: high }, &probe, 0.5).expect("ground");
        assert!((up_a_step.height_at(probe.ankle) - 0.15).abs() < 1e-5, "{up_a_step:?}");
        let off_a_step = supporting(
            &FootGround {
                ankle: hit(Vec3::new(0.0, 0.15, 0.0), Vec3::Y),
                ball: hit(Vec3::new(0.0, 0.0, 0.2), Vec3::Y),
            },
            &probe,
            0.5,
        )
        .expect("ground");
        assert!((off_a_step.height_at(probe.ankle) - 0.15).abs() < 1e-5, "{off_a_step:?}");
        let normal = Vec3::new(0.0, 1.0, -0.5).normalize();
        let slope = |z: f32| Vec3::new(0.0, 0.5 * z, z);
        let on_a_slope = supporting(
            &FootGround { ankle: hit(slope(0.0), normal), ball: hit(slope(0.2), normal) },
            &probe,
            0.5,
        )
        .expect("ground");
        assert!(on_a_slope.height_at(probe.ankle).abs() < 1e-5, "{on_a_slope:?}");
    }

    #[test]
    fn nothing_steep_counts_as_ground() {
        let probe = FootProbe { ankle: Vec3::ZERO, ball: None };
        let wall = hit(Vec3::ZERO, Vec3::new(1.0, 0.2, 0.0));
        assert_eq!(supporting(&FootGround { ankle: wall, ball: None }, &probe, 0.5), None);
        assert_eq!(supporting(&FootGround::default(), &probe, 0.5), None);
    }
}
