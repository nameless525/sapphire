//! The world database (spec §4.4, §5.6): `bevy_ecs` 0.20 with the engine's schedules.
//!
//! Owns:
//! - The pinned `bevy_ecs`, re-exported so engine code imports it through this crate. It is
//!   upgraded between milestones, never during one. A crate that *derives* `Component`,
//!   `Resource` or another bevy trait must also list `bevy_ecs.workspace = true`, because
//!   bevy's derive macros find the crate by that name; the workspace still pins one version.
//! - The engine schedules and [`Ecs`], which runs them in frame order (spec §4.2): [`Startup`]
//!   once; for each fixed step [`FixedFirst`] → [`FixedCombat`] → [`FixedPhysics`] →
//!   [`FixedPost`]; then once per frame [`Presentation`], [`Extract`] and [`Cleanup`].
//! - Clocks as ECS data: the [`WorldClock`] resource and [`EntityClock`] components, ticked at
//!   the start of every fixed step in [`SimSet::Clocks`], world clock first (spec §4.5).
//!
//! Does not own: the frame loop and the fixed-step accumulator (the app drives
//! `sapphire_time::FixedStepDriver` and calls [`Ecs::fixed_step`] once per step), the
//! `FramePacket` that extraction fills (`sapphire_render` defines it; the app's [`Extract`]
//! systems fill it), or reflection (`sapphire_reflect`; components gain `ReflectComponent` with
//! the inspector, F5).
//!
//! # Executors
//!
//! - The fixed schedules, [`Startup`] and [`Cleanup`] run single-threaded on the calling
//!   (simulation) thread. Combat runs in a chained order there by design (spec §4.4), and rule
//!   R6's zero-allocation check measures that thread.
//! - [`Presentation`] and [`Extract`] are wide passes (animation, culling, extraction) and run on
//!   the multi-threaded executor. [`Ecs::new`] starts `sapphire_jobs` first, so the executor finds
//!   the engine's compute pool instead of creating its own (spec §4.3: one task pool).
//!
//! Measured in the sandbox (M1 F1, Apple M5, release): running the seven schedules costs about
//! 6–8 µs per frame. The fixed step allocates nothing (tested). The multi-threaded executor
//! allocates once per parallel schedule run plus once per system it runs (each is a spawned
//! task), and wakes a worker; a schedule with no systems costs nothing (M1 F3). That counts
//! toward the process-wide allocation budget, not rule R6.
//!
//! # Change detection
//!
//! `Changed` and `Added` compare against each system's last run, so they see everything since
//! then, however many frames that was. `RemovedComponents` keeps a removal only until the
//! second [`Ecs::cleanup`] after it, so a system that reads it must run every frame;
//! extraction does (spec §4.6).

use bevy_ecs::schedule::{ScheduleLabel, SingleThreadedExecutor, SystemSet};
use bevy_ecs::system::ScheduleSystem;

pub use bevy_ecs;

use bevy_ecs::prelude::*;
use sapphire_time::{LocalClock, ScaledClock};

/// Everything a system module usually needs: `bevy_ecs`'s prelude plus the engine schedules,
/// sets and clock data.
pub mod prelude {
    pub use bevy_ecs::prelude::*;

    pub use crate::{
        Cleanup, Ecs, EntityClock, Extract, FixedCombat, FixedFirst, FixedPhysics, FixedPost, Interpolation,
        Presentation, SimSet, Startup, WorldClock,
    };
}

/// Runs once, before the first frame.
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Startup;

/// The start of every fixed step: queued input ([`SimSet::Input`]), then the clocks
/// ([`SimSet::Clocks`]), then anything that must see this step's time before combat.
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FixedFirst;

/// Combat, AI and the character controller, in a chained order.
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FixedCombat;

/// Physics, after combat has moved things.
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FixedPhysics;

/// The end of every fixed step: resolving queued side effects (rule R3).
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FixedPost;

/// Once per frame at the display rate: pose evaluation, secondary motion, VFX and camera, each
/// interpolated by [`Interpolation::alpha`] between the last two fixed states.
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Presentation;

/// Once per frame, after presentation: copies what the renderer needs out of the world (spec
/// §4.6). The only sync point between simulation and rendering.
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Extract;

/// The end of every frame.
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cleanup;

/// System sets in [`FixedFirst`], run in this order.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SimSet {
    /// Input queued since the last frame lands on the frame's first step (rule R3), before
    /// time advances, so a slow-motion toggle already scales that step.
    Input,
    /// The world clock ticks, then every [`EntityClock`] (spec §4.5).
    Clocks,
}

/// The world clock, scaled by pause, perfect dodge, cinematics and debug. Entity clocks are
/// issued by it.
#[derive(Resource, Debug, Default)]
pub struct WorldClock(pub ScaledClock);

impl WorldClock {
    /// A clock for one entity, with its own scales and hitstop on top of the world's.
    pub fn new_entity_clock(&mut self) -> EntityClock {
        EntityClock(self.0.new_local_clock())
    }
}

/// One entity's clock. Ticked in [`SimSet::Clocks`]; systems read the step's advance from
/// `last_delta()`.
#[derive(Component, Debug)]
pub struct EntityClock(pub LocalClock);

/// How far presentation is between the last two fixed states: 0 is the previous step, 1 the
/// latest. Set by [`Ecs::presentation`].
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct Interpolation {
    pub alpha: f32,
}

/// A [`World`] with the engine schedules, run in frame order.
pub struct Ecs {
    world: World,
}

impl Ecs {
    /// A world with the engine schedules, the clock systems, [`WorldClock`] and
    /// [`Interpolation`]. Starts the job system if nothing has yet.
    pub fn new() -> Self {
        sapphire_jobs::system();
        let mut world = World::new();
        world.insert_resource(WorldClock::default());
        world.insert_resource(Interpolation::default());

        for label in [Startup.intern(), FixedFirst.intern(), FixedCombat.intern(), FixedPhysics.intern()] {
            world.add_schedule(single_threaded(label));
        }
        world.add_schedule(single_threaded(FixedPost.intern()));
        world.add_schedule(Schedule::new(Presentation));
        world.add_schedule(Schedule::new(Extract));
        world.add_schedule(single_threaded(Cleanup.intern()));
        world.schedule_scope(FixedFirst, |_, schedule| {
            schedule.configure_sets((SimSet::Input, SimSet::Clocks).chain());
            schedule.add_systems(tick_clocks.in_set(SimSet::Clocks));
        });
        Self { world }
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Adds systems to one of the engine schedules.
    pub fn add_systems<M>(
        &mut self,
        schedule: impl ScheduleLabel,
        systems: impl IntoScheduleConfigs<ScheduleSystem, M>,
    ) -> &mut Self {
        self.world.schedule_scope(schedule, |_, schedule| {
            schedule.add_systems(systems);
        });
        self
    }

    /// Runs [`Startup`], then builds every other schedule. Call once, after adding systems and
    /// before the first frame.
    ///
    /// bevy builds a schedule on its first run, which allocates. Building them here keeps that
    /// out of the first fixed step (rule R6). Systems added later make their schedule rebuild
    /// on its next run.
    pub fn startup(&mut self) {
        self.world.run_schedule(Startup);
        for label in [
            FixedFirst.intern(),
            FixedCombat.intern(),
            FixedPhysics.intern(),
            FixedPost.intern(),
            Presentation.intern(),
            Extract.intern(),
            Cleanup.intern(),
        ] {
            self.world.schedule_scope(label, |world, schedule| {
                if let Err(error) = schedule.initialize(world) {
                    panic!("schedule {label:?} doesn't build: {}", error.to_string(schedule.graph(), world));
                }
            });
        }
    }

    /// Runs one fixed step: [`FixedFirst`], [`FixedCombat`], [`FixedPhysics`], [`FixedPost`].
    pub fn fixed_step(&mut self) {
        self.world.run_schedule(FixedFirst);
        self.world.run_schedule(FixedCombat);
        self.world.run_schedule(FixedPhysics);
        self.world.run_schedule(FixedPost);
    }

    /// Runs [`Presentation`], `alpha` of the way from the previous fixed step to the latest.
    pub fn presentation(&mut self, alpha: f32) {
        self.world.resource_mut::<Interpolation>().alpha = alpha;
        self.world.run_schedule(Presentation);
    }

    /// Runs [`Extract`].
    pub fn extract(&mut self) {
        self.world.run_schedule(Extract);
    }

    /// Runs [`Cleanup`] and ends the frame: change trackers (removed components) reset here.
    pub fn cleanup(&mut self) {
        self.world.run_schedule(Cleanup);
        self.world.clear_trackers();
    }
}

impl Default for Ecs {
    fn default() -> Self {
        Self::new()
    }
}

fn single_threaded(label: bevy_ecs::schedule::InternedScheduleLabel) -> Schedule {
    let mut schedule = Schedule::new(label);
    schedule.set_executor(SingleThreadedExecutor::new());
    schedule
}

/// The world clock, then every entity clock, in that order (spec §4.5).
fn tick_clocks(mut world: ResMut<'_, WorldClock>, mut clocks: Query<'_, '_, &mut EntityClock>) {
    world.0.tick();
    for mut clock in &mut clocks {
        clock.0.tick(&world.0);
    }
}

#[cfg(test)]
mod tests {
    use bevy_tasks::{ComputeTaskPool, TaskPool};
    use sapphire_time::{SUBTICKS_PER_TICK, ScaleSource, SimDuration};

    use super::*;

    #[derive(Resource, Default)]
    struct Log(Vec<&'static str>);

    fn log(entry: &'static str) -> impl FnMut(ResMut<'_, Log>) {
        move |mut log: ResMut<'_, Log>| log.0.push(entry)
    }

    #[test]
    fn schedules_run_in_frame_order() {
        let mut ecs = Ecs::new();
        ecs.world_mut().insert_resource(Log::default());
        ecs.add_systems(Startup, log("startup"))
            .add_systems(FixedFirst, log("input").in_set(SimSet::Input))
            .add_systems(FixedFirst, log("first").after(SimSet::Clocks))
            .add_systems(FixedCombat, log("combat"))
            .add_systems(FixedPhysics, log("physics"))
            .add_systems(FixedPost, log("post"))
            .add_systems(Presentation, log("presentation"))
            .add_systems(Extract, log("extract"))
            .add_systems(Cleanup, log("cleanup"));

        ecs.startup();
        ecs.fixed_step();
        ecs.fixed_step();
        ecs.presentation(0.5);
        ecs.extract();
        ecs.cleanup();

        let step = ["input", "first", "combat", "physics", "post"];
        let expected: Vec<&str> = ["startup"]
            .into_iter()
            .chain(step)
            .chain(step)
            .chain(["presentation", "extract", "cleanup"])
            .collect();
        assert_eq!(ecs.world().resource::<Log>().0, expected);
        assert_eq!(ecs.world().resource::<Interpolation>().alpha, 0.5);
    }

    #[test]
    fn input_lands_before_the_clocks_tick() {
        #[derive(Resource, Default)]
        struct TicksSeen(Vec<u64>);
        let mut ecs = Ecs::new();
        ecs.world_mut().insert_resource(TicksSeen::default());
        ecs.add_systems(
            FixedFirst,
            (|world: Res<'_, WorldClock>, mut seen: ResMut<'_, TicksSeen>| seen.0.push(world.0.ticks()))
                .in_set(SimSet::Input),
        );
        ecs.fixed_step();
        ecs.fixed_step();
        assert_eq!(ecs.world().resource::<TicksSeen>().0, [0, 1]);
    }

    #[test]
    fn entity_clocks_tick_with_world_scales_and_hitstop() {
        let mut ecs = Ecs::new();
        let world = ecs.world_mut();
        let clock = world.resource_mut::<WorldClock>().new_entity_clock();
        let entity = world.spawn(clock).id();

        let delta = |ecs: &Ecs| ecs.world().get::<EntityClock>(entity).unwrap().0.last_delta();
        ecs.fixed_step();
        assert_eq!(delta(&ecs), SimDuration::from_ticks(1));

        ecs.world_mut().resource_mut::<WorldClock>().0.push_scale(ScaleSource::PerfectDodge, 500).unwrap();
        ecs.fixed_step();
        assert_eq!(delta(&ecs), SimDuration::from_subticks(u64::from(SUBTICKS_PER_TICK) / 2));

        ecs.world_mut().get_mut::<EntityClock>(entity).unwrap().0.apply_hitstop(1);
        ecs.fixed_step();
        assert_eq!(delta(&ecs), SimDuration::ZERO);
    }

    #[test]
    fn wide_passes_use_the_engine_task_pool() {
        let _ecs = Ecs::new();
        // The multi-threaded executor runs on `ComputeTaskPool::get_or_init`, which must find the
        // pool sapphire_jobs created rather than make a default one.
        let executor_pool: &TaskPool = ComputeTaskPool::get_or_init(TaskPool::default);
        assert!(std::ptr::eq(executor_pool, sapphire_jobs::system().compute_pool()));
    }
}
