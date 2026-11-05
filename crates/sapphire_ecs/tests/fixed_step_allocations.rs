//! Rule R6: a fixed step makes zero heap allocations on the simulation thread, including the
//! work bevy's single-threaded executor does to run the four fixed schedules. Installs
//! `CountingAllocator` as this test binary's global allocator.

use std::alloc::System;

use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_ecs::prelude::*;
use sapphire_time::{ScaleSource, SimDuration};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

#[derive(Component, Default)]
struct Spin(f32);

#[derive(Resource, Default)]
struct Hits(u64);

#[test]
fn fixed_steps_do_not_allocate() {
    // Counts are exact per thread only where Rust uses native TLS (see `sapphire_core::alloc`).
    if !PER_THREAD_COUNTS {
        return;
    }
    let mut ecs = Ecs::new();
    for _ in 0..64 {
        let clock = ecs.world_mut().resource_mut::<WorldClock>().new_entity_clock();
        ecs.world_mut().spawn((clock, Spin::default()));
    }
    ecs.world_mut().insert_resource(Hits::default());
    ecs.add_systems(FixedFirst, (|mut hits: ResMut<'_, Hits>| hits.0 += 1).in_set(SimSet::Input))
        .add_systems(FixedCombat, |mut spins: Query<'_, '_, (&mut Spin, &EntityClock)>| {
            for (mut spin, clock) in &mut spins {
                spin.0 += clock.0.last_delta().as_secs_f32();
            }
        })
        .add_systems(FixedPhysics, |spins: Query<'_, '_, &Spin>| assert!(spins.iter().all(|s| s.0 >= 0.0)))
        .add_systems(FixedPost, |hits: Res<'_, Hits>| assert!(hits.0 > 0));

    // Counted from the very first step: `startup` builds the schedules, which allocates.
    ecs.startup();
    let ((), counts) = alloc::count_in_scope(|| {
        for _ in 0..1000 {
            ecs.fixed_step();
        }
    });
    assert!(alloc::is_installed());
    assert_eq!(counts.allocations, 0, "1000 fixed steps allocated {} times", counts.allocations);
}

#[test]
fn preparing_the_clock_query_after_assembly_preserves_the_first_step() {
    if !PER_THREAD_COUNTS {
        return;
    }

    #[derive(Component)]
    struct Shape<const N: u8>;
    #[derive(Resource, Default, Debug, PartialEq, Eq)]
    struct Seen {
        starts: u8,
        before: u64,
        after: u64,
        changed: u8,
        run_clocks: bool,
    }
    #[derive(Resource)]
    struct Marker;

    fn spawn<const N: u8>(ecs: &mut Ecs) -> Entity {
        let world = ecs.world_mut();
        let clock = world.resource_mut::<WorldClock>().new_entity_clock();
        world.spawn((clock, Shape::<N>)).id()
    }

    fn scene() -> (Ecs, Entity, [Entity; 8]) {
        let mut ecs = Ecs::new();
        ecs.world_mut().insert_resource(Seen { run_clocks: true, ..Seen::default() });
        ecs.world_mut().insert_resource(Marker);
        let clock = ecs.world_mut().resource_mut::<WorldClock>().new_entity_clock();
        let entity = ecs.world_mut().spawn(clock).id();
        ecs.world_mut().get_mut::<EntityClock>(entity).unwrap().0.apply_hitstop(1);
        ecs.world_mut().schedule_scope(FixedFirst, |_, schedule| {
            schedule.configure_sets(SimSet::Clocks.run_if(|seen: Res<'_, Seen>| seen.run_clocks));
        });
        ecs.add_systems(Startup, |mut seen: ResMut<'_, Seen>| seen.starts += 1)
            .add_systems(
                FixedFirst,
                (|clock: Res<'_, WorldClock>, mut seen: ResMut<'_, Seen>| seen.before = clock.0.ticks())
                    .in_set(SimSet::Input),
            )
            .add_systems(
                FixedFirst,
                (|clock: Res<'_, WorldClock>, marker: Res<'_, Marker>, mut seen: ResMut<'_, Seen>| {
                    seen.after = clock.0.ticks();
                    seen.changed += u8::from(marker.is_changed());
                })
                .after(SimSet::Clocks),
            );
        ecs.startup();
        let shapes = [
            spawn::<0> as fn(&mut Ecs) -> Entity,
            spawn::<1>,
            spawn::<2>,
            spawn::<3>,
            spawn::<4>,
            spawn::<5>,
            spawn::<6>,
            spawn::<7>,
        ]
        .map(|spawn| spawn(&mut ecs));
        (ecs, entity, shapes)
    }

    let clock_state = |ecs: &Ecs, entity: Entity| {
        let world = ecs.world().resource_ref::<WorldClock>();
        let local = ecs.world().entity(entity).get_ref::<EntityClock>().unwrap();
        (
            world.0.now(),
            world.0.last_delta(),
            world.0.effective_scale(),
            world.last_changed(),
            local.0.now(),
            local.0.last_delta(),
            local.0.hitstop_ticks(),
            local.last_changed(),
        )
    };

    let prepare = |ecs: &mut Ecs, entity: Entity| {
        let state = clock_state(ecs, entity);
        let tick = ecs.world().read_change_tick();
        ecs.prepare_clock_query();
        assert_eq!(clock_state(ecs, entity), state);
        assert_eq!(ecs.world().read_change_tick(), tick);
    };

    let (mut cold, cold_entity, cold_shapes) = scene();
    let (mut prepared, prepared_entity, prepared_shapes) = scene();
    prepare(&mut prepared, prepared_entity);
    assert_eq!(*prepared.world().resource::<Seen>(), Seen { starts: 1, run_clocks: true, ..Seen::default() });

    let ((), cold_counts) = alloc::count_in_scope(|| cold.fixed_step());
    let ((), prepared_counts) = alloc::count_in_scope(|| prepared.fixed_step());
    assert!(alloc::is_installed());
    assert!(cold_counts.allocations > 0, "late clock-bearing shapes must exercise query growth");
    assert_eq!(prepared_counts.allocations, 0, "prepared first fixed step: {prepared_counts:?}");

    let expected = Seen { starts: 1, before: 0, after: 1, changed: 1, run_clocks: true };
    let step_after_more_shapes = |ecs: &mut Ecs, entity: Entity, shapes: [Entity; 8], refresh: bool| {
        assert_eq!(*ecs.world().resource::<Seen>(), expected);
        let local = &ecs.world().get::<EntityClock>(entity).unwrap().0;
        assert_eq!(local.hitstop_ticks(), 0);
        assert_eq!(local.last_delta(), SimDuration::ZERO);
        for shape in shapes {
            let local = &ecs.world().get::<EntityClock>(shape).unwrap().0;
            assert_eq!(local.last_delta(), SimDuration::from_ticks(1));
        }
        ecs.world_mut().resource_mut::<Seen>().run_clocks = false;
        ecs.fixed_step();
        assert_eq!(*ecs.world().resource::<Seen>(), Seen { before: 1, run_clocks: false, ..expected });

        // Grow the same query again after it has actually run, at a non-default scale.
        let new_shapes = [
            spawn::<8> as fn(&mut Ecs) -> Entity,
            spawn::<9>,
            spawn::<10>,
            spawn::<11>,
            spawn::<12>,
            spawn::<13>,
            spawn::<14>,
            spawn::<15>,
        ]
        .map(|spawn| spawn(ecs));
        let world = ecs.world_mut();
        world.resource_mut::<Seen>().run_clocks = true;
        world.resource_mut::<WorldClock>().0.push_scale(ScaleSource::PerfectDodge, 500).unwrap();
        world.get_mut::<EntityClock>(entity).unwrap().0.apply_hitstop(2);
        if refresh {
            prepare(ecs, entity);
        }
        let ((), counts) = alloc::count_in_scope(|| ecs.fixed_step());
        assert_eq!(*ecs.world().resource::<Seen>(), Seen { before: 1, after: 2, ..expected });
        assert_eq!(ecs.world().get::<EntityClock>(entity).unwrap().0.hitstop_ticks(), 1);
        let delta = SimDuration::from_subticks(SimDuration::from_ticks(1).as_subticks() / 2);
        assert_eq!(ecs.world().resource::<WorldClock>().0.last_delta(), delta);
        for shape in new_shapes {
            assert_eq!(ecs.world().get::<EntityClock>(shape).unwrap().0.last_delta(), delta);
        }
        counts.allocations
    };

    let cold_again = step_after_more_shapes(&mut cold, cold_entity, cold_shapes, false);
    let prepared_again = step_after_more_shapes(&mut prepared, prepared_entity, prepared_shapes, true);
    assert!(cold_again > 0, "the second group must exercise query growth");
    assert_eq!(prepared_again, 0, "the fixed step after repeated preparation");
}
