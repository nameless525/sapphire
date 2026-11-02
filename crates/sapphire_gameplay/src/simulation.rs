//! [`Simulation`]: one combat world, stepped one tick at a time.

use std::time::Duration;

use bevy_ecs::schedule::{IntoScheduleConfigs, SystemSet};
use bevy_ecs::system::ScheduleSystem;
use bevy_ecs::system::SystemState;
use sapphire_core::alloc::AllocScope;
use sapphire_ecs::prelude::*;
use sapphire_math::Pcg32;
use sapphire_moves::{BakedTables, TablesError};
use sapphire_time::Stopwatch;

use crate::actions::{CurrentActions, TickActions, read_actions};
use crate::actors::{
    self, ActorId, ActorRequests, ActorSpawn, ActorState, ActorTable, ActorView, SpawnError,
};
use crate::checksum;
use crate::drill;
use crate::motion::{self, ActorTransform, RootMotion, TableId, TableLibrary};
use crate::stats::{BUDGET_DESKTOP_LOW, SimStats};

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
}

impl Default for SimConfig {
    /// Seed 0, room for 64 actors, Desktop-Low's budget.
    fn default() -> Self {
        Self { seed: 0, max_actors: 64, step_budget: BUDGET_DESKTOP_LOW }
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
    /// Root motion and the controller.
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
);

/// One combat world with no window and no GPU. See the [crate docs](crate).
pub struct Simulation {
    ecs: Ecs,
    /// Resolves spawns and despawns asked for between steps.
    resolve: SystemState<ResolveParams>,
    stats: SimStats,
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
        world.insert_resource(TableLibrary::default());
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
            .add_systems(FixedCombat, drill::apply_presses.in_set(CombatSet::Intents))
            .add_systems(
                FixedCombat,
                (drill::walk, motion::follow_root_motion).chain().in_set(CombatSet::Movement),
            )
            .add_systems(FixedPost, actors::resolve_actor_requests);
        ecs.startup();

        let resolve = SystemState::new(ecs.world_mut());
        let stats = SimStats { budget: config.step_budget, ..SimStats::default() };
        Self { ecs, resolve, stats }
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

    /// Loads baked tables for actors to follow, between steps. IDs count up from the first table
    /// added: a replay adds the same tables in the same order.
    pub fn add_tables(&mut self, tables: BakedTables) -> TableId {
        self.ecs.world_mut().resource_mut::<TableLibrary>().add(tables)
    }

    /// [`Self::add_tables`], from a cook's bytes, which must start 16-byte aligned.
    pub fn add_cooked_tables(&mut self, cooked: &[u8]) -> Result<TableId, TablesError> {
        Ok(self.add_tables(BakedTables::from_cooked(cooked)?))
    }

    /// The hash of every loaded table, in ID order: what a replay records of the move data, to tell
    /// a changed move from a broken simulation.
    #[must_use]
    pub fn tables_hash(&self) -> u64 {
        checksum::tables_hash(self.ecs.world().resource::<TableLibrary>())
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
        let (mut requests, table, clock, actors) =
            self.resolve.get_mut(self.ecs.world_mut()).expect(ACTORS_EXIST);
        let alive = requests.despawn(id, &table);
        actors::resolve_actor_requests(requests, table, clock, actors);
        alive
    }

    fn resolve_requests(&mut self) {
        let (requests, table, clock, actors) =
            self.resolve.get_mut(self.ecs.world_mut()).expect(ACTORS_EXIST);
        actors::resolve_actor_requests(requests, table, clock, actors);
    }

    /// Runs one fixed step on `actions`: the only way time moves. Allocates nothing on this thread
    /// (rule R6), tested from the first tick.
    pub fn step(&mut self, actions: &TickActions) {
        profiling::scope!("Simulation::step");
        let stopwatch = Stopwatch::start();
        let allocations = AllocScope::begin();

        self.ecs.world_mut().resource_mut::<CurrentActions>().0 = *actions;
        self.ecs.fixed_step();

        let allocated = allocations.counts().allocations;
        let elapsed = stopwatch.elapsed();
        self.record_step(elapsed, allocated);
    }

    fn record_step(&mut self, elapsed: Duration, allocated: u64) {
        let world = self.ecs.world();
        let actors = u32::try_from(world.resource::<ActorTable>().len()).unwrap_or(u32::MAX);
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
}

fn view(world: &World, id: ActorId, entity: Entity) -> ActorView {
    let actor = world.entity(entity);
    let transform = *actor.get::<ActorTransform>().expect("every pooled entity has a transform");
    let clock = actor.get::<EntityClock>().expect("every pooled entity has a clock");
    ActorView {
        id,
        position: transform.position,
        yaw: transform.yaw,
        clock_now: clock.0.now(),
        hitstop_ticks: clock.0.hitstop_ticks(),
        root_motion: *actor.get::<RootMotion>().expect("every pooled entity has root motion"),
    }
}

#[cfg(test)]
mod tests {
    use sapphire_moves::RootSample;
    use sapphire_time::{ScaleSource, SimDuration};

    use super::*;
    use crate::actions::{ActorActions, Buttons, Stick};

    fn press(buttons: Buttons) -> ActorActions {
        ActorActions { buttons, stick: Stick::CENTER }
    }

    /// A table with no two readings alike: x by the tick, z by its square, and a slow turn.
    fn curve(ticks: u32) -> BakedTables {
        let root = (0..=ticks)
            .map(|tick| {
                let t = tick as f32;
                RootSample { translation: [t * 0.25, 0.0, t * t * 0.01], yaw: t * 0.02 }
            })
            .collect();
        BakedTables::new(root, vec![]).unwrap()
    }

    fn tick_with(actor: ActorId, buttons: Buttons) -> TickActions {
        let mut actions = TickActions::NEUTRAL;
        actions.set(actor, press(buttons)).unwrap();
        actions
    }

    /// The bits of the table's reading at `subticks` against what the actor last read.
    fn assert_reads(sim: &Simulation, id: ActorId, table: &BakedTables, subticks: u64) {
        let view = sim.actor(id).unwrap();
        let expected = table.root_at(SimDuration::from_subticks(subticks));
        assert_eq!(view.root_motion.elapsed().as_subticks(), subticks, "tick {}", sim.tick());
        assert_eq!(
            view.root_motion.reading().translation.map(f32::to_bits),
            expected.translation.map(f32::to_bits)
        );
        assert_eq!(view.root_motion.reading().yaw.to_bits(), expected.yaw.to_bits());
    }

    /// A scale that isn't a whole number of sub-ticks a tick, so only the clock's carry keeps the
    /// reading exact.
    #[test]
    fn a_fractional_time_scale_reaches_the_table_through_its_carry() {
        let mut sim = Simulation::new(SimConfig::default());
        let table = curve(600);
        let id_table = sim.add_tables(table.clone());
        let id = sim.spawn(ActorSpawn { table: Some(id_table), ..ActorSpawn::default() }).unwrap();

        sim.step(&tick_with(id, Buttons::LIGHT));
        // World 200‰ × local 333‰ is 66.6 sub-ticks a tick.
        let world = sim.ecs.world_mut();
        let entity = world.resource::<ActorTable>().entity(id).unwrap();
        world.resource_mut::<WorldClock>().0.push_scale(ScaleSource::PerfectDodge, 200).unwrap();
        world.get_mut::<EntityClock>(entity).unwrap().0.push_scale(ScaleSource::Gameplay, 333).unwrap();

        for n in 1..=1000_u64 {
            sim.step(&TickActions::NEUTRAL);
            // Whole sub-ticks by the floor of the exact product, which a float can't drift from.
            assert_reads(&sim, id, &table, n * 666 / 10);
        }
    }

    /// Hitstop counts real ticks, so under slow motion it still lasts its six steps, and the reading
    /// holds its exact bits while it does.
    #[test]
    fn hitstop_holds_under_a_time_scale() {
        let mut sim = Simulation::new(SimConfig::default());
        let table = curve(60);
        let id_table = sim.add_tables(table.clone());
        let id = sim.spawn(ActorSpawn { table: Some(id_table), ..ActorSpawn::default() }).unwrap();

        sim.step(&tick_with(id, Buttons::LIGHT));
        sim.step(&TickActions::NEUTRAL);
        assert_reads(&sim, id, &table, 1000);

        // Slow the world to 200‰, the drill's own way: 200 sub-ticks a tick from the next step.
        sim.step(&tick_with(id, Buttons::ULTIMATE));
        assert_reads(&sim, id, &table, 2000);
        sim.step(&TickActions::NEUTRAL);
        assert_reads(&sim, id, &table, 2200);

        sim.step(&tick_with(id, Buttons::HEAVY));
        assert_reads(&sim, id, &table, 2400);
        for frozen in (0..crate::DRILL_HITSTOP_TICKS).rev() {
            sim.step(&TickActions::NEUTRAL);
            assert_reads(&sim, id, &table, 2400);
            assert_eq!(sim.actor(id).unwrap().hitstop_ticks, frozen);
        }
        sim.step(&TickActions::NEUTRAL);
        assert_reads(&sim, id, &table, 2600);
    }

    /// The app may keep a simulation on any thread, and a replay job may run several.
    #[test]
    fn a_simulation_can_move_between_threads() {
        fn assert_send<T: Send>() {}
        assert_send::<Simulation>();
    }

    /// The creation-order test in `tests/determinism.rs` has teeth: these two histories put the same
    /// IDs in different entities, so a system that draws numbers in a bare query's order, as the
    /// crate's rules forbid, gives them different checksums, where the ID-ordered drill agrees.
    #[test]
    fn a_bare_querys_order_would_leak_into_the_checksum() {
        fn leaky(mut rng: ResMut<'_, SimRng>, mut transforms: Query<'_, '_, &mut ActorTransform>) {
            for mut transform in &mut transforms {
                transform.yaw += rng.0.unit_f32();
            }
        }
        fn history(reverse: bool, leak: bool) -> u64 {
            let mut sim = Simulation::new(SimConfig { max_actors: 8, ..SimConfig::default() });
            let table = sim.add_tables(curve(30));
            let actors: Vec<ActorId> = (0..4)
                .map(|_| sim.spawn(ActorSpawn { table: Some(table), ..ActorSpawn::default() }).unwrap())
                .collect();
            let (a, b) = if reverse { (actors[2], actors[0]) } else { (actors[0], actors[2]) };
            assert!(sim.despawn(a));
            assert!(sim.despawn(b));
            for _ in 0..2 {
                sim.spawn(ActorSpawn { table: Some(table), ..ActorSpawn::default() }).unwrap();
            }
            if leak {
                sim.ecs.add_systems(FixedCombat, leaky);
            }
            for _ in 0..8 {
                sim.step(&TickActions::NEUTRAL);
            }
            sim.checksum()
        }
        assert_eq!(history(false, false), history(true, false), "ID order hides the entities' order");
        assert_ne!(history(false, true), history(true, true), "the two histories really differ");
    }
}
