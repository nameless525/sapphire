//! An optional simulation-owned body writer. It publishes a pre-hit pair for presentation and for
//! the separate, explicitly bound overhead contact route.

mod package;

pub use package::{
    BodyAssets, BodyBindError, BodyGraphAssets, BodyProfile, BodyProfileOptions, HeadAuthority,
};
pub use sapphire_body_pose::{BodySample, BodyTuning, MotionHistory, MotionObservation};

use bevy_ecs::system::SystemState;
use sapphire_body_pose::{
    AuthoredFootMotion, BodyRoot, BodyStep, CombatInput, FootContact, FootSettings, GraphInput,
    GraphStepStamp, GroundHit, LookInput,
};
use sapphire_ecs::prelude::*;
use sapphire_math::{Affine3A, Vec3, det};
use sapphire_physics::{Group, PhysicsWorld, QueryFilter};
use sapphire_time::SUBTICKS_PER_TICK;

use crate::actors::ActorTable;
use crate::simulation::view;
use crate::{ActorId, ActorView, CharacterWorld, GroundMode, MoveLibrary, StateHasher, Stick};

/// Converts controller state once into the common pose observation.
#[must_use]
pub fn observe_motion(view: &ActorView) -> MotionObservation {
    let state = &view.controller.state;
    MotionObservation {
        velocity: state.velocity,
        yaw: view.yaw,
        grounded: state.mode != GroundMode::Airborne,
        preparing_jump: state.preparing_jump(),
        moving_jump_preparation: state.moving_jump_preparation(),
        jumped: state.events.jumped,
        left_ground: state.events.left_ground,
        landed: state.events.landed,
    }
}

/// Casts the same scenery-only ground probe as the legacy presentation IK.
#[must_use]
pub fn ground_under(world: &PhysicsWorld, at: Vec3, settings: &FootSettings) -> Option<GroundHit> {
    let filter = QueryFilter::new(Group::World.mask());
    let from = at + Vec3::Y * settings.reach_up;
    let hit = world.cast_ray(from, Vec3::NEG_Y, settings.reach_up + settings.reach_down, &filter)?;
    (hit.distance > 0.0).then_some(GroundHit { point: hit.point, normal: hit.normal })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BodyAttachError {
    #[error("bind an authoritative body only at tick zero")]
    Started,
    #[error("this actor already has an authoritative body")]
    Bound,
    #[error("the body actor or its look target is not live")]
    Actor,
    #[error("the body was bound on other feet and look numbers than the simulation's table")]
    Tuning,
    #[error("the simulation's table changes only before its first step and its first body")]
    TableInUse,
}

/// The table a simulation's bodies solve their feet and look by, [`BodyTuning::shipped`] until told
/// otherwise. Every simulation holds one, and its `tables_hash` takes it, bodies or none, as it takes
/// targeting's weights: whatever a game's document says reaches every replay's hash the same way.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub(crate) struct BodyTable(pub(crate) BodyTuning);

#[derive(Resource, Default)]
pub(crate) struct BodyAuthority {
    owners: Vec<BodyOwner>,
}

impl BodyAuthority {
    pub(crate) fn insert(&mut self, actor: ActorId, target: Option<ActorId>, profile: BodyProfile) {
        let index = self.index(actor).expect_err("an actor can only bind once");
        self.owners.insert(index, BodyOwner::new(actor, target, profile));
    }

    fn index(&self, actor: ActorId) -> Result<usize, usize> {
        self.owners.binary_search_by_key(&actor, |owner| owner.actor)
    }

    pub(crate) fn contains(&self, actor: ActorId) -> bool {
        self.index(actor).is_ok()
    }

    pub(crate) fn restart(&mut self) {
        for owner in &mut self.owners {
            owner.restart();
        }
    }

    pub(crate) fn cut(&mut self, actor: ActorId) {
        if let Ok(index) = self.index(actor) {
            self.owners[index].cut();
        }
    }

    pub(crate) fn sample_buffer(&self, actor: ActorId) -> Option<BodySample> {
        Some(self.owners[self.index(actor).ok()?].profile.owner.sample_buffer())
    }

    pub(crate) fn read(&self, actor: ActorId, fraction: f32, sample: &mut BodySample) -> bool {
        self.index(actor).is_ok_and(|index| self.owners[index].read(fraction, sample))
    }

    pub(crate) fn motion_bound(
        &self,
        actor: ActorId,
        joint: usize,
        radius: f32,
    ) -> Result<sapphire_body_pose::BodyMotionBound, sapphire_body_pose::BodyMotionError> {
        let index = self.index(actor).map_err(|_| sapphire_body_pose::BodyMotionError::Unavailable)?;
        let owner = &self.owners[index];
        if !owner.readable {
            return Err(sapphire_body_pose::BodyMotionError::Unavailable);
        }
        owner.profile.owner.motion_bound(joint, radius)
    }

    pub(crate) fn stamp(&self, actor: ActorId) -> Option<GraphStepStamp> {
        let owner = &self.owners[self.index(actor).ok()?];
        if !owner.readable {
            return None;
        }
        owner.profile.owner.endpoint().map(|(stamp, _)| stamp)
    }

    pub(crate) fn receipt(&self, actor: ActorId) -> Option<(GraphStepStamp, FootContact, u64)> {
        self.owners[self.index(actor).ok()?].receipt()
    }

    pub(crate) fn write_identity(&self, hash: &mut StateHasher) {
        if self.owners.len() > 1 {
            hash.bytes(b"sapphire.pre-resolve-bodies.v1");
            hash.u32(self.owners.len() as u32);
        }
        for owner in &self.owners {
            owner.write_identity(hash);
        }
    }

    pub(crate) fn write_state(&self, hash: &mut StateHasher) {
        if self.owners.len() > 1 {
            hash.bytes(b"sapphire.pre-resolve-bodies.v1");
            hash.u32(self.owners.len() as u32);
        }
        for owner in &self.owners {
            owner.write_state(hash);
        }
    }
}

struct BodyOwner {
    actor: ActorId,
    target: Option<ActorId>,
    profile: BodyProfile,
    cursor: MotionHistory,
    generation: u32,
    pending_cut: bool,
    readable: bool,
}

impl BodyOwner {
    fn new(actor: ActorId, target: Option<ActorId>, profile: BodyProfile) -> Self {
        Self {
            actor,
            target,
            profile,
            cursor: MotionHistory::fixed(),
            generation: 0,
            pending_cut: true,
            readable: false,
        }
    }

    pub(crate) fn restart(&mut self) {
        self.generation = 0;
        self.cursor.clear();
        self.pending_cut = true;
        self.readable = false;
    }

    fn cut(&mut self) {
        if !self.pending_cut {
            self.generation = self.generation.wrapping_add(1);
            self.cursor.clear();
            self.pending_cut = true;
            self.readable = false;
        }
    }

    fn read(&self, fraction: f32, sample: &mut BodySample) -> bool {
        self.readable && self.profile.owner.read(fraction, sample)
    }

    fn receipt(&self) -> Option<(GraphStepStamp, FootContact, u64)> {
        if !self.readable {
            return None;
        }
        let (stamp, contact) = self.profile.owner.endpoint()?;
        let mut hash = StateHasher::new();
        self.write_state(&mut hash);
        Some((stamp, contact, hash.finish()))
    }

    pub(crate) fn write_identity(&self, hash: &mut StateHasher) {
        hash.bytes(b"sapphire.pre-resolve-body.v1");
        hash.u64(self.profile.fingerprint());
        hash.u32(self.actor.raw());
        hash.u32(self.target.map_or(0, ActorId::raw));
    }

    pub(crate) fn write_state(&self, hash: &mut StateHasher) {
        self.write_identity(hash);
        hash.u32(self.generation);
        hash.u8(u8::from(self.pending_cut));
        hash.u8(u8::from(self.readable));
        self.cursor.write_canonical(&mut |bytes| hash.bytes(bytes));
        if self.readable {
            self.profile.owner.write_canonical(&mut |bytes| hash.bytes(bytes));
        }
    }

    fn observe(&self, world: &World) -> Option<(ActorView, Option<Vec3>)> {
        let table = world.resource::<ActorTable>();
        let actor = view(world, self.actor, table.entity(self.actor)?);
        if actor.off_field {
            return None;
        }
        let look_target = self
            .target
            .and_then(|id| table.entity(id).map(|entity| view(world, id, entity)))
            .filter(|target| !target.off_field && !target.fighter.is_defeated())
            .map(|target| target.position + Vec3::Y * self.profile.look_height);
        Some((actor, look_target))
    }

    fn advance(
        &mut self,
        step: u64,
        observation: Option<(ActorView, Option<Vec3>)>,
        moves: &MoveLibrary,
        ground: &PhysicsWorld,
    ) {
        let Some((actor, look_target)) = observation else {
            self.cut();
            return;
        };
        let sample = observe_motion(&actor);
        self.cursor.observe(sample);
        let locomotion = self.cursor.input(sample);
        let combat = actor.moves.current.map(|current| CombatInput {
            name: current.name,
            serial: current.serial,
            position: current.elapsed.as_subticks() as f64 / f64::from(SUBTICKS_PER_TICK),
        });
        let defeated = actor.fighter.is_defeated();
        let fallback_contact = if !locomotion.grounded || defeated {
            FootContact::Sole
        } else if combat.is_none() {
            FootContact::RollingStance
        } else {
            authored_foot_motion(&actor, moves, self.profile.scale)
                .map_or(FootContact::Sole, FootContact::Authored)
        };
        let step = BodyStep {
            stamp: GraphStepStamp { step, owner_now: actor.clock_now, generation: self.generation },
            graph: GraphInput { locomotion, combat, defeated, ..GraphInput::default() },
            root: BodyRoot { translation: actor.position, yaw: actor.yaw, scale: self.profile.scale },
            movement_released: self.profile.grounded_gather && actor.controls.current.stick == Stick::CENTER,
            fallback_contact,
            look: LookInput { target: look_target, weight: self.profile.look_weight },
            weapon: None,
        };
        let probe = |at, settings: &FootSettings| ground_under(ground, at, settings);
        let result = if self.pending_cut {
            self.profile.owner.advance_after_cut(step, probe)
        } else {
            self.profile.owner.advance(step, probe)
        };
        if result.is_ok() {
            self.readable = true;
            self.pending_cut = false;
        } else {
            // Withhold the sample. A later valid actor state starts a fresh pair.
            self.cut();
            self.cursor.clear();
        }
    }
}

fn authored_foot_motion(actor: &ActorView, moves: &MoveLibrary, scale: f32) -> Option<AuthoredFootMotion> {
    let current = actor.moves.current?;
    let motion = moves.get(current.at)?;
    if !motion.warps.iter().any(|warp| warp.max_distance > 0.0) {
        return None;
    }
    let root = motion.tables.root_at(current.elapsed);
    Some(AuthoredFootMotion {
        serial: current.serial,
        source_from_model: Affine3A::from_scale_rotation_translation(
            Vec3::splat(scale),
            det::quat_from_rotation_y(root.yaw),
            root.offset(),
        ),
    })
}

pub(crate) type AdvanceParams<'w, 's> = ParamSet<
    'w,
    's,
    (
        &'static World,
        (ResMut<'static, BodyAuthority>, Res<'static, MoveLibrary>, Res<'static, CharacterWorld>),
    ),
>;

/// Runs after controller movement and before the hit sweep. The resource is absent in legacy fights.
pub(crate) fn advance(world: &mut World, cache: &mut SystemState<AdvanceParams<'_, '_>>) {
    if !world.contains_resource::<BodyAuthority>() {
        return;
    }
    // Keep this system exclusive, but borrow resources instead of temporarily removing them.
    let mut inputs = cache.get_mut(world).expect("the body writer is installed with its resources");
    let (step, count) = {
        let world = inputs.p0();
        (world.resource::<WorldClock>().0.ticks(), world.resource::<BodyAuthority>().owners.len())
    };
    for index in 0..count {
        let observation = {
            let world = inputs.p0();
            world.resource::<BodyAuthority>().owners[index].observe(world)
        };
        let (mut authority, moves, characters) = inputs.p1();
        authority.owners[index].advance(step, observation, &moves, characters.physics());
    }
}

#[cfg(test)]
mod tests;
