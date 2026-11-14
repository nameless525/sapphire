//! Spring bones: the hair, ribbons and cloth that follow a character (spec §5.13), simulating what a
//! rig cooks from VRM's `VRMC_springBone`.
//!
//! [`SpringBones`] sits beside a [`CharacterPose`]. It reads the pose the animation made, moves each
//! chain's tail by inertia, a pull back toward that pose, gravity, the bone's length and the rig's
//! colliders, and turns the chain's joints to point at their tails. Then [`CharacterPose::finish`]
//! runs again, so the rest of the skeleton and the twist helpers see the result, as they would a
//! clip's.
//!
//! # Time
//!
//! Springs run on fixed sub-steps of the owner clock's ticks: [`Substeps`] of them a tick, at
//! multiples of the sub-step's length from the clock's start, whatever the frame rate. A frame calls
//! [`SpringBones::update`] with its clock's presented time, and sub-steps run up to it; presented
//! time never steps back, so that is safe. So hair is on the clock the character is:
//!
//! - **Hitstop or a stopped world:** presented time stands still, so nothing runs and nothing is
//!   written, to the bit, and the springs carry on from there.
//! - **Slow motion** (the world's or the character's own) slows hair with everything else on that
//!   clock: sub-steps come further apart and do the same work, so hair takes the path it would at
//!   full speed, slowly.
//!
//! A sub-step lands between two frames, so what follows the pose reads the last frame's and this
//! frame's poses and places interpolated between them: the bigger a frame's steps, the coarser the
//! path the chain's root took, which is all that frame rate changes. `springs_at_30_60_and_120_hz`
//! states the tolerance.
//!
//! A frame draws each joint's turn from its parent `(presented − last sub-step) / sub-step` of the
//! way from the sub-step before the last to the last. Hair relative to the head is one sub-step old
//! on every frame, which never leads the body and doesn't vary with the frame's phase; hair turning
//! rigidly with the head is on time, since a parent's turn isn't delayed.
//!
//! # Space
//!
//! Tails move in the world's orientation, so a character's own motion is what swings its hair,
//! relative to the character's origin, shifted with it each frame, so a town's far corner costs no
//! precision. A chain with a `center` joint is carried along with that joint instead, as VRM says,
//! so moving that joint doesn't swing the chain.
//!
//! # Tuning
//!
//! VRM's parameters are tuned on a model at rest and in gentle motion, and a character in a fight
//! throws them far past that. On the run a head moves at up to 4.7 m/s, about 8 cm a tick, where a
//! joint's stiffness of 0.5 pulls its tail 8 mm a tick. Each joint aims only relative to its parent,
//! so what it doesn't make up carries down the chain, and nothing limits it: the character's six-joint
//! strands came 174° off the pose, with 120° between one joint and the next, which reads as rope flung
//! round the head. It isn't a resonance, which would grow, since it is there in full from the first
//! stride, nor the sub-steps' rates, since each count does it. [`SpringTuning`] holds what answers it,
//! and [`SpringTuning::VRM`] turns all of it off:
//!
//! - **`root_inertia`:** the share of the character's travel and turning through the world that chains
//!   feel. At VRM's 1 the strike's 35 cm lunge sends the long hair into curls. At 0 a lunge shows no lag.
//! - **`anchor_inertia`:** the same for the joint a chain hangs from: the rest of that joint's motion
//!   carries the chain's tails along, as a `center` joint does. It takes the drive down without
//!   taking the swing out.
//! - **`swing_limit`:** the most a joint turns from where the animated pose points it, so a strand stays
//!   near where the clip's author left it however hard it is driven.
//! - **`bend_limit`:** the most a joint bends from where its parent's turn leaves it, so a strand turns
//!   in a curve and never curls. These are plain cones, which take a square root only where a joint
//!   leaves one, and hold at any sub-step count.
//!
//! A new [`SpringBones`] takes [`CharacterMotion::shipped`](crate::CharacterMotion::shipped)'s, tuned by eye
//! on the character's run, walk, idle, hit, dodge and strike against the reference games, where hair lags and
//! swings and keeps its shape. A game's own come through [`SpringBones::set_tuning`].
//!
//! # What the parameters mean
//!
//! VRM's parameters are per frame, and authors tune them at about 60 Hz: each step keeps `1 − drag`
//! of the tail's motion and adds `stiffness / 60` and `gravity / 60` meters to it. One sub-step a tick
//! is exactly that. With `n` sub-steps, each keeps `(1 − drag)^(1/n)` of the motion, `keep`, which
//! holds how fast a swing dies away. A push added to a tail's motion every sub-step settles at
//! `push / (1 − keep)` a sub-step, where one tick's settles at `push / drag` a tick, so each sub-step's
//! share of a tick's push is `(1 − keep) / drag` if the tail is to end at the same speed. That share
//! holds how fast a swing swings too.
//!
//! Stiffness and gravity scale by it, over `n` sub-steps. So does what the bone's length and the
//! colliders move a tail: at one sub-step a tick that counts as the tail's motion whole, and counted
//! whole at more sub-steps, a lunge flings hair much further (a 40 cm strand's tip went 12 cm from
//! one sub-step's path at two, and 23 cm at four; 6 and 9 cm with it).
//!
//! # Colliders
//!
//! A joint's tail keeps its hit radius out of the spheres and capsules of its chain's collider groups.
//! Discrete tests let a fast arm jump past a strand between sub-steps, so a tail's path through the
//! sub-step is tested against the collider's, relative to it: from the first contact the tail stays
//! on the side it came from. A tail that ends the sub-step on that side goes out the nearest way, so
//! it slides along the surface; one that got past the middle goes back. The bone's length then has its
//! say: where the point it wants is covered, the joint swings round the collider to the nearest point
//! it can reach.
//!
//! A chain tests its colliders deepest joint first, so the body comes last. A tail caught between two
//! can't clear both, and the last wins: hair squeezed by an arm goes over the arm rather than into the
//! chest. A later joint of a chain hangs from the one before it, so a collider coming between the two
//! has no answer a bone can give.
//!
//! The action substep holds the swing and bend limits after proxy contact to avoid unbounded curl,
//! except under a grounded torso ([`SpringBones::update_with_torso_contact`]), which clears strands
//! after the limits.
//! A collider can overrun a constrained tail and release it without a jump across the collider's
//! axis (`overrun_give`). The heroine also resolves garment and limb contact at the drawn pose,
//! with a bounded coherent change to each strand rather than more bend at the tip.
//!
//! # Taking turns
//!
//! A frame with several characters shares secondary motion's budget among them (M5 C2): the stage's
//! schedule gives each a [`SpringPace`] a frame ([`SpringBones::set_pace`]). The one on field leads,
//! stepping every sub-step due as above, and keeps the clearance pass. The others step or wait:
//!
//! - **A step** covers all the time since its last, as one step: a sub-step stretched over that many,
//!   its `keep` the product of theirs, its pushes their sum, its tails' motion stretched to match and
//!   back after (`Rate::over`). What the bone and the colliders did counts at most whole, as at
//!   one sub-step a tick: counted for more, a swing would grow. A longer wait than [`LONGEST_STRIDE`]
//!   ticks runs as even strides of at most that. `springs_take_turns_at_30_20_and_15_hz` states the
//!   tolerance, and `a_turn_every_few_frames_is_a_frame_every_few` that waiting changes nothing the
//!   solver keeps.
//! - **A wait** steps nothing. Its chains keep their turns from their parents, so they ride with the
//!   body however it moves, drawn on from the step before the last toward the last across as long as
//!   the last step was: one step behind, as the lead is one sub-step behind, so a chain moves on
//!   smoothly through the frames that wait and never jumps on the frame that steps.
//!
//! Only the lead runs the clearance pass; the others hold it, and a lead joining the rotation sleeps it,
//! so coming back to lead solves it whole.
//!
//! # Floor
//!
//! [`SpringBones::set_floor`] gives the springs and the cloth a plane they lie on (M5 C2), which the
//! stage takes from the ground the feet's IK probes find. Every tail keeps its hit radius above it,
//! inside the step as a collider is, then again once the limits have had their say, since no limit
//! holds a tail in the paving. Pushed out, a tail's slide along it is held back as hard as it was
//! pressed in, so hair lying on paving comes to rest rather than buzzing or creeping down a slope. No
//! floor, or one nothing reaches, changes nothing to the bit.
//!
//! # Cloth
//!
//! [`SpringBones::set_cloth`] wears sheets of neighbouring chains as cloth: XPBD links across the
//! chains, solved inside the same sub-steps after the springs' own step (the `cloth` module's docs).
//!
//! # Teleports, cuts and sleeping
//!
//! [`SpringBones::reset`] settles from the pose at the next update, with no motion, so a teleport or
//! a cut never drags hair across the world. So does waking from [`SpringFrame::active`] false, which
//! is how off-screen and far characters cost nothing: their sub-steps don't run, and neither does
//! their second [`CharacterPose::finish`].
//!
//! Once a chain has stopped moving and the pose and the character stand still, updates do nothing
//! until one of them moves, so a settled character skins nothing.
//!
//! # Limits
//!
//! - Scale is taken as uniform along a chain and through its parents: a joint's first axis.
//! - Springs are presentation. They never reach the simulation, whose hit volumes come from baked
//!   tables (spec §5.22), and aren't in its checksums.
//! - Colliders read the pose the animation made, so a collider on a spring joint doesn't follow that
//!   joint's swing.

use std::sync::Arc;

use sapphire_assets::mesh::Mesh;
use sapphire_assets::rig::{ArchivedCharacterRig, ArchivedColliderShape, ArchivedSpringTip, HumanBone};
use sapphire_assets::skeleton::{ArchivedSkeleton, NO_PARENT};
use sapphire_math::{Affine3A, Quat, Vec3};

use sapphire_body_pose::{CharacterPose, JointLayout, model_of};

mod clearance;
mod cloth;

pub use cloth::{ClothError, ClothSheet, MAX_CLOTH_ITERATIONS};

/// Ticks a second on the clocks springs run on (`sapphire_time::TICK_HZ`), and the rate VRM's
/// parameters are read at.
pub const SPRING_TICK_HZ: u32 = 60;

/// A tail slower than this, in meters a second, counts as still.
const STILL_SPEED: f32 = 1e-3;
/// How long every tail must stay still before the springs stop running, in ticks. A tail swinging on
/// a low drag stands still at each end of its swing, so this is longer than a swing takes.
const STILL_TICKS: u32 = 30;
/// The most sub-steps one frame runs, and so what a frame can cost: a frame further behind than that
/// loses the time rather than falling further behind. `FixedStepDriver` hands a frame four ticks at
/// most, so at normal speed this only matters above two sub-steps a tick.
const MAX_SUBSTEPS_PER_FRAME: i64 = 16;
/// How much nearer than its limit a sleeping character must come to wake: enough that a character
/// standing at the limit doesn't flip between the two.
const WAKE_MARGIN: f32 = 0.9;

/// How many times a tail is moved off a collider its bone won't let it leave directly.
const CLEARING_PASSES: u32 = 4;
/// Marks a [`Step`] that follows its parent without swinging, and a swing with no center joint.
const NONE: u16 = u16::MAX;

/// How many sub-steps springs run per tick of the owner's clock. Always a power of two, so a joint's
/// drag converts by square roots, the same to the bit on every OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Substeps {
    One = 1,
    Two = 2,
    Four = 4,
}

impl Substeps {
    /// How many that is.
    #[must_use]
    pub const fn count(self) -> u32 {
        self as u32
    }

    /// `drag`'s share kept per step of this length, of what a whole tick keeps.
    fn root(self, kept: f32) -> f32 {
        match self {
            Self::One => kept,
            Self::Two => kept.sqrt(),
            Self::Four => kept.sqrt().sqrt(),
        }
    }
}

/// How far past a grounded torso's middle, in meters at the rig's scale, a strand's joint must sit
/// before it clears to the other side from the one it last did.
const TORSO_SIDE_HOLD: f32 = 0.02;

/// The side, ±1, a strand whose joint sits `offset` along the character's +x from a grounded torso's
/// middle clears it to, given the side it `held` (0 for none): the one it held, unless the joint is more
/// than `hold` past the middle on the other.
fn torso_side(held: f32, offset: f32, hold: f32) -> f32 {
    if held != 0.0 && offset * held > -hold {
        held
    } else if offset < 0.0 {
        -1.0
    } else {
        1.0
    }
}

/// How a quality tier runs springs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringQuality {
    pub substeps: Substeps,
    /// How far from the camera a character's springs run, in meters. Beyond it hair is too small to
    /// read.
    pub distance: f32,
    /// How often the clothes' clearance solves, where a character has one.
    pub clearance: ClearanceQuality,
    /// How the characters other than the one on the field share the frame (M5 C2).
    pub stagger: StaggerQuality,
}

impl SpringQuality {
    /// Desktop-High's: 120 Hz out to 40 m, and clearance solved on every frame near the camera.
    pub const DESKTOP_HIGH: Self = Self {
        substeps: Substeps::Two,
        distance: 40.0,
        clearance: ClearanceQuality::FULL,
        stagger: StaggerQuality::DESKTOP_HIGH,
    };
    /// Desktop-Low's: Desktop-High's springs, with clearance solved every fourth tick.
    pub const DESKTOP_LOW: Self = Self {
        substeps: Substeps::Two,
        distance: 40.0,
        clearance: ClearanceQuality::DESKTOP_LOW,
        stagger: StaggerQuality::LIGHT,
    };
    /// The phones': one sub-step a tick, the 60 Hz VRM's parameters are tuned at, out to 25 m, with
    /// clearance solved every fourth tick.
    pub const MOBILE: Self = Self {
        substeps: Substeps::One,
        distance: 25.0,
        clearance: ClearanceQuality::MOBILE,
        stagger: StaggerQuality::LIGHT,
    };
    /// Every setting that ships, for tests that must hold on each.
    pub const SHIPPED: [Self; 3] = [Self::DESKTOP_HIGH, Self::DESKTOP_LOW, Self::MOBILE];

    /// Whether springs at `distance` meters from the camera run, given whether they ran last frame.
    #[must_use]
    pub fn in_range(&self, distance: f32, running: bool) -> bool {
        distance <= if running { self.distance } else { self.distance * WAKE_MARGIN }
    }
}

/// How a tier shares a frame's secondary motion among the characters other than the one on the field
/// (M5 C2): the stage's schedule steps this many of them a frame, in a fixed rotation, and each of the
/// rest waits its turn ([`SpringPace`]). Counts, never times, so two runs of a scene schedule alike on any
/// machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaggerQuality {
    /// How many of the others step in a frame.
    pub per_frame: u32,
    /// The most frames any of them waits between steps: more others than this many frames can carry at
    /// [`Self::per_frame`] step more a frame instead.
    pub longest_wait: u32,
}

impl StaggerQuality {
    /// Desktop-High's, which has no budget row: two others a frame, so a party's both step every frame.
    pub const DESKTOP_HIGH: Self = Self { per_frame: 2, longest_wait: 2 };
    /// Desktop-Low's and the phones': one other a frame, so a party's two take turns, stepping every other
    /// frame, and more others step more a frame rather than wait longer: a step over two frames holds
    /// the tolerance better than one over three or four (`springs_take_turns_at_30_20_and_15_hz`).
    pub const LIGHT: Self = Self { per_frame: 1, longest_wait: 2 };

    /// How many of `others` step in a frame: [`Self::per_frame`], or more if they wouldn't all have
    /// stepped within [`Self::longest_wait`] frames.
    #[must_use]
    pub fn stepping(&self, others: u32) -> u32 {
        self.per_frame.max(others.div_ceil(self.longest_wait.max(1))).max(1)
    }
}

/// How a character's springs take this frame, which the stage's schedule decides (M5 C2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpringPace {
    /// Every sub-step of the tier as it falls due, and the clothes' clearance at its interval: the
    /// character on the field, and any character alone.
    #[default]
    Lead,
    /// Its turn in the rotation: one step over all the time since its last, in strides of at most
    /// [`LONGEST_STRIDE`] ticks, with the clearance held as it last solved.
    Step,
    /// Waiting for its turn: nothing steps, and the chains keep their turns from their parents, drawn
    /// on toward the last step's, so they ride with the body.
    Wait,
}

/// The longest one step a character taking turns runs, in ticks: what waited longer runs as several.
/// Springs and cloth stay stable and within their tolerance of the 120 Hz run at steps of 2, 3 and 4
/// ticks (`springs_take_turns_at_30_20_and_15_hz`, `cloth_takes_turns_at_30_20_and_15_hz`); at 60 Hz the
/// tiers' rotations step every other frame, 2 ticks, and at 30 Hz 4.
pub const LONGEST_STRIDE: u32 = 4;

/// The ground under a character, which its tails stay above ([`SpringBones::set_floor`]): a plane, in
/// the model's space as the frame's `world_from_model` places it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringFloor {
    /// A point on it.
    pub point: Vec3,
    /// Up out of it, unit length.
    pub normal: Vec3,
}

/// How hard the floor holds a tail resting on it from sliding: the most it stops, as a share of how far
/// it pushed the tail out. A tail pressed into the paving by more than its pull along it is held still.
const FLOOR_FRICTION: f32 = 1.0;

/// How often a tier solves a character's clothes' clearance (the heroine's shirt against the hair and
/// face), by distance from the camera. Between solves the last one's corrections are held, carried
/// with the body: a strand turned off the shirt stays turned, relative to the chest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClearanceQuality {
    /// Ticks from one solve to the next out to [`Self::near`], on tick boundaries, so a replay solves
    /// on the same frames every time. 1 solves on every frame that draws springs, however short.
    pub interval: u32,
    /// Out to here, in meters from the camera, it solves every `interval`; beyond, every twice that.
    pub near: f32,
    /// Beyond this it holds what it last solved, at the cost of writing it.
    pub far: f32,
}

impl ClearanceQuality {
    /// Every frame out to 12 m, every other tick out to 24 m.
    pub const FULL: Self = Self { interval: 1, near: 12.0, far: 24.0 };
    /// Every fourth tick, as the phones: 15 solves a second, near, each spread over four frames at 60
    /// frames a second, with no forecast ahead (it runs only under 4 ticks). Every third tick until
    /// M4 H7, whose half-weight forecast and three-way spread held p99 over Desktop-Low's budget.
    pub const DESKTOP_LOW: Self = Self { interval: 4, ..Self::FULL };
    /// Every fourth tick: 15 solves a second, every other frame at 30 frames a second.
    pub const MOBILE: Self = Self { interval: 4, ..Self::FULL };

    /// The ticks between solves for a character `distance` meters from the camera, or `None` to hold.
    #[must_use]
    pub fn interval_at(&self, distance: f32) -> Option<u32> {
        if distance > self.far {
            None
        } else if distance > self.near {
            Some(self.interval.max(1) * 2)
        } else {
            Some(self.interval.max(1))
        }
    }
}

/// How a character's chains take a game's motion, on top of what VRM's parameters say. Off
/// ([`Self::VRM`]) it is VRM's step exactly. See the `springs` module's docs.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpringTuning {
    /// The share of the character's travel and turning through the world its chains feel, 0 to 1.
    pub root_inertia: f32,
    /// The share of the motion of the joint a chain hangs from that its tails feel, 0 to 1. The rest
    /// carries them along with that joint. A chain with a `center` joint follows that instead.
    pub anchor_inertia: f32,
    /// The most a joint turns from where the animated pose points it, in radians.
    pub swing_limit: f32,
    /// The most a joint bends from where its parent's turn leaves it, in radians: what keeps a chain
    /// from curling.
    pub bend_limit: f32,
}

impl SpringTuning {
    /// VRM's step as it is written, with no limits.
    pub const VRM: Self = Self {
        root_inertia: 1.0,
        anchor_inertia: 1.0,
        swing_limit: std::f32::consts::PI,
        bend_limit: std::f32::consts::PI,
    };
}

impl SpringTuning {
    fn is_limited(&self) -> bool {
        self.swing_limit < std::f32::consts::PI || self.bend_limit < std::f32::consts::PI
    }
}

/// A cone around a direction, by its half angle's cosine and sine.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cone {
    cos: f32,
    sin: f32,
}

impl Cone {
    fn new(angle: f32) -> Self {
        let (sin, cos) = angle.clamp(0.0, std::f32::consts::PI).sin_cos();
        // At a half turn nothing is outside it, rounding included.
        if angle >= std::f32::consts::PI { Self { cos: -2.0, sin } } else { Self { cos, sin } }
    }

    /// `aim` as it is, or the nearest direction in the cone around `center`: both unit vectors.
    #[inline]
    fn hold(self, aim: Vec3, center: Vec3) -> Vec3 {
        let cos = aim.dot(center);
        if cos >= self.cos {
            return aim;
        }
        let side = (aim - center * cos).try_normalize().unwrap_or_else(|| center.any_orthonormal_vector());
        center * self.cos + side * self.sin
    }
}

/// What one frame tells [`SpringBones::update`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringFrame {
    /// Where the character is in the world this frame, at [`Self::ticks`]. Its motion swings its
    /// hair. A rigid transform, or one with a uniform scale.
    pub world_from_model: Affine3A,
    /// The time this frame draws on the owner's clock, in ticks:
    /// `LocalClock::presented_ticks(alpha)`.
    pub ticks: f64,
    /// Whether the pose holds a new evaluation, finished, with nothing written over it since. False
    /// on a frame that evaluated nothing, whose pose still holds the springs' own last result.
    pub pose_moved: bool,
    /// Whether the character is on screen and near enough. While it isn't, nothing runs.
    pub active: bool,
}

/// What one [`SpringBones::update`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpringWork {
    /// Sub-steps it ran.
    pub substeps: u32,
    /// Whether it wrote the pose and finished it, so its palette needs writing again. When it didn't,
    /// the pose is as it was.
    pub wrote_pose: bool,
}

/// A swinging joint's tail, in the world ([`SpringBones::tails`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringTail {
    pub joint: u16,
    pub position: Vec3,
    /// How far it is from its joint at rest.
    pub length: f32,
    /// How far colliders keep it off them.
    pub hit_radius: f32,
}

/// A collider, in the world ([`SpringBones::colliders`]). A sphere's two ends are the same point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringCollider {
    pub start: Vec3,
    pub end: Vec3,
    pub radius: f32,
}

impl SpringCollider {
    /// How far `point` is from its surface, negative inside.
    #[must_use]
    pub fn distance(&self, point: Vec3) -> f32 {
        let axis = self.end - self.start;
        let along = axis.length_squared();
        let nearest = if along == 0.0 {
            self.start
        } else {
            self.start + axis * ((point - self.start).dot(axis) / along).clamp(0.0, 1.0)
        };
        point.distance(nearest) - self.radius
    }
}

/// A place and turn in the simulation's space, with the scale it applies below it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    rotation: Quat,
    position: Vec3,
    scale: f32,
}

impl Frame {
    const IDENTITY: Self = Self { rotation: Quat::IDENTITY, position: Vec3::ZERO, scale: 1.0 };

    /// A model transform as a frame. A collapsed joint, which has no rotation, keeps none.
    fn of(transform: &Affine3A) -> Self {
        let (scale, rotation, position) = transform.to_scale_rotation_translation();
        let rotation = if rotation.is_finite() { rotation.normalize() } else { Quat::IDENTITY };
        Self { rotation, position, scale: scale.x }
    }

    /// `self`, a frame in the model's space, in the world's orientation from the model's origin.
    fn in_world(&self, world: &Self) -> Self {
        Self {
            rotation: (world.rotation * self.rotation).normalize(),
            position: world.rotation * (self.position * world.scale),
            scale: world.scale * self.scale,
        }
    }

    fn lerp(&self, other: &Self, u: f32) -> Self {
        Self {
            rotation: self.rotation.lerp(other.rotation, u),
            position: self.position.lerp(other.position, u),
            scale: self.scale + (other.scale - self.scale) * u,
        }
    }
}

/// One joint the simulation walks through, parents first: a swinging joint, or one on the way between
/// two of them, which follows its parent.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Step {
    joint: u16,
    /// Its parent's slot: an anchor's, or an earlier step's.
    parent: u16,
    /// Its entry in [`SpringBones::swings`], or [`NONE`].
    swing: u16,
}

/// One joint that swings toward a tail, and what it takes from the rig.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Swing {
    joint: u16,
    /// Toward its tail, in its own space at the pose: a unit vector.
    tail: Vec3,
    /// How far its tail is, at the scale of its own space.
    length: f32,
    hit_radius: f32,
    stiffness: f32,
    /// Direction times power: an acceleration, by VRM's meaning of the word.
    gravity: Vec3,
    drag: f32,
    /// Its chain's colliders in [`SpringBones::collider_lists`].
    colliders: (u32, u32),
    /// What carries it: its chain's entry in [`SpringBones::centers`].
    center: u16,
}

/// A joint that carries chains along.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Center {
    /// Its slot.
    slot: u16,
    /// Whether it is a chain's `center`, which carries it whole, rather than what it hangs from.
    whole: bool,
    /// The share of its motion it carries them by.
    carried: f32,
}

/// What a center's motion over a sub-step does to the tails it carries.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Carry {
    turn: Quat,
    from: Vec3,
    to: Vec3,
}

/// What a joint's parameters come to at the current sub-step's length. See the module docs.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rate {
    /// The share of the tail's motion a sub-step keeps.
    keep: f32,
    /// Meters a sub-step adds per unit of stiffness or gravity.
    force: f32,
    /// The share of what the bone and the colliders moved it that counts as its motion.
    carry: f32,
    /// For a stride (`Rate::over`): the share of the motion and the meters of push its last sub-step
    /// would end on, which is the motion it keeps. A sub-step's own are `keep` and `force`.
    keep_end: f32,
    force_end: f32,
}

impl Rate {
    fn new(drag: f32, substeps: Substeps) -> Self {
        let tick = 1.0 / SPRING_TICK_HZ as f32;
        let count = substeps.count() as f32;
        let keep = substeps.root(1.0 - drag);
        // Each sub-step's share of a tick's push. It tends to `1 / n` as drag goes to nothing.
        let carry = if drag > 1e-4 { (1.0 - keep) / drag } else { 1.0 / count };
        Self { keep, force: tick * carry / count, carry, keep_end: keep, force_end: tick * carry / count }
    }

    /// The rate of a step `stride` sub-steps long, for a character taking turns, with its tails' motion
    /// counted over the whole stride: the sub-steps' own recurrence, summed. Each keeps `r` of the motion
    /// and adds a push `f`, so over `L` of them a tail moving `v` a sub-step goes `v·A + f·B`, where
    /// `A = r(1 − rᴸ)/(1 − r)` and `B = (L − A)/(1 − r)`, and ends moving `v·rᴸ + f(1 − rᴸ)/(1 − r)` a
    /// sub-step. What the bone and the colliders did counts so that taking out the motion the tail went
    /// with leaves none, as at a sub-step. Powers by multiplying, the same to the bit on every OS.
    fn over(drag: f32, substeps: Substeps, stride: u32) -> Self {
        let one = Self::new(drag, substeps);
        if stride <= 1 {
            return one;
        }
        let length = stride as f32;
        let r = one.keep;
        let last = (0..stride).fold(1.0, |kept, _| kept * r);
        let (sum, ends) = if 1.0 - r > 1e-6 {
            let sum = r * (1.0 - last) / (1.0 - r);
            (sum, (length - sum) / (1.0 - r))
        } else {
            (length, length * (length + 1.0) * 0.5)
        };
        let settles = if 1.0 - r > 1e-6 { (1.0 - last) / (1.0 - r) } else { length };
        // The motion it steps with is the stride's, `length` sub-steps' worth.
        Self {
            keep: sum / length,
            force: one.force * ends,
            // What a sub-step's correction does to the motion it moves with, in the share the stride's
            // last sub-step keeps of the motion it went with: a correction taking out all the motion it
            // went with, as the bone's length does of the radial, leaves none.
            carry: one.carry * (length * last / sum),
            keep_end: last,
            force_end: one.force * settles * length,
        }
    }
}

/// A tail's place at the last sub-step and the one before, whose difference is its motion.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Tail {
    now: Vec3,
    before: Vec3,
}

/// A swinging bone at a sub-step, as its colliders see it.
#[derive(Clone, Copy, Debug)]
struct Bone {
    head: Vec3,
    length: f32,
    hit_radius: f32,
    /// Where it points when nothing else says: as the pose has it.
    fallback: Vec3,
}

/// A collider on a joint, in that joint's space.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Collider {
    /// The joint's slot.
    anchor: u16,
    offset: Vec3,
    /// From the capsule's start to its end: zero for a sphere.
    axis: Vec3,
    radius: f32,
    /// A spine or chest collider can take priority when the body is on the ground.
    torso: bool,
}

/// A collider at one sub-step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Placed {
    start: Vec3,
    axis: Vec3,
    radius: f32,
}

impl Placed {
    /// The nearest point of its axis to `point`.
    #[inline]
    fn nearest(&self, point: Vec3) -> Vec3 {
        let along = self.axis.length_squared();
        if along == 0.0 {
            return self.start;
        }
        self.start + self.axis * ((point - self.start).dot(self.axis) / along).clamp(0.0, 1.0)
    }
}

/// Where a tail going from `from` to `to` belongs against a collider going from `was` to `now`, or
/// `None` if it stayed clear: tested relative to the collider, so neither's speed can step over the
/// other. A tail that ends on the side it came from goes out the nearest way, so it slides along the
/// surface; one that got past the middle goes back to the side it came from.
#[inline]
fn contact(from: Vec3, to: Vec3, was: &Placed, now: &Placed, hit_radius: f32) -> Option<Vec3> {
    let reach = now.radius + hit_radius;
    let square = reach * reach;
    let nearest = now.nearest(to);
    let end = to - nearest;
    let end_square = end.length_squared();
    let start = from - was.nearest(from);
    let start_square = start.length_squared();
    let path = end - start;
    let toward = start.dot(path);
    // The side it came from: where it began, if that was inside, otherwise where it first touched.
    let side = if start_square < square {
        start
    } else {
        if toward >= 0.0 {
            return None;
        }
        let along = path.length_squared();
        let discriminant = toward * toward - along * (start_square - square);
        if discriminant < 0.0 {
            return None;
        }
        let first = (-toward - discriminant.sqrt()) / along;
        if first > 1.0 {
            return None;
        }
        start + path * first
    };
    let same_side = end.dot(side) > 0.0;
    if end_square >= square && same_side {
        return None;
    }
    let out = if same_side { end } else { side };
    Some(nearest + out.try_normalize().or_else(|| side.try_normalize()).unwrap_or(Vec3::Y) * reach)
}

/// How hard a collider pushes a tail that began a sub-step `share` of the way out from its axis to its
/// reach: fully near the surface, and not at all from half way in. A limit can leave a tail inside a
/// collider, and the way out flips as the collider's axis crosses it, which would throw the tail across
/// in a tick. So a collider that has overrun a tail lets it be, and takes it up again as it leaves.
fn overrun_give(share: f32) -> f32 {
    let along = ((share - 0.5) * 2.0).clamp(0.0, 1.0);
    along * along * (3.0 - 2.0 * along)
}

/// The point `length` from `head` toward `out`, or, where `placed` covers that, the nearest such
/// point that it doesn't: a bone can only clear a collider that has moved a long way by swinging
/// around it, on `out`'s side. With `head` so deep in that no point clears it, the furthest out.
fn clear_of(head: Vec3, length: f32, out: Vec3, placed: &Placed, reach: f32, fallback: Vec3) -> Vec3 {
    let mut point = head + (out - head).try_normalize().unwrap_or(fallback) * length;
    // A sphere takes one pass. Around a capsule the nearest point moves with the tail.
    for _ in 0..CLEARING_PASSES {
        let center = placed.nearest(point);
        if (point - center).length_squared() >= reach * reach {
            break;
        }
        let offset = center - head;
        let square = offset.length_squared();
        let distance = square.sqrt();
        if distance < 1e-6 {
            break;
        }
        let toward = offset / distance;
        // Where the two spheres meet, along the line between their centers.
        let along = (square + length * length - reach * reach) / (2.0 * distance);
        if along <= -length {
            return head - toward * length;
        }
        if along >= length {
            break;
        }
        let ring = (length * length - along * along).sqrt();
        let wanted = point - head;
        let side = (wanted - toward * wanted.dot(toward))
            .try_normalize()
            .unwrap_or_else(|| toward.any_orthonormal_vector());
        point = head + toward * along + side * ring;
    }
    point
}

/// Where a tail going from `from` to `next` over a step belongs above a floor `level` along `up`: pushed
/// out of it with its hit radius, its slide along it held back by friction as hard as it was pressed in,
/// so a strand lying on paving comes to rest, then swung to the nearest point its bone reaches that isn't
/// below it. A head so far under the floor that no point clears it points straight up.
fn on_floor(up: Vec3, level: f32, bone: &Bone, from: Vec3, next: Vec3) -> Vec3 {
    let Bone { head, length, hit_radius, fallback } = *bone;
    let level = level + hit_radius;
    let depth = level - up.dot(next);
    if depth <= 0.0 {
        return next;
    }
    let out = next + up * depth;
    let travel = out - from;
    let slide = travel - up * up.dot(travel);
    let (slid, held) = (slide.length(), FLOOR_FRICTION * depth);
    let out = if slid <= held { out - slide } else { out - slide * (held / slid) };
    // How far the floor's level is above the bone's head.
    let height = level - up.dot(head);
    if height >= length {
        return head + up * length;
    }
    let point = head + (out - head).try_normalize().unwrap_or(fallback) * length;
    if up.dot(point) >= level {
        return point;
    }
    // Where the bone's reach meets the floor, on the side the tail went.
    let ring = (length * length - height * height).max(0.0).sqrt();
    let toward = out - head;
    let side = (toward - up * up.dot(toward)).try_normalize().unwrap_or_else(|| up.any_orthonormal_vector());
    head + up * height + side * ring
}

/// One character's springs: what its rig says, read once, and where its tails are. The `springs`
/// module's docs say how they run.
#[derive(Clone, Debug, PartialEq)]
pub struct SpringBones {
    layout: Arc<JointLayout>,
    /// The joints the pose places for the simulation: parents of the chains, colliders' joints and
    /// center joints; [`NO_PARENT`] is the model's origin. Each has a slot, then each [`Step`].
    anchors: Vec<u16>,
    steps: Vec<Step>,
    swings: Vec<Swing>,
    colliders: Vec<Collider>,
    /// Every chain's colliders, as indices into [`Self::colliders`].
    collider_lists: Vec<u16>,
    /// The joints that carry chains.
    centers: Vec<Center>,

    substeps: Substeps,
    rates: Vec<Rate>,

    tails: Vec<Tail>,
    /// Each swing's turn from its parent after the sub-step before the last, and after the last.
    solved: Vec<[Quat; 2]>,
    /// The side, along the character's +x, each swing last cleared a grounded torso to, ±1, or 0 if it
    /// hasn't since it settled: kept until its joint is clearly on the other side.
    torso_sides: Vec<f32>,
    /// Each swing's turn from its parent in the pose: what its stiffness pulls toward.
    targets: Vec<Quat>,
    /// The anchors in the model's space, in the pose.
    model_anchors: Vec<Frame>,
    /// The anchors as the last frame had them, in the simulation's space, and as this frame does.
    anchors_was: Vec<Frame>,
    anchors_now: Vec<Frame>,
    /// Every slot, at the sub-step running.
    slots: Vec<Frame>,
    /// Each step's turn at the sub-step running, as the animated pose has it, with no swing above it.
    posed: Vec<Quat>,
    colliders_was: Vec<Placed>,
    colliders_now: Vec<Placed>,
    /// Each center at the last sub-step.
    centers_was: Vec<Frame>,
    /// What a center's motion over this sub-step does to the tails carried with it.
    center_turns: Vec<Carry>,

    /// How its chains take the character's motion ([`Self::set_tuning`]).
    tuning: SpringTuning,
    swing_cone: Cone,
    bend_cone: Cone,
    /// Whether either cone can hold a joint back.
    limited: bool,
    /// The world transform as of the last update.
    world: Affine3A,
    /// The world's rotation as of the last update.
    world_rotation: Quat,
    /// The world's translation as of the last update, which the simulation's space starts from.
    origin: Vec3,
    /// The presented time of the last update.
    drawn_at: f64,
    /// The last sub-step, counted from the clock's start.
    index: i64,
    /// Settle from the pose at the next update.
    settling: bool,
    /// The pose moved while nothing was reading it.
    targets_stale: bool,
    /// Sub-steps every tail has stayed still.
    still_for: u32,
    /// Everything has stopped, so an update has nothing to do.
    resting: bool,
    /// Optional contact against the drawn surface of the character's clothes.
    clearance: Option<Box<clearance::Clearance>>,
    /// Ticks between the clearance's solves, `None` to hold what it last solved
    /// ([`Self::set_clearance_interval`]).
    clearance_interval: Option<u32>,
    /// Sheets of its chains worn as cloth ([`Self::set_cloth`]).
    cloth: Option<Box<cloth::Cloth>>,

    /// How it takes this frame ([`Self::set_pace`]).
    pace: SpringPace,
    /// The sub-steps the step running covers: 1 but in a stride of a character taking turns.
    stride: u32,
    /// Each swing's rate over [`Self::stride`], when that is more than one.
    stride_rates: Vec<Rate>,
    /// The sub-steps the last step covered, which a frame draws the chains across.
    drawn_stride: u32,
    /// The presented time last drawn.
    shown_at: f64,
    /// The pose moved while it waited its turn, so the next step reads its places again.
    moved_waiting: bool,
    /// The ground under it, which its tails stay above ([`Self::set_floor`]).
    floor: Option<SpringFloor>,
    /// That floor this frame, in the simulation's space: up, and how far up it is along that.
    plane: Option<(Vec3, f32)>,
}

/// Presentation contact fitted to the heroine's cooked strands and shirt ([`SpringBones::with_clearance`]):
/// tens of milliseconds of work over the character's mesh, which a boot does on a worker, off the frame
/// (M4 H4). It reads only the mesh, the skeleton and the rig.
pub struct HeroineClearance(Box<clearance::Clearance>);

impl HeroineClearance {
    /// The fit for `mesh`, `skeleton` and `rig`, or `None` for a character it doesn't fit.
    #[must_use]
    pub fn new(mesh: &Mesh, skeleton: &ArchivedSkeleton, rig: &ArchivedCharacterRig) -> Option<Self> {
        clearance::Clearance::new(mesh, skeleton, rig).map(|clearance| Self(Box::new(clearance)))
    }
}

impl SpringBones {
    /// Springs for `pose` from `rig`, or `None` if it has none, or was made for another skeleton: a
    /// character with no springs holds nothing and costs nothing. They settle at the first update.
    /// Allocates; updating doesn't.
    #[must_use]
    pub fn new(rig: &ArchivedCharacterRig, pose: &CharacterPose) -> Option<Self> {
        let joints = pose.joint_count();
        if rig.joint_count.to_native() as usize != joints {
            return None;
        }
        let rest = rest_model(pose);

        let mut swings: Vec<Swing> = Vec::new();
        let mut collider_lists: Vec<u16> = Vec::new();
        let mut centers: Vec<Center> = Vec::new();
        let mut center_joints: Vec<(u16, bool)> = Vec::new();
        for spring in rig.springs.iter() {
            let first = collider_lists.len();
            for group in spring.collider_groups.iter() {
                let group = &rig.collider_groups[group.to_native() as usize];
                for collider in group.colliders.iter() {
                    let collider = collider.to_native() as u16;
                    if !collider_lists[first..].contains(&collider) {
                        collider_lists.push(collider);
                    }
                }
            }
            // Where a tail is caught between two, the last wins: the body over the limbs that swing
            // past it, so hair squeezed by an arm goes over the arm rather than into the chest.
            collider_lists[first..].sort_by_key(|&collider| {
                std::cmp::Reverse(depth(pose, rig.colliders[usize::from(collider)].joint.to_native()))
            });
            let colliders = (first as u32, collider_lists.len() as u32);
            let center = spring.center.as_ref().map_or(NONE, |center| {
                let center = (center.to_native(), true);
                let index = center_joints.iter().position(|&known| known == center).unwrap_or_else(|| {
                    center_joints.push(center);
                    center_joints.len() - 1
                });
                index as u16
            });
            for (at, entry) in spring.joints.iter().enumerate() {
                let joint = entry.joint.to_native();
                let own = &rest[usize::from(joint)];
                let tail = match (spring.joints.get(at + 1), &spring.tip) {
                    (Some(next), _) => {
                        Some(Vec3::from(rest[usize::from(next.joint.to_native())].translation))
                    }
                    (None, ArchivedSpringTip::Joint) => None,
                    (None, ArchivedSpringTip::Extended { length }) => {
                        // Along the bone from its parent, this many meters past the joint.
                        pose.parent(usize::from(joint)).and_then(|parent| {
                            let bone = Vec3::from(own.translation - rest[parent].translation);
                            bone.try_normalize()
                                .map(|bone| Vec3::from(own.translation) + bone * length.to_native())
                        })
                    }
                };
                let Some(tail) = tail else { continue };
                // In its own space, so the length carries its scale. A joint in two chains swings for
                // the first, and one on top of its tail has nowhere to point.
                let tail = own.inverse().transform_point3(tail);
                let length = tail.length();
                if length < 1e-6 || swings.iter().any(|swing| swing.joint == joint) {
                    continue;
                }
                let gravity = Vec3::from(entry.gravity_dir.map(|c| c.to_native()));
                swings.push(Swing {
                    joint,
                    tail: tail / length,
                    length,
                    hit_radius: entry.hit_radius.to_native(),
                    stiffness: entry.stiffness.to_native(),
                    gravity: gravity * entry.gravity_power.to_native(),
                    drag: entry.drag_force.to_native(),
                    colliders,
                    center,
                });
            }
        }
        if swings.is_empty() {
            return None;
        }
        swings.sort_by_key(|swing| swing.joint);

        // Parents first, for the walk: every swinging joint, and any joint between two of them.
        let parent_of = |joint: usize| pose.parent(joint);
        let mut swinging = vec![NONE; joints];
        for (index, swing) in swings.iter().enumerate() {
            swinging[usize::from(swing.joint)] = index as u16;
        }
        let mut below_a_swing = vec![false; joints];
        for joint in 0..joints {
            below_a_swing[joint] =
                swinging[joint] != NONE || parent_of(joint).is_some_and(|parent| below_a_swing[parent]);
        }
        let mut walked: Vec<bool> = swinging.iter().map(|&swing| swing != NONE).collect();
        for joint in (0..joints).rev() {
            if walked[joint]
                && let Some(parent) = parent_of(joint)
                && below_a_swing[parent]
            {
                walked[parent] = true;
            }
        }
        // A chain with no center of its own is carried by what it hangs from.
        for swing in &mut swings {
            if swing.center != NONE {
                continue;
            }
            let mut joint = usize::from(swing.joint);
            let hangs_from = loop {
                match parent_of(joint) {
                    Some(parent) if walked[parent] => joint = parent,
                    Some(parent) => break parent as u16,
                    None => break NO_PARENT,
                }
            };
            let carrier = (hangs_from, false);
            let index = center_joints.iter().position(|&known| known == carrier).unwrap_or_else(|| {
                center_joints.push(carrier);
                center_joints.len() - 1
            });
            swing.center = index as u16;
        }

        let mut anchors: Vec<u16> = Vec::new();
        let anchor = |anchors: &mut Vec<u16>, joint: u16| -> u16 {
            let at = anchors.iter().position(|&known| known == joint).unwrap_or_else(|| {
                anchors.push(joint);
                anchors.len() - 1
            });
            at as u16
        };
        let colliders: Vec<Collider> = rig
            .colliders
            .iter()
            .map(|collider| {
                let (offset, tail, radius) = match &collider.shape {
                    ArchivedColliderShape::Sphere { offset, radius } => {
                        let offset = offset.map(|c| c.to_native());
                        (offset, offset, radius.to_native())
                    }
                    ArchivedColliderShape::Capsule { offset, radius, tail } => {
                        (offset.map(|c| c.to_native()), tail.map(|c| c.to_native()), radius.to_native())
                    }
                };
                Collider {
                    anchor: anchor(&mut anchors, collider.joint.to_native()),
                    offset: Vec3::from(offset),
                    axis: Vec3::from(tail) - Vec3::from(offset),
                    radius,
                    torso: rig.humanoid.iter().any(|entry| {
                        entry.joint == collider.joint
                            && matches!(
                                entry.bone,
                                HumanBone::Spine | HumanBone::Chest | HumanBone::UpperChest
                            )
                    }),
                }
            })
            .collect();
        let tuning = crate::CharacterMotion::shipped().springs;
        for (joint, whole) in center_joints {
            let slot = anchor(&mut anchors, joint);
            centers.push(Center { slot, whole, carried: carried(whole, &tuning) });
        }
        // The walk's own parents last, so a step's slot is the anchors' count and its place: an anchor
        // is a joint as the pose places it, so one that is also walked is read from the pose.
        for joint in (0..joints).filter(|&joint| walked[joint]) {
            match parent_of(joint) {
                Some(parent) if walked[parent] => {}
                Some(parent) => {
                    anchor(&mut anchors, parent as u16);
                }
                None => {
                    anchor(&mut anchors, NO_PARENT);
                }
            }
        }
        let mut step_of = vec![0_u16; joints];
        let mut steps = Vec::new();
        for joint in (0..joints).filter(|&joint| walked[joint]) {
            let parent = match parent_of(joint) {
                Some(parent) if walked[parent] => anchors.len() as u16 + step_of[parent],
                Some(parent) => anchor(&mut anchors, parent as u16),
                None => anchor(&mut anchors, NO_PARENT),
            };
            step_of[joint] = steps.len() as u16;
            steps.push(Step { joint: joint as u16, parent, swing: swinging[joint] });
        }

        let substeps = SpringQuality::DESKTOP_HIGH.substeps;
        let (swing_count, anchor_count, collider_count) = (swings.len(), anchors.len(), colliders.len());
        let center_count = centers.len();
        let step_count = steps.len();
        let slot_count = anchor_count + step_count;
        Some(Self {
            layout: pose.joint_layout(),
            rates: swings.iter().map(|swing| Rate::new(swing.drag, substeps)).collect(),
            anchors,
            steps,
            swings,
            colliders,
            collider_lists,
            centers,
            substeps,
            tails: vec![Tail::default(); swing_count],
            solved: vec![[Quat::IDENTITY; 2]; swing_count],
            torso_sides: vec![0.0; swing_count],
            targets: vec![Quat::IDENTITY; swing_count],
            model_anchors: vec![Frame::IDENTITY; anchor_count],
            anchors_was: vec![Frame::IDENTITY; anchor_count],
            anchors_now: vec![Frame::IDENTITY; anchor_count],
            slots: vec![Frame::IDENTITY; slot_count],
            posed: vec![Quat::IDENTITY; step_count],
            colliders_was: vec![Placed::default(); collider_count],
            colliders_now: vec![Placed::default(); collider_count],
            centers_was: vec![Frame::IDENTITY; center_count],
            center_turns: vec![
                Carry { turn: Quat::IDENTITY, from: Vec3::ZERO, to: Vec3::ZERO };
                center_count
            ],
            swing_cone: Cone::new(tuning.swing_limit),
            bend_cone: Cone::new(tuning.bend_limit),
            limited: tuning.is_limited(),
            tuning,
            world: Affine3A::IDENTITY,
            world_rotation: Quat::IDENTITY,
            origin: Vec3::ZERO,
            drawn_at: 0.0,
            index: 0,
            settling: true,
            targets_stale: true,
            still_for: 0,
            resting: false,
            clearance: None,
            clearance_interval: Some(1),
            cloth: None,
            pace: SpringPace::Lead,
            stride: 1,
            stride_rates: vec![
                Rate { keep: 1.0, force: 0.0, carry: 1.0, keep_end: 1.0, force_end: 0.0 };
                swing_count
            ],
            drawn_stride: 1,
            shown_at: 0.0,
            moved_waiting: false,
            floor: None,
            plane: None,
        })
    }

    /// Fits presentation contact to the heroine's cooked strands and shirt.
    #[must_use]
    pub fn with_heroine_clearance(
        self,
        mesh: &Mesh,
        skeleton: &ArchivedSkeleton,
        rig: &ArchivedCharacterRig,
    ) -> Self {
        self.with_clearance(HeroineClearance::new(mesh, skeleton, rig))
    }

    /// [`Self::with_heroine_clearance`], with a fit made beforehand: off the frame, as a boot makes it (M4 H4).
    #[must_use]
    pub fn with_clearance(mut self, clearance: Option<HeroineClearance>) -> Self {
        self.clearance = clearance.map(|clearance| clearance.0);
        self
    }

    /// How many joints swing.
    #[must_use]
    pub fn swing_count(&self) -> usize {
        self.swings.len()
    }

    /// The sub-steps it runs.
    #[must_use]
    pub fn substeps(&self) -> Substeps {
        self.substeps
    }

    /// Whether everything has stopped until the pose or the character moves.
    #[must_use]
    pub fn is_resting(&self) -> bool {
        self.resting
    }

    /// Every swinging joint's tail as of the last sub-step, for tests and debug views.
    pub fn tails(&self) -> impl Iterator<Item = SpringTail> + '_ {
        self.swings.iter().zip(&self.tails).map(|(swing, tail)| SpringTail {
            joint: swing.joint,
            position: tail.now + self.origin,
            length: swing.length,
            hit_radius: swing.hit_radius,
        })
    }

    /// Every collider as of the last sub-step, in the rig's order, for tests and debug views.
    pub fn colliders(&self) -> impl Iterator<Item = SpringCollider> + '_ {
        self.colliders_now.iter().map(|placed| SpringCollider {
            start: placed.start + self.origin,
            end: placed.start + placed.axis + self.origin,
            radius: placed.radius,
        })
    }

    /// Takes `tuning` from now on. [`SpringTuning::VRM`] is VRM's step as it is written.
    pub fn set_tuning(&mut self, tuning: SpringTuning) {
        let tuning = SpringTuning {
            root_inertia: tuning.root_inertia.clamp(0.0, 1.0),
            anchor_inertia: tuning.anchor_inertia.clamp(0.0, 1.0),
            ..tuning
        };
        self.tuning = tuning;
        self.swing_cone = Cone::new(tuning.swing_limit);
        self.bend_cone = Cone::new(tuning.bend_limit);
        self.limited = tuning.is_limited();
        for center in &mut self.centers {
            center.carried = carried(center.whole, &tuning);
        }
    }

    /// Solves the clothes' clearance every `interval` ticks from now on, on tick boundaries, holding
    /// what it last solved between; `Some(1)`, as it starts, solves on every update, and `None` holds
    /// ([`ClearanceQuality::interval_at`]). A character with no clearance ignores it.
    pub fn set_clearance_interval(&mut self, interval: Option<u32>) {
        self.clearance_interval = interval;
    }

    /// Takes the next update as `pace` says: in the lead, a step in its turn, or waiting for it. Its
    /// tails go on from wherever they are, so a character changing pace doesn't pop. Leaving the lead,
    /// the clothes' clearance holds what it last solved from then on; coming back, it solves whole.
    pub fn set_pace(&mut self, pace: SpringPace) {
        if pace == self.pace {
            return;
        }
        if self.pace == SpringPace::Lead
            && let Some(clearance) = &mut self.clearance
        {
            clearance.sleep();
        }
        self.pace = pace;
    }

    /// How it takes the next update.
    #[must_use]
    pub fn pace(&self) -> SpringPace {
        self.pace
    }

    /// Keeps its tails, with their hit radius, above `floor` from the next update, or above nothing for
    /// `None`. The plane is in the model's space, as the next frame's `world_from_model` places it, so a
    /// caller gives it each frame from what it finds under the feet (spec §5.13, M5 C2).
    pub fn set_floor(&mut self, floor: Option<SpringFloor>) {
        self.floor = floor.filter(|floor| floor.normal.is_finite() && floor.point.is_finite());
    }

    /// The floor it keeps its tails above.
    #[must_use]
    pub fn floor(&self) -> Option<SpringFloor> {
        self.floor
    }

    /// Runs `substeps` from now on. A tail's speed is kept across the change, and its place.
    pub fn set_substeps(&mut self, substeps: Substeps) {
        if substeps == self.substeps {
            return;
        }
        self.drawn_stride = 1;
        let ratio = self.substeps.count() as f32 / substeps.count() as f32;
        for tail in &mut self.tails {
            tail.before = tail.now - (tail.now - tail.before) * ratio;
        }
        self.substeps = substeps;
        for (rate, swing) in self.rates.iter_mut().zip(&self.swings) {
            *rate = Rate::new(swing.drag, substeps);
        }
        self.index = self.index_at(self.drawn_at);
    }

    /// Settles from the pose at the next update, with no motion: for a teleport or a camera cut, so
    /// hair never whips across the world. A plain call for whatever knows one happened.
    pub fn reset(&mut self) {
        self.settling = true;
        self.resting = false;
        self.still_for = 0;
        if let Some(clearance) = &mut self.clearance {
            clearance.reset();
        }
    }

    /// Takes over `other`'s motion for a new version of the character: its tails and clock, when it
    /// swings the same joints the same way. Otherwise this settles at its next update. Cloth carries
    /// its iterations over; the sheets are this one's own ([`Self::set_cloth`]), set first.
    pub fn continue_from(&mut self, other: &Self) {
        if let (Some(cloth), Some(old)) = (&mut self.cloth, &other.cloth) {
            cloth.continue_from(old);
        }
        let same = self.layout == other.layout
            && self.steps == other.steps
            && self.anchors == other.anchors
            && self.colliders.len() == other.colliders.len()
            && self.centers.len() == other.centers.len();
        if !same || other.settling {
            self.set_substeps(other.substeps);
            self.reset();
            if same && let (Some(clearance), Some(old)) = (&mut self.clearance, &other.clearance) {
                self.drawn_at = other.drawn_at;
                clearance.continue_from(old);
            }
            return;
        }
        self.substeps = other.substeps;
        self.clearance_interval = other.clearance_interval;
        for (rate, swing) in self.rates.iter_mut().zip(&self.swings) {
            *rate = Rate::new(swing.drag, self.substeps);
        }
        self.tails.clone_from(&other.tails);
        self.solved.clone_from(&other.solved);
        self.torso_sides.clone_from(&other.torso_sides);
        self.anchors_was.clone_from(&other.anchors_was);
        self.colliders_now.clone_from(&other.colliders_now);
        self.colliders_was.clone_from(&other.colliders_was);
        self.centers_was.clone_from(&other.centers_was);
        self.set_tuning(other.tuning);
        self.world = other.world;
        self.world_rotation = other.world_rotation;
        self.origin = other.origin;
        self.drawn_at = other.drawn_at;
        self.index = other.index;
        self.drawn_stride = other.drawn_stride;
        self.shown_at = other.shown_at;
        self.pace = other.pace;
        self.floor = other.floor;
        self.still_for = 0;
        self.settling = false;
        self.resting = false;
        self.targets_stale = true;
        if let (Some(clearance), Some(old)) = (&mut self.clearance, &other.clearance) {
            clearance.continue_from(old);
        }
    }

    /// Whether it ran on the last update, rather than sleeping or waiting to settle.
    #[must_use]
    pub fn is_running(&self) -> bool {
        !self.settling
    }

    /// Whether an update at `ticks`, with the character at `world` and a pose that didn't move, would
    /// do anything. A frame that evaluated nothing asks this before giving the character a job.
    #[must_use]
    pub fn is_due(&self, ticks: f64, world: &Affine3A) -> bool {
        if self.pace == SpringPace::Wait {
            // Waiting draws its chains on toward the last step, and nothing more.
            return self.settling || (!self.resting && ticks > self.shown_at);
        }
        self.settling || (ticks > self.drawn_at && (!self.resting || *world != self.world))
    }

    /// Stops running until active again, when it settles from the pose.
    pub fn sleep(&mut self) {
        self.settling = true;
        self.resting = false;
        self.still_for = 0;
        if let Some(clearance) = &mut self.clearance {
            clearance.sleep();
        }
    }

    /// The one presentation-only morph owned by the loaded underarm clearance.
    pub fn underarm_morph(&self) -> Option<(&'static str, usize)> {
        self.clearance.as_ref()?.underarm_morph()
    }

    /// A raw spring or clearance write that would overlap the bound contact closure.
    pub fn protected_write(&self, protected: &[bool]) -> Option<(&'static str, usize)> {
        let conflicts = |joint| protected.get(joint) != Some(&false);
        self.swings
            .iter()
            .map(|swing| usize::from(swing.joint))
            .find(|&joint| conflicts(joint))
            .map(|joint| ("spring swing", joint))
            .or_else(|| self.clearance.as_ref()?.protected_write(conflicts))
    }

    /// Runs the sub-steps due at `frame`, writes the chains' joints into `pose` and finishes it again.
    /// Call it after the pose's [`CharacterPose::finish`] on a frame that evaluated one. Allocates
    /// nothing.
    pub fn update(&mut self, pose: &mut CharacterPose, frame: &SpringFrame) -> SpringWork {
        self.update_with_torso_contact(pose, frame, 0.0)
    }

    /// Runs springs with additional torso contact as a grounded body falls onto its hair or clothing.
    /// At full contact the torso has priority over the motion limits. A weight of zero uses the
    /// ordinary action solve, including the limits that keep upright hair and clothing in shape.
    pub fn update_with_torso_contact(
        &mut self,
        pose: &mut CharacterPose,
        frame: &SpringFrame,
        torso_contact: f32,
    ) -> SpringWork {
        let torso_contact = torso_contact.clamp(0.0, 1.0);
        if !frame.active {
            self.sleep();
            self.targets_stale |= frame.pose_moved;
            return SpringWork::default();
        }
        let read = frame.pose_moved || self.targets_stale;
        if read {
            self.read_pose(pose);
        }
        let world_moved = frame.world_from_model != self.world;
        if self.settling {
            let wake_elapsed = (frame.ticks - self.drawn_at).max(0.0) as f32;
            self.settle(pose, frame);
            if let Some(clearance) = &mut self.clearance {
                if self.pace == SpringPace::Lead {
                    clearance.settle(pose, wake_elapsed);
                } else {
                    clearance.update(pose, frame.ticks, 0.0, None);
                }
            }
            return SpringWork { substeps: 0, wrote_pose: true };
        }
        let ticks = frame.ticks.max(self.drawn_at);
        if self.pace == SpringPace::Wait {
            return self.wait(pose, ticks, read);
        }
        let moved_waiting = std::mem::take(&mut self.moved_waiting);
        let pose_moved = read || moved_waiting;
        let last = self.index_at(ticks);
        if !pose_moved && !world_moved && (self.resting || ticks == self.drawn_at) {
            self.index = last;
            self.drawn_at = ticks;
            return SpringWork::default();
        }
        self.resting = false;
        self.place_anchors(frame, world_moved, pose_moved);
        self.place_floor(&frame.world_from_model);

        let due = (last - self.index).clamp(0, MAX_SUBSTEPS_PER_FRAME);
        let span = ticks - self.drawn_at;
        let count = f64::from(self.substeps.count());
        let drawn_at = self.drawn_at;
        let between = |step: i64| {
            let between = if span > 0.0 { ((step as f64 / count - drawn_at) / span) as f32 } else { 1.0 };
            between.clamp(0.0, 1.0)
        };
        let mut fastest = 0.0_f32;
        if self.pace == SpringPace::Lead {
            for step in (last - due + 1)..=last {
                fastest = fastest.max(self.substep(pose, between(step), torso_contact));
            }
            self.drawn_stride = 1;
        } else if due > 0 {
            // One step over the time it waited, in as few strides as the longest allows, as even as they
            // can be.
            let longest = i64::from(LONGEST_STRIDE * self.substeps.count());
            let strides = (due + longest - 1) / longest;
            let mut end = last - due;
            for stride in 0..strides {
                let length = due / strides + i64::from(stride < due % strides);
                end += length;
                let at = between(end);
                fastest = fastest.max(self.stride(pose, length as u32, at, torso_contact));
                self.drawn_stride = length as u32;
            }
        }
        self.index = last;
        self.drawn_at = ticks;
        self.anchors_was.copy_from_slice(&self.anchors_now);
        if let Some(cloth) = &mut self.cloth {
            cloth.end_frame();
        }

        self.note_stillness(due as u32, fastest, pose_moved || world_moved);
        self.draw(pose, ticks);
        if let Some(clearance) = &mut self.clearance {
            match self.pace {
                SpringPace::Lead => clearance.update(pose, ticks, span, self.clearance_interval),
                _ => clearance.update(pose, ticks, 0.0, None),
            }
        }
        SpringWork { substeps: due as u32, wrote_pose: true }
    }

    /// A frame waiting its turn: nothing steps, and the chains keep their turns from their parents, drawn
    /// on toward the last step's, so they ride with the body however it moves. `read` says the pose held
    /// a new evaluation, which the next step must place again since this one writes over it.
    fn wait(&mut self, pose: &mut CharacterPose, ticks: f64, read: bool) -> SpringWork {
        self.moved_waiting |= read;
        if !read && (self.resting || ticks <= self.shown_at) {
            return SpringWork::default();
        }
        self.draw(pose, ticks);
        if let Some(clearance) = &mut self.clearance {
            clearance.update(pose, ticks, 0.0, None);
        }
        SpringWork { substeps: 0, wrote_pose: true }
    }

    /// One step `length` sub-steps long, ending `between` of the way from the last frame to this one:
    /// a sub-step with its rates and its tails' motion stretched to cover them.
    fn stride(&mut self, pose: &CharacterPose, length: u32, between: f32, torso_contact: f32) -> f32 {
        if length <= 1 {
            return self.substep(pose, between, torso_contact);
        }
        let scale = length as f32;
        for tail in &mut self.tails {
            tail.before = tail.now - (tail.now - tail.before) * scale;
        }
        for (rate, swing) in self.stride_rates.iter_mut().zip(&self.swings) {
            *rate = Rate::over(swing.drag, self.substeps, length);
        }
        self.stride = length;
        let fastest = self.substep(pose, between, torso_contact);
        self.stride = 1;
        for tail in &mut self.tails {
            tail.before = tail.now - (tail.now - tail.before) / scale;
        }
        // Its motion as a sub-step's, which is what stillness measures.
        fastest / (scale * scale)
    }

    /// Swing `at`'s rate over the step running.
    fn rate(&self, at: usize) -> Rate {
        if self.stride > 1 { self.stride_rates[at] } else { self.rates[at] }
    }

    /// The floor this frame in the simulation's space, for a character at `world_from_model`.
    fn place_floor(&mut self, world_from_model: &Affine3A) {
        self.plane = self.floor.map(|floor| {
            let world = Frame::of(world_from_model);
            let normal = (world.rotation * floor.normal).normalize_or(Vec3::Y);
            let point = world.rotation * (floor.point * world.scale);
            (normal, normal.dot(point))
        });
    }

    fn index_at(&self, ticks: f64) -> i64 {
        (ticks * f64::from(self.substeps.count())).floor() as i64
    }

    /// Takes what it reads from a pose nothing has written over.
    fn read_pose(&mut self, pose: &CharacterPose) {
        for (target, swing) in self.targets.iter_mut().zip(&self.swings) {
            *target = pose.locals()[usize::from(swing.joint)].rotation;
        }
        for (frame, &joint) in self.model_anchors.iter_mut().zip(&self.anchors) {
            *frame = if joint == NO_PARENT {
                Frame::IDENTITY
            } else {
                Frame::of(&pose.model()[usize::from(joint)])
            };
        }
        if let Some(mut cloth) = self.cloth.take() {
            cloth.read_pose(self, pose.model());
            self.cloth = Some(cloth);
        }
        self.targets_stale = false;
    }

    /// Where the pose and the character put the anchors this frame, moving the simulation's space
    /// with the character.
    fn place_anchors(&mut self, frame: &SpringFrame, world_moved: bool, pose_moved: bool) {
        if !(world_moved || pose_moved) {
            return;
        }
        let world = Frame::of(&frame.world_from_model);
        if world_moved {
            // Everything from before keeps to the world, but for what the chains are carried with.
            let shift = (world.position - self.origin) * self.tuning.root_inertia;
            let carried = if world.rotation == self.world_rotation {
                Quat::IDENTITY
            } else {
                let turn = world.rotation * self.world_rotation.inverse();
                Quat::IDENTITY.lerp(turn, 1.0 - self.tuning.root_inertia)
            };
            let carry = |point: Vec3| carried * (point - shift);
            for tail in &mut self.tails {
                tail.now = carry(tail.now);
                tail.before = carry(tail.before);
            }
            for frame in self.anchors_was.iter_mut().chain(&mut self.centers_was) {
                frame.position = carry(frame.position);
                frame.rotation = carried * frame.rotation;
            }
            for placed in self.colliders_now.iter_mut().chain(&mut self.colliders_was) {
                placed.start = carry(placed.start);
                placed.axis = carried * placed.axis;
            }
            self.origin = world.position;
            self.world_rotation = world.rotation;
            self.world = frame.world_from_model;
            if let Some(cloth) = &mut self.cloth {
                cloth.scale = world.scale;
            }
        }
        for (now, model) in self.anchors_now.iter_mut().zip(&self.model_anchors) {
            *now = model.in_world(&world);
        }
    }

    /// Back to the pose, with no motion.
    fn settle(&mut self, pose: &mut CharacterPose, frame: &SpringFrame) {
        self.settling = false;
        self.torso_sides.fill(0.0);
        self.resting = false;
        self.still_for = 0;
        let world = Frame::of(&frame.world_from_model);
        self.world = frame.world_from_model;
        self.world_rotation = world.rotation;
        self.origin = world.position;
        self.drawn_at = frame.ticks;
        self.shown_at = frame.ticks;
        self.drawn_stride = 1;
        self.moved_waiting = false;
        self.index = self.index_at(frame.ticks);
        for (now, model) in self.anchors_now.iter_mut().zip(&self.model_anchors) {
            *now = model.in_world(&world);
        }
        self.anchors_was.copy_from_slice(&self.anchors_now);
        if let Some(cloth) = &mut self.cloth {
            cloth.scale = world.scale;
            cloth.end_frame();
        }
        let anchors = self.anchors.len();
        self.slots[..anchors].copy_from_slice(&self.anchors_now);
        place_colliders(&self.colliders, &self.slots, &mut self.colliders_now);
        self.colliders_was.copy_from_slice(&self.colliders_now);
        for (was, center) in self.centers_was.iter_mut().zip(&self.centers) {
            *was = self.slots[usize::from(center.slot)];
        }
        for (index, step) in self.steps.iter().enumerate() {
            let parent = self.slots[usize::from(step.parent)];
            let local = &pose.locals()[usize::from(step.joint)];
            let position = parent.position + parent.rotation * (local.translation * parent.scale);
            let scale = parent.scale * local.scale.x;
            let rotation = if step.swing == NONE {
                parent.rotation * local.rotation
            } else {
                let swing = usize::from(step.swing);
                let target = self.targets[swing];
                let rotation = parent.rotation * target;
                let tail =
                    position + rotation * self.swings[swing].tail * (self.swings[swing].length * scale);
                self.tails[swing] = Tail { now: tail, before: tail };
                self.solved[swing] = [target; 2];
                rotation
            };
            self.slots[anchors + index] = Frame { rotation, position, scale };
        }
        for (swing, target) in self.swings.iter().zip(&self.targets) {
            pose.locals_mut()[usize::from(swing.joint)].rotation = *target;
        }
        pose.finish();
    }

    /// One sub-step, `between` of the way from the last frame's pose and place to this frame's.
    /// Returns the square of the fastest tail's motion.
    fn substep(&mut self, pose: &CharacterPose, between: f32, torso_contact: f32) -> f32 {
        let anchors = self.anchors.len();
        // Out of the way while the springs step, so they can read it.
        let mut cloth = self.cloth.take();
        let solving = cloth.as_ref().is_some_and(|cloth| cloth.is_solving());
        if let Some(cloth) = &mut cloth
            && solving
        {
            cloth.begin(&self.tails);
        }
        for ((slot, was), now) in self.slots.iter_mut().zip(&self.anchors_was).zip(&self.anchors_now) {
            *slot = was.lerp(now, between);
        }
        std::mem::swap(&mut self.colliders_was, &mut self.colliders_now);
        place_colliders(&self.colliders, &self.slots, &mut self.colliders_now);
        for ((was, carry), center) in
            self.centers_was.iter_mut().zip(&mut self.center_turns).zip(&self.centers)
        {
            let now = self.slots[usize::from(center.slot)];
            let turn = (now.rotation * was.rotation.inverse()).normalize();
            *carry = if center.carried >= 1.0 {
                Carry { turn, from: was.position, to: now.position }
            } else {
                Carry {
                    turn: Quat::IDENTITY.lerp(turn, center.carried),
                    from: was.position,
                    to: was.position.lerp(now.position, center.carried),
                }
            };
            *was = now;
        }

        let mut fastest = 0.0_f32;
        let locals = pose.locals();
        for (index, step) in self.steps.iter().enumerate() {
            let parent = self.slots[usize::from(step.parent)];
            let local = &locals[usize::from(step.joint)];
            let position = parent.position + parent.rotation * (local.translation * parent.scale);
            let scale = parent.scale * local.scale.x;
            let parent_slot = usize::from(step.parent);
            let parent_posed =
                if parent_slot < anchors { parent.rotation } else { self.posed[parent_slot - anchors] };
            let rotation = if step.swing == NONE {
                self.posed[index] = parent_posed * local.rotation;
                parent.rotation * local.rotation
            } else {
                let at = usize::from(step.swing);
                let swing = &self.swings[at];
                let Rate { keep, force, carry, keep_end, force_end } = self.rate(at);
                let mut tail = self.tails[at];
                let center = usize::from(swing.center);
                if self.centers[center].carried > 0.0 {
                    let Carry { turn, from, to } = self.center_turns[center];
                    tail.now = to + turn * (tail.now - from);
                    tail.before = to + turn * (tail.before - from);
                }
                let pose_rotation = parent.rotation * self.targets[at];
                let toward_pose = pose_rotation * swing.tail;
                let posed = parent_posed * self.targets[at];
                self.posed[index] = posed;
                let length = swing.length * scale;
                let velocity = tail.now - tail.before;
                let push = toward_pose * swing.stiffness + swing.gravity;
                let moved = tail.now + velocity * keep + push * force;
                let next = position + (moved - position).try_normalize().unwrap_or(toward_pose) * length;
                let (first, end) = swing.colliders;
                let hit_radius = swing.hit_radius * scale;
                let bone = Bone { head: position, length, hit_radius, fallback: toward_pose };
                let next = self.collide(swing.colliders, &self.colliders_was, &bone, tail.now, next);
                // Bound the dynamics before the final pose's surface contact. The swing goes first
                // and the bend last to protect the curve of the strand.
                let aim = (next - position).try_normalize().unwrap_or(toward_pose);
                let mut aim = self.bend_cone.hold(self.swing_cone.hold(aim, posed * swing.tail), toward_pose);
                let mut next = position + aim * length;
                // A grounded torso may rotate through the space its hair needs to settle.
                // Give its colliders a final pass without the upright overrun concessions.
                if torso_contact > 0.0 {
                    let mut clear = next;
                    for &collider in &self.collider_lists[first as usize..end as usize] {
                        let collider = usize::from(collider);
                        if !self.colliders[collider].torso {
                            continue;
                        }
                        let was = &self.colliders_was[collider];
                        let now = &self.colliders_now[collider];
                        let reach = now.radius + hit_radius;
                        // There is little space behind the torso on the ground. Clear each strand
                        // toward the side where its joint sits, so it can spread beside the body. A
                        // joint near the torso's middle keeps the side it had, or it would flip
                        // with every rounding of a dot product near zero.
                        let lateral = self.world_rotation * Vec3::X;
                        let offset = (position - now.nearest(position)).dot(lateral);
                        let sign = torso_side(self.torso_sides[at], offset, TORSO_SIDE_HOLD * scale);
                        self.torso_sides[at] = sign;
                        let aside = |at| now.nearest(at) + lateral * (sign * reach);
                        if contact(tail.now, clear, was, now, hit_radius).is_some() {
                            clear = clear_of(position, length, aside(clear), now, reach, toward_pose);
                        }
                        // A curved strand can cross the torso between two clear endpoints.
                        let midpoint = position.lerp(clear, 0.5);
                        if contact(midpoint, midpoint, now, now, hit_radius).is_some() {
                            let clear_mid =
                                clear_of(position, length * 0.5, aside(midpoint), now, reach, toward_pose);
                            clear = position + (clear_mid - position) * 2.0;
                        }
                    }
                    if clear != next {
                        let blended = next.lerp(clear, torso_contact) - position;
                        aim = blended.try_normalize().unwrap_or(aim);
                        next = position + aim * length;
                    }
                }
                // The ground has the last word: no limit holds a tail in the paving.
                if let Some((up, level)) = self.plane {
                    let held = on_floor(up, level, &bone, tail.now, next);
                    if held != next {
                        aim = (held - position).try_normalize().unwrap_or(aim);
                        next = position + aim * length;
                    }
                }
                // A tail the cloth moves is measured once the cloth has.
                if !(solving && cloth.as_ref().is_some_and(|cloth| cloth.moves(at))) {
                    fastest = fastest.max((next - tail.now).length_squared());
                }
                let rotation = Quat::from_rotation_arc(toward_pose, aim) * pose_rotation;
                let turned = (parent.rotation.inverse() * rotation).normalize();
                // What the bone and the colliders did counts toward its motion by this sub-step's share.
                let motion = if self.stride > 1 {
                    // A stride keeps the motion its last sub-step would end on.
                    velocity * keep_end + push * force_end + (next - moved) * carry
                } else {
                    (moved - tail.now) + (next - moved) * carry
                };
                self.tails[at] = Tail { now: next, before: next - motion };
                self.solved[at] = [self.solved[at][1], turned];
                rotation
            };
            self.slots[anchors + index] = Frame { rotation, position, scale };
        }
        if let Some(mut cloth) = cloth {
            if solving {
                fastest = fastest.max(self.solve_cloth(&mut cloth, locals, between));
            }
            self.cloth = Some(cloth);
        }
        fastest
    }

    /// Where a tail going from `from` to `next` over a sub-step belongs against its chain's
    /// `colliders`, which went from `was` to where they are now. See the module docs' "Colliders".
    fn collide(
        &self,
        colliders: (u32, u32),
        was: &[Placed],
        bone: &Bone,
        from: Vec3,
        mut next: Vec3,
    ) -> Vec3 {
        let Bone { head, length, hit_radius, fallback } = *bone;
        let (first, end) = colliders;
        for &collider in &self.collider_lists[first as usize..end as usize] {
            let was = &was[usize::from(collider)];
            let now = &self.colliders_now[usize::from(collider)];
            if let Some(out) = contact(from, next, was, now, hit_radius) {
                let reach = now.radius + hit_radius;
                let (out, reach) = if self.limited {
                    // Likewise a bone whose own head it has overrun: nothing it can do reads.
                    let began = from.distance(was.nearest(from));
                    let overrun = head.distance(now.nearest(head));
                    let give = overrun_give(began / reach) * overrun_give(overrun / reach);
                    (next.lerp(out, give), began + (reach - began).max(0.0) * give)
                } else {
                    (out, reach)
                };
                next = clear_of(head, length, out, now, reach, fallback);
            }
        }
        match self.plane {
            Some((up, level)) => on_floor(up, level, bone, from, next),
            None => next,
        }
    }

    /// Whether every tail has stopped for long enough, with nothing moving them.
    fn note_stillness(&mut self, substeps: u32, fastest: f32, moved: bool) {
        let tick = 1.0 / SPRING_TICK_HZ as f32;
        let still = STILL_SPEED * tick / self.substeps.count() as f32;
        if moved || fastest > still * still {
            self.still_for = 0;
        } else {
            self.still_for += substeps;
        }
        if self.still_for >= STILL_TICKS * self.substeps.count() {
            self.resting = true;
            for solved in &mut self.solved {
                solved[0] = solved[1];
            }
        }
    }

    /// Writes each swing's turn at `ticks` into `pose`, and finishes it: one step behind, across the last
    /// step, which for a character taking turns is as long as it waited.
    fn draw(&mut self, pose: &mut CharacterPose, ticks: f64) {
        self.shown_at = ticks;
        let count = f64::from(self.substeps.count());
        let along =
            ((ticks * count - self.index as f64) / f64::from(self.drawn_stride)).clamp(0.0, 1.0) as f32;
        let locals = pose.locals_mut();
        for (swing, [before, last]) in self.swings.iter().zip(&self.solved) {
            locals[usize::from(swing.joint)].rotation = before.lerp(*last, along);
        }
        pose.finish();
    }
}

/// The share of a center's motion the chains it carries take.
fn carried(whole: bool, tuning: &SpringTuning) -> f32 {
    if whole { 1.0 } else { 1.0 - tuning.anchor_inertia }
}

/// Places every collider at its joint's slot.
fn place_colliders(colliders: &[Collider], slots: &[Frame], placed: &mut [Placed]) {
    for (placed, collider) in placed.iter_mut().zip(colliders) {
        let slot = &slots[usize::from(collider.anchor)];
        *placed = Placed {
            start: slot.position + slot.rotation * (collider.offset * slot.scale),
            axis: slot.rotation * (collider.axis * slot.scale),
            radius: collider.radius * slot.scale,
        };
    }
}

/// How many parents `joint` has.
fn depth(pose: &CharacterPose, joint: u16) -> u32 {
    let mut count = 0;
    let mut at = usize::from(joint);
    while let Some(parent) = pose.parent(at) {
        count += 1;
        at = parent;
    }
    count
}

/// The skeleton's rest pose in the model's space.
fn rest_model(pose: &CharacterPose) -> Vec<Affine3A> {
    let mut model = vec![Affine3A::IDENTITY; pose.joint_count()];
    for joint in 0..pose.joint_count() {
        let parent = pose.parent(joint).map_or(NO_PARENT, |parent| parent as u16);
        model[joint] = model_of(parent, &pose.rest()[joint], &model);
    }
    model
}

const _: () = {
    const fn holds_across_threads<T: Send + Sync>() {}
    holds_across_threads::<SpringBones>();
};

#[cfg(test)]
mod tests;
