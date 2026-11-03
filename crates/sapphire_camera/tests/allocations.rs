//! No heap allocation per frame once built. Installs `CountingAllocator` as this test binary's global
//! allocator, and counts from the very first frame: the director has nothing to warm up.

#![allow(clippy::print_stdout, reason = "with --nocapture, the measurements the crate's docs quote")]

mod common;

use std::alloc::System;

use common::{Moment, arena, combat_director, fight, ground_with, sweeping_look};
use sapphire_camera::{CameraDirector, Rig};
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_math::Vec3;
use sapphire_physics::Shape;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// A minute of everything a frame can do: rigs changing, shakes firing past the stack's size, a target
/// coming and going, collisions, a pause, a cut and a takeover.
fn busy_minute(camera: &mut CameraDirector, world: &sapphire_physics::PhysicsWorld) {
    let shake = camera.tuning().shake_id("hit_heavy").expect("a shipped preset");
    let dt = 1.0 / 60.0;
    for frame in 1..=3600_u32 {
        let t = frame as f32 * dt;
        if frame % 2 == 0 {
            camera.fire_shake(shake, 1.0);
        }
        if frame % 200 == 0 {
            camera.set_rig(Rig::LockOn, frame % 400 == 0);
        }
        if frame % 500 == 0 {
            camera.recenter();
        }
        if frame % 1100 == 0 {
            camera.cut();
        }
        if frame % 900 == 0 {
            camera.set_takeover(Some(camera.pose()));
        }
        if frame % 900 == 300 {
            camera.set_takeover(None);
        }
        let mut moment = fight(t);
        moment.look_rate = sweeping_look(t);
        if frame % 1300 < 100 {
            moment.target = None;
        }
        let step = if frame % 800 < 10 { 0.0 } else { dt };
        let pose = camera.update(&moment.frame(step), world);
        std::hint::black_box(pose);
        std::hint::black_box(camera.heading());
    }
}

#[test]
fn a_frame_allocates_nothing() {
    assert!(alloc::is_installed());
    if !PER_THREAD_COUNTS {
        return;
    }
    let world = arena();
    let mut camera = combat_director();

    let ((), counts) = alloc::count_in_scope(|| busy_minute(&mut camera, &world));

    assert_eq!(counts.allocations, 0, "{} bytes allocated in a minute of frames", counts.bytes);
    let stats = camera.take_stats();
    println!("{stats:?}");
    assert!(stats.frames > 3000, "{stats:?}");
    assert!(stats.casts > 3000, "{stats:?}");
    assert!(stats.shakes_replaced > 0, "the stack must have been full: {stats:?}");
}

/// A view turning round what's behind it, and comparing views to do it, allocates nothing either.
#[test]
fn turning_round_a_wall_allocates_nothing() {
    assert!(alloc::is_installed());
    if !PER_THREAD_COUNTS {
        return;
    }
    let world =
        ground_with(&[(Shape::Box { half_extents: Vec3::new(3.0, 2.5, 0.8) }, Vec3::new(0.0, 2.5, -1.8))]);
    let mut camera = combat_director();
    let script =
        |t: f32| Moment::against(Vec3::new(2.0 * (t * 0.5).sin(), 0.0, 0.0), Vec3::new(0.0, 0.0, 5.0));

    let ((), counts) = alloc::count_in_scope(|| {
        for frame in 1..=3600 {
            let pose = camera.update(&script(frame as f32 / 60.0).frame(1.0 / 60.0), &world);
            std::hint::black_box(pose);
        }
    });

    assert_eq!(counts.allocations, 0, "{} bytes allocated", counts.bytes);
    let stats = camera.take_stats();
    assert!(stats.swings > 0, "it has to have compared views: {stats:?}");
}

/// The walls and pillars of a scene that keeps the arm busy allocate nothing either.
#[test]
fn giving_way_allocates_nothing() {
    assert!(alloc::is_installed());
    if !PER_THREAD_COUNTS {
        return;
    }
    let world = ground_with(&[
        (Shape::Box { half_extents: Vec3::new(30.0, 5.0, 1.0) }, Vec3::new(0.0, 5.0, -2.5)),
        (Shape::Capsule { half_height: 2.0, radius: 0.35 }, Vec3::new(3.0, 2.35, -0.5)),
    ]);
    let mut camera = common::director();
    let script = |t: f32| Moment {
        look_rate: sapphire_math::Vec2::new(-1.2, 0.2 * t.sin()),
        ..Moment::alone(Vec3::new(1.5 * t.sin(), 0.0, 0.5))
    };

    let ((), counts) = alloc::count_in_scope(|| {
        for frame in 1..=3600 {
            let pose = camera.update(&script(frame as f32 / 60.0).frame(1.0 / 60.0), &world);
            std::hint::black_box(pose);
        }
    });

    assert_eq!(counts.allocations, 0, "{} bytes allocated", counts.bytes);
    let stats = camera.take_stats();
    assert!(stats.contacts > 0 && stats.pulled_frames > 0, "{stats:?}");
}
