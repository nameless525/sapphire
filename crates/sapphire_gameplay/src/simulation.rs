//! [`Simulation`]: one combat world, stepped one tick at a time.

use std::time::Duration;

use bevy_ecs::schedule::{IntoScheduleConfigs, SystemSet};
use bevy_ecs::system::ScheduleSystem;
use bevy_ecs::system::SystemState;
use sapphire_core::Name;
use sapphire_core::alloc::AllocScope;
use sapphire_ecs::prelude::*;
use sapphire_math::Pcg32;
use sapphire_moves::{MoveSet, MoveSetError};
use sapphire_physics::PhysicsWorld;
use sapphire_time::Stopwatch;

use crate::actions::{CurrentActions, TickActions, read_actions};
use crate::actors::{
    self, ActorId, ActorRequests, ActorSpawn, ActorState, ActorTable, ActorView, SpawnError,
};
use crate::checksum;
use crate::controller::{
    self, CharacterController, CharacterWorld, ControllerProfile, PhysicsWorldError, ProfileError, ProfileId,
};
use crate::cues::{CueEvent, CueQueue};
use crate::hits::HitQuery;
use crate::intents::{self, IntentRing};
use crate::movement::{self, ActorTransform};
use crate::moves::{self, MoveLibrary, MoveOutput, MoveRead, MoveRunner, MoveSetId, MoveStats, WarpTarget};
use crate::stats::{BUDGET_DESKTOP_LOW, PHYSICS_BUDGET_DESKTOP_LOW, SimStats};

/// [`Pcg32`]'s sequence for combat's numbers (spec §5.4: seeded per subsystem). A subsystem that
/// draws its own takes another, so one's draws can't move another's.
const COMBAT_STREAM: u64 = 1;

/// Why [`SystemState::get_mut`] succeeds: [`Simulation::new`] inserts what it reads, and nothing
/// takes it away.
const ACTORS_EXIST: &str = "the actor resources exist from the moment the simulation does";

/// What a simulation starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimConfig {
    /// Seeds all randomness: a replay records it.
    pub seed: u64,
    /// The pool's size: most actors alive at once, counting those queued to spawn. A duel has few;
    /// projectiles and effects that can hit count too. Pooling them all up front is what keeps a
    /// spawn in the fixed step from allocating.
    pub max_actors: u32,
    /// What a step is held to in [`SimStats::over_budget_steps`]: the app gives its tier's, from
    /// [`BUDGET_DESKTOP_LOW`] or [`crate::BUDGET_MOBILE_LOW`].
    pub step_budget: Duration,
    /// What a step's physics is held to in [`SimStats::controllers_over_budget_steps`]: from
    /// [`PHYSICS_BUDGET_DESKTOP_LOW`] or [`crate::PHYSICS_BUDGET_MOBILE_LOW`]. It reaches no state, so
    /// a replay doesn't record it.
    pub physics_budget: Duration,
}

impl Default for SimConfig {
    /// Seed 0, room for 64 actors, Desktop-Low's budgets.
    fn default() -> Self {
        Self {
            seed: 0,
            max_actors: 64,
            step_budget: BUDGET_DESKTOP_LOW,
            physics_budget: PHYSICS_BUDGET_DESKTOP_LOW,
        }
    }
}

/// Combat's randomness (spec §5.4). No system reads the OS for numbers, and the state is in the
/// checksum.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimRng(pub Pcg32);

/// Spec §5.22's fixed-step order for combat, chained inside
/// [`FixedCombat`]. Each package's systems go in their step.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CombatSet {
    /// The intent ring: what actors want to do this tick.
    Intents,
    /// The move graph checks cancel windows.
    Cancels,
    /// Each move advances on its local clock.
    Moves,
    /// Root motion and the controller, then hurtboxes: `movement::apply_root_motion` writes each
    /// character's drive, `controller::move_characters` moves its capsule, and M2 G3's hurtboxes come
    /// third in that chain, because they read where the controller left the actor.
    Movement,
    /// Hit queries, queuing hit events.
    Hits,
    /// Attributes, effects and tags: damage, poise, stagger and reactions.
    Resolve,
    /// Hitstop and time scale, which take effect from the next tick's clocks.
    TimeScale,
    /// Cues, which are presentation only.
    Cues,
}

type ResolveParams = (
    ResMut<'static, ActorRequests>,
    ResMut<'static, ActorTable>,
    ResMut<'static, WorldClock>,
    Query<'static, 'static, ActorState<'static>>,
    ResMut<'static, CharacterWorld>,
);

/// One combat world with no window and no GPU. See the [crate docs](crate).
pub struct Simulation {
    ecs: Ecs,
    /// Resolves spawns and despawns asked for between steps.
    resolve: SystemState<ResolveParams>,
    stats: SimStats,
    physics_budget: Duration,
}

impl Simulation {
    /// A world at tick 0, its pool of actors spawned ahead of time and its schedules built, so the
    /// first step allocates no more than any other.
    #[must_use]
    pub fn new(config: SimConfig) -> Self {
        let mut ecs = Ecs::new();
        let world = ecs.world_mut();
        world.insert_resource(SimRng(Pcg32::new(config.seed, COMBAT_STREAM)));
        world.insert_resource(CurrentActions::default());
        world.insert_resource(MoveLibrary::default());
        world.insert_resource(MoveStats::default());
        world.insert_resource(CueQueue::default());
        world.insert_resource(HitQuery::new());
        let empty = PhysicsWorld::new(Vec::new(), actors::body_capacity(config.max_actors))
            .expect("a world with no colliders has nothing to refuse");
        world.insert_resource(CharacterWorld::new(empty));
        actors::create_pool(world, config.max_actors);

        world.schedule_scope(FixedCombat, |_, schedule| {
            schedule.configure_sets(
                (
                    CombatSet::Intents,
                    CombatSet::Cancels,
                    CombatSet::Moves,
                    CombatSet::Movement,
                    CombatSet::Hits,
                    CombatSet::Resolve,
                    CombatSet::TimeScale,
                    CombatSet::Cues,
                )
                    .chain(),
            );
        });
        ecs.add_systems(FixedFirst, read_actions.in_set(SimSet::Input))
            .add_systems(FixedCombat, intents::press_intents.in_set(CombatSet::Intents))
            .add_systems(FixedCombat, moves::settle_cancels.in_set(CombatSet::Cancels))
            .add_systems(FixedCombat, moves::advance_moves.in_set(CombatSet::Moves))
            .add_systems(
                FixedCombat,
                (movement::apply_root_motion, controller::move_characters)
                    .chain()
                    .in_set(CombatSet::Movement),
            )
            .add_systems(FixedPost, actors::resolve_actor_requests);
        ecs.startup();

        let resolve = SystemState::new(ecs.world_mut());
        let stats = SimStats { budget: config.step_budget, ..SimStats::default() };
        Self { ecs, resolve, stats, physics_budget: config.physics_budget }
    }

    /// Adds a system to a [`CombatSet`], for a world built to test the simulation itself, such as the
    /// one that shows replays' allocation gate bites. What a real fight runs goes in [`Self::new`].
    /// It rebuilds the schedule here, so that isn't paid inside the next step.
    pub fn add_combat_system<M>(
        &mut self,
        set: CombatSet,
        system: impl IntoScheduleConfigs<ScheduleSystem, M>,
    ) {
        self.ecs.add_systems(FixedCombat, system.in_set(set));
        self.ecs.world_mut().schedule_scope(FixedCombat, |world, schedule| {
            if let Err(error) = schedule.initialize(world) {
                panic!("the combat schedule doesn't build: {}", error.to_string(schedule.graph(), world));
            }
        });
    }

    /// Loads a set of moves, between steps. IDs count up from the first set added: a replay adds the
    /// same sets in the same order.
    pub fn add_move_set(&mut self, set: MoveSet) -> MoveSetId {
        self.ecs.world_mut().resource_mut::<MoveLibrary>().add(set)
    }

    /// [`Self::add_move_set`], from a cook's bytes, which must start 16-byte aligned.
    pub fn add_cooked_move_set(&mut self, cooked: &[u8]) -> Result<MoveSetId, MoveSetError> {
        Ok(self.add_move_set(MoveSet::from_cooked(cooked)?))
    }

    /// Replaces a loaded set, between steps, and returns the new revision's number, or `None` for an
    /// ID this simulation never gave. Moves already running finish on what they started with; the
    /// next move to start reads this (M2 T2). See the `moves` module.
    pub fn replace_move_set(&mut self, id: MoveSetId, set: MoveSet) -> Option<u32> {
        let world = self.ecs.world_mut();
        let in_use: Vec<u32> = world
            .resource::<ActorTable>()
            .iter()
            .filter_map(|(_, entity)| world.get::<MoveRunner>(entity)?.running)
            .filter(|running| running.at.set == id)
            .map(|running| running.at.revision)
            .collect();
        world.resource_mut::<MoveLibrary>().replace(id, set, |revision| in_use.contains(&revision))
    }

    /// The hash of the move data and controller profiles this simulation runs on, tables included, in
    /// ID order: what a replay records of them, to tell changed moves or tuning from a broken simulation.
    #[must_use]
    pub fn tables_hash(&self) -> u64 {
        let world = self.ecs.world();
        checksum::tables_hash(world.resource::<MoveLibrary>(), world.resource::<CharacterWorld>())
    }

    /// The loaded sets of moves.
    #[must_use]
    pub fn move_library(&self) -> &MoveLibrary {
        self.ecs.world().resource::<MoveLibrary>()
    }

    /// Gives characters a world to move in, between steps and before any has spawned: a new
    /// simulation's is empty. It needs room for [`Self::body_capacity`] bodies.
    pub fn set_physics_world(&mut self, physics: PhysicsWorld) -> Result<(), PhysicsWorldError> {
        let needed = self.body_capacity();
        self.ecs.world_mut().resource_mut::<CharacterWorld>().replace_physics(physics, needed)
    }

    /// How many bodies a world must have room for here: a capsule and every hurtbox for each actor in
    /// the pool, so a full cast is refused by nothing.
    #[must_use]
    pub fn body_capacity(&self) -> usize {
        actors::body_capacity(self.ecs.world().resource::<ActorRequests>().capacity())
    }

    /// The world characters move in.
    #[must_use]
    pub fn physics(&self) -> &PhysicsWorld {
        self.ecs.world().resource::<CharacterWorld>().physics()
    }

    /// Checks and loads a character's tuning, between steps. IDs count up from the first added: a
    /// replay adds the same ones in the same order.
    pub fn add_controller_profile(&mut self, profile: &ControllerProfile) -> Result<ProfileId, ProfileError> {
        self.ecs.world_mut().resource_mut::<CharacterWorld>().add_profile(profile)
    }

    /// The hash of every loaded controller profile, in ID order: with [`Self::tables_hash`], what a
    /// replay records to tell changed tuning from a broken simulation.
    #[must_use]
    pub fn controller_profiles_hash(&self) -> u64 {
        self.ecs.world().resource::<CharacterWorld>().profiles_hash()
    }

    /// Spawns an actor between steps, at once. Inside a step, systems queue spawns on
    /// [`ActorRequests`]. The same spawns in the same order give the same IDs.
    pub fn spawn(&mut self, spawn: ActorSpawn) -> Result<ActorId, SpawnError> {
        let id = self.ecs.world_mut().resource_mut::<ActorRequests>().spawn(spawn)?;
        self.resolve_requests();
        Ok(id)
    }

    /// Removes an actor between steps, at once, and returns whether it was alive. Its ID is never
    /// used again.
    pub fn despawn(&mut self, id: ActorId) -> bool {
        let (mut requests, table, clock, actors, characters) =
            self.resolve.get_mut(self.ecs.world_mut()).expect(ACTORS_EXIST);
        let alive = requests.despawn(id, &table);
        actors::resolve_actor_requests(requests, table, clock, actors, characters);
        alive
    }

    fn resolve_requests(&mut self) {
        let (requests, table, clock, actors, characters) =
            self.resolve.get_mut(self.ecs.world_mut()).expect(ACTORS_EXIST);
        actors::resolve_actor_requests(requests, table, clock, actors, characters);
    }

    /// Runs one fixed step on `actions`: the only way time moves. Allocates nothing on this thread
    /// (rule R6), tested from the first tick.
    pub fn step(&mut self, actions: &TickActions) {
        profiling::scope!("Simulation::step");
        let stopwatch = Stopwatch::start();
        let allocations = AllocScope::begin();

        let world = self.ecs.world_mut();
        world.resource_mut::<CurrentActions>().0 = *actions;
        world.resource_mut::<CueQueue>().clear();
        self.ecs.fixed_step();

        let allocated = allocations.counts().allocations;
        let elapsed = stopwatch.elapsed();
        self.record_step(elapsed, allocated);
    }

    fn record_step(&mut self, elapsed: Duration, allocated: u64) {
        let world = self.ecs.world();
        let actors = u32::try_from(world.resource::<ActorTable>().len()).unwrap_or(u32::MAX);
        let physics_budget = self.physics_budget;
        let stats = &mut self.stats;
        stats.steps += 1;
        stats.last_step = elapsed;
        stats.longest_step = stats.longest_step.max(elapsed);
        stats.total_step_time += elapsed;
        stats.over_budget_steps += u64::from(elapsed > stats.budget);
        stats.allocations += allocated;
        stats.actors = actors;
        stats.peak_actors = stats.peak_actors.max(actors);
        stats.spawns_refused = world.resource::<ActorRequests>().refused();
        let moves = world.resource::<MoveStats>();
        stats.intents_pressed = moves.intents_pressed;
        stats.intents_expired = moves.intents_expired;
        stats.intents_overwritten = moves.intents_overwritten;
        stats.moves_started = moves.moves_started;
        stats.cancels_fired = moves.cancels_fired;
        stats.volumes_dropped = moves.volumes_dropped;
        stats.transition_limit_steps = moves.transition_limit;
        let cues = world.resource::<CueQueue>();
        stats.cues_published = cues.published();
        stats.cues_dropped = cues.dropped();
        let characters = world.resource::<CharacterWorld>();
        stats.controllers_last = characters.last_time;
        stats.controllers_longest = stats.controllers_longest.max(characters.last_time);
        stats.controllers_over_budget_steps += u64::from(characters.last_time > physics_budget);
        stats.controllers_refused = characters.refused;
        stats.physics = characters.physics().take_stats();
    }

    /// The checksum of all simulated state: the same inputs give the same one on every run for an
    /// architecture (spec §9, gate 2). Reads the state, so it can be called at any tick, and
    /// allocates nothing.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        checksum::state_checksum(self.ecs.world())
    }

    /// Fixed steps run so far.
    #[must_use]
    pub fn tick(&self) -> u64 {
        self.ecs.world().resource::<WorldClock>().0.ticks()
    }

    /// A live actor's state.
    #[must_use]
    pub fn actor(&self, id: ActorId) -> Option<ActorView> {
        let world = self.ecs.world();
        let entity = world.resource::<ActorTable>().entity(id)?;
        Some(view(world, id, entity))
    }

    /// Every live actor's state, in ID order.
    pub fn actors(&self) -> impl ExactSizeIterator<Item = ActorView> + '_ {
        let world = self.ecs.world();
        world.resource::<ActorTable>().iter().map(|(id, entity)| view(world, id, entity))
    }

    /// What the latest step's moves published for presentation. E3 drains these after each step.
    pub fn cues(&self) -> impl ExactSizeIterator<Item = CueEvent> + '_ {
        self.ecs.world().resource::<CueQueue>().iter()
    }

    /// A live actor's buffered intents, as T1's view draws them.
    #[must_use]
    pub fn intents(&self, id: ActorId) -> Option<&IntentRing> {
        let world = self.ecs.world();
        world.get::<IntentRing>(world.resource::<ActorTable>().entity(id)?)
    }

    /// Presses an intent on a live actor without a button, between steps, as a test or a tool would.
    /// Returns whether the actor's set has it. A brain does the same from inside a step, in
    /// [`CombatSet::Intents`], with [`IntentRing::press`].
    pub fn press_intent(&mut self, id: ActorId, intent: Name) -> bool {
        let world = self.ecs.world_mut();
        let Some(entity) = world.resource::<ActorTable>().entity(id) else { return false };
        let Some(set) = world.get::<MoveRunner>(entity).and_then(MoveRunner::set) else { return false };
        let Some(intent) = world
            .resource::<MoveLibrary>()
            .current(set)
            .and_then(|set| set.intent(intent).map(|(_, intent)| intent.clone()))
        else {
            return false;
        };
        let clock = world.get::<EntityClock>(entity).expect("every pooled entity has a clock").0.clone();
        let mut ring = world.get_mut::<IntentRing>(entity).expect("every pooled entity has a ring");
        let overwrote = ring.press(&intent, &clock);
        let mut stats = world.resource_mut::<MoveStats>();
        stats.intents_pressed += 1;
        stats.intents_overwritten += u64::from(overwrote);
        true
    }

    /// Starts a move on a live actor on the next step, whatever it's doing: [`MoveRunner::force`].
    pub fn force_move(&mut self, id: ActorId, name: Name) -> bool {
        let world = self.ecs.world_mut();
        let Some(entity) = world.resource::<ActorTable>().entity(id) else { return false };
        world.get_mut::<MoveRunner>(entity).expect("every pooled entity has a runner").force(name);
        true
    }

    /// Gives a live actor something for warp windows to bend toward, between steps. Whoever does
    /// targeting writes the [`WarpTarget`] component from inside a step instead.
    pub fn set_warp_target(&mut self, id: ActorId, target: WarpTarget) -> bool {
        let world = self.ecs.world_mut();
        let Some(entity) = world.resource::<ActorTable>().entity(id) else { return false };
        *world.get_mut::<WarpTarget>(entity).expect("every pooled entity has a target") = target;
        true
    }

    /// A live actor's clock, between steps: hitstop and time scales from outside the simulation, as
    /// in a test. [`CombatSet::Resolve`] does this itself (M2 G3).
    pub fn clock_mut(&mut self, id: ActorId) -> Option<Mut<'_, EntityClock>> {
        let world = self.ecs.world_mut();
        let entity = world.resource::<ActorTable>().entity(id)?;
        world.get_mut::<EntityClock>(entity)
    }

    /// The world's clock, between steps: the same for time scales on everything.
    pub fn world_clock_mut(&mut self) -> Mut<'_, WorldClock> {
        self.ecs.world_mut().resource_mut::<WorldClock>()
    }

    /// Hit queries' latest events and counters.
    #[must_use]
    pub fn hits(&self) -> &HitQuery {
        self.ecs.world().resource::<HitQuery>()
    }

    /// The simulation's counters (rule R9).
    #[must_use]
    pub fn stats(&self) -> SimStats {
        self.stats
    }

    /// The world, for presentation to read.
    #[must_use]
    pub fn world(&self) -> &World {
        self.ecs.world()
    }

    /// The engine's schedules and world, for this crate's own tests.
    #[cfg(test)]
    pub(crate) fn ecs_mut(&mut self) -> &mut Ecs {
        &mut self.ecs
    }
}

fn view(world: &World, id: ActorId, entity: Entity) -> ActorView {
    let actor = world.entity(entity);
    let transform = *actor.get::<ActorTransform>().expect("every pooled entity has a transform");
    let clock = actor.get::<EntityClock>().expect("every pooled entity has a clock");
    let runner = actor.get::<MoveRunner>().expect("every pooled entity has a runner");
    let output = actor.get::<MoveOutput>().expect("every pooled entity has an output");
    ActorView {
        id,
        position: transform.position,
        yaw: transform.yaw,
        clock_now: clock.0.now(),
        hitstop_ticks: clock.0.hitstop_ticks(),
        moves: MoveRead::of(runner, output, world.resource::<MoveLibrary>()),
        controller: *actor.get::<CharacterController>().expect("every pooled entity has a controller"),
    }
}
