//! What the state checksum sees of hit queries: every field of everything they keep.

use sapphire_core::Name;
use sapphire_math::Vec3;
use sapphire_moves::{BakedPath, PathSample};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};
use sapphire_time::SimDuration;

use super::*;
use crate::simulation::{SimConfig, Simulation};
use crate::{ActorTransform, HurtboxId, OwnerStep, VolumeSweep};

fn actor(raw: u32) -> ActorId {
    ActorId::from_raw(raw).unwrap()
}

/// A query holding a record, a stopped volume and an event: its target at x = 1, a wall at x = 2.
fn busy_query() -> HitQuery {
    let wall = StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(0.1, 2.0, 2.0) }),
        pose: Pose::at(Vec3::new(2.0, 1.0, 0.0)),
        memberships: Group::World.mask(),
    };
    let mut world = PhysicsWorld::new(vec![wall], 4).unwrap();
    let local = PathSample { translation: [1.0, 1.0, 0.0], ..PathSample::IDENTITY };
    let target = HurtboxId::new(actor(2), 1).unwrap();
    target.set(&mut world, Some(Shape::Sphere { radius: 0.2 }), ActorTransform::default(), local).unwrap();

    let samples = [0.0, 4.0].map(|x| PathSample { translation: [x, 1.0, 0.0], ..PathSample::IDENTITY });
    let path = BakedPath::new(Name::new("blade"), samples.to_vec());
    let root = ActorTransform::default();
    let mut hits = HitQuery::new();
    hits.sweep(
        &world,
        &VolumeSweep {
            attacker: actor(1),
            instance: 7,
            volume: 3,
            group: Name::new("swing"),
            rearm: &[],
            shape: Shape::Sphere { radius: 0.1 },
            path: &path,
            blocked_by: Group::World.mask(),
            owner: OwnerStep {
                delta: SimDuration::from_ticks(1),
                reading: SimDuration::from_ticks(1),
                root_before: root,
                root_after: root,
            },
            from: SimDuration::ZERO,
            to: SimDuration::from_ticks(1),
        },
    );
    hits.finish_step();
    assert_eq!(hits.events.len(), 1);
    assert_eq!(hits.records.entries.len(), 1);
    assert_eq!(hits.stopped.entries.len(), 1);
    hits
}

fn state_hash(hits: &HitQuery) -> u64 {
    let mut hasher = StateHasher::new();
    hits.write_canonical(&mut hasher);
    hasher.finish()
}

#[test]
fn every_field_of_a_querys_state_moves_the_hash() {
    let base = state_hash(&busy_query());
    assert_eq!(state_hash(&busy_query()), base, "the same state hashes alike");

    type Edit = fn(&mut HitQuery);
    let edits: &[(&str, Edit)] = &[
        ("a record's attacker", |h| h.records.entries[0].attacker = actor(9)),
        ("a record's instance", |h| h.records.entries[0].instance += 1),
        ("a record's group", |h| h.records.entries[0].key.0 = Name::new("x")),
        ("a record's target", |h| h.records.entries[0].key.1 = actor(9)),
        ("a record's volley", |h| h.records.entries[0].volley += 1),
        ("the record count", |h| h.records.entries.clear()),
        ("a stop's attacker", |h| h.stopped.entries[0].attacker = actor(9)),
        ("a stop's instance", |h| h.stopped.entries[0].instance += 1),
        ("a stop's group", |h| h.stopped.entries[0].key.0 = Name::new("x")),
        ("a stop's path", |h| h.stopped.entries[0].key.1 = Name::new("x")),
        ("a stop's volley", |h| h.stopped.entries[0].volley += 1),
        ("the stop count", |h| h.stopped.entries.clear()),
        ("an event's attacker", |h| h.events[0].attacker = actor(9)),
        ("an event's instance", |h| h.events[0].instance += 1),
        ("an event's target", |h| h.events[0].target = actor(9)),
        ("an event's hurtbox", |h| h.events[0].hurtbox += 1),
        ("an event's group", |h| h.events[0].group = Name::new("x")),
        ("an event's volley", |h| h.events[0].volley += 1),
        ("an event's volume", |h| h.events[0].volume += 1),
        ("an event's point", |h| h.events[0].point.z += 0.25),
        ("an event's normal", |h| h.events[0].normal.y += 0.25),
        ("an event's time", |h| h.events[0].at = SimDuration::from_subticks(1)),
        ("an event's travel", |h| h.events[0].travel += 0.25),
        ("the event count", |h| h.events.clear()),
        ("a contact held over", |h| {
            let event = h.events[0];
            h.contacts.push(event);
        }),
    ];
    for (what, edit) in edits {
        let mut hits = busy_query();
        edit(&mut hits);
        assert_ne!(state_hash(&hits), base, "{what} isn't in the checksum");
    }
}

/// Only what can change a later result is state: a counter never reads back.
#[test]
fn counters_are_not_state() {
    let base = state_hash(&busy_query());
    let mut hits = busy_query();
    hits.stats.sweeps += 1;
    assert_eq!(state_hash(&hits), base);
}

#[test]
fn the_simulations_checksum_holds_the_query() {
    let mut sim = Simulation::new(SimConfig::default());
    let empty = sim.checksum();
    *sim.ecs_mut().world_mut().resource_mut::<HitQuery>() = busy_query();
    let busy = sim.checksum();
    assert_ne!(busy, empty, "a simulation's checksum reads its hit queries");
    sim.ecs_mut().world_mut().resource_mut::<HitQuery>().records.entries[0].volley += 1;
    assert_ne!(sim.checksum(), busy, "all the way down");
}

#[test]
fn a_book_stays_sorted_and_refuses_when_full() {
    let mut book = Book::<u16>::with_capacity(3);
    assert!(book.write(actor(2), 1, 5, 0));
    assert!(book.write(actor(1), 9, 7, 0));
    assert!(book.write(actor(2), 1, 3, 0));
    let sorted: Vec<_> = book.entries.iter().map(|e| (e.attacker.raw(), e.instance, e.key)).collect();
    assert_eq!(sorted, [(1, 9, 7), (2, 1, 3), (2, 1, 5)]);

    assert!(!book.write(actor(3), 1, 1, 0), "no room");
    assert_eq!(book.entries.len(), 3);
    assert!(book.write(actor(2), 1, 3, 4), "what's already there has room");
    assert!(book.write(actor(2), 1, 3, 2), "an earlier volley");
    assert_eq!(book.volley_of(actor(2), 1, 3), Some(4), "keeps the later one");
    assert_eq!(book.volley_of(actor(2), 2, 3), None);

    // Retiring takes this attacker's earlier instances, and no one else's or any later.
    let mut book = Book::<u16>::with_capacity(8);
    for (attacker, instance) in [(1, 1), (1, 2), (2, 1), (2, 2), (2, 3), (3, 1)] {
        assert!(book.write(actor(attacker), instance, 0, 0));
    }
    book.retire(actor(2), 3);
    let left: Vec<_> = book.entries.iter().map(|e| (e.attacker.raw(), e.instance)).collect();
    assert_eq!(left, [(1, 1), (1, 2), (2, 3), (3, 1)]);
    book.retire(actor(2), 3);
    assert_eq!(book.entries.len(), 4, "already done");
}
