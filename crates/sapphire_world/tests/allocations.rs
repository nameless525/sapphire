//! Time of day allocates nothing per step or per frame: ticking runs inside the fixed step (rule
//! R6), and resolving runs every frame. Installs `CountingAllocator` as this test binary's
//! global allocator.

use std::alloc::System;
use std::sync::Arc;

use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_math::Vec3;
use sapphire_render::FramePacket;
use sapphire_time::ScaledClock;
use sapphire_world::tod::{ShotTime, TimeOfDay, TodCutscene, TodKey, TodTimeline, TodVolume, VolumeShape};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

#[test]
fn ticking_and_resolving_allocate_nothing() {
    let mut world = ScaledClock::new();
    let clock = world.new_local_clock();
    let mut tod = TimeOfDay::new(TodTimeline::default_day(), clock, 17.0).expect("complete");
    tod.clock_mut().set_day_length(Some(60.0));
    let zone = TodTimeline::new(vec![TodKey { exposure: Some(1.0), ..TodKey::default() }]).expect("valid");
    let shape = VolumeShape::Sphere { center: Vec3::ZERO, radius: 2.0 };
    tod.add_volume(TodVolume { blend_distance: 4.0, ..TodVolume::new(shape, Arc::new(zone)) });
    let shot =
        TodCutscene::new(1.0, 0.25, 0.25, Some(ShotTime { hours: 21.0, hours_per_second: 0.0 }), Vec::new())
            .expect("valid");
    tod.play_cutscene(Arc::new(shot));
    let mut packet = FramePacket::default();

    let ((), counts) = alloc::count_in_scope(|| {
        // A minute of frames: through two keys, in and out of the zone, the whole shot.
        for frame in 0..3600_u32 {
            world.tick();
            tod.tick(&world);
            let probe = Vec3::new((frame % 600) as f32 * 0.01, 0.0, 0.0);
            tod.resolve(probe, 0.5).write(&mut packet);
        }
    });
    if PER_THREAD_COUNTS {
        assert_eq!(counts.allocations, 0, "rule R6");
    }
    assert!(tod.stats().resolves >= 3600);
}
