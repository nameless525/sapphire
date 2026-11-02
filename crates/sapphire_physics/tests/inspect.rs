//! Looking at a world: what tools read is what went in, and reading changes nothing.

use std::alloc::System;

use glam::{Quat, Vec3};
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_physics::{
    BodyDesc, BodyId, ColliderId, CollisionGroups, Group, GroupMask, PhysicsStats, PhysicsWorld, Pose,
    QueryFilter, Shape, StaticCollider, StaticGeometryView, StaticShape,
};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const TRIANGLE: [Vec3; 3] = [Vec3::ZERO, Vec3::X, Vec3::Z];

fn turned() -> Pose {
    Pose::new(Vec3::new(1.0, 2.0, 3.0), Quat::from_xyzw(0.0, 0.6, 0.0, 0.8))
}

fn world() -> PhysicsWorld {
    let statics = vec![
        StaticCollider {
            id: ColliderId(7),
            shape: StaticShape::Mesh { vertices: TRIANGLE.to_vec(), indices: vec![[0, 1, 2]] },
            pose: Pose::at(Vec3::Y),
            memberships: Group::World.mask(),
        },
        StaticCollider {
            id: ColliderId(1),
            shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(1.0, 0.5, 2.0) }),
            pose: turned(),
            memberships: Group::World.mask(),
        },
        StaticCollider {
            id: ColliderId(2),
            shape: StaticShape::Convex(Shape::Capsule { half_height: 0.5, radius: 0.25 }),
            pose: Pose::IDENTITY,
            memberships: Group::CameraBlocker.mask(),
        },
        StaticCollider {
            id: ColliderId(3),
            shape: StaticShape::Convex(Shape::Sphere { radius: 2.0 }),
            pose: Pose::at(Vec3::X),
            memberships: Group::World.mask(),
        },
    ];
    let mut world = PhysicsWorld::new(statics, 4).unwrap();
    for (id, shape, pose) in [
        (BodyId(9), Shape::Capsule { half_height: 0.5, radius: 0.3 }, Pose::at(Vec3::Z)),
        (BodyId(2), Shape::Sphere { radius: 0.2 }, turned()),
    ] {
        let groups = CollisionGroups::new(Group::Character.mask(), GroupMask::NONE);
        world.add_body(BodyDesc { id, shape, pose, groups }).unwrap();
    }
    world
}

#[test]
fn statics_read_as_they_were_built_in_id_order() {
    let world = world();
    let statics: Vec<_> = world.statics().collect();
    assert_eq!(statics.iter().map(|view| view.id.0).collect::<Vec<_>>(), [1, 2, 3, 7]);
    assert_eq!(
        statics[0].geometry,
        StaticGeometryView::Convex(Shape::Box { half_extents: Vec3::new(1.0, 0.5, 2.0) })
    );
    assert_eq!(statics[0].pose, turned());
    assert_eq!(
        statics[1].geometry,
        StaticGeometryView::Convex(Shape::Capsule { half_height: 0.5, radius: 0.25 })
    );
    assert_eq!(statics[1].memberships, Group::CameraBlocker.mask());
    assert_eq!(statics[2].geometry, StaticGeometryView::Convex(Shape::Sphere { radius: 2.0 }));
    let StaticGeometryView::Mesh { vertices, triangles } = statics[3].geometry else {
        panic!("{:?}", statics[3].geometry);
    };
    assert_eq!((vertices, triangles), (&TRIANGLE[..], &[[0, 1, 2]][..]));
    assert_eq!(statics[3].pose, Pose::at(Vec3::Y));
}

#[test]
fn bodies_read_as_they_stand_now_in_id_order() {
    let mut world = world();
    let moved = Pose::at(Vec3::new(4.0, 0.0, 0.0));
    world.set_body_pose(BodyId(9), moved).unwrap();
    let bodies: Vec<_> = world.bodies().collect();
    assert_eq!(bodies.iter().map(|body| body.id.0).collect::<Vec<_>>(), [2, 9]);
    assert_eq!(bodies[0].shape, Shape::Sphere { radius: 0.2 });
    assert_eq!(bodies[0].pose, turned());
    assert_eq!(bodies[1].pose, moved);
    assert_eq!(bodies[1].groups.memberships, Group::Character.mask());
}

fn canonical(world: &PhysicsWorld) -> Vec<u8> {
    let mut bytes = Vec::new();
    world.write_canonical(&mut |chunk| bytes.extend_from_slice(chunk));
    bytes
}

/// A view that looks every frame can't move a checksum, or fill the stats with its own work.
#[test]
fn looking_changes_no_state_and_no_counter_and_allocates_nothing() {
    assert!(alloc::is_installed());
    let world = world();
    // What a query does to the counters, to show they'd see a read that counted.
    let filter = QueryFilter::new(Group::World.mask());
    world.cast_ray(Vec3::new(0.0, 10.0, 0.0), Vec3::NEG_Y, 100.0, &filter);
    assert_ne!(world.take_stats().ray_casts, 0, "a query counts");

    let before = canonical(&world);
    let ((statics, bodies), counts) = alloc::count_in_scope(|| {
        let mut seen = (0, 0);
        for view in world.statics() {
            seen.0 += 1 + usize::from(matches!(view.geometry, StaticGeometryView::Mesh { .. }));
        }
        for body in world.bodies() {
            seen.1 += usize::from(body.shape.is_valid());
        }
        seen
    });
    assert_eq!((statics, bodies), (5, 2));
    if PER_THREAD_COUNTS {
        assert_eq!(counts.allocations, 0);
    }
    assert_eq!(canonical(&world), before);
    let at_rest = PhysicsStats { statics: 4, bodies: 2, ..PhysicsStats::default() };
    assert_eq!(world.take_stats(), at_rest, "looking isn't work");
}
