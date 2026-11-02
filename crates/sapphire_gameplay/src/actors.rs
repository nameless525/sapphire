//! Actors: stable IDs over a pool of entities spawned ahead of time.
//!
//! Spawning an entity allocates, and a new archetype grows every query's cache (spec §5.6, rule R6),
//! so the fixed step never does either. [`crate::Simulation::new`] spawns the whole pool once, each
//! entity with every component an actor can have. A spawn takes a free entity and resets it, and
//! a despawn gives it back, with no structural change to the world.
//!
//! Which entity an actor gets depends on what died before it, so an entity says nothing about
//! the order of anything. What does is the [`ActorId`]: handed out in spawn order, never reused
//! within a run, and what [`ActorTable::iter`] goes in.
//!
//! Spawns and despawns asked for during a step wait in [`ActorRequests`], and resolve at the step's
//! end, in `FixedPost` (rule R3). An actor spawned during a step first acts in the next one. A spawn
//! gets its ID and its entity at once, so a pool that's full refuses it there and then.

use std::num::NonZeroU32;

use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_moves::MoveSet;
use sapphire_time::{SimDuration, SimTime};

use crate::actions::Controls;
use crate::brain::Brain;
use crate::controller::{CharacterController, CharacterWorld, ProfileId};
use crate::hits::{HitQuery, MAX_HURTBOXES};
use crate::intents::IntentRing;
use crate::movement::ActorTransform;
use crate::moves::{MoveLibrary, MoveOutput, MoveRead, MoveRunner, MoveSetId, WarpTarget};
use crate::resolve::Fighter;

/// How many bodies a pool of `max_actors` can put in a world: each actor's capsule, all its hurtboxes
/// and its perfect-dodge volume.
pub(crate) const fn body_capacity(max_actors: u32) -> usize {
    max_actors as usize * (2 + MAX_HURTBOXES as usize)
}

/// An actor's stable identity (the explicit ID review RS-005 left for M2). Handed out in spawn order
/// from 1, and never reused within a run, so a replay's actions and a kept reference can't land
/// on whatever spawned later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActorId(NonZeroU32);

impl ActorId {
    /// The ID with this number, or `None` for 0, which no actor has.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match NonZeroU32::new(raw) {
            Some(raw) => Some(Self(raw)),
            None => None,
        }
    }

    /// This ID's number.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0.get()
    }
}

/// What a new actor starts with.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActorSpawn {
    pub position: Vec3,
    /// Radians about +Y; 0 faces +Z.
    pub yaw: f32,
    /// The set of moves it plays. With none, it stands where it is.
    pub moves: Option<MoveSetId>,
    /// Gives the actor a character controller: a capsule in the world, standing at
    /// [`Self::position`]. See [`crate::Simulation::add_controller_profile`].
    pub controller: Option<ProfileId>,
}

/// A spawn that couldn't be done. Reaching one is deterministic, so it replays too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SpawnError {
    #[error("the actor pool is full: {capacity} actors, counting those queued this step")]
    PoolFull { capacity: u32 },
    #[error("this run has used every actor ID")]
    OutOfIds,
}

/// The live actors, in ID order. Read this to go through actors in an order that can't depend on
/// the world's internals; see the [crate docs](crate).
#[derive(Resource, Debug)]
pub struct ActorTable {
    /// Sorted: IDs only grow, so a new actor goes at the end, and a removal shifts the rest down.
    live: Vec<(ActorId, Entity)>,
}

impl ActorTable {
    fn with_capacity(capacity: usize) -> Self {
        Self { live: Vec::with_capacity(capacity) }
    }

    /// Each live actor and its entity, in ID order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (ActorId, Entity)> + '_ {
        self.live.iter().copied()
    }

    /// How many actors are alive.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live.len()
    }

    /// Whether no actor is alive.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// The entity of a live actor.
    #[must_use]
    pub fn entity(&self, id: ActorId) -> Option<Entity> {
        let index = self.live.binary_search_by_key(&id, |&(id, _)| id).ok()?;
        Some(self.live[index].1)
    }
}

/// A queued spawn, with what it was promised.
#[derive(Clone, Copy, Debug)]
struct PendingSpawn {
    id: ActorId,
    entity: Entity,
    spawn: ActorSpawn,
}

/// The queue of spawns and despawns, resolved at the end of each step. Systems take this rather
/// than touching other actors (rule R3). Its vectors never grow: the pool's size bounds them all.
#[derive(Resource, Debug)]
pub struct ActorRequests {
    capacity: u32,
    next_id: u32,
    /// Entities not in use. Which one comes out next is arbitrary, and nothing may depend on it.
    free: Vec<Entity>,
    spawns: Vec<PendingSpawn>,
    despawns: Vec<ActorId>,
    refused: u64,
}

impl ActorRequests {
    fn with_pool(pool: Vec<Entity>) -> Self {
        let capacity = pool.len();
        Self {
            capacity: u32::try_from(capacity).unwrap_or(u32::MAX),
            next_id: 1,
            free: pool,
            spawns: Vec::with_capacity(capacity),
            despawns: Vec::with_capacity(capacity),
            refused: 0,
        }
    }

    /// Queues a spawn and returns the new actor's ID. It acts from the next step on. Takes an entity
    /// from the pool now, so a full pool refuses here, and counts the refusal
    /// ([`crate::SimStats::spawns_refused`]).
    pub fn spawn(&mut self, spawn: ActorSpawn) -> Result<ActorId, SpawnError> {
        let result = self.reserve(spawn);
        if result.is_err() {
            self.refused += 1;
        }
        result
    }

    fn reserve(&mut self, spawn: ActorSpawn) -> Result<ActorId, SpawnError> {
        let id = ActorId::from_raw(self.next_id).ok_or(SpawnError::OutOfIds)?;
        let entity = self.free.pop().ok_or(SpawnError::PoolFull { capacity: self.capacity })?;
        // Wraps to 0, which no actor can have, after the last ID.
        self.next_id = self.next_id.wrapping_add(1);
        self.spawns.push(PendingSpawn { id, entity, spawn });
        Ok(id)
    }

    /// Queues a live actor's despawn, at this step's end. A second request for one actor does nothing,
    /// and neither does one for an actor that's already gone or not yet spawned: those return
    /// `false`.
    pub fn despawn(&mut self, id: ActorId, table: &ActorTable) -> bool {
        if table.entity(id).is_none() || self.despawns.contains(&id) {
            return false;
        }
        self.despawns.push(id);
        true
    }

    /// The next spawn's ID number: every actor ever spawned has a lower one.
    #[must_use]
    pub fn next_id(&self) -> u32 {
        self.next_id
    }

    /// Spawns refused so far.
    #[must_use]
    pub fn refused(&self) -> u64 {
        self.refused
    }

    /// The pool's size.
    #[must_use]
    pub fn capacity(&self) -> u32 {
        self.capacity
    }
}

/// Everything a pooled entity carries. A component a later package adds to actors goes here and in
/// [`reset_actor`], so every actor has it from the pool on (spec §5.6).
#[derive(Bundle)]
struct ActorBundle {
    clock: EntityClock,
    controls: Controls,
    transform: ActorTransform,
    runner: MoveRunner,
    ring: IntentRing,
    output: MoveOutput,
    target: WarpTarget,
    controller: CharacterController,
    fighter: Fighter,
    brain: Brain,
}

/// Spawns the pool and creates the actor resources, before the world's first step.
pub(crate) fn create_pool(world: &mut World, capacity: u32) {
    let mut pool = Vec::with_capacity(capacity as usize);
    for _ in 0..capacity {
        let clock = world.resource_mut::<WorldClock>().new_entity_clock();
        let entity = world.spawn(ActorBundle {
            clock,
            controls: Controls::default(),
            transform: ActorTransform::default(),
            runner: MoveRunner::default(),
            ring: IntentRing::default(),
            output: MoveOutput::default(),
            target: WarpTarget::default(),
            controller: CharacterController::default(),
            fighter: Fighter::default(),
            brain: Brain::default(),
        });
        pool.push(entity.id());
    }
    world.insert_resource(ActorTable::with_capacity(capacity as usize));
    world.insert_resource(ActorRequests::with_pool(pool));
}

/// The components [`reset_actor`] writes.
pub(crate) type ActorState<'w> = (
    &'w mut EntityClock,
    &'w mut Controls,
    &'w mut ActorTransform,
    &'w mut MoveRunner,
    &'w mut IntentRing,
    &'w mut MoveOutput,
    &'w mut WarpTarget,
    &'w mut CharacterController,
    &'w mut Fighter,
    &'w mut Brain,
);

/// What a query for [`ActorState`] hands over.
type ActorStateItem<'w> = (
    Mut<'w, EntityClock>,
    Mut<'w, Controls>,
    Mut<'w, ActorTransform>,
    Mut<'w, MoveRunner>,
    Mut<'w, IntentRing>,
    Mut<'w, MoveOutput>,
    Mut<'w, WarpTarget>,
    Mut<'w, CharacterController>,
    Mut<'w, Fighter>,
    Mut<'w, Brain>,
);

/// `FixedPost`, and straight after a spawn or despawn between steps: resolves everything queued, in
/// the order it was asked for.
pub(crate) fn resolve_actor_requests(
    mut requests: ResMut<'_, ActorRequests>,
    mut table: ResMut<'_, ActorTable>,
    mut world_clock: ResMut<'_, WorldClock>,
    library: Res<'_, MoveLibrary>,
    mut hits: ResMut<'_, HitQuery>,
    mut actors: Query<'_, '_, ActorState<'_>>,
    mut characters: ResMut<'_, CharacterWorld>,
) {
    let requests = &mut *requests;
    for id in requests.despawns.drain(..) {
        if let Ok(index) = table.live.binary_search_by_key(&id, |&(id, _)| id) {
            let (_, entity) = table.live.remove(index);
            requests.free.push(entity);
            characters.despawned(id);
            hits.forget_actor(id);
        }
    }
    for PendingSpawn { id, entity, spawn } in requests.spawns.drain(..) {
        if let Ok(actor) = actors.get_mut(entity) {
            reset_actor(actor, id, spawn, &mut world_clock, &mut characters, &library);
            table.live.push((id, entity));
        }
    }
}

/// Makes a pooled entity a new actor. Clock IDs come from the world clock in spawn order, so a replay
/// gets the same ones (and a reused entity's old scale handles can't touch its new clock).
fn reset_actor(
    (
        mut clock,
        mut controls,
        mut transform,
        mut runner,
        mut ring,
        mut output,
        mut target,
        mut controller,
        mut fighter,
        mut brain,
    ): ActorStateItem<'_>,
    id: ActorId,
    spawn: ActorSpawn,
    world_clock: &mut WorldClock,
    characters: &mut CharacterWorld,
    library: &MoveLibrary,
) {
    *clock = world_clock.new_entity_clock();
    *controls = Controls::default();
    *transform = ActorTransform { position: spawn.position, yaw: spawn.yaw };
    *runner = MoveRunner::new(spawn.moves);
    ring.reset();
    *output = MoveOutput::default();
    *target = WarpTarget::None;
    *controller = characters.spawned(id, spawn.controller, &transform);
    let def = spawn.moves.and_then(|set| library.current(set)).and_then(MoveSet::fighter);
    *fighter = def.map_or_else(Fighter::default, Fighter::new);
    *brain = Brain::default();
}

/// A live actor's public state, read between steps. Presentation interpolates between two of
/// these; see [`crate::Simulation::actor`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActorView {
    pub id: ActorId,
    pub position: Vec3,
    /// Radians about +Y, unwrapped.
    pub yaw: f32,
    /// This actor's own time, which hitstop stops and time scales slow.
    pub clock_now: SimTime,
    /// How far that time moved in the last step: zero while frozen.
    pub clock_delta: SimDuration,
    /// Frozen ticks still to come.
    pub hitstop_ticks: u16,
    /// What its moves are doing: the move, its phase and local time, and what counts this step.
    pub moves: MoveRead,
    /// Its character controller, inactive if it has none.
    pub controller: CharacterController,
    /// Its health and poise.
    pub fighter: Fighter,
    /// What's thinking for it, if anything: see [`crate::Simulation::give_brain`].
    pub brain: Brain,
}
