//! Each query and move against answers worked out by hand, in the comments beside them.

use glam::{Quat, Vec3};
use sapphire_physics::{
    BodyDesc, BodyId, ColliderId, CollisionGroups, Group, GroupMask, HitTarget, MoveOptions, Overlaps,
    PhysicsStats, PhysicsWorld, Pose, QueryFilter, Shape, ShapeHit, StaticCollider, StaticShape, WorldError,
};

const ALL: QueryFilter = QueryFilter::new(GroupMask::ALL);
const TOLERANCE: f32 = 2e-4;

#[track_caller]
fn assert_close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < TOLERANCE, "{actual} is not {expected}");
}

#[track_caller]
fn assert_near(actual: Vec3, expected: Vec3) {
    assert!((actual - expected).length() < TOLERANCE, "{actual} is not {expected}");
}

fn collider(id: u32, shape: Shape, position: Vec3) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Convex(shape),
        pose: Pose::at(position),
        memberships: Group::World.mask(),
    }
}

/// A horizontal triangle big enough to count as a plane near the origin, at height `y`.
fn plane_mesh(id: u32, y: f32) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Mesh {
            vertices: vec![
                Vec3::new(-100.0, 0.0, -100.0),
                Vec3::new(-100.0, 0.0, 300.0),
                Vec3::new(300.0, 0.0, -100.0),
            ],
            indices: vec![[0, 1, 2]],
        },
        pose: Pose::at(Vec3::new(0.0, y, 0.0)),
        memberships: Group::World.mask(),
    }
}

const UNIT_BOX: Shape = Shape::Box { half_extents: Vec3::ONE };
const SPHERE: Shape = Shape::Sphere { radius: 0.5 };
const CAPSULE: Shape = Shape::Capsule { half_height: 1.0, radius: 0.5 };

fn character(id: u32, position: Vec3) -> BodyDesc {
    BodyDesc {
        id: BodyId(id),
        shape: CAPSULE,
        pose: Pose::at(position),
        groups: CollisionGroups::new(Group::Character.mask(), Group::World | Group::Character),
    }
}

fn world(statics: Vec<StaticCollider>) -> PhysicsWorld {
    PhysicsWorld::new(statics, 8).expect("the test's colliders build")
}

// --- Rays ---

#[test]
fn a_ray_hits_a_box_top() {
    // From (0, 5, 0) straight down to the top of a 1 m half-extent box at the origin: y = 1, 4 away.
    let world = world(vec![collider(7, UNIT_BOX, Vec3::ZERO)]);
    let hit = world.cast_ray(Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y, 100.0, &ALL).expect("a hit");
    assert_eq!(hit.target, HitTarget::Static(ColliderId(7)));
    assert_close(hit.distance, 4.0);
    assert_near(hit.point, Vec3::Y);
    assert_near(hit.normal, Vec3::Y);
}

#[test]
fn a_ray_hits_a_sphere_and_a_capsule() {
    // A 2 m sphere at x = 10 is met at x = 8. A capsule of radius 0.5 at z = 5 is met at z = 4.5.
    let world = world(vec![
        collider(1, Shape::Sphere { radius: 2.0 }, Vec3::new(10.0, 0.0, 0.0)),
        collider(2, CAPSULE, Vec3::new(0.0, 0.0, 5.0)),
    ]);
    let sphere = world.cast_ray(Vec3::ZERO, Vec3::X, 100.0, &ALL).expect("a hit");
    assert_eq!(sphere.target, HitTarget::Static(ColliderId(1)));
    assert_close(sphere.distance, 8.0);
    assert_near(sphere.normal, Vec3::NEG_X);
    let capsule = world.cast_ray(Vec3::ZERO, Vec3::Z, 100.0, &ALL).expect("a hit");
    assert_eq!(capsule.target, HitTarget::Static(ColliderId(2)));
    assert_close(capsule.distance, 4.5);
    assert_near(capsule.normal, Vec3::NEG_Z);
    // Over the capsule's cylinder, its cap: y = 1 is the cap's center, so at y = 1.3 the sphere of
    // radius 0.5 reaches sqrt(0.25 - 0.09) = 0.4 toward the ray, which meets it at z = 4.6.
    let cap = world.cast_ray(Vec3::new(0.0, 1.3, 0.0), Vec3::Z, 100.0, &ALL).expect("a hit");
    assert_close(cap.distance, 4.6);
}

#[test]
fn a_ray_meets_a_turned_box_on_its_face() {
    // A box at (5, 0, 5) turned 45° about Y has faces square to the diagonal. A ray along that
    // diagonal starts 5√2 from its center and meets the face 1 m short of it.
    // The quaternion's half angle is 22.5°: its sine and cosine, written out.
    let rotation = Quat::from_xyzw(0.0, 0.382_683_43, 0.0, 0.923_879_5);
    let world = world(vec![StaticCollider {
        pose: Pose::new(Vec3::new(5.0, 0.0, 5.0), rotation),
        ..collider(1, UNIT_BOX, Vec3::ZERO)
    }]);
    let diagonal = Vec3::new(1.0, 0.0, 1.0).normalize();
    let hit = world.cast_ray(Vec3::ZERO, diagonal, 100.0, &ALL).expect("a hit");
    assert_close(hit.distance, 5.0 * std::f32::consts::SQRT_2 - 1.0);
    assert_near(hit.normal, -diagonal);
}

#[test]
fn a_ray_hits_a_mesh_from_either_side() {
    // Triangles are two-sided: from 3 m above or 2 m below the plane at y = 0.
    let world = world(vec![plane_mesh(1, 0.0)]);
    let above = world.cast_ray(Vec3::new(1.0, 3.0, 1.0), Vec3::NEG_Y, 100.0, &ALL).expect("from above");
    assert_close(above.distance, 3.0);
    assert_near(above.point, Vec3::new(1.0, 0.0, 1.0));
    assert_near(above.normal, Vec3::Y);
    let below = world.cast_ray(Vec3::new(1.0, -2.0, 1.0), Vec3::Y, 100.0, &ALL).expect("from below");
    assert_close(below.distance, 2.0);
    assert_near(below.normal, Vec3::NEG_Y);
}

#[test]
fn a_ray_reports_the_nearest_of_statics_and_bodies() {
    let mut world = world(vec![collider(1, UNIT_BOX, Vec3::new(10.0, 0.0, 0.0))]);
    world.add_body(character(5, Vec3::new(4.0, 0.0, 0.0))).expect("fits");
    // The capsule's surface at x = 3.5 comes before the box's at x = 9.
    let hit = world.cast_ray(Vec3::ZERO, Vec3::X, 100.0, &ALL).expect("a hit");
    assert_eq!(hit.target, HitTarget::Body(BodyId(5)));
    assert_close(hit.distance, 3.5);
    // Asked to skip that body, the box.
    let hit = world.cast_ray(Vec3::ZERO, Vec3::X, 100.0, &ALL.excluding(BodyId(5))).expect("a hit");
    assert_eq!(hit.target, HitTarget::Static(ColliderId(1)));
    assert_close(hit.distance, 9.0);
    // A static nearer than the body wins too.
    world.set_body_pose(BodyId(5), Pose::at(Vec3::new(20.0, 0.0, 0.0))).expect("valid");
    let hit = world.cast_ray(Vec3::ZERO, Vec3::X, 100.0, &ALL).expect("a hit");
    assert_eq!(hit.target, HitTarget::Static(ColliderId(1)));
}

#[test]
fn a_ray_respects_range_groups_and_direction() {
    let world = world(vec![collider(1, UNIT_BOX, Vec3::new(10.0, 0.0, 0.0))]);
    // The face is 9 away: not within 8.
    assert!(world.cast_ray(Vec3::ZERO, Vec3::X, 8.0, &ALL).is_none());
    assert!(world.cast_ray(Vec3::ZERO, Vec3::X, 9.5, &ALL).is_some());
    assert!(world.cast_ray(Vec3::ZERO, Vec3::NEG_X, 100.0, &ALL).is_none());
    assert!(world.cast_ray(Vec3::ZERO, Vec3::ZERO, 100.0, &ALL).is_none());
    assert!(world.cast_ray(Vec3::ZERO, Vec3::X, 100.0, &QueryFilter::new(Group::Hurtbox.mask())).is_none());
    // A direction needn't be unit length.
    let hit = world.cast_ray(Vec3::ZERO, Vec3::X * 40.0, 100.0, &ALL).expect("a hit");
    assert_close(hit.distance, 9.0);
}

#[test]
fn a_ray_starting_inside_hits_at_once() {
    let world = world(vec![collider(1, UNIT_BOX, Vec3::ZERO), collider(2, SPHERE, Vec3::new(5.0, 0.0, 0.0))]);
    for (origin, direction) in [(Vec3::ZERO, Vec3::X), (Vec3::new(5.0, 0.1, 0.0), Vec3::Y)] {
        let hit = world.cast_ray(origin, direction, 100.0, &ALL).expect("a hit");
        assert_eq!(hit.distance, 0.0);
        assert_near(hit.point, origin);
        assert_near(hit.normal, -direction);
    }
}

// --- A query's range ---

fn in_the_world(id: u32, shape: Shape, position: Vec3) -> BodyDesc {
    BodyDesc {
        id: BodyId(id),
        shape,
        pose: Pose::at(position),
        groups: CollisionGroups::new(Group::Hurtbox.mask(), GroupMask::NONE),
    }
}

/// What every query says of one target: one world, and where to look from.
struct RangeCase {
    what: &'static str,
    world: PhysicsWorld,
    from: Vec3,
    along: Vec3,
}

fn range_cases() -> Vec<RangeCase> {
    let ball = Shape::Sphere { radius: 1.0 };
    let at = Vec3::new(3.0, 0.0, 0.0);
    let mut body = world(Vec::new());
    body.add_body(in_the_world(1, ball, at)).expect("fits");
    let mut bodies = world(vec![collider(9, UNIT_BOX, Vec3::new(0.0, 50.0, 0.0))]);
    bodies.add_body(in_the_world(1, ball, at)).expect("fits");
    vec![
        RangeCase {
            what: "a sphere alone in the tree",
            world: world(vec![collider(1, ball, at)]),
            from: Vec3::ZERO,
            along: Vec3::X,
        },
        RangeCase {
            what: "a sphere in a tree of two",
            world: world(vec![collider(1, ball, at), collider(2, ball, Vec3::new(0.0, 50.0, 0.0))]),
            from: Vec3::ZERO,
            along: Vec3::X,
        },
        RangeCase {
            what: "a box",
            world: world(vec![collider(1, UNIT_BOX, at)]),
            from: Vec3::ZERO,
            along: Vec3::X,
        },
        RangeCase {
            what: "a mesh",
            world: world(vec![plane_mesh(1, 2.0)]),
            from: Vec3::new(1.0, 7.0, 1.0),
            along: Vec3::NEG_Y,
        },
        RangeCase { what: "a body", world: body, from: Vec3::ZERO, along: Vec3::X },
        RangeCase { what: "a body beside a static", world: bodies, from: Vec3::ZERO, along: Vec3::X },
    ]
}

/// A range is the distance a query is asked about, its end included, for first hits as for all of
/// them: what touches at exactly that distance is within it, and a float short of it isn't.
#[test]
fn a_querys_range_includes_its_end() {
    let mut hits = [ShapeHit::NONE; 4];
    for case in range_cases() {
        let RangeCase { what, world, from, along } = case;
        let ray = |limit: f32| world.cast_ray(from, along, limit, &ALL).map(|hit| hit.distance);
        let touches = ray(100.0).unwrap_or_else(|| panic!("{what}: a ray should reach it"));
        assert_eq!(ray(touches), Some(touches), "{what}: a ray that ends on it");
        assert_eq!(ray(touches.next_up()), Some(touches), "{what}: a ray a float past it");
        assert_eq!(ray(touches.next_down()), None, "{what}: a ray a float short of it");

        let ball = Shape::Sphere { radius: 1.0 };
        let pose = Pose::at(from);
        let first = |limit: f32| world.cast_shape(&ball, pose, along, limit, &ALL).map(|hit| hit.distance);
        let touches = first(100.0).unwrap_or_else(|| panic!("{what}: a cast should reach it"));
        assert_eq!(first(touches), Some(touches), "{what}: a cast that ends on it");
        assert_eq!(first(touches.next_up()), Some(touches), "{what}: a cast a float past it");
        assert_eq!(first(touches.next_down()), None, "{what}: a cast a float short of it");
        let mut all = |limit: f32| {
            let found = world.cast_shape_all(&ball, pose, along, limit, &ALL, &mut hits);
            (found.written == 1).then_some(hits[0].distance)
        };
        assert_eq!(all(touches), Some(touches), "{what}: all hits, ending on it");
        assert_eq!(all(touches.next_up()), Some(touches), "{what}: all hits, a float past it");
        assert_eq!(all(touches.next_down()), None, "{what}: all hits, a float short of it");
    }
}

/// A range of nothing still says what a shape starts in or on, and the two kinds of query say the same.
#[test]
fn a_range_of_nothing_says_what_a_shape_starts_in() {
    let ball = Shape::Sphere { radius: 1.0 };
    let mut hits = [ShapeHit::NONE; 4];
    // The sphere at the origin against one at these distances, as a static and as a body.
    for (apart, found, what) in [(1.5, true, "overlapping"), (2.0, true, "touching"), (2.5, false, "apart")] {
        let at = Vec3::new(apart, 0.0, 0.0);
        let mut body = world(Vec::new());
        body.add_body(in_the_world(1, ball, at)).expect("fits");
        for world in [world(vec![collider(1, ball, at)]), body] {
            let first = world.cast_shape(&ball, Pose::IDENTITY, Vec3::X, 0.0, &ALL);
            assert_eq!(first.is_some(), found, "first hit, {what}");
            let all = world.cast_shape_all(&ball, Pose::IDENTITY, Vec3::X, 0.0, &ALL, &mut hits);
            assert_eq!(all.written == 1, found, "all hits, {what}");
            if let Some(first) = first {
                assert_eq!(first.distance, 0.0, "{what}");
                assert_eq!(first, hits[0], "{what}");
                assert_eq!(first.started_overlapping, apart < 2.0, "{what}");
            }
        }
    }
    // A ray from inside something, with no range at all.
    let world = world(vec![collider(1, ball, Vec3::ZERO)]);
    let hit = world.cast_ray(Vec3::ZERO, Vec3::X, 0.0, &ALL).expect("it starts inside");
    assert_eq!(hit.distance, 0.0);
}

/// Taking the end of a range in changes who wins no tie: a static over a body and a lower ID over a
/// higher, wherever the tie falls.
#[test]
fn ties_at_a_querys_limit_go_as_every_tie_does() {
    let ball = Shape::Sphere { radius: 1.0 };
    let (near, far) = (Vec3::new(3.0, 0.0, 0.0), Vec3::new(-3.0, 0.0, 0.0));
    // A ray from the origin touches each after 2, and a cast of this size after 1, both exactly: each
    // pair is where the limit falls on the tie, a float past it, and well past it.
    let limits = [(2.0, 1.0), (2.0_f32.next_up(), 1.0_f32.next_up()), (100.0, 100.0)];
    let hits_first = |world: &PhysicsWorld, target: HitTarget| {
        for (ray, cast) in limits {
            let by_ray = world.cast_ray(Vec3::ZERO, Vec3::X, ray, &ALL).expect("a ray reaches it");
            assert_eq!((by_ray.target, by_ray.distance), (target, 2.0), "within {ray}");
            let by_cast =
                world.cast_shape(&ball, Pose::IDENTITY, Vec3::X, cast, &ALL).expect("a cast reaches it");
            assert_eq!((by_cast.target, by_cast.distance), (target, 1.0), "within {cast}");
        }
    };
    // A static and a body, and the body's ID lower.
    let mut mixed = world(vec![collider(9, ball, near), collider(8, ball, far)]);
    mixed.add_body(in_the_world(1, ball, near)).expect("fits");
    hits_first(&mixed, HitTarget::Static(ColliderId(9)));
    // Two bodies, added high to low.
    let mut bodies = world(Vec::new());
    bodies.add_body(in_the_world(5, ball, near)).expect("fits");
    bodies.add_body(in_the_world(2, ball, near)).expect("fits");
    hits_first(&bodies, HitTarget::Body(BodyId(2)));
}

/// A move's reach goes four skins past its motion, and a surface exactly there is still past the
/// motion: it goes the whole way, and nothing was hit.
#[test]
fn a_surface_at_the_end_of_a_moves_reach_doesnt_stop_it() {
    let options = MoveOptions::default();
    let motion = 0.5;
    let touches_after = motion + 4.0 * options.skin;
    // A radius of 0.5 and of 1 touch 1.5 apart.
    let target = collider(1, Shape::Sphere { radius: 1.0 }, Vec3::new(touches_after + 1.5, 0.0, 0.0));
    let mut world = world(vec![target]);
    let body = BodyDesc {
        id: BodyId(1),
        shape: SPHERE,
        pose: Pose::IDENTITY,
        groups: CollisionGroups::new(Group::Character.mask(), Group::World.mask()),
    };
    world.add_body(body).expect("fits");
    let outcome = world.move_and_collide(BodyId(1), Vec3::X * motion, &options).expect("moves");
    assert!(outcome.hits().is_empty(), "{:?}", outcome.hits());
    assert_close(outcome.position.x, motion);
}

// --- Shape casts ---

#[test]
fn shapes_cast_down_onto_a_box() {
    let world = world(vec![collider(1, UNIT_BOX, Vec3::ZERO)]);
    // Each starts at y = 5 over a top at y = 1. A sphere's underside is 0.5 below its center, a
    // capsule's 1.5, a half-metre box's 0.5.
    let casts = [(SPHERE, 3.5), (CAPSULE, 2.5), (Shape::Box { half_extents: Vec3::splat(0.5) }, 3.5)];
    for (shape, expected) in casts {
        let hit = world
            .cast_shape(&shape, Pose::at(Vec3::new(0.0, 5.0, 0.0)), Vec3::NEG_Y, 10.0, &ALL)
            .expect("a hit");
        assert_eq!(hit.target, HitTarget::Static(ColliderId(1)));
        assert_close(hit.distance, expected);
        assert_close(hit.fraction, expected / 10.0);
        assert_near(hit.normal, Vec3::Y);
        assert_close(hit.point.y, 1.0);
        assert!(!hit.started_overlapping);
    }
}

#[test]
fn shapes_cast_against_round_targets() {
    let world = world(vec![
        collider(1, Shape::Sphere { radius: 2.0 }, Vec3::new(10.0, 0.0, 0.0)),
        collider(2, CAPSULE, Vec3::new(0.0, 0.0, 10.0)),
    ]);
    // Radii 1 and 2 with centers 10 apart touch after 7. The point is on the target, at x = 8.
    let ball = Shape::Sphere { radius: 1.0 };
    let hit = world.cast_shape(&ball, Pose::IDENTITY, Vec3::X, 20.0, &ALL).expect("a hit");
    assert_eq!(hit.target, HitTarget::Static(ColliderId(1)));
    assert_close(hit.distance, 7.0);
    assert_near(hit.normal, Vec3::NEG_X);
    assert_near(hit.point, Vec3::new(8.0, 0.0, 0.0));
    // Two upright capsules of radius 0.5, cylinders side by side, touch when 1 apart: after 9.
    let hit = world.cast_shape(&CAPSULE, Pose::IDENTITY, Vec3::Z, 20.0, &ALL).expect("a hit");
    assert_eq!(hit.target, HitTarget::Static(ColliderId(2)));
    assert_close(hit.distance, 9.0);
    assert_near(hit.normal, Vec3::NEG_Z);
}

#[test]
fn a_turned_cast_shape_uses_its_turn() {
    let world = world(vec![collider(1, UNIT_BOX, Vec3::ZERO)]);
    // A half turn about X keeps a capsule upright, so nothing changes: 2.5 as before. A quarter
    // turn about X (sin and cos of 45° below) lays it flat: its underside is only 0.5 down, so 3.5.
    let half_turn = Quat::from_xyzw(1.0, 0.0, 0.0, 0.0);
    let start = Vec3::new(0.0, 5.0, 0.0);
    let hit =
        world.cast_shape(&CAPSULE, Pose::new(start, half_turn), Vec3::NEG_Y, 10.0, &ALL).expect("a hit");
    assert_close(hit.distance, 2.5);
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let flat = Quat::from_xyzw(s, 0.0, 0.0, s);
    let hit = world.cast_shape(&CAPSULE, Pose::new(start, flat), Vec3::NEG_Y, 10.0, &ALL).expect("a hit");
    assert_close(hit.distance, 3.5);
}

#[test]
fn a_shape_cast_hits_a_mesh() {
    let world = world(vec![plane_mesh(1, 2.0)]);
    let hit = world
        .cast_shape(&CAPSULE, Pose::at(Vec3::new(1.0, 7.0, 1.0)), Vec3::NEG_Y, 10.0, &ALL)
        .expect("a hit");
    assert_close(hit.distance, 3.5); // 7 - 1.5 down to 2.
    assert_near(hit.normal, Vec3::Y);
    assert_close(hit.point.y, 2.0);
    // A turned mesh: a half turn about Z flips its triangle's X, so it spans x from -300 to 100 and
    // still holds the point below; and the normal still faces the cast.
    let flipped = StaticCollider {
        pose: Pose::new(Vec3::new(0.0, 2.0, 0.0), Quat::from_xyzw(0.0, 0.0, 1.0, 0.0)),
        ..plane_mesh(1, 2.0)
    };
    let turned = self::world(vec![flipped]);
    let hit = turned
        .cast_shape(&CAPSULE, Pose::at(Vec3::new(1.0, 7.0, 1.0)), Vec3::NEG_Y, 10.0, &ALL)
        .expect("a hit");
    assert_close(hit.distance, 3.5);
    assert_near(hit.normal, Vec3::Y);
}

#[test]
fn a_cast_starting_in_something_says_so_whichever_way_it_goes() {
    let world = world(vec![collider(1, UNIT_BOX, Vec3::ZERO)]);
    for direction in [Vec3::X, Vec3::Y, Vec3::NEG_Y] {
        for shape in [SPHERE, CAPSULE, Shape::Box { half_extents: Vec3::splat(0.25) }] {
            let hit = world
                .cast_shape(&shape, Pose::at(Vec3::new(0.0, 0.5, 0.0)), direction, 5.0, &ALL)
                .expect("a hit");
            assert_eq!(hit.distance, 0.0, "{shape:?}");
            assert!(hit.started_overlapping, "{shape:?}");
        }
    }
}

#[test]
fn cast_shape_all_sorts_and_keeps_the_nearest() {
    // Spheres along +x at 2, 4, 6 and 8, each 0.5 in radius. A 0.5 sphere from the origin touches
    // each 1 before its center: at 1, 3, 5 and 7. Listed out of order, so IDs and places differ.
    let mut world = world(vec![
        collider(4, SPHERE, Vec3::new(8.0, 0.0, 0.0)),
        collider(1, SPHERE, Vec3::new(4.0, 0.0, 0.0)),
        collider(3, SPHERE, Vec3::new(2.0, 0.0, 0.0)),
    ]);
    world
        .add_body(BodyDesc {
            id: BodyId(9),
            shape: SPHERE,
            pose: Pose::at(Vec3::new(6.0, 0.0, 0.0)),
            groups: CollisionGroups::new(Group::Hurtbox.mask(), GroupMask::NONE),
        })
        .expect("fits");

    let mut hits = [ShapeHit::NONE; 8];
    let found = world.cast_shape_all(&SPHERE, Pose::IDENTITY, Vec3::X, 20.0, &ALL, &mut hits);
    assert_eq!(found, Overlaps { found: 4, written: 4 });
    let order: Vec<HitTarget> = hits[..4].iter().map(|hit| hit.target).collect();
    assert_eq!(
        order,
        [
            HitTarget::Static(ColliderId(3)),
            HitTarget::Static(ColliderId(1)),
            HitTarget::Body(BodyId(9)),
            HitTarget::Static(ColliderId(4)),
        ]
    );
    for (hit, expected) in hits[..4].iter().zip([1.0, 3.0, 5.0, 7.0]) {
        assert_close(hit.distance, expected);
    }

    // Two fit: the two nearest stay, the count says what was lost, and the stats notice.
    let mut two = [ShapeHit::NONE; 2];
    world.take_stats();
    let found = world.cast_shape_all(&SPHERE, Pose::IDENTITY, Vec3::X, 20.0, &ALL, &mut two);
    assert_eq!(found, Overlaps { found: 4, written: 2 });
    assert_eq!(two[0].target, HitTarget::Static(ColliderId(3)));
    assert_eq!(two[1].target, HitTarget::Static(ColliderId(1)));
    assert_eq!(world.take_stats().results_dropped, 2);

    // Range cuts the list; a group narrows it to the hurtbox alone, as M2 D4 will ask.
    let found = world.cast_shape_all(&SPHERE, Pose::IDENTITY, Vec3::X, 3.5, &ALL, &mut hits);
    assert_eq!(found.found, 2);
    let hurtboxes = QueryFilter::new(Group::Hurtbox.mask());
    let found = world.cast_shape_all(&SPHERE, Pose::IDENTITY, Vec3::X, 20.0, &hurtboxes, &mut hits);
    assert_eq!(found.found, 1);
    assert_eq!(hits[0].target, HitTarget::Body(BodyId(9)));
    assert_close(hits[0].distance, 5.0);
}

// --- Overlaps and penetration ---

#[test]
fn overlaps_list_what_a_shape_touches() {
    // Box top at y = 1. A sphere of radius 1 reaches it from y = 2 exactly: 0.5 above stays clear,
    // 0.5 below overlaps.
    let mut world =
        world(vec![collider(2, UNIT_BOX, Vec3::ZERO), collider(1, UNIT_BOX, Vec3::new(1.0, 0.0, 0.0))]);
    world.add_body(character(3, Vec3::new(0.0, 1.5, 5.0))).expect("fits");
    let ball = Shape::Sphere { radius: 1.0 };
    let mut out = [HitTarget::Body(BodyId(0)); 4];
    assert_eq!(world.overlap(&ball, Pose::at(Vec3::new(0.0, 2.5, 0.0)), &ALL, &mut out).found, 0);
    let found = world.overlap(&ball, Pose::at(Vec3::new(0.0, 1.5, 0.0)), &ALL, &mut out);
    assert_eq!(found, Overlaps { found: 2, written: 2 });
    assert_eq!(
        out[..2],
        [HitTarget::Static(ColliderId(1)), HitTarget::Static(ColliderId(2))],
        "sorted by ID"
    );
    // Beside the capsule, whose axis stands at z = 5 with a radius of 0.5. Radii together reach 1.5:
    // from z = 3.75 they overlap, and from z = 3.25 they don't. The boxes are out of reach of both.
    let found = world.overlap(&ball, Pose::at(Vec3::new(0.0, 1.5, 3.75)), &ALL, &mut out);
    assert_eq!(found.found, 1);
    assert_eq!(out[0], HitTarget::Body(BodyId(3)));
    assert_eq!(world.overlap(&ball, Pose::at(Vec3::new(0.0, 1.5, 3.25)), &ALL, &mut out).found, 0);
    // Filters.
    let found =
        world.overlap(&ball, Pose::at(Vec3::new(0.0, 1.5, 3.75)), &ALL.excluding(BodyId(3)), &mut out);
    assert_eq!(found.found, 0);
    let found = world.overlap(
        &ball,
        Pose::at(Vec3::new(0.0, 1.5, 0.0)),
        &QueryFilter::new(Group::Hurtbox.mask()),
        &mut out,
    );
    assert_eq!(found.found, 0);
    // A buffer too small keeps the lowest IDs.
    let mut one = [HitTarget::Body(BodyId(0)); 1];
    let found = world.overlap(&ball, Pose::at(Vec3::new(0.0, 1.5, 0.0)), &ALL, &mut one);
    assert_eq!(found, Overlaps { found: 2, written: 1 });
    assert_eq!(one[0], HitTarget::Static(ColliderId(1)));
}

#[test]
fn overlaps_reach_mesh_triangles() {
    let world = world(vec![plane_mesh(1, 0.0)]);
    let mut out = [HitTarget::Body(BodyId(0)); 2];
    let ball = Shape::Sphere { radius: 1.0 };
    assert_eq!(world.overlap(&ball, Pose::at(Vec3::new(1.0, 1.25, 1.0)), &ALL, &mut out).found, 0);
    assert_eq!(world.overlap(&ball, Pose::at(Vec3::new(1.0, 0.75, 1.0)), &ALL, &mut out).found, 1);
    assert_eq!(world.overlap(&CAPSULE, Pose::at(Vec3::new(1.0, 1.25, 1.0)), &ALL, &mut out).found, 1);
}

#[test]
fn penetration_finds_the_way_out() {
    let world = world(vec![collider(1, UNIT_BOX, Vec3::ZERO)]);
    // A capsule's underside is 1.5 below its center: at y = 2.25 it's 0.25 into the top at y = 1.
    let found = world.penetration(&CAPSULE, Pose::at(Vec3::new(0.0, 2.25, 0.0)), &ALL).expect("overlaps");
    assert_eq!(found.target, HitTarget::Static(ColliderId(1)));
    assert_near(found.normal, Vec3::Y);
    assert_close(found.depth, 0.25);
    assert_near(found.point, Vec3::Y);
    // Beside the box: 0.5 radius at x = 1.25 is 0.25 into the +x face.
    let found = world.penetration(&CAPSULE, Pose::at(Vec3::new(1.25, 0.0, 0.0)), &ALL).expect("overlaps");
    assert_near(found.normal, Vec3::X);
    assert_close(found.depth, 0.25);
    // Core inside: at x = 0.75 the nearest face is +x, 0.25 away, plus the radius.
    let found = world.penetration(&SPHERE, Pose::at(Vec3::new(0.75, 0.0, 0.0)), &ALL).expect("overlaps");
    assert_near(found.normal, Vec3::X);
    assert_close(found.depth, 0.75);
    // Clear, and a box, which has no core to measure from.
    assert!(world.penetration(&CAPSULE, Pose::at(Vec3::new(0.0, 2.75, 0.0)), &ALL).is_none());
    assert!(world.penetration(&UNIT_BOX, Pose::at(Vec3::new(0.0, 1.5, 0.0)), &ALL).is_none());
}

#[test]
fn penetration_takes_the_deepest_and_reads_meshes() {
    let world = world(vec![plane_mesh(1, 0.0), collider(2, UNIT_BOX, Vec3::new(3.0, 1.0, 0.0))]);
    // A sphere of radius 0.5 at y = 0.25: into the plane by 0.25, and at x = 1.6 into the box's -x
    // face, at x = 2, by 0.1.
    let found = world.penetration(&SPHERE, Pose::at(Vec3::new(1.6, 0.25, 0.0)), &ALL).expect("overlaps");
    assert_eq!(found.target, HitTarget::Static(ColliderId(1)));
    assert_near(found.normal, Vec3::Y);
    assert_close(found.depth, 0.25);
    assert_near(found.point, Vec3::new(1.6, 0.0, 0.0));
}

/// What [`sapphire_physics::Penetration`] promises: moving along its normal by its depth separates the
/// shapes, from one target, wherever they stand. `what` says which pair, and its depth is returned.
#[track_caller]
fn depth_separates(what: &str, target: Shape, at: Pose, shape: Shape, pose: Pose) -> f32 {
    let collider = StaticCollider {
        id: ColliderId(1),
        shape: StaticShape::Convex(target),
        pose: at,
        memberships: Group::World.mask(),
    };
    let world = world(vec![collider]);
    let found = world.penetration(&shape, pose, &ALL).unwrap_or_else(|| panic!("{what}: they overlap"));
    assert!(found.depth > 0.0, "{what}: {found:?}");
    assert!((found.normal.length() - 1.0).abs() < 1e-5, "{what}: {found:?}");
    let out = Pose { position: pose.position + found.normal * (found.depth + 1e-4), ..pose };
    let left = world.penetration(&shape, out, &ALL);
    assert_eq!(left, None, "{what}: out by {} along {} wasn't enough", found.depth, found.normal);
    found.depth
}

fn capsule(half_height: f32, radius: f32) -> Shape {
    Shape::Capsule { half_height, radius }
}

/// Upright capsules on one line whose cores overlap: only their radii are in the way going sideways,
/// and going up they'd have their whole lengths to clear.
#[test]
fn capsules_along_one_line_leave_sideways_by_their_radii() {
    let character = capsule(0.5, 0.3);
    let rows = [
        ("thin and identical", capsule(0.55, 0.1), Vec3::ZERO, 0.2),
        ("characters, identical", character, Vec3::ZERO, 0.6),
        ("characters, one a little up", character, Vec3::new(0.0, 0.3, 0.0), 0.6),
        ("characters, one a little down", character, Vec3::new(0.0, -0.3, 0.0), 0.6),
        ("characters, cores only just overlapping", character, Vec3::new(0.0, 0.99, 0.0), 0.6),
    ];
    for (what, shape, offset, radii) in rows {
        let depth = depth_separates(what, shape, Pose::IDENTITY, shape, Pose::at(offset));
        assert_close(depth, radii);
    }
    // Different lengths and sizes.
    let depth = depth_separates(
        "long and short",
        capsule(10.0, 0.1),
        Pose::IDENTITY,
        capsule(0.5, 0.3),
        Pose::IDENTITY,
    );
    assert_close(depth, 0.4);
    // Far from the origin, and lying down on one line.
    let lying = Quat::from_xyzw(std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2);
    let at = Vec3::new(40.0, -3.0, 60.0);
    let depth = depth_separates(
        "lying",
        character,
        Pose::new(at, lying),
        character,
        Pose::new(at + Vec3::new(0.0, 0.0, 0.2), lying),
    );
    assert_close(depth, 0.6);
}

/// A sphere and a capsule with a center exactly on the other's core, and spheres on one point.
#[test]
fn a_sphere_on_a_core_leaves_by_their_radii() {
    let ball = Shape::Sphere { radius: 0.25 };
    let character = capsule(0.5, 0.3);
    let up = Pose::at(Vec3::new(0.0, 0.2, 0.0));
    assert_close(depth_separates("a sphere in a capsule", character, Pose::IDENTITY, ball, up), 0.55);
    assert_close(depth_separates("a capsule round a sphere", ball, up, character, Pose::IDENTITY), 0.55);
    assert_close(depth_separates("spheres on one point", ball, up, ball, up), 0.5);
}

/// Capsules whose cores cross at any angle, anywhere along them: a way out at right angles to both
/// takes just their radii, which no direction can better.
#[test]
fn crossing_capsules_leave_at_right_angles_to_both() {
    let mut state = 0x9e37_79b9_u32;
    let mut unit = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state >> 8) as f32 / (1u32 << 24) as f32
    };
    for case in 0..2000 {
        let mut turn = || {
            #[allow(
                clippy::disallowed_methods,
                reason = "a test's random turn: its last bit matters to nothing"
            )]
            Quat::from_xyzw(unit() - 0.5, unit() - 0.5, unit() - 0.5, unit() - 0.5).normalize()
        };
        let (first, second) = (turn(), turn());
        let (first_half, first_radius) = (0.2 + unit() * 2.0, 0.05 + unit() * 0.4);
        let (second_half, second_radius) = (0.2 + unit() * 2.0, 0.05 + unit() * 0.4);
        let crossing = Vec3::new(unit() * 80.0 - 40.0, unit() * 80.0 - 40.0, unit() * 80.0 - 40.0);
        // Each center is back along its own axis from the crossing, within its core.
        let first_along = (unit() * 2.0 - 1.0) * first_half;
        let second_along = (unit() * 2.0 - 1.0) * second_half;
        let at = Pose::new(crossing - first * Vec3::Y * first_along, first);
        let pose = Pose::new(crossing - second * Vec3::Y * second_along, second);
        let depth = depth_separates(
            &format!("case {case}"),
            capsule(first_half, first_radius),
            at,
            capsule(second_half, second_radius),
            pose,
        );
        let radii = first_radius + second_radius;
        let sine = (first * Vec3::Y).cross(second * Vec3::Y).length();
        if sine > 0.01 {
            assert!(depth < radii + 1e-3, "case {case}: {depth} to clear radii of {radii}");
        }
    }
}

// --- Moves ---

/// A body whose core lies along another's clears it in one call, by one push, sideways, and doesn't pop
/// up by four pushes that each fall short. It allocates nothing, by `tests/allocations.rs`.
#[test]
fn a_body_on_anothers_line_clears_it_in_one_move() {
    let shape = capsule(0.55, 0.1);
    let obstacle = StaticCollider {
        id: ColliderId(1),
        shape: StaticShape::Convex(shape),
        pose: Pose::IDENTITY,
        memberships: Group::World.mask(),
    };
    for offset in [0.0, 0.3, -0.3, 1.0] {
        let mut world = world(vec![obstacle.clone()]);
        let groups = CollisionGroups::new(Group::Character.mask(), Group::World.mask());
        let start = Vec3::new(0.0, offset, 0.0);
        let body = BodyDesc { id: BodyId(1), shape, pose: Pose::at(start), groups };
        world.add_body(body).expect("fits");
        let options = MoveOptions::default();
        let outcome = world.move_and_collide(BodyId(1), Vec3::ZERO, &options).expect("moves");
        assert!(outcome.started_overlapping(), "from {offset}");
        assert_close(outcome.overlap_push.length(), 0.2 + options.skin);
        assert_close(outcome.position.y, offset);
        let others = ALL.excluding(BodyId(1));
        assert_eq!(world.penetration(&shape, Pose::at(outcome.position), &others), None, "from {offset}");
        assert_eq!(world.take_stats().overlaps_resolved, 1, "from {offset}");
    }
}

fn moving_world(statics: Vec<StaticCollider>, start: Vec3) -> (PhysicsWorld, BodyId) {
    let mut world = world(statics);
    world.add_body(character(1, start)).expect("fits");
    (world, BodyId(1))
}

const WALL_X3: fn(u32) -> StaticCollider =
    |id| collider(id, Shape::Box { half_extents: Vec3::new(0.5, 50.0, 50.0) }, Vec3::new(3.5, 0.0, 0.0));

#[test]
fn a_clear_move_goes_all_the_way() {
    let (mut world, id) = moving_world(vec![WALL_X3(1)], Vec3::ZERO);
    let motion = Vec3::new(1.0, 0.25, -2.0);
    let outcome = world.move_and_collide(id, motion, &MoveOptions::default()).expect("moves");
    assert_eq!(outcome.position, motion);
    assert_eq!(outcome.moved, motion);
    assert_eq!(outcome.remaining, Vec3::ZERO);
    assert!(outcome.hits().is_empty());
    assert!(!outcome.started_overlapping());
    assert_eq!(world.body_pose(id).expect("there").position, motion);
}

#[test]
fn a_move_stops_a_skin_short_and_reports_the_wall() {
    // Wall face at x = 3, radius 0.5: the capsule's center can reach 2.5, less the skin of 0.01.
    let (mut world, id) = moving_world(vec![WALL_X3(4)], Vec3::ZERO);
    let options = MoveOptions { max_sweeps: 1, ..MoveOptions::default() };
    let outcome = world.move_and_collide(id, Vec3::new(10.0, 0.0, 0.0), &options).expect("moves");
    assert_close(outcome.position.x, 2.49);
    assert_near(outcome.remaining, Vec3::new(10.0 - 2.49, 0.0, 0.0));
    let [hit] = outcome.hits() else {
        panic!("one hit, not {:?}", outcome.hits());
    };
    assert_eq!(hit.target, HitTarget::Static(ColliderId(4)));
    assert_near(hit.normal, Vec3::NEG_X);
    assert_close(hit.point.x, 3.0);
    assert_close(hit.fraction, 0.249);
    assert!(!hit.started_overlapping);
}

#[test]
fn a_move_slides_along_a_wall() {
    // +x is blocked at 2.49; the +z part of the motion carries on.
    let (mut world, id) = moving_world(vec![WALL_X3(1)], Vec3::ZERO);
    let outcome =
        world.move_and_collide(id, Vec3::new(5.0, 0.0, 4.0), &MoveOptions::default()).expect("moves");
    assert_close(outcome.position.x, 2.49);
    assert_close(outcome.position.z, 4.0);
    assert_eq!(outcome.remaining, Vec3::ZERO);
    assert_eq!(outcome.hits().len(), 1);
}

#[test]
fn a_move_across_a_floor_keeps_its_skin() {
    // Floor at y = 0, capsule underside 1.5 below center: the center rests at 1.5 + skin. Falling
    // and running at once keeps all the running. Resting there, a sideways move touches nothing.
    let floor =
        collider(1, Shape::Box { half_extents: Vec3::new(50.0, 1.0, 50.0) }, Vec3::new(0.0, -1.0, 0.0));
    let (mut world, id) = moving_world(vec![floor], Vec3::new(0.0, 1.6, 0.0));
    let outcome =
        world.move_and_collide(id, Vec3::new(1.0, -1.0, 0.0), &MoveOptions::default()).expect("moves");
    assert_close(outcome.position.y, 1.51);
    assert_close(outcome.position.x, 1.0);
    assert_near(outcome.hits()[0].normal, Vec3::Y);

    for step in 1..=30 {
        let outcome =
            world.move_and_collide(id, Vec3::new(0.1, -0.05, 0.0), &MoveOptions::default()).expect("moves");
        assert_close(outcome.position.y, 1.51);
        assert_close(outcome.position.x, 1.0 + 0.1 * step as f32);
        assert_near(outcome.hits()[0].normal, Vec3::Y);
    }
    let sideways =
        world.move_and_collide(id, Vec3::new(0.1, 0.0, 0.0), &MoveOptions::default()).expect("moves");
    assert!(sideways.hits().is_empty(), "{:?}", sideways.hits());
}

#[test]
fn a_move_climbs_a_slope_by_sliding() {
    // A ramp rising along +x at 45°: normal (-1, 1, 0)/√2. A capsule walking +x slides up it. All
    // its motion along the normal is lost, leaving (1, 1, 0)/2 of each metre asked for.
    let ramp = StaticCollider {
        shape: StaticShape::Mesh {
            vertices: vec![
                Vec3::new(-50.0, -50.0, -50.0),
                Vec3::new(-50.0, -50.0, 50.0),
                Vec3::new(50.0, 50.0, 50.0),
                Vec3::new(50.0, 50.0, -50.0),
            ],
            indices: vec![[0, 1, 2], [0, 2, 3]],
        },
        ..plane_mesh(1, 0.0)
    };
    let ramp = StaticCollider { pose: Pose::IDENTITY, ..ramp };
    // Touching the slope: a sphere of radius 0.5 sits 0.5 along the normal, the lower cap's center
    // 1 below the capsule's. Center at (-1, 1, 0) * 0.5/√2 + (0, 1, 0), plus some gap.
    let gap = 0.1;
    let normal = Vec3::new(-1.0, 1.0, 0.0).normalize();
    let start = normal * (0.5 + gap) + Vec3::Y;
    let (mut world, id) = moving_world(vec![ramp], start);
    let outcome =
        world.move_and_collide(id, Vec3::new(1.0, 0.0, 0.0), &MoveOptions::default()).expect("moves");
    assert_near(outcome.hits()[0].normal, normal);
    // Heading in at 45°, the gap of 0.1 less the skin takes (0.1 - 0.01)·√2 along x. What's left
    // slides at half: (1 - 0.09√2) · (0.5, 0.5).
    let used = 0.09 * std::f32::consts::SQRT_2;
    let expected = Vec3::new(used + (1.0 - used) * 0.5, (1.0 - used) * 0.5, 0.0);
    assert_near(outcome.moved, expected);
    assert_near(outcome.hits()[0].normal, normal);
}

#[test]
fn a_move_into_a_corner_stops_in_it() {
    // Walls at x = 3 and z = 3, center reach 2.49 on each, then y carries on.
    let walls = vec![
        WALL_X3(1),
        collider(2, Shape::Box { half_extents: Vec3::new(50.0, 50.0, 0.5) }, Vec3::new(0.0, 0.0, 3.5)),
    ];
    let (mut world, id) = moving_world(walls, Vec3::ZERO);
    let outcome =
        world.move_and_collide(id, Vec3::new(9.0, 2.0, 9.0), &MoveOptions::default()).expect("moves");
    assert_close(outcome.position.x, 2.49);
    assert_close(outcome.position.z, 2.49);
    assert!(outcome.position.y > 0.5 && outcome.position.y <= 2.0 + TOLERANCE, "{}", outcome.position);
    assert!(outcome.hits().len() >= 2);
}

#[test]
fn a_move_out_of_an_overlap_pushes_first() {
    let floor =
        collider(1, Shape::Box { half_extents: Vec3::new(50.0, 1.0, 50.0) }, Vec3::new(0.0, -1.0, 0.0));
    // Center at y = 1.25: 0.25 into the floor, so pushed up by that and the skin to 1.51.
    let (mut world, id) = moving_world(vec![floor], Vec3::new(0.0, 1.25, 0.0));
    let outcome = world.move_and_collide(id, Vec3::ZERO, &MoveOptions::default()).expect("moves");
    assert!(outcome.started_overlapping());
    assert_near(outcome.overlap_push, Vec3::new(0.0, 0.26, 0.0));
    assert_close(outcome.position.y, 1.51);
    assert_eq!(world.take_stats().overlaps_resolved, 1);

    // Then it's clear, and stays so.
    let outcome = world.move_and_collide(id, Vec3::ZERO, &MoveOptions::default()).expect("moves");
    assert!(!outcome.started_overlapping());
    assert_close(outcome.position.y, 1.51);
}

#[test]
fn a_body_overlapping_from_both_sides_still_goes_on() {
    // Walls 0.5 apart around a capsule of diameter 1: no push can clear both. It mustn't hang,
    // mustn't produce NaN and mustn't end further in than a push from each side leaves it.
    let walls = vec![
        collider(1, Shape::Box { half_extents: Vec3::new(1.0, 50.0, 50.0) }, Vec3::new(-1.25, 0.0, 0.0)),
        collider(2, Shape::Box { half_extents: Vec3::new(1.0, 50.0, 50.0) }, Vec3::new(1.25, 0.0, 0.0)),
    ];
    let (mut world, id) = moving_world(walls, Vec3::ZERO);
    let outcome =
        world.move_and_collide(id, Vec3::new(0.0, 0.0, 1.0), &MoveOptions::default()).expect("moves");
    assert!(outcome.position.is_finite());
    assert!(outcome.started_overlapping());
    assert!(outcome.position.x.abs() < 1.0, "{}", outcome.position);
}

#[test]
fn bodies_block_by_their_groups() {
    let mut world = world(vec![]);
    world.add_body(character(1, Vec3::ZERO)).expect("fits");
    world.add_body(character(2, Vec3::new(3.0, 0.0, 0.0))).expect("fits");
    world
        .add_body(BodyDesc {
            id: BodyId(3),
            shape: CAPSULE,
            pose: Pose::at(Vec3::new(1.5, 0.0, 0.0)),
            groups: CollisionGroups::new(Group::Hurtbox.mask(), GroupMask::NONE),
        })
        .expect("fits");
    let options = MoveOptions { max_sweeps: 1, ..MoveOptions::default() };
    // The hurtbox at 1.5 is passed through. The other capsule's surface at 2.5, less this one's
    // radius and the skin, stops it at 1.99.
    let outcome = world.move_and_collide(BodyId(1), Vec3::new(5.0, 0.0, 0.0), &options).expect("moves");
    assert_close(outcome.position.x, 1.99);
    assert_eq!(outcome.hits()[0].target, HitTarget::Body(BodyId(2)));
    assert_near(outcome.hits()[0].normal, Vec3::NEG_X);
    assert_close(outcome.hits()[0].fraction, 1.99 / 5.0);
}

#[test]
fn a_body_moving_doesnt_hit_itself_and_only_it_moves() {
    let mut world = world(vec![]);
    world.add_body(character(1, Vec3::ZERO)).expect("fits");
    world.add_body(character(2, Vec3::new(0.5, 0.0, 0.0))).expect("fits");
    // Started overlapping the other capsule by 0.5, along +x from it: pushed -x by that and the
    // skin. The other stays put.
    let outcome = world.move_and_collide(BodyId(1), Vec3::ZERO, &MoveOptions::default()).expect("moves");
    assert_near(outcome.position, Vec3::new(-0.51, 0.0, 0.0));
    assert_eq!(world.body_pose(BodyId(2)).expect("there").position, Vec3::new(0.5, 0.0, 0.0));
}

#[test]
fn cast_body_probes_from_where_a_body_stands() {
    let floor =
        collider(1, Shape::Box { half_extents: Vec3::new(50.0, 1.0, 50.0) }, Vec3::new(0.0, -1.0, 0.0));
    let (world, id) = moving_world(vec![floor], Vec3::new(0.0, 1.6, 0.0));
    // Underside at y = 0.1: a ground probe finds the floor 0.1 down.
    let hit = world.cast_body(id, Vec3::ZERO, Vec3::NEG_Y, 1.0).expect("known").expect("a hit");
    assert_close(hit.distance, 0.1);
    assert_near(hit.normal, Vec3::Y);
    // From a metre up: 1.1. Up, nothing. Short of it, nothing.
    let hit = world.cast_body(id, Vec3::Y, Vec3::NEG_Y, 2.0).expect("known").expect("a hit");
    assert_close(hit.distance, 1.1);
    assert!(world.cast_body(id, Vec3::ZERO, Vec3::Y, 1.0).expect("known").is_none());
    assert!(world.cast_body(id, Vec3::ZERO, Vec3::NEG_Y, 0.05).expect("known").is_none());
    assert!(matches!(
        world.cast_body(BodyId(9), Vec3::ZERO, Vec3::Y, 1.0),
        Err(WorldError::UnknownBody(BodyId(9)))
    ));
}

// --- The same tick ---

/// Spec §5.22 runs Movement, then Hits, inside combat: hit queries must read this tick's positions
/// with no physics step between. A capsule that moved, or a hurtbox that followed it, is where
/// every query looks from the moment the call returns.
#[test]
fn a_run_of_excluded_bodies_takes_no_test_and_no_room() {
    let mut world = world(vec![]);
    for id in 10..=14 {
        world
            .add_body(BodyDesc {
                id: BodyId(id),
                shape: SPHERE,
                pose: Pose::at(Vec3::new(id as f32, 0.0, 0.0)),
                groups: CollisionGroups::new(Group::Hurtbox.mask(), GroupMask::NONE),
            })
            .expect("fits");
    }
    let hurtboxes = QueryFilter::new(Group::Hurtbox.mask());
    let thin = Shape::Sphere { radius: 0.01 };
    let cast = |filter: &QueryFilter| {
        // Room for one result, so an excluded body in a list would push out what's behind it.
        let mut out = [ShapeHit::NONE; 1];
        world.take_stats();
        let found = world.cast_shape_all(&thin, Pose::at(Vec3::ZERO), Vec3::X, 100.0, filter, &mut out);
        (out[0].target, found.found, world.take_stats().narrow_tests)
    };

    assert_eq!(cast(&hurtboxes), (HitTarget::Body(BodyId(10)), 5, 5));
    // Both ends of the run are excluded, and neither body past them is.
    let middle = hurtboxes.excluding_bodies(BodyId(10), BodyId(13));
    assert_eq!(cast(&middle), (HitTarget::Body(BodyId(14)), 1, 1));
    let upper = hurtboxes.excluding_bodies(BodyId(11), BodyId(14));
    assert_eq!(cast(&upper), (HitTarget::Body(BodyId(10)), 1, 1));
    // Alongside one body on its own, and in every other query.
    let both = hurtboxes.excluding_bodies(BodyId(11), BodyId(14)).excluding(BodyId(10));
    assert_eq!(cast(&both).1, 0);
    assert!(world.cast_ray(Vec3::ZERO, Vec3::X, 100.0, &both).is_none());
    let hit = world.cast_ray(Vec3::ZERO, Vec3::X, 100.0, &middle).expect("the ray finds the last");
    assert_eq!(hit.target, HitTarget::Body(BodyId(14)));
    let hit = world.cast_shape(&thin, Pose::at(Vec3::ZERO), Vec3::X, 100.0, &upper).expect("and the cast");
    assert_eq!(hit.target, HitTarget::Body(BodyId(10)));
}

#[test]
fn a_body_moved_this_tick_is_found_at_once() {
    let mut world = world(vec![]);
    world.add_body(character(1, Vec3::new(100.0, 0.0, 0.0))).expect("fits");
    world
        .add_body(BodyDesc {
            id: BodyId(2),
            shape: SPHERE,
            pose: Pose::at(Vec3::new(100.0, 0.0, 50.0)),
            groups: CollisionGroups::new(Group::Hurtbox.mask(), GroupMask::NONE),
        })
        .expect("fits");
    let characters = QueryFilter::new(Group::Character.mask());
    let hurtboxes = QueryFilter::new(Group::Hurtbox.mask());
    let mut targets = [HitTarget::Body(BodyId(0)); 2];
    let mut hits = [ShapeHit::NONE; 2];
    let thin = Shape::Sphere { radius: 0.01 };

    // Before: the capsule is 100 m off, so a ray down at the origin finds nothing.
    let probe = Vec3::new(0.0, 5.0, 0.0);
    assert!(world.cast_ray(probe, Vec3::NEG_Y, 10.0, &characters).is_none());

    // A move and no other call: every kind of query sees it there.
    let moved = world
        .move_and_collide(BodyId(1), Vec3::new(-100.0, 0.0, 0.0), &MoveOptions::default())
        .expect("moves");
    assert_near(moved.position, Vec3::ZERO);
    // The capsule's top is 1.5 above its center, so 3.5 below the probe.
    let hit = world.cast_ray(probe, Vec3::NEG_Y, 10.0, &characters).expect("the ray finds it");
    assert_eq!(hit.target, HitTarget::Body(BodyId(1)));
    assert_close(hit.distance, 3.5);
    let hit =
        world.cast_shape(&thin, Pose::at(probe), Vec3::NEG_Y, 10.0, &characters).expect("the cast finds it");
    assert_close(hit.distance, 3.49);
    let found = world.cast_shape_all(&thin, Pose::at(probe), Vec3::NEG_Y, 10.0, &characters, &mut hits);
    assert_eq!(found.found, 1);
    let found = world.overlap(&SPHERE, Pose::IDENTITY, &characters, &mut targets);
    assert_eq!(found.found, 1);
    let found = world.penetration(&SPHERE, Pose::IDENTITY, &characters).expect("it overlaps");
    assert_eq!(found.target, HitTarget::Body(BodyId(1)));
    // And it has left where it was, as far as any query knows.
    let old = Pose::at(Vec3::new(100.0, 0.0, 0.0));
    assert_eq!(world.overlap(&SPHERE, old, &characters, &mut targets).found, 0);

    // A hurtbox set by its table is the same, in its own group.
    world.set_body_pose(BodyId(2), Pose::at(Vec3::new(2.0, 0.0, 0.0))).expect("valid");
    let swing = Pose::at(Vec3::new(0.0, 0.0, 0.0));
    let found = world.cast_shape_all(&thin, swing, Vec3::X, 5.0, &hurtboxes, &mut hits);
    assert_eq!(found.found, 1);
    assert_eq!(hits[0].target, HitTarget::Body(BodyId(2)));
    assert_close(hits[0].distance, 1.49);

    // A second capsule's move later in the tick sees the first where it is now.
    world.add_body(character(3, Vec3::new(5.0, 0.0, 0.0))).expect("fits");
    let options = MoveOptions { max_sweeps: 1, ..MoveOptions::default() };
    let blocked = world.move_and_collide(BodyId(3), Vec3::new(-9.0, 0.0, 0.0), &options).expect("moves");
    assert_eq!(blocked.hits()[0].target, HitTarget::Body(BodyId(1)));
    assert_close(blocked.position.x, 1.01);
}

// --- Tunnelling ---

/// The speed of a fast dash in one tick, and then absurd ones. A 2 cm wall has nowhere near a
/// tick's travel of thickness, so only continuous sweeps can stop these.
#[test]
fn no_speed_tunnels_through_a_thin_wall() {
    let thin_wall =
        collider(1, Shape::Box { half_extents: Vec3::new(0.01, 50.0, 50.0) }, Vec3::new(3.0, 0.0, 0.0));
    let thin_mesh = StaticCollider {
        id: ColliderId(2),
        shape: StaticShape::Mesh {
            vertices: vec![
                Vec3::new(0.0, -100.0, -100.0),
                Vec3::new(0.0, -100.0, 300.0),
                Vec3::new(0.0, 300.0, -100.0),
            ],
            indices: vec![[0, 1, 2]],
        },
        pose: Pose::at(Vec3::new(-3.0, 0.0, 0.0)),
        memberships: Group::World.mask(),
    };
    for speed_per_tick in [0.2, 3.5, 40.0, 1_000.0, 1e7] {
        let (mut world, id) = moving_world(vec![thin_wall.clone(), thin_mesh.clone()], Vec3::ZERO);
        let options = MoveOptions::default();
        for _ in 0..60 {
            let outcome =
                world.move_and_collide(id, Vec3::new(speed_per_tick, 0.0, 0.0), &options).expect("moves");
            // Face at 2.99, radius 0.5.
            assert!(
                outcome.position.x <= 2.49 + TOLERANCE,
                "through the wall at {speed_per_tick}: {}",
                outcome.position
            );
        }
        assert_close(world.body_pose(id).expect("there").position.x, 2.48);

        // The other way, a single triangle at x = -3.
        for _ in 0..60 {
            let outcome =
                world.move_and_collide(id, Vec3::new(-speed_per_tick, 0.0, 0.0), &options).expect("moves");
            assert!(
                outcome.position.x >= -2.49 - TOLERANCE,
                "through the mesh at {speed_per_tick}: {}",
                outcome.position
            );
        }
    }
}

#[test]
fn casts_and_rays_dont_tunnel_either() {
    let thin_wall =
        collider(1, Shape::Box { half_extents: Vec3::new(0.001, 5.0, 5.0) }, Vec3::new(1000.0, 0.0, 0.0));
    let world = world(vec![thin_wall]);
    let hit = world.cast_ray(Vec3::ZERO, Vec3::X, 2000.0, &ALL).expect("a hit");
    assert_close(hit.distance, 999.999);
    let hit = world.cast_shape(&SPHERE, Pose::IDENTITY, Vec3::X, 2000.0, &ALL).expect("a hit");
    assert!((hit.distance - 999.499).abs() < 2e-2, "{}", hit.distance);
}

// --- Errors and bookkeeping ---

#[test]
fn a_world_refuses_what_it_cant_build() {
    let duplicate = PhysicsWorld::new(vec![collider(1, SPHERE, Vec3::ZERO), collider(1, SPHERE, Vec3::X)], 0);
    assert!(matches!(duplicate, Err(WorldError::DuplicateCollider(ColliderId(1)))));
    let flat = PhysicsWorld::new(vec![collider(1, Shape::Sphere { radius: 0.0 }, Vec3::ZERO)], 0);
    assert!(flat.err().expect("refused").to_string().contains("collider 1"));
    let lost = PhysicsWorld::new(vec![collider(2, SPHERE, Vec3::splat(f32::NAN))], 0);
    assert!(lost.err().expect("refused").to_string().contains("collider 2"));
    let noisy = StaticCollider {
        pose: Pose::new(Vec3::ZERO, Quat::from_xyzw(0.0, 0.0, 0.0, 3.0)),
        ..collider(3, SPHERE, Vec3::ZERO)
    };
    assert!(PhysicsWorld::new(vec![noisy], 0).err().expect("refused").to_string().contains("unit"));
    let groupless = StaticCollider { memberships: GroupMask::NONE, ..collider(4, SPHERE, Vec3::ZERO) };
    assert!(PhysicsWorld::new(vec![groupless], 0).err().expect("refused").to_string().contains("no group"));
    let broken = StaticCollider {
        shape: StaticShape::Mesh { vertices: vec![Vec3::ZERO, Vec3::X], indices: vec![[0, 1, 2]] },
        ..collider(5, SPHERE, Vec3::ZERO)
    };
    assert!(
        PhysicsWorld::new(vec![broken], 0).err().expect("refused").to_string().contains("names vertex 2")
    );
}

#[test]
fn bodies_are_checked() {
    let mut world = world(vec![]);
    world.add_body(character(1, Vec3::ZERO)).expect("fits");
    assert!(matches!(world.add_body(character(1, Vec3::X)), Err(WorldError::DuplicateBody(BodyId(1)))));
    assert!(matches!(world.add_body(character(9, Vec3::NAN)), Err(WorldError::InvalidBody(BodyId(9)))));
    assert!(matches!(world.set_body_pose(BodyId(1), Pose::at(Vec3::NAN)), Err(WorldError::InvalidBody(_))));
    assert!(matches!(world.set_body_pose(BodyId(9), Pose::IDENTITY), Err(WorldError::UnknownBody(_))));
    assert!(matches!(
        world.move_and_collide(BodyId(9), Vec3::X, &MoveOptions::default()),
        Err(WorldError::UnknownBody(BodyId(9)))
    ));
    assert!(matches!(
        world.move_and_collide(BodyId(1), Vec3::NAN, &MoveOptions::default()),
        Err(WorldError::InvalidBody(BodyId(1)))
    ));
    for id in 2..=8 {
        world.add_body(character(id, Vec3::new(id as f32 * 3.0, 0.0, 0.0))).expect("fits");
    }
    assert!(matches!(
        world.add_body(character(100, Vec3::ZERO)),
        Err(WorldError::TooManyBodies { capacity: 8 })
    ));
    assert!(!world.remove_body(BodyId(100)));
    assert!(world.remove_body(BodyId(5)));
    world.add_body(character(100, Vec3::ZERO)).expect("room again");

    let mut boxes = self::world(vec![]);
    boxes
        .add_body(BodyDesc {
            id: BodyId(1),
            shape: UNIT_BOX,
            pose: Pose::IDENTITY,
            groups: CollisionGroups::new(Group::Hurtbox.mask(), GroupMask::NONE),
        })
        .expect("fits");
    assert!(matches!(
        boxes.move_and_collide(BodyId(1), Vec3::X, &MoveOptions::default()),
        Err(WorldError::CannotMove(BodyId(1)))
    ));
}

#[test]
fn an_empty_world_answers_nothing() {
    let world = world(vec![]);
    assert!(world.cast_ray(Vec3::ZERO, Vec3::X, 10.0, &ALL).is_none());
    assert!(world.cast_shape(&SPHERE, Pose::IDENTITY, Vec3::X, 10.0, &ALL).is_none());
    assert!(world.penetration(&SPHERE, Pose::IDENTITY, &ALL).is_none());
    assert_eq!(world.overlap(&SPHERE, Pose::IDENTITY, &ALL, &mut []), Overlaps::default());
}

#[test]
fn a_single_collider_world_works() {
    // `parry3d`'s tree has a special case for a root with one leaf.
    let world = world(vec![collider(1, UNIT_BOX, Vec3::new(5.0, 0.0, 0.0))]);
    let hit = world.cast_shape(&SPHERE, Pose::IDENTITY, Vec3::X, 10.0, &ALL).expect("a hit");
    assert_close(hit.distance, 3.5);
    assert!(world.cast_shape(&SPHERE, Pose::IDENTITY, Vec3::X, 3.0, &ALL).is_none());
}

#[test]
fn stats_count_the_work_and_reset() {
    let (mut world, id) = moving_world(vec![WALL_X3(1)], Vec3::ZERO);
    assert_eq!(
        world.take_stats(),
        PhysicsStats { statics: 1, bodies: 1, ..PhysicsStats::default() },
        "nothing done yet"
    );
    world.move_and_collide(id, Vec3::new(10.0, 0.0, 0.0), &MoveOptions::default()).expect("moves");
    world.cast_ray(Vec3::ZERO, Vec3::X, 10.0, &ALL);
    world.cast_shape(&SPHERE, Pose::IDENTITY, Vec3::X, 10.0, &ALL);
    world.overlap(&SPHERE, Pose::IDENTITY, &ALL, &mut []);
    world.penetration(&SPHERE, Pose::IDENTITY, &ALL);
    let stats = world.take_stats();
    assert_eq!(stats.moves, 1);
    assert_eq!(stats.move_hits, 1);
    assert!(stats.move_sweeps >= 1);
    assert_eq!(stats.ray_casts, 1);
    assert_eq!(stats.shape_casts, 1);
    assert_eq!(stats.overlap_queries, 1);
    assert_eq!(stats.penetration_queries, 1);
    assert!(stats.narrow_tests >= 2);
    assert_eq!(world.take_stats().moves, 0);
}

/// Queries take `&self` and a world is `Sync`, so animation jobs (M2 N4's foot casts) can share one
/// between steps.
#[test]
fn a_world_can_be_shared_between_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<PhysicsWorld>();
    let (world, _) = moving_world(vec![WALL_X3(1)], Vec3::ZERO);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                let hit = world.cast_ray(
                    Vec3::new(0.0, 5.0, 0.0),
                    Vec3::X,
                    10.0,
                    &QueryFilter::new(Group::World.mask()),
                );
                assert_close(hit.expect("a hit").distance, 3.0);
            });
        }
    });
}
