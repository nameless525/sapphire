//! Rule R6: a fixed step makes zero heap allocations on the simulation thread, including the
//! work bevy's single-threaded executor does to run the four fixed schedules. Installs
//! `CountingAllocator` as this test binary's global allocator.

use std::alloc::System;

use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_ecs::prelude::*;

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
