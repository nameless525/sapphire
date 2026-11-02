//! The simulation allocates nothing per frame once warmed (spec §5.16, rule R6's budget): a whole
//! duel's worth of effects spawned, stepped and resolved, again and again, under the counting
//! allocator. Effects spawn from inside the fixed step and the frame's write, so neither may touch
//! the heap.

use glam::Vec3;
use sapphire_core::alloc::{CountingAllocator, count_in_scope};
use sapphire_look::VfxFrame;
use sapphire_vfx::{EffectLibrary, OwnerId, OwnerSteps, Placement, VfxBudget, VfxWorld};

#[global_allocator]
static ALLOCATOR: CountingAllocator<std::alloc::System> = CountingAllocator::new(std::alloc::System);

const TICK: f32 = 1.0 / 60.0;

#[test]
fn stepping_spawning_and_resolving_allocate_nothing_once_warmed() {
    let library = EffectLibrary::default_effects();
    let spark = library.find("hit_spark").expect("an effect");
    let sword = library.find("sword_trail").expect("an effect");
    let dust = library.find("dust_puff").expect("an effect");
    let mut world = VfxWorld::new(library, VfxBudget::DESKTOP_LOW, 7);
    let mut frame = VfxFrame::default();
    let (attacker, victim) = (OwnerId(1), OwnerId(2));
    let mut blade = None;

    // One cycle of the show: a swing with its trail, sampled at four sub-ticks, a hit with a
    // hitstop that freezes both fighters, and dust at the feet.
    let mut tick = 0_u32;
    let mut play = |world: &mut VfxWorld, frame: &mut VfxFrame, ticks: u32| {
        for _ in 0..ticks {
            let moment = tick % 60;
            let frozen = (20..26).contains(&moment);
            let held = [(attacker, 0.0), (victim, 0.0)];
            let owners: &[(OwnerId, f32)] = if frozen { &held } else { &[] };
            world.advance(&OwnerSteps { world: TICK, owners });
            if moment == 0 {
                blade = world.spawn(sword, attacker, Placement::default());
                world.spawn(dust, OwnerId::WORLD, Placement::at(Vec3::new(1.0, 0.0, 0.0)));
            }
            if moment == 20 {
                for _ in 0..8 {
                    world.spawn(spark, victim, Placement::at(Vec3::Y));
                }
            }
            if let Some(blade) = blade
                && !frozen
            {
                for sub in 0..4 {
                    let angle = (tick * 4 + sub) as f32 * 0.02;
                    let ago = TICK * (3 - sub) as f32 / 4.0;
                    let (sin, cos) = angle.sin_cos();
                    world.sample_trail(
                        blade,
                        Vec3::new(cos, sin, 0.0) * 0.3,
                        Vec3::new(cos, sin, 0.0) * 1.2,
                        ago,
                    );
                }
            }
            if moment == 40
                && let Some(blade) = blade.take()
            {
                world.stop(blade);
            }
            // Between two samples, where a frame's strip ends on a stretch of its own.
            world.write_frame(frame, 0.5);
            tick += 1;
        }
    };

    play(&mut world, &mut frame, 240);
    let peak = world.stats();
    let ((), counts) = count_in_scope(|| play(&mut world, &mut frame, 600));
    assert_eq!(counts.allocations, 0, "after warming, {peak:?}");
    assert!(frame.stats.dropped_particles == 0 && frame.stats.dropped_ribbon_points == 0);
}

/// The frame's capacity is all that warms: a frame holding a duel's capacity needs nothing else,
/// from the very first frame, in a world that's only just been made.
#[test]
fn a_new_world_needs_only_its_frames_capacity() {
    let library = EffectLibrary::default_effects();
    let spark = library.find("hit_spark").expect("an effect");
    let mut world = VfxWorld::new(library, VfxBudget::DESKTOP_LOW, 7);
    let mut frame = VfxFrame::default();
    frame.sprites.reserve(256);
    frame.ribbon_points.reserve(256);
    let ((), counts) = count_in_scope(|| {
        for _ in 0..4 {
            world.spawn(spark, OwnerId::WORLD, Placement::default());
        }
        for _ in 0..8 {
            world.advance(&OwnerSteps { world: TICK, owners: &[] });
            world.write_frame(&mut frame, 1.0);
        }
    });
    assert_eq!(counts.allocations, 0);
    assert!(!frame.sprites.is_empty());
}
