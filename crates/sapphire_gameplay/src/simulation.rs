//! [`Simulation`]: one combat world, stepped one tick at a time.

use std::time::Duration;

use bevy_ecs::schedule::{IntoScheduleConfigs, ScheduleLabel, SingleThreadedExecutor, SystemSet};
use bevy_ecs::system::ScheduleSystem;
use bevy_ecs::system::SystemState;
use sapphire_core::Name;
use sapphire_core::alloc::AllocScope;
use sapphire_ecs::prelude::*;
use sapphire_math::Pcg32;
use sapphire_moves::{MoveSet, MoveSetError};
use sapphire_navmesh::NavMeshError;
use sapphire_physics::PhysicsWorld;
use sapphire_time::{SimDuration, Stopwatch};
use sapphire_timeline::{TimelineError, TimelineSimulation};

use crate::actions::{Controls, CurrentActions, TickActions, read_actions};
use crate::actors::{
    self, ActorId, ActorRequests, ActorSpawn, ActorState, ActorTable, ActorView, SpawnError,
};
use crate::body::{self, BodyAttachError, BodyAuthority, BodyProfile, BodySample};
use crate::brain::{self, Brain};
use crate::checksum;
use crate::controller::{
    self, CharacterController, CharacterWorld, ControllerProfile, PhysicsWorldError, ProfileError, ProfileId,
    ResolvedProfile,
};
use crate::cues::{CueEvent, CueQueue};
use crate::encounter::{self, SimZone, ZoneCast, ZoneStartError};
use crate::hits::{self, HitQuery, remove_hurtboxes};
use crate::intents::{self, IntentRing};
use crate::movement::{self, ActorTransform};
use crate::moves::{
    self, MoveLibrary, MoveOutput, MoveRead, MoveRunner, MoveSetId, MoveStats, Tuned, WarpTarget,
};
use crate::navigation::SimNavigation;
use crate::party::{self, Party, PartyError, PartySlot, PartyTuning};
use crate::resolve::{self, AppliedHits, Fighter, ResolveStats, SlowMotion};
use crate::script::{self, ScriptWorld};
use crate::stats::{BUDGET_DESKTOP_LOW, PHYSICS_BUDGET_DESKTOP_LOW, SimStats};
use crate::targeting::{self, SoftLock, TargetingTuning};
use crate::timeline::{
    self, CinematicError, TimelineEvent, TimelineId, TimelineLibrary, TimelinePlayError, TimelineProgress,
    TimelineRun,
};
use sapphire_serialize::AssetId;
use sapphire_zone::ArchivedZone;

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

/// Runs after every step, outside its time and its allocation count, for what belongs at a tick boundary
/// and isn't simulation: a script VM's collector (M3 L3). Whatever steps a simulation, a game or a replay's
/// runner, runs it, since [`Simulation::step`] does. Nothing here may change simulated state.
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BetweenSteps;

/// Combat's randomness (spec §5.4). No system reads the OS for numbers, and the state is in the
/// checksum.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimRng(pub Pcg32);

/// Spec §5.22's fixed-step order for combat, chained inside
/// [`FixedCombat`]. Each package's systems go in their step.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CombatSet {
    /// A playing timeline's beats, ahead of everything that might read what they told gameplay.
    Timeline,
    /// The intent ring: what actors want to do this tick.
    Intents,
    /// A party's swap, with every press in the ring and before any cancel reads one: so the assist starts
    /// on the press's own step, and everything from here on sees who came in, where it now stands.
    Party,
    /// The move graph checks cancel windows.
    Cancels,
    /// A move the cancels just started starts its cinematic, if it names one (M3 Q3b): after every move
    /// that starts this step has, and before anything reads one, so the run starts on the move's first tick.
    Cinematics,
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
    /// What this step did, for hooks to answer: only with [`Simulation::enable_scripts`].
    HookEvents,
    /// Hooks answer it, in `sapphire_script`, which reads and asks but changes nothing.
    Hooks,
    /// The one point what they asked for takes effect.
    Commands,
    /// Cues, which are presentation only.
    Cues,
}

type ResolveParams = (
    ResMut<'static, ActorRequests>,
    ResMut<'static, ActorTable>,
    ResMut<'static, WorldClock>,
    Res<'static, MoveLibrary>,
    ResMut<'static, HitQuery>,
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
    /// Presentation identity for a fresh run over retained storage; never simulation state.
    run_generation: u64,
    /// Setup opt-ins, kept in actor ID order even after an opted-in actor despawns.
    standing_jump_actors: Vec<ActorId>,
    /// Whether anything's in [`BetweenSteps`], so a world with nothing there doesn't look for it.
    between_steps: bool,
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
        world.insert_resource(AppliedHits::default());
        world.insert_resource(ResolveStats::default());
        world.insert_resource(SlowMotion::default());
        world.insert_resource(Party::default());
        world.insert_resource(TimelineLibrary::default());
        world.insert_resource(TimelineRun::default());
        world.insert_resource(TargetingTuning::shipped());
        let empty = PhysicsWorld::new(Vec::new(), actors::body_capacity(config.max_actors))
            .expect("a world with no colliders has nothing to refuse");
        world.insert_resource(CharacterWorld::new(empty));
        actors::create_pool(world, config.max_actors);

        world.schedule_scope(FixedCombat, |_, schedule| {
            schedule.configure_sets(
                (
                    CombatSet::Timeline,
                    CombatSet::Intents,
                    CombatSet::Party,
                    CombatSet::Cancels,
                    CombatSet::Cinematics,
                    CombatSet::Moves,
                    CombatSet::Movement,
                    CombatSet::Hits,
                    CombatSet::Resolve,
                    CombatSet::TimeScale,
                    CombatSet::HookEvents,
                    CombatSet::Hooks,
                    CombatSet::Commands,
                    CombatSet::Cues,
                )
                    .chain(),
            );
        });
        ecs.add_systems(FixedFirst, read_actions.in_set(SimSet::Input))
            .add_systems(FixedCombat, timeline::advance_timeline.in_set(CombatSet::Timeline))
            .add_systems(
                FixedCombat,
                (intents::press_intents, encounter::direct, targeting::choose_soft_locks, brain::think)
                    .chain()
                    .in_set(CombatSet::Intents),
            )
            .add_systems(FixedCombat, party::settle_swaps.in_set(CombatSet::Party))
            .add_systems(FixedCombat, moves::settle_cancels.in_set(CombatSet::Cancels))
            .add_systems(FixedCombat, timeline::start_cinematics.in_set(CombatSet::Cinematics))
            .add_systems(FixedCombat, moves::advance_moves.in_set(CombatSet::Moves))
            .add_systems(
                FixedCombat,
                (
                    resolve::push_knockback,
                    movement::apply_root_motion,
                    controller::move_characters,
                    hits::place_hurtboxes,
                )
                    .chain()
                    .in_set(CombatSet::Movement),
            )
            .add_systems(FixedCombat, hits::sweep_hits.in_set(CombatSet::Hits))
            .add_systems(
                FixedCombat,
                (resolve::apply_hits, resolve::fire_perfect_dodges).chain().in_set(CombatSet::Resolve),
            )
            .add_systems(FixedCombat, resolve::apply_time.in_set(CombatSet::TimeScale))
            .add_systems(FixedCombat, resolve::publish_hit_cues.in_set(CombatSet::Cues))
            .add_systems(FixedPost, actors::resolve_actor_requests);
        ecs.startup();

        let resolve = SystemState::new(ecs.world_mut());
        let stats = SimStats { budget: config.step_budget, ..SimStats::default() };
        Self {
            ecs,
            resolve,
            stats,
            physics_budget: config.physics_budget,
            run_generation: 0,
            standing_jump_actors: Vec::new(),
            between_steps: false,
        }
    }

    /// Only `FightWorld` calls this: its private Simulation cannot gain arbitrary systems, resources,
    /// navigation or timelines. Keep this list aligned with new() and the canonical state writer.
    /// Schedules/queries have no gameplay Local state or change-detection filters to reset.
    pub(crate) fn restart_builtin_fight(&mut self, config: SimConfig) {
        self.run_generation = self.run_generation.wrapping_add(1);
        self.standing_jump_actors.clear();
        let world = self.ecs.world_mut();
        debug_assert!(world.get_resource::<SimNavigation>().is_none());
        *world.resource_mut::<WorldClock>() = WorldClock::default();
        *world.resource_mut::<SimRng>() = SimRng(Pcg32::new(config.seed, COMBAT_STREAM));
        *world.resource_mut::<CurrentActions>() = CurrentActions::default();
        *world.resource_mut::<MoveStats>() = MoveStats::default();
        *world.resource_mut::<CueQueue>() = CueQueue::default();
        world.resource_mut::<HitQuery>().reset();
        world.resource_mut::<AppliedHits>().0.clear();
        *world.resource_mut::<ResolveStats>() = ResolveStats::default();
        *world.resource_mut::<SlowMotion>() = SlowMotion::default();
        *world.resource_mut::<Party>() = Party::default();
        *world.resource_mut::<TimelineRun>() = TimelineRun::default();
        *world.resource_mut::<Interpolation>() = Interpolation::default();
        if let Some(mut body) = world.get_resource_mut::<BodyAuthority>() {
            body.restart();
        }
        let (requests, table, clock, _, _, actors, characters) =
            self.resolve.get_mut(world).expect(ACTORS_EXIST);
        actors::restart_pool(requests, table, clock, characters, actors);
        self.ecs.world_mut().resource_mut::<MoveLibrary>().restart();
        self.stats = SimStats { budget: config.step_budget, ..SimStats::default() };
        self.physics_budget = config.physics_budget;
    }

    /// Adds a system to [`BetweenSteps`], which every step runs once it's done and counted: what
    /// `sapphire_script` collects a VM's garbage with. It builds the schedule here, as
    /// [`Self::add_combat_system`] does.
    pub fn add_between_steps_system<M>(&mut self, system: impl IntoScheduleConfigs<ScheduleSystem, M>) {
        let world = self.ecs.world_mut();
        if !self.between_steps {
            let mut schedule = Schedule::new(BetweenSteps);
            schedule.set_executor(SingleThreadedExecutor::new());
            world.add_schedule(schedule);
            self.between_steps = true;
        }
        world.schedule_scope(BetweenSteps, |world, schedule| {
            schedule.add_systems(system);
            if let Err(error) = schedule.initialize(world) {
                panic!(
                    "the between-steps schedule doesn't build: {}",
                    error.to_string(schedule.graph(), world)
                );
            }
        });
    }

    /// Adds a system to a [`CombatSet`], for a crate above gameplay (`sapphire_script` fires its hooks
    /// this way) and for a world built to test the simulation itself, such as the one that shows
    /// replays' allocation gate bites. What every fight runs goes in [`Self::new`].
    /// It rebuilds the schedule here, so that isn't paid inside the next step. Say its order against what's
    /// in that set (`.after(...)`) if it could matter: left to the schedule, that order can change when any
    /// other system is added, and the checksums with it.
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

    /// Gives this simulation what scripts need, between steps: [`ScriptWorld`], and the systems either side
    /// of [`CombatSet::Hooks`], where whoever runs them adds its own. From here on its state is in
    /// [`Self::checksum`] and its chunk in [`Self::tables_hash`]; a simulation that never asks hashes
    /// neither. Asking again does nothing. Each of the three sets holds just its one system, so their chain
    /// is the order: a system added to one of them says its order against what's there.
    pub fn enable_scripts(&mut self) {
        if self.scripts().is_some() {
            return;
        }
        let actors = script::capacity_of(self.ecs.world().resource::<ActorRequests>());
        self.ecs.world_mut().insert_resource(ScriptWorld::new(actors));
        self.add_combat_system(CombatSet::HookEvents, script::gather_hook_events);
        self.add_combat_system(CombatSet::Commands, script::apply_script_commands);
    }

    /// Its script state, if [`Self::enable_scripts`] has given it one.
    #[must_use]
    pub fn scripts(&self) -> Option<&ScriptWorld> {
        self.ecs.world().get_resource::<ScriptWorld>()
    }

    /// Its script state, between steps: a chunk bound, or a test's numbers. What it changes is on the
    /// caller's checksum.
    pub fn scripts_mut(&mut self) -> Option<Mut<'_, ScriptWorld>> {
        self.ecs.world_mut().get_resource_mut::<ScriptWorld>()
    }

    /// Puts a resource in the world, where a system from [`Self::add_combat_system`] keeps its state.
    /// [`Self::world`] reads it; [`Self::resource_mut`] writes it between steps.
    pub fn insert_resource<R: Resource>(&mut self, resource: R) {
        self.ecs.world_mut().insert_resource(resource);
    }

    /// A resource put there by [`Self::insert_resource`], or one of the simulation's own, to change
    /// between steps. What it changes is on the caller's checksum.
    ///
    /// # Panics
    ///
    /// If the world has no such resource.
    pub fn resource_mut<R: Resource + Component<Mutability = bevy_ecs::component::Mutable>>(
        &mut self,
    ) -> Mut<'_, R> {
        self.ecs.world_mut().resource_mut::<R>()
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
    /// next move to start reads this (M2 T2). So do hurtboxes and reactions, from the next step. A
    /// fighter takes the new numbers at once, each meter keeping what it's lost
    /// ([`Fighter`]). See the `moves` module.
    ///
    /// **Its `movement` is what the next step moves by.** Nothing moves now: position, velocity and
    /// what it's doing carry over, so a character in a jump finishes that jump's arc under the new
    /// gravity, and its new jump speed waits for its next jump. A new capsule stands on the same feet.
    /// Only a gravity too light for what someone's carrying cuts it ([`ResolvedProfile::carried`]).
    pub fn replace_move_set(&mut self, id: MoveSetId, set: MoveSet) -> Option<u32> {
        let world = self.ecs.world_mut();
        let moved_by = world.resource::<MoveLibrary>().movement(id).copied();
        let in_use: Vec<u32> = world
            .resource::<ActorTable>()
            .iter()
            .filter_map(|(_, entity)| world.get::<MoveRunner>(entity)?.running)
            .filter(|running| running.at.set == id)
            .map(|running| running.at.revision)
            .collect();
        let playing: Vec<ActorId> = world
            .resource::<ActorTable>()
            .iter()
            .filter(|&(_, entity)| world.get::<MoveRunner>(entity).is_some_and(|run| run.set() == Some(id)))
            .map(|(actor, _)| actor)
            .collect();
        let replaced =
            world.resource_mut::<MoveLibrary>().replace(id, set, |revision| in_use.contains(&revision))?;
        // The next step puts back what the new revision lists, under its numbers.
        let mut characters = world.resource_mut::<CharacterWorld>();
        for &actor in &playing {
            remove_hurtboxes(characters.physics_mut(), actor);
        }
        let def = world.resource::<MoveLibrary>().current(id).and_then(MoveSet::fighter).cloned();
        let moves_by = world.resource::<MoveLibrary>().movement(id).copied();
        for actor in playing {
            let Some(entity) = world.resource::<ActorTable>().entity(actor) else { continue };
            if let Some(mut fighter) = world.get_mut::<Fighter>(entity) {
                fighter.refit(def.as_ref());
            }
            let spawned = world.get::<CharacterController>(entity).and_then(CharacterController::profile);
            let Some(transform) = world.get::<ActorTransform>(entity).copied() else { continue };
            let mut characters = world.resource_mut::<CharacterWorld>();
            let spawned = spawned.and_then(|profile| characters.profile(profile)).copied();
            if let (Some(was), Some(now)) = (moved_by.or(spawned), moves_by.or(spawned)) {
                characters.refit_body(actor, &was, &now, &transform);
                if was != now
                    && let Some(mut controller) = world.get_mut::<CharacterController>(entity)
                {
                    controller.state.velocity = now.carried(controller.state.velocity);
                }
            }
        }
        Some(replaced)
    }

    /// [`Self::replace_move_set`], for a document someone saved: bytes the same as the current
    /// revision's replace nothing, so saving a file again is no edit, and no recording ends over it.
    /// `None` for an ID this simulation never gave.
    pub fn tune_move_set(&mut self, id: MoveSetId, set: MoveSet) -> Option<Tuned> {
        let library = self.move_library();
        let revision = library.current_revision(id)?;
        if library.is_current(id, &set) {
            return Some(Tuned::Unchanged { revision });
        }
        self.replace_move_set(id, set).map(|revision| Tuned::Replaced { revision })
    }

    /// Makes `set` what the set added as `id` will be, whatever it's added with, before a scenario sets up:
    /// revision 0, with whatever its setup spawns and draws from it taking its numbers, as if the scenario
    /// had loaded it (see [`LiveMoves`](crate::replay::LiveMoves)).
    pub(crate) fn stand_in_move_set(&mut self, id: MoveSetId, set: MoveSet) {
        self.ecs.world_mut().resource_mut::<MoveLibrary>().stand_in(id, set);
    }

    /// The hash of the move data, controller setup and collision world this simulation runs on,
    /// tables included, in ID order: what a replay records to identify the setup it ran against.
    #[must_use]
    pub fn tables_hash(&self) -> u64 {
        let world = self.ecs.world();
        let tables = checksum::tables_hash(world, &self.standing_jump_actors);
        let Some(body) = world.get_resource::<BodyAuthority>() else { return tables };
        let mut hash = checksum::StateHasher::new();
        hash.u64(tables);
        body.write_identity(&mut hash);
        hits::overhead::write_delivery_identity(world, &mut hash);
        hash.finish()
    }

    /// Binds an immutable profile to one live actor before the first fixed step. The existing hit
    /// sweep remains active. An explicitly configured profile is included in the replay identity.
    pub fn attach_body(
        &mut self,
        actor: ActorId,
        look_target: Option<ActorId>,
        profile: BodyProfile,
    ) -> Result<(), BodyAttachError> {
        if self.tick() != 0 {
            return Err(BodyAttachError::Started);
        }
        let world = self.ecs.world_mut();
        if world.get_resource::<BodyAuthority>().is_some_and(|body| body.contains(actor)) {
            return Err(BodyAttachError::Bound);
        }
        let table = world.resource::<ActorTable>();
        if table.entity(actor).is_none()
            || look_target.is_some_and(|target| target == actor || table.entity(target).is_none())
        {
            return Err(BodyAttachError::Actor);
        }
        if let Some(mut body) = world.get_resource_mut::<BodyAuthority>() {
            body.insert(actor, look_target, profile);
        } else {
            let mut body = BodyAuthority::default();
            body.insert(actor, look_target, profile);
            world.insert_resource(body);
            self.add_combat_system(CombatSet::Movement, body::advance.after(hits::place_hurtboxes));
        }
        Ok(())
    }

    /// Marks an explicit discontinuity for a scripted reposition or other non-swept replacement.
    pub fn cut_body(&mut self, actor: ActorId) {
        let world = self.ecs.world_mut();
        if let Some(mut body) = world.get_resource_mut::<BodyAuthority>() {
            body.cut(actor);
            world.resource_mut::<HitQuery>().cut_overhead_delivery(actor);
        }
    }

    /// Creates a reusable read buffer for the configured body outside the fixed step.
    #[must_use]
    pub fn body_sample_buffer(&self, actor: ActorId) -> Option<BodySample> {
        self.ecs.world().get_resource::<BodyAuthority>()?.sample_buffer(actor)
    }

    /// Reads the already solved pre-Resolve pair. This cannot advance any pose or event history.
    pub fn read_body(&self, actor: ActorId, fraction: f32, sample: &mut BodySample) -> bool {
        let world = self.ecs.world();
        world.resource::<ActorTable>().entity(actor).is_some()
            && !world.resource::<Party>().is_off_field(actor)
            && world.get_resource::<BodyAuthority>().is_some_and(|body| body.read(actor, fraction, sample))
    }

    /// The current readable body generation and clock without hashing the pose.
    #[doc(hidden)]
    pub fn body_stamp(&self, actor: ActorId) -> Option<sapphire_body_pose::GraphStepStamp> {
        let world = self.ecs.world();
        world.resource::<ActorTable>().entity(actor)?;
        if world.resource::<Party>().is_off_field(actor) {
            return None;
        }
        world.get_resource::<BodyAuthority>()?.stamp(actor)
    }

    /// The immutable endpoint's step stamp, contact state and canonical checksum.
    #[must_use]
    pub fn body_receipt(
        &self,
        actor: ActorId,
    ) -> Option<(sapphire_body_pose::GraphStepStamp, sapphire_body_pose::FootContact, u64)> {
        let world = self.ecs.world();
        world.resource::<ActorTable>().entity(actor)?;
        if world.resource::<Party>().is_off_field(actor) {
            return None;
        }
        world.get_resource::<BodyAuthority>()?.receipt(actor)
    }

    /// The weights a soft lock is chosen by in this simulation, [`TargetingTuning::shipped`] until told
    /// otherwise. Whatever reads a choice, in a step or out, reads them here.
    #[must_use]
    pub fn targeting(&self) -> TargetingTuning {
        *self.ecs.world().resource::<TargetingTuning>()
    }

    /// Makes `tuning` the weights, between steps. Says whether that changed anything: if it did, so did
    /// [`Self::tables_hash`], so a file recording this run has to end ([`crate::replay::Recording`]).
    pub fn tune_targeting(&mut self, tuning: TargetingTuning) -> bool {
        let mut current = self.ecs.world_mut().resource_mut::<TargetingTuning>();
        let changed = *current != tuning;
        *current = tuning;
        changed
    }

    /// Gives this simulation a zone, between steps and before any character has spawned: its colliders if it
    /// names any, and its triggers and encounters, copied, so nothing here streams and nothing is read again.
    /// `read` hands over what the zone references, cooked, by ID. Its digest goes in [`Self::tables_hash`],
    /// without the navmesh, which [`Self::set_navigation`] takes apart, and what it's doing in
    /// [`Self::checksum`]. A simulation that's never given one hashes neither. See [`crate::encounter`].
    ///
    /// # Errors
    ///
    /// If what it names isn't there or isn't good, in which case it keeps what it had.
    pub fn set_zone<'a>(
        &mut self,
        zone: &ArchivedZone,
        cast: ZoneCast<'_>,
        mut read: impl FnMut(AssetId) -> Option<&'a [u8]>,
    ) -> Result<(), ZoneStartError> {
        let held = SimZone::new(zone, cast, self.move_library(), &mut read)?;
        if let Some(physics) = SimZone::physics_of(zone, self.body_capacity(), &mut read)? {
            self.set_physics_world(physics)?;
        }
        self.ecs.world_mut().insert_resource(held);
        Ok(())
    }

    /// Its zone, if it's been given one.
    #[must_use]
    pub fn zone(&self) -> Option<&SimZone> {
        self.ecs.world().get_resource::<SimZone>()
    }

    /// Gives this simulation a zone's navmesh, from a cook's bytes, which must start 16-byte aligned,
    /// between steps. From here on its digest is in [`Self::tables_hash`] and its paths in
    /// [`Self::checksum`]; a simulation that's never given one hashes neither.
    ///
    /// # Errors
    ///
    /// If the bytes aren't a good navmesh, in which case it keeps what it had.
    pub fn set_navigation(&mut self, cooked: &[u8]) -> Result<(), NavMeshError> {
        let navigation = SimNavigation::new(cooked)?;
        self.ecs.world_mut().insert_resource(navigation);
        Ok(())
    }

    /// Its navigation, if it's been given a navmesh.
    #[must_use]
    pub fn navigation(&self) -> Option<&SimNavigation> {
        self.ecs.world().get_resource::<SimNavigation>()
    }

    /// Its navigation, between steps: to add a path, or read what paths have cost.
    pub fn navigation_mut(&mut self) -> Option<Mut<'_, SimNavigation>> {
        self.ecs.world_mut().get_resource_mut::<SimNavigation>()
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

    /// Opt this character into grounded jump preparation during scenario setup.
    pub(crate) fn prepare_standing_jump(&mut self, actor: ActorId) {
        let world = self.ecs.world_mut();
        let entity = world.resource::<ActorTable>().entity(actor).expect("the player was spawned");
        world
            .get_mut::<CharacterController>(entity)
            .expect("the player has a controller")
            .state
            .jump_preparation = Some(SimDuration::ZERO);
        if let Err(index) = self.standing_jump_actors.binary_search(&actor) {
            self.standing_jump_actors.insert(index, actor);
        }
    }

    /// What an actor moves by now: its set's `movement`, if its document has one, and otherwise the
    /// profile it spawned with. `None` for an actor with no controller.
    #[must_use]
    pub fn movement_of(&self, actor: ActorId) -> Option<&ResolvedProfile> {
        let world = self.ecs.world();
        let entity = world.resource::<ActorTable>().entity(actor)?;
        let spawned = world.get::<CharacterController>(entity)?.profile()?;
        let set = world.get::<MoveRunner>(entity).and_then(MoveRunner::set);
        set.and_then(|set| world.resource::<MoveLibrary>().movement(set))
            .or_else(|| world.resource::<CharacterWorld>().profile(spawned))
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
        let (mut requests, table, clock, library, hits, actors, characters) =
            self.resolve.get_mut(self.ecs.world_mut()).expect(ACTORS_EXIST);
        let alive = requests.despawn(id, &table);
        actors::resolve_actor_requests(requests, table, clock, library, hits, actors, characters);
        if alive {
            self.cut_body(id);
        }
        alive
    }

    fn resolve_requests(&mut self) {
        let (requests, table, clock, library, hits, actors, characters) =
            self.resolve.get_mut(self.ecs.world_mut()).expect(ACTORS_EXIST);
        actors::resolve_actor_requests(requests, table, clock, library, hits, actors, characters);
    }

    /// Runs one fixed step on `actions`: the only way time moves. Allocates nothing on this thread
    /// (rule R6), tested from the first tick. Then runs [`BetweenSteps`], if anything's in it, outside the
    /// step's time and count.
    pub fn step(&mut self, actions: &TickActions) {
        profiling::scope!("Simulation::step");
        let stopwatch = Stopwatch::start();
        let allocations = AllocScope::begin();

        let world = self.ecs.world_mut();
        let held = world.resource::<TimelineRun>().holds_input();
        world.resource_mut::<CurrentActions>().0 = if held { TickActions::NEUTRAL } else { *actions };
        world.resource_mut::<CueQueue>().clear();
        self.ecs.fixed_step();

        let allocated = allocations.counts().allocations;
        let elapsed = stopwatch.elapsed();
        self.record_step(elapsed, allocated);
        if self.between_steps {
            self.ecs.world_mut().run_schedule(BetweenSteps);
        }
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
        stats.counters_fired = moves.counters_fired;
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
        let resolved = world.resource::<ResolveStats>();
        stats.hits_applied = resolved.hits_applied;
        stats.hits_absorbed = resolved.hits_absorbed;
        stats.reactions_forced = resolved.reactions_forced;
        stats.poise_breaks = resolved.poise_breaks;
        stats.defeats = resolved.defeats;
        stats.hurtboxes_refused = resolved.hurtboxes_refused;
        stats.perfect_dodges = resolved.perfect_dodges;
        let party = world.resource::<Party>();
        stats.swaps = party.swaps();
        stats.forced_swaps = party.forced_swaps();
        let timeline = world.resource::<TimelineRun>();
        stats.timelines_played = timeline.played();
        stats.timeline_events = timeline.events_fired();
        stats.timeline_scales_refused = timeline.scales_refused();
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

    /// Gives a live actor a soft lock, between steps: from its next step on, each step chooses who it's
    /// aiming at from where it stands and where its stick points, and writes the [`WarpTarget`]. Returns
    /// whether the actor is alive. See [`crate::targeting`].
    pub fn give_soft_lock(&mut self, id: ActorId) -> bool {
        let world = self.ecs.world_mut();
        let Some(entity) = world.resource::<ActorTable>().entity(id) else { return false };
        *world.get_mut::<SoftLock>(entity).expect("every pooled entity has a soft lock") = SoftLock::ON;
        true
    }

    /// Gives a live actor a brain, from its next step, between steps: it picks its own foes. Returns whether
    /// the actor's set says how it fights ([`sapphire_moves::MoveSet::brain`]). See [`crate::brain`].
    pub fn give_brain(&mut self, id: ActorId) -> bool {
        let world = self.ecs.world_mut();
        let Some(entity) = world.resource::<ActorTable>().entity(id) else { return false };
        let set = world.get::<MoveRunner>(entity).and_then(MoveRunner::set);
        let library = world.resource::<MoveLibrary>();
        let Some(def) = set.and_then(|set| library.current(set)).and_then(MoveSet::brain) else {
            return false;
        };
        let now = world.get::<EntityClock>(entity).expect("every pooled entity has a clock").0.now();
        let mut rng = world.resource::<SimRng>().0;
        let brain = Brain::fighting(def, now, &mut rng);
        world.resource_mut::<SimRng>().0 = rng;
        *world.get_mut::<Brain>(entity).expect("every pooled entity has a brain") = brain;
        true
    }

    /// Makes `members` a party, between steps: the first stays on the field where it stands, and its ID is
    /// what input names from here on, whoever's on the field. The others leave it at once. Each needs a
    /// set of moves with a `swap` intent and a `swap_in` move. A simulation has one party, for good. See
    /// the `party` module.
    pub fn form_party(&mut self, members: &[ActorId], tuning: PartyTuning) -> Result<(), PartyError> {
        party::form(self.ecs.world_mut(), members, tuning)?;
        for &benched in &members[1..] {
            self.cut_body(benched);
        }
        Ok(())
    }

    /// The formation was validated before resetting. Newly spawned off-field actors already have
    /// the neutral component state that form() establishes; only their bodies need removing.
    pub(crate) fn restore_builtin_party(&mut self, party: Party) {
        let mut characters = self.ecs.world_mut().resource_mut::<CharacterWorld>();
        for slot in party.slots().filter(|slot| party.is_off_field(slot.actor)) {
            characters.despawned(slot.actor);
        }
        *self.ecs.world_mut().resource_mut::<Party>() = party;
    }

    /// The party, if it's formed: who's on the field, and each slot's readiness.
    #[must_use]
    pub fn party(&self) -> &Party {
        self.ecs.world().resource::<Party>()
    }

    /// How long until a party's slot can next change place, by the clock of who's in it: the rest of its
    /// [`PartyTuning::swap_cooldown`] on the field, or of its [`PartyTuning::rest`] off it. `None` for a
    /// member that's gone. What a HUD's swap button reads.
    #[must_use]
    pub fn party_ready_in(&self, slot: PartySlot) -> Option<SimDuration> {
        let now = self.actor(slot.actor)?.clock_now;
        Some(if slot.ready_at > now { slot.ready_at.since(now) } else { SimDuration::ZERO })
    }

    /// Slows the world for a number of steps, between steps: [`SlowMotion::request`], as a test or a
    /// tool would. A system does the same with `ResMut<SlowMotion>`.
    pub fn request_slow_motion(&mut self, per_mille: u32, steps: u16) {
        self.ecs.world_mut().resource_mut::<SlowMotion>().request(per_mille, steps);
    }

    /// Loads a timeline's simulation half, between steps, for [`Self::play_timeline`]. It goes in
    /// [`Self::tables_hash`], so a replay that plays it says so if it changes. A scenario loads what a
    /// replay of it could play, up front.
    pub fn add_timeline(&mut self, timeline: TimelineSimulation) -> Result<TimelineId, TimelineError> {
        self.ecs.world_mut().resource_mut::<TimelineLibrary>().add(timeline, None)
    }

    /// Loads a move's cinematic under the `name` its moves give it, between steps and before the sets
    /// that name it (M3 Q3b). As [`Self::add_timeline`], it goes in [`Self::tables_hash`], its name too.
    /// It has to run on the [`Steps`](sapphire_timeline::TimelineClock::Steps) clock, beneath its move.
    pub fn add_cinematic(
        &mut self,
        name: &str,
        timeline: TimelineSimulation,
    ) -> Result<TimelineId, CinematicError> {
        let library = self.ecs.world().resource::<TimelineLibrary>();
        if library.find(Name::intern(name)).is_some() {
            return Err(CinematicError::Taken(name.to_owned()));
        }
        if timeline.ticks == 0 {
            return Err(CinematicError::Held(name.to_owned()));
        }
        let library = &mut self.ecs.world_mut().resource_mut::<TimelineLibrary>();
        Ok(library.add(timeline, Some(Name::intern(name)))?)
    }

    /// Refuses a fight whose move sets name a cinematic it hasn't loaded, naming the first: what a fight's
    /// preparation runs once its sets and timelines are in, so that no ultimate goes missing silently.
    pub fn check_cinematics(&self) -> Result<(), CinematicError> {
        let world = self.ecs.world();
        let timelines = world.resource::<TimelineLibrary>();
        for (set, moves) in world.resource::<MoveLibrary>().current_sets() {
            for mv in moves.moves() {
                if let Some(name) = mv.cinematic()
                    && timelines.find(name).is_none()
                {
                    return Err(CinematicError::Missing { set: set.0, mv: mv.label().to_owned(), name });
                }
            }
        }
        Ok(())
    }

    /// Starts a timeline, between steps: it does nothing until the next. A replay records this, as a
    /// [`TimelineCommand`](crate::TimelineCommand). The `timeline` module's docs say why there's no way to
    /// stop or skip one here.
    pub fn play_timeline(&mut self, id: TimelineId) -> Result<(), TimelinePlayError> {
        let world = self.ecs.world_mut();
        let mut run = *world.resource::<TimelineRun>();
        run.start(world.resource::<TimelineLibrary>(), id, None)?;
        *world.resource_mut::<TimelineRun>() = run;
        Ok(())
    }

    /// The digest of the loaded timeline `id` ([`TimelineSimulation::digest`]), for presentation to find the
    /// cooked timeline that plays beside it: the one whose simulation half is this one.
    #[must_use]
    pub fn timeline_digest(&self, id: TimelineId) -> Option<u64> {
        self.ecs.world().resource::<TimelineLibrary>().digest(id)
    }

    /// How many timelines have started in this run: presentation follows a run that moved this on.
    #[must_use]
    pub fn timelines_played(&self) -> u64 {
        self.ecs.world().resource::<TimelineRun>().played()
    }

    /// How many moves' cinematics couldn't start, because another timeline was playing or the fight never
    /// loaded it: a fight that [`Self::check_cinematics`] passed counts only the first.
    #[must_use]
    pub fn cinematics_refused(&self) -> u64 {
        self.ecs.world().resource::<TimelineRun>().cinematics_refused()
    }

    /// The timeline playing, if one is.
    #[must_use]
    pub fn timeline(&self) -> Option<TimelineProgress> {
        self.ecs.world().resource::<TimelineRun>().progress()
    }

    /// The steps a run holding input has left. A skip takes exactly these, on [`TickActions::NEUTRAL`] or
    /// on anything else, since it's held, and lands where watching would have.
    #[must_use]
    pub fn timeline_steps_held(&self) -> u32 {
        self.ecs.world().resource::<TimelineRun>().steps_held()
    }

    /// What the latest step's timeline told gameplay.
    pub fn timeline_events(&self) -> impl Iterator<Item = TimelineEvent> + '_ {
        let world = self.ecs.world();
        world.resource::<TimelineRun>().events(world.resource::<TimelineLibrary>())
    }

    /// World-clock slow motion as it stands: whether it's running and what it has refused.
    #[must_use]
    pub fn slow_motion(&self) -> SlowMotion {
        *self.ecs.world().resource::<SlowMotion>()
    }

    /// A live actor's clock, between steps: hitstop and time scales from outside the simulation, as
    /// in a test. [`CombatSet::TimeScale`] does this for hits.
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

    /// Which run occupies this world's retained storage, for presentation to detect a restart even
    /// at the same tick. Starts at zero and changes only on a built-in fight reset; it is not hashed.
    /// Replacing the Simulation entirely is a separate owner lifecycle, requiring its own reset.
    #[must_use]
    pub fn run_generation(&self) -> u64 {
        self.run_generation
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

pub(crate) fn view(world: &World, id: ActorId, entity: Entity) -> ActorView {
    let actor = world.entity(entity);
    let transform = *actor.get::<ActorTransform>().expect("every pooled entity has a transform");
    let clock = actor.get::<EntityClock>().expect("every pooled entity has a clock");
    let runner = actor.get::<MoveRunner>().expect("every pooled entity has a runner");
    let output = actor.get::<MoveOutput>().expect("every pooled entity has an output");
    let fighter = *actor.get::<Fighter>().expect("every pooled entity has a fighter");
    let brain = *actor.get::<Brain>().expect("every pooled entity has a brain");
    ActorView {
        id,
        position: transform.position,
        yaw: transform.yaw,
        clock_now: clock.0.now(),
        clock_delta: clock.0.last_delta(),
        hitstop_ticks: clock.0.hitstop_ticks(),
        moves: MoveRead::of(runner, output, world.resource::<MoveLibrary>()),
        controller: *actor.get::<CharacterController>().expect("every pooled entity has a controller"),
        fighter,
        brain,
        soft_lock: *actor.get::<SoftLock>().expect("every pooled entity has a soft lock"),
        controls: *actor.get::<Controls>().expect("every pooled entity has controls"),
        off_field: world.resource::<Party>().is_off_field(id),
    }
}
