//! A committed collider document, from RON through the cooked format to a world that a capsule
//! walks in: what a zone's document does, without the asset server.

use glam::Vec3;
use sapphire_colliders::{ColliderDocument, ColliderSet};
use sapphire_physics::{
    BodyDesc, BodyId, ColliderId, CollisionGroups, Group, GroupMask, HitTarget, MoveOptions, PhysicsWorld,
    Pose, QueryFilter, Shape,
};
use sapphire_serialize::cooked::AlignedVec;

const ARENA: &str = include_str!("../../../content/physics/test_arena.colliders.ron");

fn cooked() -> AlignedVec {
    ColliderDocument::from_ron(ARENA)
        .expect("the document reads")
        .cook()
        .expect("it cooks")
        .to_cooked()
        .expect("it writes")
}

fn body() -> BodyDesc {
    BodyDesc {
        id: BodyId(1),
        shape: Shape::Capsule { half_height: 0.5, radius: 0.5 },
        // A skin over the ground, whose top is y = 0, on the capsule's lower cap 1 m under its origin.
        pose: Pose::at(Vec3::new(0.0, 1.01, 0.0)),
        groups: CollisionGroups::new(
            Group::Character.mask(),
            Group::World.mask() | Group::CharacterBlocker.mask(),
        ),
    }
}

#[test]
fn the_committed_document_builds_a_world_by_its_names() {
    let bytes = cooked();
    let set = ColliderSet::read(&bytes).expect("the cooked set reads");
    let world = PhysicsWorld::from_cooked(set, 4).expect("the world builds");
    assert_eq!(world.static_count(), 9);
    let named = |name: &str| HitTarget::Static(ColliderId(set.id_of(name).expect("it's in the set")));

    // Straight down from over the crate, 3 m up: its top is at y = 1.
    let world_group = QueryFilter::new(Group::World.mask());
    let hit = world.cast_ray(Vec3::new(3.0, 3.0, 2.0), Vec3::NEG_Y, 10.0, &world_group).expect("the crate");
    assert_eq!(hit.target, named("crate"));
    assert!((hit.distance - 2.0).abs() < 1e-5, "{}", hit.distance);

    // The ramp, halfway up its rise at x = 7, its normal leaning back along -x by the slope's 1 in 4.
    let hit = world.cast_ray(Vec3::new(7.0, 3.0, -6.0), Vec3::NEG_Y, 10.0, &world_group).expect("the ramp");
    assert_eq!(hit.target, named("ramp"));
    assert!((hit.distance - 2.5).abs() < 1e-5, "{}", hit.distance);
    let slope = Vec3::new(-1.0, 4.0, 0.0).normalize();
    assert!((hit.normal - slope).length() < 1e-5, "{}", hit.normal);

    // The edges block characters, not rays looking for the world.
    let east = Vec3::new(10.0, 1.0, 10.0);
    assert!(world.cast_ray(east, Vec3::X, 5.0, &world_group).is_none());
    let hit = world.cast_ray(east, Vec3::X, 5.0, &QueryFilter::new(GroupMask::ALL)).expect("the edge");
    assert_eq!(hit.target, named("edge_east"));
    assert!((hit.distance - 2.0).abs() < 1e-5, "{}", hit.distance);
}

#[test]
fn a_capsule_walks_the_arena_to_its_edge() {
    let bytes = cooked();
    let set = ColliderSet::read(&bytes).expect("the cooked set reads");
    let mut world = PhysicsWorld::from_cooked(set, 4).expect("the world builds");
    world.add_body(body()).expect("it fits");

    // Running at +z along x = 0, 16 m in all, a capsule of radius 0.5 stops a skin short of 11.5,
    // where the edge at z = 12 reaches it. Nothing else stands on this line.
    let options = MoveOptions::default();
    let motion = Vec3::new(0.0, 0.0, 16.0 / 120.0);
    let mut stopped = None;
    for _ in 0..120 {
        let outcome = world.move_and_collide(BodyId(1), motion, &options).expect("it moves");
        if let Some(hit) = outcome.hits().first() {
            stopped = Some(*hit);
        }
    }
    let hit = stopped.expect("the edge stops it");
    assert_eq!(hit.target, HitTarget::Static(ColliderId(set.id_of("edge_north").expect("it's in the set"))));
    assert!((hit.normal - Vec3::NEG_Z).length() < 1e-5, "{}", hit.normal);
    let z = world.body_pose(BodyId(1)).expect("it's there").position.z;
    assert!((z - (11.5 - options.skin)).abs() < 1e-3, "{z}");
}

/// Listing the colliders in another order changes neither IDs nor results.
#[test]
fn a_reordered_document_cooks_to_the_same_bytes() {
    let mut document = ColliderDocument::from_ron(ARENA).expect("the document reads");
    let in_order = document.cook().expect("it cooks").to_cooked().expect("it writes");
    document.colliders.reverse();
    let reversed = document.cook().expect("it cooks").to_cooked().expect("it writes");
    assert_eq!(in_order.as_slice(), reversed.as_slice());
}

#[test]
fn a_truncated_cooked_set_never_reaches_a_world() {
    let bytes = cooked();
    let mut cut: AlignedVec = AlignedVec::new();
    cut.extend_from_slice(&bytes[..bytes.len() / 2]);
    assert!(ColliderSet::read(&cut).is_err());
}
