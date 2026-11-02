//! Rule R6: the fixed step never allocates. Installs `CountingAllocator` as this test binary's
//! global allocator. The answer to M2 D3's second question.

mod common;

use std::alloc::System;

use common::{CHARACTERS, Script, world};
use glam::Vec3;
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_physics::{
    BodyDesc, BodyId, CollisionGroups, Group, GroupMask, MoveOptions, Pose, QueryFilter, Shape,
};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// Two capsules against 226 colliders for 1,000 steps with 20 queries each, counted from the very
/// first step: nothing here needs warming up.
#[test]
fn a_thousand_steps_allocate_nothing() {
    assert!(alloc::is_installed());
    if !PER_THREAD_COUNTS {
        return;
    }
    let mut world = world();
    let mut script = Script::default();
    let mut hits = 0u64;

    let ((), counts) = alloc::count_in_scope(|| {
        for tick in 0..1000 {
            script.step(&mut world, tick, &mut |bytes| hits += bytes.len() as u64);
        }
    });

    assert_eq!(counts.allocations, 0, "{} bytes allocated in the fixed step", counts.bytes);
    assert!(hits > 100_000, "the script must have done something: {hits} bytes logged");
    let stats = world.take_stats();
    assert_eq!(stats.moves, 2000);
    assert_eq!(stats.ray_casts, 8000);
    assert_eq!(stats.overlap_queries, 5000);
    assert!(stats.move_hits > 500, "the capsules must meet things: {stats:?}");
    assert!(stats.overlaps_resolved > 0, "the script must exercise overlaps: {stats:?}");
}

/// The cases `parry3d` runs EPA for, whose `Vec`s the crate's own contact code exists to avoid.
#[test]
fn overlapping_shapes_allocate_nothing() {
    assert!(alloc::is_installed());
    if !PER_THREAD_COUNTS {
        return;
    }
    let mut world = world();
    // In the ground and in the first pillar's way, in and past a box on the grid, in the mesh.
    let starts = [
        Vec3::new(0.0, 0.3, 0.0),
        Vec3::new(-2.0, 0.2, -2.0),
        Vec3::new(2.0, 0.9, 2.0),
        Vec3::new(6.0, 0.5, 6.0),
        Vec3::new(-10.0, 0.1, -10.0),
    ];
    let everything = QueryFilter::new(GroupMask::ALL).excluding(CHARACTERS[0]);
    let sphere = Shape::Sphere { radius: 0.5 };
    let cube = Shape::Box { half_extents: Vec3::splat(0.5) };
    let mut deepest = 0.0f32;

    let ((), counts) = alloc::count_in_scope(|| {
        for start in starts {
            let pose = Pose::at(start);
            for shape in [&sphere, &cube, &common::CAPSULE] {
                std::hint::black_box(world.cast_shape(shape, pose, Vec3::NEG_Y, 1.0, &everything));
                std::hint::black_box(world.cast_shape(shape, pose, Vec3::Y, 1.0, &everything));
                std::hint::black_box(world.cast_shape(shape, pose, Vec3::X, 1.0, &everything));
                std::hint::black_box(world.overlap(shape, pose, &everything, &mut []));
                if let Some(found) = world.penetration(shape, pose, &everything) {
                    deepest = deepest.max(found.depth);
                }
            }
            world.set_body_pose(CHARACTERS[0], pose).expect("a valid pose");
            let outcome = world
                .move_and_collide(CHARACTERS[0], Vec3::new(0.5, -0.5, 0.0), &MoveOptions::default())
                .expect("a capsule moves");
            std::hint::black_box(outcome);
        }
    });

    assert_eq!(counts.allocations, 0, "{} bytes allocated resolving overlaps", counts.bytes);
    assert!(deepest > 0.1, "these starts must overlap: deepest {deepest}");
}

/// A fixed-step spawn, within the capacity the world was built with.
#[test]
fn bodies_come_and_go_without_allocating() {
    assert!(alloc::is_installed());
    if !PER_THREAD_COUNTS {
        return;
    }
    let mut world = world();
    let free = world.body_capacity() - world.body_count();
    assert!(free >= 4);
    let body = |id| BodyDesc {
        id: BodyId(id),
        shape: Shape::Sphere { radius: 0.3 },
        pose: Pose::at(Vec3::new(0.0, 5.0, 0.0)),
        groups: CollisionGroups::new(Group::Hurtbox.mask(), GroupMask::NONE),
    };

    let ((), counts) = alloc::count_in_scope(|| {
        for _ in 0..100 {
            for id in [500, 400, 600, 300] {
                world.add_body(body(id)).expect("within capacity");
            }
            for id in [400, 300, 600, 500] {
                assert!(world.remove_body(BodyId(id)));
            }
        }
    });

    assert_eq!(counts.allocations, 0, "{} bytes allocated by spawns", counts.bytes);
}
