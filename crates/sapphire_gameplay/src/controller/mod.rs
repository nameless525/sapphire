//! The character controller (spec §5.22): a kinematic capsule that walks, runs, jumps, falls, slides,
//! takes steps, comes off ledges and lands, built on `sapphire_physics`' `move_and_collide` and
//! `cast_body`.
//!
//! It has two faces. [`step_controller`] is one character for one tick: plain values in, plain values
//! out and no ECS, which is what the scenario tests drive. [`CharacterController`] is the component
//! every pooled actor carries, and `move_characters` runs it in
//! [`CombatSet::Movement`](crate::CombatSet) on each actor's own clock, in ID order.
//!
//! # A move's root motion: the seam
//!
//! A move's motion for a tick is a world-space delta, which `movement::apply_root_motion` puts in
//! [`CharacterController::drive`] just before `move_characters` runs. The controller moves by it,
//! colliding as usual, so a lunge or a warp stops at a wall, at any speed:
//!
//! ```
//! # use sapphire_gameplay::{CharacterController, ControllerDrive};
//! # use sapphire_math::Vec3;
//! # let mut controller = CharacterController::default();
//! # let delta = Vec3::X;
//! controller.drive = ControllerDrive::strike(delta);
//! ```
//!
//! [`ControllerDrive::locomotion`] and [`ControllerDrive::gravity`] say whether the stick and gravity
//! still apply. A drive lasts one tick, so one not renewed hands control back to the stick. Yaw from a
//! table goes straight on [`ActorTransform::yaw`], and the controller turns the body to match.
//!
//! # The combat movement mode
//!
//! [`MovementMode::Combat`] is what a grounded strike runs in, tick by tick, as the move decides:
//!
//! - **No step-ups,** the spec's point: a lunge meeting a curb stops or slides along it, rather than
//!   popping up and back down in mid-swing. A curb low enough for the capsule's round foot to roll
//!   over (under about 12 cm with the default profile) is still rolled over, smoothly.
//! - **No leaving a ledge.** A move that would carry the character off stops where its tick began, so
//!   a strike's active frames can't turn into a fall. Over ground that only drops a little, the feet
//!   still follow.
//!
//! # Time
//!
//! Everything runs on the owner's clock: motion by its last delta, coyote time in its sub-ticks. A
//! frozen owner doesn't move and doesn't accumulate, and velocity integrates exactly, so a jump at half
//! speed reaches the same height. A jump pressed in a freeze waits for the clock, as what's in an intent's
//! ring does.
//!
//! # Where the numbers come from
//!
//! A fighter's document says how it moves, in its `movement` section, and [`MoveLibrary`] holds what its
//! current revision works out to ([`ResolvedProfile`]). A step reads that first, and an actor whose
//! document has no section reads the [`ProfileId`] it spawned with. So a save reaches the next step
//! ([`crate::Simulation::tune_move_set`]): nothing about that actor's state changes for it, so a jump in
//! the air finishes its arc under the new gravity, unless that's so light the arc would leave the world
//! ([`ResolvedProfile::carried`]), and only the capsule is touched, around the same feet. An overlap that
//! leaves it is what a spawn inside a wall gets, a push on its next move. Both real documents write out
//! today's numbers. `sapphire_moves`' `MOVEMENT_LIMITS` says what a document may hold.
//!
//! # Who owns position
//!
//! For an actor with a controller the world's body is the truth, and [`ActorTransform::position`] is
//! written from it each tick: its feet, a skin under the capsule, so they're exactly on flat ground.
//! On a slope of angle θ the capsule's round foot leaves them `radius × (1 / cos θ − 1)` over the
//! surface, which presentation closes from [`ControllerState::ground_normal`] (M2 N3's foot IK).
//!
//! # Doesn't own
//!
//! Animation, the camera, the move system, hit detection, or pushing other characters: capsules block
//! each other, and that's all. Another capsule underfoot isn't ground, so a character slides off one, or
//! the stick walks it off the gentle top where there's no slope to slide on, or a press jumps from there.

mod profile;
mod step;

use std::time::Duration;

use sapphire_ecs::prelude::*;
use sapphire_math::{Vec2, Vec3};
use sapphire_physics::{BodyId, PhysicsWorld};
use sapphire_time::{SimDuration, Stopwatch};

pub use profile::ResolvedProfile;
pub use sapphire_moves::{ControllerProfile, ProfileError};
pub use step::step_controller;

use crate::actions::{Buttons, Controls};
use crate::actors::{ActorId, ActorTable};
use crate::checksum::StateHasher;
use crate::hits::remove_hurtboxes;
use crate::movement::ActorTransform;
use crate::moves::{MoveLibrary, MoveRunner};
use crate::party::Party;

pub(crate) const STANDING_JUMP_PREPARATION: SimDuration = SimDuration::from_ticks(5);
pub(crate) const MOVING_JUMP_PREPARATION: SimDuration = SimDuration::from_ticks(2);
pub(crate) const REBOUND_JUMP_PREPARATION: SimDuration = SimDuration::from_ticks(3);
pub(crate) const JUMP_REBOUND_WINDOW: SimDuration = SimDuration::from_ticks(6);
pub(crate) const MOVING_JUMP_MIN_SPEED: f32 = 1.2;
pub(crate) const MOVING_JUMP_MIN_ALIGNMENT: f32 = 0.8;
pub(crate) const REBOUND_MIN_LANDING_SPEED: f32 = 2.0;
pub(crate) const STANDING_JUMP_MAX_SPEED: f32 = 0.15;

/// Which [`ControllerProfile`] an actor runs on, handed out in the order they were added.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProfileId(u32);

impl ProfileId {
    /// This ID's number, as checksums and tools show it.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// The body an actor's capsule is, in the world. Other bodies an app or a later package adds to the
/// same world, such as hurtboxes, pick IDs clear of these.
#[must_use]
pub const fn actor_body(id: ActorId) -> BodyId {
    BodyId(id.raw())
}

/// What's under a character.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroundMode {
    /// On ground it can stand on: the feet follow it, and the stick accelerates over it.
    Grounded,
    /// On ground too steep to stand on: gravity takes it down, and the stick only steers it across.
    Sliding,
    /// Nothing under it. What a spawn starts as, and its first tick looks.
    #[default]
    Airborne,
}

impl GroundMode {
    fn code(self) -> u8 {
        match self {
            Self::Grounded => 0,
            Self::Sliding => 1,
            Self::Airborne => 2,
        }
    }
}

/// How a character takes what it meets: the `controller` module's docs say why.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MovementMode {
    /// Takes steps and walks off ledges.
    #[default]
    Free,
    /// Takes no steps, and stays on the ledge it's on.
    Combat,
}

impl MovementMode {
    fn code(self) -> u8 {
        match self {
            Self::Free => 0,
            Self::Combat => 1,
        }
    }
}

/// What a move asks of the controller for one tick: the seam a move system writes to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControllerDrive {
    /// How far the move carries the character this tick, in world space, colliding as usual: the
    /// difference between two readings of its table, turned by the yaw it started at. While grounded
    /// with gravity on, only its horizontal part counts, because the floor decides the height. With
    /// gravity off, an upward part lifts the character off the ground.
    pub root_motion: Vec3,
    /// Whether the stick still moves and turns the character. Off, what velocity it has is dropped
    /// (except on a slide, which the slope drives), so the table is all the motion and its distance
    /// is exact.
    pub locomotion: bool,
    /// Whether gravity pulls and the feet follow the ground. Off, the character holds its height but
    /// for the table, as a leap or an air move wants.
    pub gravity: bool,
    /// Whether this tick takes steps and leaves ledges.
    pub mode: MovementMode,
    /// Jump this tick if the character can, though no press came: how a buffered intent fires.
    pub jump: bool,
    /// Turn toward this yaw this tick, at the profile's turn rate, instead of toward the stick: walking
    /// one way while facing another, as an enemy circling its target does.
    pub face: Option<f32>,
}

impl ControllerDrive {
    /// No move running: the stick and gravity have it.
    pub const NONE: Self = Self {
        root_motion: Vec3::ZERO,
        locomotion: true,
        gravity: true,
        mode: MovementMode::Free,
        jump: false,
        face: None,
    };

    /// A strike on the ground: the table's motion alone, kept to the floor in the combat mode.
    #[must_use]
    pub const fn strike(root_motion: Vec3) -> Self {
        Self {
            root_motion,
            locomotion: false,
            gravity: true,
            mode: MovementMode::Combat,
            jump: false,
            face: None,
        }
    }

    /// A move that owns the character's height, such as a leap or an air attack.
    #[must_use]
    pub const fn flight(root_motion: Vec3) -> Self {
        Self {
            root_motion,
            locomotion: false,
            gravity: false,
            mode: MovementMode::Combat,
            jump: false,
            face: None,
        }
    }

    fn write_canonical(&self, hasher: &mut StateHasher) {
        let Self { root_motion, locomotion, gravity, mode, jump, face } = *self;
        hasher.vec3(root_motion);
        hasher.u8(u8::from(locomotion));
        hasher.u8(u8::from(gravity));
        hasher.u8(mode.code());
        hasher.u8(u8::from(jump));
        match face {
            Some(face) => {
                hasher.u8(1);
                hasher.f32(face);
            }
            None => hasher.u8(0),
        }
    }
}

impl Default for ControllerDrive {
    fn default() -> Self {
        Self::NONE
    }
}

/// What [`step_controller`] takes for one tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ControllerInput {
    /// World-space: x along world +X, y along world +Z. Longer than 1 counts as 1.
    pub stick: Vec2,
    /// Jump now, if the character can.
    pub jump: bool,
    pub drive: ControllerDrive,
}

/// What happened in a character's latest tick, for presentation and the move system. Cleared each
/// tick, a frozen one too.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ControllerEvents {
    pub landed: bool,
    /// Downward speed at a landing, in m/s. A spawn's first tick lands at about one tick of gravity.
    pub landing_speed: f32,
    pub jumped: bool,
    /// Walked off a ledge, went onto a slope too steep, or a table lifted it.
    pub left_ground: bool,
    pub stepped_up: bool,
    /// How far the feet went this tick by a step-up (up) or by following the ground (down), in metres,
    /// beyond their motion: what presentation may smooth on the pelvis and the camera.
    pub height_pop: f32,
    /// The combat mode stopped a move at a ledge.
    pub ledge_held: bool,
    /// The capsule began inside something and was pushed out: a spawn inside a wall, say.
    pub pushed_out: bool,
}

/// All a controller carries from tick to tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControllerState {
    pub mode: GroundMode,
    /// In m/s. Horizontal while grounded, whatever the slope: a tick lays it along the ground.
    pub velocity: Vec3,
    /// The surface under the character while grounded or sliding, and otherwise the last one.
    pub ground_normal: Vec3,
    /// The owner's time left in which a jump still works after walking off a ledge.
    pub coyote: SimDuration,
    /// A jump was pressed while the owner's clock stood still, and goes when it moves.
    pub jump_waiting: bool,
    /// Remaining owner time before a supported jump, when this actor opts in.
    pub(crate) jump_preparation: Option<SimDuration>,
    pub(crate) jump_preparation_moving: bool,
    pub(crate) jump_rebound: SimDuration,
    pub events: ControllerEvents,
}

impl Default for ControllerState {
    fn default() -> Self {
        Self {
            mode: GroundMode::default(),
            velocity: Vec3::ZERO,
            ground_normal: Vec3::Y,
            coyote: SimDuration::ZERO,
            jump_waiting: false,
            jump_preparation: None,
            jump_preparation_moving: false,
            jump_rebound: SimDuration::ZERO,
            events: ControllerEvents::default(),
        }
    }
}

impl ControllerState {
    /// Whether a stationary jump is preparing on a supporting surface.
    #[must_use]
    pub fn preparing_jump(self) -> bool {
        self.mode == GroundMode::Grounded
            && !self.jump_preparation_moving
            && self.jump_preparation.is_some_and(|time| time != SimDuration::ZERO)
    }

    /// Where a traveling jump is between loading the support (0) and pushing off (1).
    #[must_use]
    pub fn moving_jump_preparation(self) -> Option<f32> {
        if self.mode != GroundMode::Grounded || !self.jump_preparation_moving {
            return None;
        }
        self.jump_preparation.filter(|&remaining| remaining != SimDuration::ZERO).map(|remaining| {
            ((MOVING_JUMP_PREPARATION - remaining).as_subticks() as f32
                / SimDuration::from_ticks(1).as_subticks() as f32)
                .min(1.0)
        })
    }

    /// Written field by field, destructured so a new field can't be forgotten.
    fn write_canonical(&self, hasher: &mut StateHasher) {
        let Self {
            mode,
            velocity,
            ground_normal,
            coyote,
            jump_waiting,
            jump_preparation,
            jump_preparation_moving,
            jump_rebound,
            events,
        } = *self;
        hasher.u8(mode.code());
        hasher.vec3(velocity);
        hasher.vec3(ground_normal);
        hasher.u64(coyote.as_subticks());
        // Nothing unless it's set, as a party writes nothing without one: only a run that presses jump in
        // a freeze has this to say, and every other keeps its checksums.
        if jump_waiting {
            hasher.u8(1);
        }
        let ControllerEvents {
            landed,
            landing_speed,
            jumped,
            left_ground,
            stepped_up,
            height_pop,
            ledge_held,
            pushed_out,
        } = events;
        for flag in [landed, jumped, left_ground, stepped_up, ledge_held, pushed_out] {
            hasher.u8(u8::from(flag));
        }
        hasher.f32(landing_speed);
        hasher.f32(height_pop);
        // Only the actors that opt in carry the grounded jump policy.
        if let Some(remaining) = jump_preparation {
            hasher.u8(1);
            hasher.u64(remaining.as_subticks());
            hasher.u8(u8::from(jump_preparation_moving));
            hasher.u64(jump_rebound.as_subticks());
        }
    }
}

/// An actor's character controller. Every pooled actor carries one, and one with no profile does
/// nothing: its transform is someone else's to move. What it moves by is its set's `movement` section,
/// if its document has one, and otherwise the profile it spawned with.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct CharacterController {
    profile: Option<ProfileId>,
    /// The state as of its latest tick.
    pub state: ControllerState,
    /// What the running move asks for this tick. `move_characters` takes it, leaving
    /// [`ControllerDrive::NONE`].
    pub drive: ControllerDrive,
}

impl CharacterController {
    /// Whether this actor has a capsule in the world.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.profile.is_some()
    }

    /// The profile it runs on.
    #[must_use]
    pub const fn profile(&self) -> Option<ProfileId> {
        self.profile
    }

    pub(crate) fn write_canonical(&self, hasher: &mut StateHasher) {
        let Self { profile, state, drive } = self;
        hasher.u32(profile.map_or(u32::MAX, ProfileId::raw));
        state.write_canonical(hasher);
        drive.write_canonical(hasher);
    }
}

/// The world a simulation's characters move in, with their profiles.
#[derive(Resource)]
pub struct CharacterWorld {
    physics: PhysicsWorld,
    profiles: Vec<ResolvedProfile>,
    /// See [`crate::SimStats::controllers_refused`].
    pub(crate) refused: u64,
    /// Wall time of the latest step's `move_characters`. Performance only: it never reaches state.
    pub(crate) last_time: Duration,
}

impl CharacterWorld {
    pub(crate) fn new(physics: PhysicsWorld) -> Self {
        Self { physics, profiles: Vec::new(), refused: 0, last_time: Duration::ZERO }
    }

    /// The collision world, for queries: cameras and hit tests read this.
    #[must_use]
    pub const fn physics(&self) -> &PhysicsWorld {
        &self.physics
    }

    /// The collision world, for what else lives in it: hurtboxes, set in
    /// [`CombatSet::Movement`](crate::CombatSet::Movement) after `move_characters`. A character's
    /// capsule ([`actor_body`]) is `move_characters`' to move.
    pub const fn physics_mut(&mut self) -> &mut PhysicsWorld {
        &mut self.physics
    }

    pub(crate) fn add_profile(&mut self, profile: &ControllerProfile) -> Result<ProfileId, ProfileError> {
        let resolved = ResolvedProfile::new(profile)?;
        let id = ProfileId(u32::try_from(self.profiles.len()).expect("fewer than 2³² profiles"));
        self.profiles.push(resolved);
        Ok(id)
    }

    /// The profile behind an ID.
    #[must_use]
    pub fn profile(&self, id: ProfileId) -> Option<&ResolvedProfile> {
        self.profiles.get(id.0 as usize)
    }

    /// Hashes every profile, in ID order, as a replay records move data.
    pub(crate) fn profiles_hash(&self) -> u64 {
        let mut hasher = StateHasher::new();
        hasher.u32(self.profiles.len() as u32);
        for profile in &self.profiles {
            profile::write_canonical(profile.authored(), &mut hasher);
        }
        hasher.finish()
    }

    /// Replaces the collision world, keeping the profiles. Bodies live in it, so this can only come
    /// before any has been added.
    pub(crate) fn replace_physics(
        &mut self,
        physics: PhysicsWorld,
        needed: usize,
    ) -> Result<(), PhysicsWorldError> {
        if self.physics.body_count() != 0 {
            return Err(PhysicsWorldError::BodiesExist);
        }
        if physics.body_capacity() < needed {
            return Err(PhysicsWorldError::TooSmall { capacity: physics.body_capacity(), needed });
        }
        self.physics = physics;
        Ok(())
    }

    /// Gives a newly spawned actor its capsule, or leaves it without a controller if it can't have
    /// one, and counts that: a spawn inside a step has nowhere to report to. `movement` is what its
    /// document says it moves by, if it says.
    pub(crate) fn spawned(
        &mut self,
        id: ActorId,
        profile: Option<ProfileId>,
        movement: Option<&ResolvedProfile>,
        transform: &ActorTransform,
    ) -> CharacterController {
        let Some(profile) = profile else { return CharacterController::default() };
        let added = movement
            .or_else(|| self.profile(profile))
            .map(|resolved| resolved.body(actor_body(id), transform.position, transform.yaw));
        if !added.is_some_and(|body| self.physics.add_body(body).is_ok()) {
            self.refused += 1;
            return CharacterController::default();
        }
        CharacterController { profile: Some(profile), ..CharacterController::default() }
    }

    /// Fits an actor's capsule to what a live edit made of its numbers, around feet that stay exactly
    /// where they are. Something it now overlaps pushes it out when it next moves, as a spawn inside a
    /// wall does, by no more than it grew. One with no body, off the field, gets a new one as it comes on.
    pub(crate) fn refit_body(
        &mut self,
        id: ActorId,
        was: &ResolvedProfile,
        now: &ResolvedProfile,
        transform: &ActorTransform,
    ) {
        let body = actor_body(id);
        if now.same_body(was) || !self.physics.remove_body(body) {
            return;
        }
        // It has just made room, and a resolved profile's capsule is one the world takes.
        let added = self.physics.add_body(now.body(body, transform.position, transform.yaw));
        debug_assert!(added.is_ok(), "{added:?}");
    }

    /// Takes an actor's bodies out of the world: its capsule and any hurtboxes, whose IDs nothing will
    /// use again, so leaving them would fill the world.
    pub(crate) fn despawned(&mut self, id: ActorId) {
        self.physics.remove_body(actor_body(id));
        remove_hurtboxes(&mut self.physics, id);
    }
}

/// A collision world a simulation can't take.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PhysicsWorldError {
    #[error("actors with controllers already have bodies in the current world")]
    BodiesExist,
    #[error("the world holds {capacity} bodies, and the actor pool needs {needed}")]
    TooSmall { capacity: usize, needed: usize },
}

/// [`crate::CombatSet::Movement`], after whatever writes [`CharacterController::drive`]: every
/// character in ID order, on its own clock. One moved earlier in the tick is where a later one's casts
/// find it.
pub(crate) fn move_characters(
    table: Res<'_, ActorTable>,
    party: Res<'_, Party>,
    library: Res<'_, MoveLibrary>,
    mut characters: ResMut<'_, CharacterWorld>,
    mut actors: Query<
        '_,
        '_,
        (&Controls, &EntityClock, &MoveRunner, &mut CharacterController, &mut ActorTransform),
    >,
) {
    let stopwatch = Stopwatch::start();
    let characters = &mut *characters;
    for (id, entity) in table.iter().filter(|&(id, _)| !party.is_off_field(id)) {
        let Ok((controls, clock, runner, mut controller, mut transform)) = actors.get_mut(entity) else {
            continue;
        };
        let Some(spawned) = controller.profile.and_then(|id| characters.profiles.get(id.0 as usize)) else {
            continue;
        };
        let profile = runner.set().and_then(|set| library.movement(set)).unwrap_or(spawned);
        let controller = &mut *controller;
        let drive = std::mem::take(&mut controller.drive);
        let pressed_jump = drive.locomotion && controls.pressed().contains(Buttons::JUMP);
        let input = ControllerInput {
            stick: controls.current.stick.to_vec2(),
            jump: pressed_jump || drive.jump,
            drive,
        };
        // A body comes and goes with its actor, and only spheres and capsules move, so this has no
        // way to fail.
        let stepped = step_controller(
            &mut characters.physics,
            actor_body(id),
            &mut transform,
            &mut controller.state,
            profile,
            &input,
            clock.0.last_delta(),
        );
        debug_assert!(stepped.is_ok(), "{stepped:?}");
    }
    characters.last_time = stopwatch.elapsed();
}

#[cfg(test)]
mod tests;
