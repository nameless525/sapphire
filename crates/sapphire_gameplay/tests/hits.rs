//! M2 D4's hit queries, over plain inputs and a real collision world: hand-computed geometry, what
//! a sweep can and can't reach, clocks, windows, hurtboxes, hit groups, overflow and order.
//!
//! Distances here are worked out by hand in each test's comments. World space is metres, Y up, with
//! a root's yaw turning its +Z toward +X.

mod support;

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use sapphire_core::Name;
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, ActorTable, ActorTransform, Buttons, HitEvent, HitQuery, HurtboxId,
    MAX_CAST_RESULTS, MAX_CONTACTS, MAX_HIT_EVENTS, MAX_HIT_RECORDS, MAX_HURTBOXES, MAX_SAG, MAX_SUB_STEPS,
    MoveOutput, OwnerStep, SimConfig, Simulation, Stick, SubStep, TickActions, VolumeSweep, hit_shape,
    dodge_body, dodger_of, published_hurtbox, remove_hurtboxes, volume_pose,
};
use sapphire_math::{Vec3, det};
use sapphire_moves::{BakedPath, MoveSet, MoveSetDocument, PathSample, Span, Stretch};
use sapphire_physics::{
    ColliderId, Group, GroupMask, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape,
};
use sapphire_time::{ScaleSource, ScaledClock, SimDuration};

const SWING: Name = Name::new("swing");
const OTHER: Name = Name::new("other");
const BLADE: Name = Name::new("blade");

const SPHERE: Shape = Shape::Sphere { radius: 0.1 };

fn actor(raw: u32) -> ActorId {
    ActorId::from_raw(raw).unwrap()
}

fn ticks(count: u64) -> SimDuration {
    SimDuration::from_ticks(count)
}

fn sub(count: u64) -> SimDuration {
    SimDuration::from_subticks(count)
}

/// A path through these points, one per tick, unrotated.
fn path_through(points: &[Vec3]) -> BakedPath {
    let samples = points
        .iter()
        .map(|point| PathSample { translation: point.to_array(), ..PathSample::IDENTITY })
        .collect();
    BakedPath::new(BLADE, samples)
}

/// A path holding one point and turning about +Z, `step` radians a tick.
fn turning_path(at: Vec3, step: f32, samples: u32) -> BakedPath {
    let samples = (0..samples)
        .map(|tick| PathSample {
            translation: at.to_array(),
            rotation: det::quat_from_rotation_z(tick as f32 * step).to_array(),
        })
        .collect();
    BakedPath::new(BLADE, samples)
}

/// A path around a circle in the XZ plane about the root at y = 1, from +X through +Z (a quarter turn
/// each `PI / 2`), in `intervals` equal parts.
fn arc_path(radius: f32, angle: f32, intervals: u32) -> BakedPath {
    let points: Vec<Vec3> = (0..=intervals)
        .map(|i| {
            let (sin, cos) = det::sin_cos(angle * i as f32 / intervals as f32);
            Vec3::new(radius * cos, 1.0, radius * sin)
        })
        .collect();
    path_through(&points)
}

/// A world with room for a crowd and no scenery.
fn empty_world() -> PhysicsWorld {
    PhysicsWorld::new(Vec::new(), 256).unwrap()
}

fn add_target(world: &mut PhysicsWorld, owner: u32, index: u8, shape: Shape, at: Vec3) {
    let id = HurtboxId::new(actor(owner), index).unwrap();
    let local = PathSample { translation: at.to_array(), ..PathSample::IDENTITY };
    id.set(world, Some(shape), ActorTransform::default(), local).unwrap();
}

/// A volume of attacker 1 sweeping a whole span, with a root that stays at the origin facing +Z and a
/// clock that covered exactly the span, or a tick if the span is one instant.
fn swing<'a>(path: &'a BakedPath, shape: Shape, from: SimDuration, to: SimDuration) -> VolumeSweep<'a> {
    let covered = to.as_subticks().saturating_sub(from.as_subticks());
    let delta = if covered == 0 { ticks(1) } else { sub(covered) };
    VolumeSweep {
        attacker: actor(1),
        instance: 1,
        volume: 0,
        group: SWING,
        rearm: &[],
        shape,
        path,
        blocked_by: GroupMask::NONE,
        dodgeable: true,
        owner: OwnerStep {
            delta,
            reading: to,
            root_before: ActorTransform::default(),
            root_after: ActorTransform::default(),
        },
        from,
        to,
    }
}

/// One step of one volume.
fn step(hits: &mut HitQuery, world: &PhysicsWorld, volume: &VolumeSweep<'_>) -> Vec<HitEvent> {
    hits.sweep(world, volume);
    hits.finish_step();
    hits.events().to_vec()
}

/// All of a volume's sub-steps.
fn sub_steps(volume: &VolumeSweep<'_>) -> Vec<SubStep> {
    let mut all = Vec::new();
    volume.walk(|sub| all.push(*sub));
    all
}

#[track_caller]
fn assert_close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 2e-3, "{actual} is not {expected}");
}

#[track_caller]
fn assert_near(actual: Vec3, expected: Vec3) {
    assert!(actual.abs_diff_eq(expected, 2e-3), "{actual} is not {expected}");
}

// ------------------------------------------------------------------------------------------------
// Geometry by hand, for each shape.
// ------------------------------------------------------------------------------------------------

#[test]
fn a_sphere_meets_a_sphere_where_the_numbers_say() {
    // A 0.1 sphere from x = 0 to x = 2 toward a 0.2 sphere at x = 1: their surfaces touch when the
    // centers are 0.3 apart, at x = 0.7, which is 35% of the way and so 350 sub-ticks in.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(1.0, 1.0, 0.0));
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(2.0, 1.0, 0.0)]);
    let mut hits = HitQuery::new();

    let events = step(&mut hits, &world, &swing(&path, SPHERE, ticks(0), ticks(1)));

    assert_eq!(events.len(), 1);
    let event = events[0];
    assert_eq!((event.attacker, event.target, event.hurtbox), (actor(1), actor(2), 0));
    assert_eq!((event.group, event.volley, event.volume, event.instance), (SWING, 0, 0, 1));
    assert_close(event.travel, 0.7);
    assert_near(event.point, Vec3::new(0.8, 1.0, 0.0));
    assert_near(event.normal, Vec3::NEG_X);
    assert_eq!(event.at, sub(350));
}

#[test]
fn a_capsule_meets_a_box_where_the_numbers_say() {
    // A capsule along world +Z (the path turns its +Y a quarter about X), radius 0.05, half height
    // 0.5, along +X toward a box whose near face is at x = 0.9. Its center touches at x = 0.85, a metre
    // into a 4 m run from x = -0.15: 25% of the way, 250 sub-ticks in.
    let mut world = empty_world();
    let wall = Shape::Box { half_extents: Vec3::new(0.1, 1.0, 1.0) };
    add_target(&mut world, 2, 0, wall, Vec3::new(1.0, 1.0, 0.0));
    let rotation = det::quat_from_rotation_x(FRAC_PI_2).to_array();
    let sample = |x: f32| PathSample { translation: [x, 1.0, 0.0], rotation };
    let path = BakedPath::new(BLADE, vec![sample(-0.15), sample(3.85)]);
    let mut hits = HitQuery::new();

    let capsule = Shape::Capsule { half_height: 0.5, radius: 0.05 };
    let events = step(&mut hits, &world, &swing(&path, capsule, ticks(0), ticks(1)));

    assert_eq!(events.len(), 1);
    assert_close(events[0].travel, 1.0);
    assert_close(events[0].point.x, 0.9);
    assert_near(events[0].normal, Vec3::NEG_X);
    assert_eq!(events[0].at, sub(250));

    // The box ends at z = 1, and the capsule reaches 0.55 from its center along z. At z = 1.5 its end
    // still overlaps the box's edge.
    let sample = |x: f32| PathSample { translation: [x, 1.0, 1.5], rotation };
    let edge = BakedPath::new(BLADE, vec![sample(-0.15), sample(3.85)]);
    let mut hits = HitQuery::new();
    assert_eq!(step(&mut hits, &world, &swing(&edge, capsule, ticks(0), ticks(1))).len(), 1);
    // And at z = 1.6 it passes 0.05 clear.
    let sample = |x: f32| PathSample { translation: [x, 1.0, 1.6], rotation };
    let clear = BakedPath::new(BLADE, vec![sample(-0.15), sample(3.85)]);
    let mut hits = HitQuery::new();
    assert!(step(&mut hits, &world, &swing(&clear, capsule, ticks(0), ticks(1))).is_empty());
}

#[test]
fn an_oriented_box_meets_a_sphere_corner_first() {
    // A 0.1 cube turned 45° about Y leads with an edge sqrt(2) * 0.1 = 0.14142 from its center. A 0.2
    // sphere at x = 1 has its surface at x = 0.8, so the cube's center stops at 0.65858.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(1.0, 1.0, 0.0));
    let rotation = det::quat_from_rotation_y(FRAC_PI_4).to_array();
    let sample = |x: f32| PathSample { translation: [x, 1.0, 0.0], rotation };
    let path = BakedPath::new(BLADE, vec![sample(0.0), sample(2.0)]);
    let cube = Shape::Box { half_extents: Vec3::splat(0.1) };
    let mut hits = HitQuery::new();

    let events = step(&mut hits, &world, &swing(&path, cube, ticks(0), ticks(1)));

    assert_eq!(events.len(), 1);
    assert_close(events[0].travel, 0.8 - 0.141_42);
    assert_near(events[0].point, Vec3::new(0.8, 1.0, 0.0));
    assert_near(events[0].normal, Vec3::NEG_X);

    // Unturned, its face is only 0.1 out.
    let sample = |x: f32| PathSample { translation: [x, 1.0, 0.0], ..PathSample::IDENTITY };
    let path = BakedPath::new(BLADE, vec![sample(0.0), sample(2.0)]);
    let mut hits = HitQuery::new();
    let events = step(&mut hits, &world, &swing(&path, cube, ticks(0), ticks(1)));
    assert_close(events[0].travel, 0.7);
}

#[test]
fn a_path_rides_the_roots_position_and_yaw() {
    // A root at (5, 0, 5) with yaw 90° turns local +Z onto world +X. A path at local (0, 1, 1)
    // is at world (6, 1, 5), and its unrotated +Y stays up. Local +X turns onto world -Z.
    let root = ActorTransform { position: Vec3::new(5.0, 0.0, 5.0), yaw: FRAC_PI_2 };
    let local = PathSample { translation: [0.0, 1.0, 1.0], ..PathSample::IDENTITY };
    let pose = volume_pose(root, local);
    assert_near(pose.position, Vec3::new(6.0, 1.0, 5.0));
    assert_near(pose.rotation * Vec3::Z, Vec3::X);
    assert_near(pose.rotation * Vec3::X, Vec3::NEG_Z);

    // A path's own rotation goes under the root's: a capsule laid along local +Z by a quarter turn
    // about X lies along world +X under that yaw.
    let laid = PathSample {
        translation: [0.0, 1.0, 1.0],
        rotation: det::quat_from_rotation_x(FRAC_PI_2).to_array(),
    };
    assert_near(volume_pose(root, laid).rotation * Vec3::Y, Vec3::X);

    // Swept in the world: the path runs local z from 1 to 3, world x from 6 to 8, into a 0.2 sphere
    // at (7, 1, 5). It touches 0.3 short, at x = 6.7.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(7.0, 1.0, 5.0));
    let path = path_through(&[Vec3::new(0.0, 1.0, 1.0), Vec3::new(0.0, 1.0, 3.0)]);
    let mut volume = swing(&path, SPHERE, ticks(0), ticks(1));
    volume.owner.root_before = root;
    volume.owner.root_after = root;
    let mut hits = HitQuery::new();
    let events = step(&mut hits, &world, &volume);
    assert_eq!(events.len(), 1);
    assert_close(events[0].travel, 0.7);
    assert_near(events[0].point, Vec3::new(6.8, 1.0, 5.0));
}

#[test]
fn a_moving_root_carries_the_volume_through_the_step() {
    // The path holds still in the root's space, a metre up and a metre in front. The root runs from
    // z = 0 to z = 4, so the volume runs from z = 1 to z = 5, and a 0.2 sphere at z = 3 is touched at
    // z = 2.7: 1.7 of 4 metres, 425 sub-ticks in.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(0.0, 1.0, 3.0));
    let path = path_through(&[Vec3::new(0.0, 1.0, 1.0); 2]);
    let mut volume = swing(&path, SPHERE, ticks(0), ticks(1));
    volume.owner.root_after = ActorTransform { position: Vec3::new(0.0, 0.0, 4.0), yaw: 0.0 };
    let mut hits = HitQuery::new();

    let events = step(&mut hits, &world, &volume);

    assert_eq!(events.len(), 1);
    assert_close(events[0].travel, 1.7);
    assert_eq!(events[0].at, sub(425));
    assert_near(events[0].point, Vec3::new(0.0, 1.0, 2.8));
}

// ------------------------------------------------------------------------------------------------
// The sub-step rule.
// ------------------------------------------------------------------------------------------------

/// Counts worked out by hand from the rule in `VolumeSweep::walk`.
#[test]
fn sub_steps_follow_the_stated_rule() {
    let far = Vec3::new(0.0, 1.0, 100.0);
    let capsule = Shape::Capsule { half_height: 0.5, radius: 0.05 };

    // A straight line takes one cast, however fast.
    let line = path_through(&[Vec3::ZERO, far]);
    assert_eq!(sub_steps(&swing(&line, SPHERE, ticks(0), ticks(1))).len(), 1);
    assert_eq!(sub_steps(&swing(&line, capsule, ticks(0), ticks(1))).len(), 1);

    // Every table sample inside the span starts a stretch, whichever part of a tick it starts in:
    // 500 to 2500 crosses samples 1 and 2, so three stretches.
    let run = path_through(&[Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z]);
    let walked = sub_steps(&swing(&run, SPHERE, sub(500), sub(2500)));
    let times: Vec<_> = walked.iter().map(|s| (s.from.as_subticks(), s.to.as_subticks())).collect();
    assert_eq!(times, [(500, 1000), (1000, 2000), (2000, 2500)]);

    // Turning: a capsule whose axis end is 0.5 out, 0.05 thick, turning 0.3 rad in a tick, moves its
    // end 0.15 m, or three thicknesses.
    let turning = turning_path(Vec3::Y, 0.3, 2);
    assert_eq!(sub_steps(&swing(&turning, capsule, ticks(0), ticks(1))).len(), 3);
    // A sphere looks the same however it's turned.
    assert_eq!(sub_steps(&swing(&turning, SPHERE, ticks(0), ticks(1))).len(), 1);
    // A box's far point is its corner: sqrt(3) * 0.1 = 0.1732 out, 0.1 thick, so 0.052 m at 0.3 rad is
    // within a thickness.
    let cube = Shape::Box { half_extents: Vec3::splat(0.1) };
    assert_eq!(sub_steps(&swing(&turning, cube, ticks(0), ticks(1))).len(), 1);
    // A thin plate has the thickness floor: corner sqrt(0.25 + 0.01 + 0.0001) = 0.51 out, floor
    // 0.05 and not 0.01, so 0.51 * 0.3 / 0.05 = 3.06, which rounds up to 4.
    let plate = Shape::Box { half_extents: Vec3::new(0.5, 0.1, 0.01) };
    assert_eq!(sub_steps(&swing(&turning, plate, ticks(0), ticks(1))).len(), 4);

    // The root's turn: a path 1.5 m from its axis, the root turning a half turn in the step, needs
    // PI * sqrt(1.5 / (8 * 0.01)) = 13.6, so 14 casts.
    assert_eq!(MAX_SAG, 0.01);
    let still = path_through(&[Vec3::new(0.0, 1.0, 1.5); 2]);
    let mut spun = swing(&still, SPHERE, ticks(0), ticks(1));
    spun.owner.root_after.yaw = PI;
    let walked = sub_steps(&spun);
    assert_eq!(walked.len(), 14);

    // And those casts hold to the sag they were chosen for. A chord between two points of the circle
    // sags most at its middle, so that's where to measure.
    for cast in &walked {
        let middle = (cast.start.position + cast.end.position) * 0.5;
        let sag = 1.5 - Vec3::new(middle.x, 0.0, middle.z).length();
        assert!(sag <= MAX_SAG, "sag {sag}");
    }
    // A root's turn turns a far-reaching volume too: a capsule on the same path has its axis end 0.5
    // out, turning PI in the step: 0.5 * PI / 0.05 = 31.4, so 32.
    let mut spun = swing(&still, capsule, ticks(0), ticks(1));
    spun.owner.root_after.yaw = -PI;
    assert_eq!(sub_steps(&spun).len(), 32, "a turn counts whichever way");
}

#[test]
fn sub_steps_join_up_and_land_exactly_on_both_ends() {
    let path = arc_path(1.5, PI, 6);
    let mut volume = swing(&path, Shape::Capsule { half_height: 0.5, radius: 0.05 }, sub(1250), sub(4750));
    volume.owner.delta = sub(4000);
    volume.owner.reading = sub(5000);
    volume.owner.root_before = ActorTransform { position: Vec3::new(1.0, 0.0, 2.0), yaw: 0.25 };
    volume.owner.root_after = ActorTransform { position: Vec3::new(3.0, 0.0, 1.0), yaw: 0.75 };

    let walked = sub_steps(&volume);

    assert!(walked.len() >= 4);
    let (first, last) = (walked.first().unwrap(), walked.last().unwrap());
    assert_eq!(first.from, volume.from);
    assert_eq!(first.start, volume.pose_at(volume.from));
    assert_eq!(last.to, volume.to);
    assert_eq!(last.end, volume.pose_at(volume.to));
    for pair in walked.windows(2) {
        assert_eq!(pair[0].to, pair[1].from, "no time is skipped or counted twice");
        assert_eq!(pair[0].end, pair[1].start, "no ground is either");
        assert!(pair[0].from < pair[0].to);
    }
}

#[test]
fn a_root_is_exact_at_both_ends_of_the_clocks_span_and_blends_between() {
    let before = ActorTransform { position: Vec3::new(1.0, 2.0, 3.0), yaw: 0.1 };
    let after = ActorTransform { position: Vec3::new(3.0, 2.0, 7.0), yaw: 0.9 };
    let owner = OwnerStep { delta: sub(1000), reading: sub(2000), root_before: before, root_after: after };
    assert_eq!(owner.root_at(sub(1000)), before);
    assert_eq!(owner.root_at(sub(2000)), after);
    assert_eq!(owner.root_at(sub(500)), before, "held before the span");
    assert_eq!(owner.root_at(sub(3000)), after, "held after it");
    let middle = owner.root_at(sub(1500));
    assert_near(middle.position, Vec3::new(2.0, 2.0, 5.0));
    assert_close(middle.yaw, 0.5);

    // A move a cancel started half way through this step: at its first instant the root has come half
    // of the step's way.
    let fresh = OwnerStep { delta: sub(1000), reading: sub(500), root_before: before, root_after: after };
    assert!(!fresh.is_frozen());
    assert_near(fresh.root_at(sub(0)).position, Vec3::new(2.0, 2.0, 5.0));
    assert_eq!(fresh.root_at(sub(500)), after);

    // Past a half turn stays the long way round: yaw isn't wrapped.
    let spun = OwnerStep {
        delta: sub(1000),
        reading: sub(1000),
        root_before: ActorTransform { yaw: 0.0, ..before },
        root_after: ActorTransform { yaw: TAU, ..before },
    };
    assert_close(spun.root_at(sub(500)).yaw, PI);

    // Only the clock says frozen: one instant of a span on a clock that moved isn't.
    let frozen = OwnerStep { delta: SimDuration::ZERO, ..owner };
    assert!(frozen.is_frozen());
    assert_eq!(frozen.root_at(sub(1999)), before);
    assert_eq!(frozen.root_at(sub(2000)), after);
}

#[test]
fn past_the_cap_the_last_cast_goes_all_the_way_and_is_counted() {
    // Ten times the casts allowed, one table sample each, along a straight line, so nothing is cut
    // short but the casts, and a target at the far end still can't be missed.
    let far = MAX_SUB_STEPS as u32 * 10;
    let points: Vec<Vec3> = (0..=far).map(|tick| Vec3::new(tick as f32, 1.0, 0.0)).collect();
    let path = path_through(&points);
    let end = u64::from(far);
    let volume = swing(&path, SPHERE, ticks(0), ticks(end));

    let walked = sub_steps(&volume);
    assert_eq!(walked.len() as u64, MAX_SUB_STEPS);
    assert_eq!(walked.last().unwrap().to, ticks(end));
    assert_eq!(walked.last().unwrap().end, volume.pose_at(ticks(end)));

    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.01 }, Vec3::new(far as f32 - 0.5, 1.0, 0.0));
    let mut hits = HitQuery::new();
    let events = step(&mut hits, &world, &volume);
    assert_eq!(events.len(), 1, "the cast past the cap still finds what's on it");
    assert_eq!(hits.stats().sweeps_capped, 1);
    assert_eq!(hits.stats().sub_steps, MAX_SUB_STEPS);

    // Spinning past what's allowed in one stretch gets as many as are left, and also gets counted.
    let still = path_through(&[Vec3::new(0.0, 1.0, 1.5); 2]);
    let mut spun = swing(&still, Shape::Capsule { half_height: 1.0, radius: 0.05 }, ticks(0), ticks(1));
    spun.owner.root_after.yaw = TAU * 4.0;
    let mut hits = HitQuery::new();
    hits.sweep(&empty_world(), &spun);
    assert_eq!(hits.stats().sweeps_capped, 1);
    assert_eq!(hits.stats().sub_steps, MAX_SUB_STEPS);
}

// ------------------------------------------------------------------------------------------------
// Arcs: what only a curve reaches, and what only a chord would.
// ------------------------------------------------------------------------------------------------

/// A half circle of radius 1.5, in six table intervals all in one step, as a time scale of six gives.
/// Its chord runs through the root from (1.5, 1, 0) to (-1.5, 1, 0).
fn half_circle() -> BakedPath {
    arc_path(1.5, PI, 6)
}

#[test]
fn only_a_curved_path_reaches_the_top_of_its_arc() {
    // A target at the arc's top, 1.5 m from the chord. Straight from end to end misses it by far.
    let mut world = empty_world();
    let top = Vec3::new(0.0, 1.0, 1.5);
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.05 }, top);
    let path = half_circle();
    let volume = swing(&path, SPHERE, ticks(0), ticks(6));
    let mut hits = HitQuery::new();

    let events = step(&mut hits, &world, &volume);

    assert_eq!(events.len(), 1, "the arc passes through the target");
    // By the table's six chords, the top is the sample at tick 3, with 3 chords of 2 * 1.5 * sin(15°)
    // = 0.7765 behind it, and a contact 0.15 short.
    let chord = 2.0 * 1.5 * det::sin(PI / 12.0);
    assert!((events[0].travel - (chord * 3.0 - 0.15)).abs() < 0.03, "{}", events[0].travel);
    assert!(events[0].at > ticks(2) && events[0].at < ticks(3), "{:?}", events[0].at);
    // The swing comes down from +X here, so the normal points back that way.
    assert!(events[0].normal.x > 0.9, "{}", events[0].normal);
}

#[test]
fn the_straight_chord_would_falsely_hit_what_the_arc_goes_around() {
    // A target on the chord, at the root, well inside the circle: the arc never comes within 1.3 m.
    // A cast straight from one end to the other goes through it, which proves the geometry.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.05 }, Vec3::new(0.0, 1.0, 0.0));
    let path = half_circle();
    let mut hits = HitQuery::new();

    assert!(step(&mut hits, &world, &swing(&path, SPHERE, ticks(0), ticks(6))).is_empty());

    let chord = path_through(&[Vec3::new(1.5, 1.0, 0.0), Vec3::new(-1.5, 1.0, 0.0)]);
    let mut hits = HitQuery::new();
    assert_eq!(step(&mut hits, &world, &swing(&chord, SPHERE, ticks(0), ticks(1))).len(), 1);
}

#[test]
fn a_roots_turn_within_one_tick_sweeps_a_true_arc() {
    // The table has one interval and the path holds still, so all the curve is the root's half turn
    // in one step. A root's yaw takes +Z toward +X, so the volume runs from (0, 1, 1.5) by (1.5, 1, 0)
    // to (0, 1, -1.5).
    let still = path_through(&[Vec3::new(0.0, 1.0, 1.5); 2]);
    let mut volume = swing(&still, SPHERE, ticks(0), ticks(1));
    volume.owner.root_after.yaw = PI;

    // A target at the arc's middle.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.05 }, Vec3::new(1.5, 1.0, 0.0));
    let mut hits = HitQuery::new();
    let events = step(&mut hits, &world, &volume);
    assert_eq!(events.len(), 1);
    assert!(events[0].at > sub(400) && events[0].at < sub(500), "{:?}", events[0].at);

    // A target on the chord, which is inside the arc all the way.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.05 }, Vec3::new(0.0, 1.0, 0.0));
    let mut hits = HitQuery::new();
    assert!(step(&mut hits, &world, &volume).is_empty());

    // The accuracy the rule promises. The volume's surface reaches out to 1.6 m and in to 1.4, and
    // a chord can pull that in by MAX_SAG at most. A thin target 3 cm clear outside is missed.
    let target = Shape::Sphere { radius: 0.01 };
    let mut world = empty_world();
    add_target(&mut world, 2, 0, target, Vec3::new(1.64, 1.0, 0.0));
    let mut hits = HitQuery::new();
    assert!(step(&mut hits, &world, &volume).is_empty(), "outside");
    // One 2 cm inside the surface all around the arc is hit, wherever the casts' corners fall.
    for angle in [0.3, 0.71, 1.1, FRAC_PI_2, 1.9, 2.37, 2.8] {
        let (sin, cos) = det::sin_cos(angle);
        let mut world = empty_world();
        add_target(&mut world, 2, 0, target, Vec3::new(1.57 * sin, 1.0, 1.57 * cos));
        let mut hits = HitQuery::new();
        assert_eq!(step(&mut hits, &world, &volume).len(), 1, "inside at {angle}");
    }
}

#[test]
fn a_turning_capsule_sweeps_what_lies_between_its_ends() {
    // A capsule at (0, 1, 0), half height 1, radius 0.1, turning about +Z from standing to lying
    // along -X. Its center never moves, so no cast has any length: only turning finds anything. Its
    // end traces a quarter circle of radius 1.
    let path = turning_path(Vec3::new(0.0, 1.0, 0.0), FRAC_PI_2, 2);
    let capsule = Shape::Capsule { half_height: 1.0, radius: 0.1 };
    let volume = swing(&path, capsule, ticks(0), ticks(1));
    let thin = Shape::Sphere { radius: 0.01 };

    // A thin target anywhere on that circle: the casts overlap, so none can slip between them.
    for angle in [0.2, 0.5, FRAC_PI_4, 1.0, 1.3] {
        let (sin, cos) = det::sin_cos(angle);
        let at = Vec3::new(0.0, 1.0, 0.0) + Vec3::new(-sin, cos, 0.0);
        let mut world = empty_world();
        add_target(&mut world, 2, 0, thin, at);
        let mut hits = HitQuery::new();
        let events = step(&mut hits, &world, &volume);
        assert_eq!(events.len(), 1, "at {angle}");
        let expected = 1000.0 * angle / FRAC_PI_2;
        let at = events[0].at.as_subticks() as f32;
        assert!((at - expected).abs() < 150.0, "at {angle}: touched at {at} and not {expected}");
    }

    // A thin target just past its end's reach of 1.1, which is 1.15 from the center: clear.
    let mut world = empty_world();
    let (sin, cos) = det::sin_cos(FRAC_PI_4);
    add_target(&mut world, 2, 0, thin, Vec3::new(0.0, 1.0, 0.0) + Vec3::new(-sin, cos, 0.0) * 1.15);
    let mut hits = HitQuery::new();
    assert!(step(&mut hits, &world, &volume).is_empty());

    // A target on the other side it never turns toward.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, thin, Vec3::new(0.0, 1.0, 0.0) + Vec3::new(sin, cos, 0.0));
    let mut hits = HitQuery::new();
    assert!(step(&mut hits, &world, &volume).is_empty());
}

// ------------------------------------------------------------------------------------------------
// Tunnelling.
// ------------------------------------------------------------------------------------------------

#[test]
fn no_speed_passes_a_thin_hurtbox() {
    // A wall 2 cm thick, faced at x = 3.99, against a swing at every speed, ticking along until it's
    // past: one hit, always, at the same place, whichever step it falls in.
    let mut world = empty_world();
    let thin = Shape::Box { half_extents: Vec3::new(0.01, 1.0, 1.0) };
    add_target(&mut world, 2, 0, thin, Vec3::new(4.0, 1.0, 0.0));

    for speed in [0.01_f32, 0.1, 0.5, 1.0, 2.5, 7.9, 8.0, 8.1, 40.0, 500.0, 5_000.0] {
        // Enough samples to get past the wall by more than a tick, whatever the speed.
        let samples = ((6.0 / speed).ceil() as usize + 2).min(640);
        let points: Vec<Vec3> = (0..samples).map(|tick| Vec3::new(tick as f32 * speed, 1.0, 0.0)).collect();
        let path = path_through(&points);
        let mut hits = HitQuery::new();
        let mut found = Vec::new();
        for tick in 0..samples as u64 - 1 {
            found.extend(step(&mut hits, &world, &swing(&path, SPHERE, ticks(tick), ticks(tick + 1))));
        }

        assert_eq!(found.len(), 1, "at {speed} m a tick");
        let event = found[0];
        assert_close(event.point.x, 3.99);
        assert_near(event.normal, Vec3::NEG_X);
        // Its center stops 0.1 short of the face, at x = 3.89: this many ticks in at that speed.
        let expected = 3.89 / speed * 1000.0;
        let at = event.at.as_subticks() as f32;
        assert!((at - expected).abs() <= 1.0 + expected * 1e-4, "at {speed}: {at} and not {expected}");
    }
}

#[test]
fn a_tiny_volume_finds_a_tiny_target_at_any_speed() {
    let thin = Shape::Sphere { radius: 0.005 };
    let mut world = empty_world();
    add_target(&mut world, 2, 0, thin, Vec3::new(50.0, 1.0, 0.0));
    for speed in [0.25_f32, 20.0, 1_000.0] {
        let path = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(speed, 1.0, 0.0)]);
        let mut hits = HitQuery::new();
        let tick_count = (50.0 / speed).floor() as u64 + 1;
        let mut found = 0;
        for tick in 0..tick_count {
            // The whole world slides along the path a tick at a time by the root.
            let mut volume = swing(&path, thin, ticks(0), ticks(1));
            let root = |at: u64| ActorTransform { position: Vec3::X * (at as f32 * speed), yaw: 0.0 };
            volume.owner.root_before = root(tick);
            volume.owner.root_after = root(tick);
            found += step(&mut hits, &world, &volume).len();
        }
        assert_eq!(found, 1, "at {speed}");
    }
}

#[test]
fn a_whole_arc_in_one_tick_finds_a_thin_target_anywhere_on_it() {
    let thin = Shape::Box { half_extents: Vec3::new(0.01, 0.5, 0.01) };
    // Everything a clock can do with a half circle: over twelve steps, three, or all in one.
    for (intervals, per_step) in [(12_u32, 1_u64), (12, 4), (12, 12), (6, 6), (24, 24), (48, 48)] {
        let path = arc_path(1.5, PI, intervals);
        for angle in [0.2_f32, 0.9, FRAC_PI_2, 2.2, 3.0] {
            let (sin, cos) = det::sin_cos(angle);
            let mut world = empty_world();
            add_target(&mut world, 2, 0, thin, Vec3::new(1.5 * cos, 1.0, 1.5 * sin));
            let mut hits = HitQuery::new();
            let mut found = Vec::new();
            let mut at = 0;
            while at < u64::from(intervals) {
                let next = (at + per_step).min(u64::from(intervals));
                found.extend(step(&mut hits, &world, &swing(&path, SPHERE, ticks(at), ticks(next))));
                at = next;
            }
            assert_eq!(found.len(), 1, "{intervals} intervals, {per_step} a step, at {angle}");
            // The volume's surface reaches the target 0.11 m of arc before its center would.
            let expected = (angle - 0.11 / 1.5) / PI * intervals as f32;
            let touched = found[0].at.as_subticks() as f32 / 1000.0;
            let chords = intervals as f32 / 12.0 * 0.05;
            assert!(
                (touched - expected).abs() < chords + 0.02,
                "{intervals}, {per_step}, at {angle}: tick {touched}, not {expected}",
            );
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Clocks.
// ------------------------------------------------------------------------------------------------

/// A move run on a real clock under a scale, from the first tick: what each step's reading is.
struct MoveClock {
    world: ScaledClock,
    local: sapphire_time::LocalClock,
    started: sapphire_time::SimTime,
    previous: Option<SimDuration>,
}

impl MoveClock {
    fn at_scale(per_mille: u32) -> Self {
        let mut world = ScaledClock::new();
        let mut local = world.new_local_clock();
        local.push_scale(ScaleSource::PerfectDodge, per_mille).unwrap();
        let started = local.now();
        Self { world, local, started, previous: None }
    }

    /// One fixed step of the clock: the reading before, and now.
    fn tick(&mut self) -> (Option<SimDuration>, SimDuration) {
        self.world.tick();
        let delta = self.local.tick(&self.world);
        let reading = self.local.now().since(self.started);
        let before = self.previous;
        assert_eq!(covered(before, reading), delta, "what `clocked` works out is `last_delta`");
        self.previous = Some(reading);
        (before, reading)
    }
}

/// How far a clock moved from one reading to the next: `LocalClock::last_delta`.
fn covered(previous: Option<SimDuration>, reading: SimDuration) -> SimDuration {
    sub(reading.as_subticks() - previous.map_or(0, SimDuration::as_subticks))
}

/// A swing's volume this step, on a clock's reading, inside a whole-move window.
fn clocked<'a>(
    path: &'a BakedPath,
    shape: Shape,
    previous: Option<SimDuration>,
    reading: SimDuration,
) -> VolumeSweep<'a> {
    let last = ticks(path.samples().len() as u64 - 1);
    let from = previous.unwrap_or(SimDuration::ZERO).min(last);
    let mut volume = swing(path, shape, from, reading.min(last));
    volume.owner.delta = covered(previous, reading);
    volume.owner.reading = reading;
    volume
}

/// Runs a whole swing at a time scale and returns each hit, and the most casts any step made.
fn run_swing(world: &PhysicsWorld, path: &BakedPath, shape: Shape, per_mille: u32) -> (Vec<HitEvent>, u64) {
    let end = ticks(path.samples().len() as u64 - 1);
    let mut clock = MoveClock::at_scale(per_mille);
    let mut hits = HitQuery::new();
    let mut found = Vec::new();
    let mut most_casts = 0;
    loop {
        let (previous, reading) = clock.tick();
        let before = hits.stats().sub_steps;
        found.extend(step(&mut hits, world, &clocked(path, shape, previous, reading)));
        most_casts = most_casts.max(hits.stats().sub_steps - before);
        if reading >= end {
            return (found, most_casts);
        }
    }
}

#[test]
fn a_swing_lands_the_same_at_a_tenth_and_at_three_times() {
    // Three thin targets along a half circle. A swing hits all three in order, at the same times on
    // its own clock, whichever speed the clock runs.
    let mut world = empty_world();
    let thin = Shape::Sphere { radius: 0.02 };
    let angles = [0.5_f32, FRAC_PI_2, 2.6];
    for (index, angle) in angles.iter().enumerate() {
        let (sin, cos) = det::sin_cos(*angle);
        add_target(&mut world, 2 + index as u32, 0, thin, Vec3::new(1.5 * cos, 1.0, 1.5 * sin));
    }
    let path = arc_path(1.5, PI, 30);

    let (normal, _) = run_swing(&world, &path, SPHERE, 1000);
    assert_eq!(normal.len(), 3);
    assert_eq!(normal.iter().map(|e| e.target).collect::<Vec<_>>(), [actor(2), actor(3), actor(4)]);

    for per_mille in [100, 300, 500, 1500, 3000] {
        let (scaled, _) = run_swing(&world, &path, SPHERE, per_mille);
        assert_eq!(scaled.len(), 3, "at {per_mille}‰");
        for (a, b) in normal.iter().zip(&scaled) {
            assert_eq!(a.target, b.target, "at {per_mille}‰");
            let apart = a.at.as_subticks().abs_diff(b.at.as_subticks());
            assert!(apart <= 2, "at {per_mille}‰, {:?} and {:?}", a.at, b.at);
            assert!(a.point.abs_diff_eq(b.point, 1e-3), "at {per_mille}‰, {} and {}", a.point, b.point);
        }
    }
}

#[test]
fn sub_steps_follow_the_clock() {
    let world = empty_world();
    // A straight run: a cast per table sample a step covers.
    let points: Vec<Vec3> = (0..=30).map(|tick| Vec3::new(tick as f32, 1.0, 0.0)).collect();
    let line = path_through(&points);
    assert_eq!(run_swing(&world, &line, SPHERE, 1000).1, 1, "one tick of table a step");
    assert_eq!(run_swing(&world, &line, SPHERE, 100).1, 1, "a tenth of a tick of it");
    assert_eq!(run_swing(&world, &line, SPHERE, 3000).1, 3, "three ticks of it");

    // A blade turning fast: 0.5 * 0.88 / 0.05 = 8.8, so the rule asks for nine casts a table interval,
    // however many steps a clock takes over it. A tenth's step turns a tenth as far, and takes one.
    let blade = Shape::Capsule { half_height: 0.5, radius: 0.05 };
    let turning = turning_path(Vec3::new(0.0, 1.0, 0.0), 0.88, 31);
    assert_eq!(run_swing(&world, &turning, blade, 1000).1, 9);
    assert_eq!(run_swing(&world, &turning, blade, 100).1, 1);
    assert_eq!(run_swing(&world, &turning, blade, 3000).1, 27);
}

#[test]
fn a_frozen_owner_sweeps_nothing_and_never_rehits() {
    // A swing already touching its target when hitstop lands.
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(2.0, 1.0, 0.0)]);
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(1.0, 1.0, 0.0));
    let mut clock = MoveClock::at_scale(1000);
    let mut hits = HitQuery::new();

    let (previous, reading) = clock.tick();
    assert_eq!(step(&mut hits, &world, &clocked(&path, SPHERE, previous, reading)).len(), 1);
    let touching = world.take_stats();
    assert!(touching.shape_casts > 0);

    // Frozen for ten ticks, with a second target walking right into the blade. The clock's reading
    // stands still, so nothing casts.
    clock.local.apply_hitstop(10);
    add_target(&mut world, 3, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(1.0, 1.0, 0.0));
    for _ in 0..10 {
        let (previous, reading) = clock.tick();
        assert_eq!(previous, Some(reading), "a clock frozen by hitstop reads what it read");
        let volume = clocked(&path, SPHERE, previous, reading);
        assert!(volume.owner.is_frozen());
        assert_eq!(sub_steps(&volume), []);
        assert!(step(&mut hits, &world, &volume).is_empty());
    }
    assert_eq!(world.take_stats().shape_casts, 0, "a frozen owner makes no casts at all");
    assert_eq!(hits.stats().frozen_skips, 10);

    // When it thaws, the swing picks up where it stopped: its first target isn't hit again, and the
    // one that walked in is.
    let (previous, reading) = clock.tick();
    let events = step(&mut hits, &world, &clocked(&path, SPHERE, previous, reading));
    assert_eq!(events.iter().map(|e| e.target).collect::<Vec<_>>(), [actor(3)]);
}

#[test]
fn hitstop_loses_no_ground() {
    // The same swing with a freeze in the middle touches the same things at the same times on its
    // own clock.
    let mut world = empty_world();
    let thin = Shape::Sphere { radius: 0.02 };
    for (index, angle) in [0.2_f32, 0.8, 1.4, 2.0, 2.6].into_iter().enumerate() {
        let (sin, cos) = det::sin_cos(angle);
        add_target(&mut world, 2 + index as u32, 0, thin, Vec3::new(1.5 * cos, 1.0, 1.5 * sin));
    }
    let path = arc_path(1.5, PI, 20);
    let (plain, _) = run_swing(&world, &path, SPHERE, 1000);
    assert_eq!(plain.len(), 5);

    let mut clock = MoveClock::at_scale(1000);
    let mut hits = HitQuery::new();
    let mut frozen = Vec::new();
    for tick in 0..60 {
        if tick == 7 || tick == 14 {
            clock.local.apply_hitstop(5);
        }
        let (previous, reading) = clock.tick();
        frozen.extend(step(&mut hits, &world, &clocked(&path, SPHERE, previous, reading)));
    }
    assert_eq!(hits.stats().frozen_skips, 10);
    assert_eq!(frozen, plain);
}

// ------------------------------------------------------------------------------------------------
// Windows.
// ------------------------------------------------------------------------------------------------

/// Runs a straight path of a metre a tick from x = 0 through a window at a steady scale, and returns
/// the x at which each hit landed. The root doesn't move.
fn x_of_hits(world: &PhysicsWorld, open: u32, close: u32, per_mille: u32) -> Vec<f32> {
    let points: Vec<Vec3> = (0..=20).map(|tick| Vec3::new(tick as f32, 1.0, 0.0)).collect();
    let path = path_through(&points);
    let mut clock = MoveClock::at_scale(per_mille);
    let mut hits = HitQuery::new();
    let mut found = Vec::new();
    for _ in 0..300 {
        let (previous, reading) = clock.tick();
        if reading > ticks(20) {
            break;
        }
        let window = Span { from: open, to: close };
        let Some((from, to)) = window.sweep(Stretch::read(previous, reading)) else { continue };
        let mut volume = clocked(&path, SPHERE, previous, reading);
        volume.from = from;
        volume.to = to;
        found.extend(step(&mut hits, world, &volume).into_iter().map(|e| e.point.x));
    }
    found
}

#[test]
fn a_window_opening_this_tick_cant_reach_back() {
    // The path passes x = 2.5 between ticks 2 and 3. A window opening at tick 3 is past it.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.05 }, Vec3::new(2.5, 1.0, 0.0));
    for per_mille in [100, 500, 1000, 2000, 3000] {
        assert_eq!(x_of_hits(&world, 3, 8, per_mille), [] as [f32; 0], "{per_mille}‰ reached back");
        // The same window a tick earlier gets it, so the volume really was on its way there.
        assert_eq!(x_of_hits(&world, 2, 8, per_mille).len(), 1, "at {per_mille}‰");
    }

    // A big step whose last tick lies in the window: at 3×, readings 3 and 6 both pass ticks a
    // window starting at tick 5 doesn't want. The step ending at tick 6 starts from tick 5.
    let path = path_through(&(0..=20).map(|tick| Vec3::new(tick as f32, 1.0, 0.0)).collect::<Vec<_>>());
    let (from, to) = Span { from: 5, to: 9 }.sweep(Stretch::read(Some(ticks(3)), ticks(6))).unwrap();
    assert_eq!((from, to), (ticks(5), ticks(6)));
    let mut volume = clocked(&path, SPHERE, Some(ticks(3)), ticks(6));
    volume.from = from;
    volume.to = to;
    let walked = sub_steps(&volume);
    assert_eq!(walked[0].from, ticks(5));
    assert_near(walked[0].start.position, Vec3::new(5.0, 1.0, 0.0));
    assert_eq!(walked.len(), 1);
}

#[test]
fn a_window_opening_part_way_through_a_step_carries_the_roots_share() {
    // The root runs 3 m along z over a step covering ticks 0 to 3. A window opening at tick 2 starts
    // two thirds of the way along the root's run.
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0); 4]);
    let mut volume = swing(&path, SPHERE, ticks(2), ticks(3));
    volume.owner.delta = ticks(3);
    volume.owner.root_after = ActorTransform { position: Vec3::new(0.0, 0.0, 3.0), yaw: 0.0 };
    let walked = sub_steps(&volume);
    assert_eq!(walked.len(), 1);
    assert_near(walked[0].start.position, Vec3::new(0.0, 1.0, 2.0));
    assert_near(walked[0].end.position, Vec3::new(0.0, 1.0, 3.0));

    // So it misses what stands behind its start.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.05 }, Vec3::new(0.0, 1.0, 1.0));
    let mut hits = HitQuery::new();
    assert!(step(&mut hits, &world, &volume).is_empty());
}

#[test]
fn a_window_closing_inside_a_step_stops_there() {
    // The window's arc ends at tick 3 (its last open tick), which is x = 3, so a target at x = 3.5 is
    // never reached at any speed, nor one at x = 3.2. One at x = 2.8 always is.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.05 }, Vec3::new(3.5, 1.0, 0.0));
    for per_mille in [100, 1000, 2000, 3000, 8000] {
        assert_eq!(x_of_hits(&world, 0, 4, per_mille), [] as [f32; 0], "{per_mille}‰ overran");
    }
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.05 }, Vec3::new(2.8, 1.0, 0.0));
    for per_mille in [100, 1000, 2000, 3000, 8000] {
        assert_eq!(x_of_hits(&world, 0, 4, per_mille).len(), 1, "at {per_mille}‰");
    }
}

#[test]
fn a_window_opening_and_closing_inside_one_step_still_counts() {
    // At 8× a window `2..6` lies entirely within the step from tick 0 to tick 8. Its arc is x = 2 to 5.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.05 }, Vec3::new(3.0, 1.0, 0.0));
    assert_eq!(x_of_hits(&world, 2, 6, 8000).len(), 1, "inside it");

    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.05 }, Vec3::new(1.0, 1.0, 0.0));
    assert_eq!(x_of_hits(&world, 2, 6, 8000), [] as [f32; 0], "before it");

    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.05 }, Vec3::new(5.5, 1.0, 0.0));
    assert_eq!(x_of_hits(&world, 2, 6, 8000), [] as [f32; 0], "after it");

    // A one-tick window at 8× is one instant: the path doesn't move through it, but it's tested.
    let one_tick = Span { from: 2, to: 3 }.sweep(Stretch::read(Some(ticks(0)), ticks(8)));
    assert_eq!(one_tick, Some((ticks(2), ticks(2))));
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.05 }, Vec3::new(2.1, 1.0, 0.0));
    assert_eq!(x_of_hits(&world, 2, 3, 8000).len(), 1, "on that instant");
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.05 }, Vec3::new(2.5, 1.0, 0.0));
    assert_eq!(x_of_hits(&world, 2, 3, 8000), [] as [f32; 0], "just past it");
}

#[test]
fn an_instant_tests_where_the_volume_stands() {
    // One sub-tick's span is one still sub-step, even with a clock that moved: the volume has only
    // just appeared here, or holds at the end of its arc.
    let path = path_through(&[Vec3::new(1.0, 1.0, 0.0); 2]);
    let volume = swing(&path, SPHERE, ticks(0), ticks(0));
    assert!(!volume.owner.is_frozen());
    let walked = sub_steps(&volume);
    assert_eq!(walked.len(), 1);
    assert_eq!(walked[0].start, walked[0].end);
    assert_eq!(walked[0].motion, 0.0);

    for (shape, at) in [
        (Shape::Sphere { radius: 0.2 }, Vec3::new(1.1, 1.0, 0.0)),
        (Shape::Box { half_extents: Vec3::splat(0.2) }, Vec3::new(1.1, 1.0, 0.0)),
        (Shape::Capsule { half_height: 0.5, radius: 0.2 }, Vec3::new(1.1, 1.0, 0.0)),
    ] {
        let mut world = empty_world();
        add_target(&mut world, 2, 0, shape, at);
        let mut hits = HitQuery::new();
        let events = step(&mut hits, &world, &volume);
        assert_eq!(events.len(), 1, "{shape:?}");
        assert_eq!(events[0].at, ticks(0));
        assert_eq!(events[0].travel, 0.0);
        assert!(events[0].normal.is_normalized());
    }

    // Too far off to touch: nothing, by any shape.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(1.31, 1.0, 0.0));
    let mut hits = HitQuery::new();
    assert!(step(&mut hits, &world, &volume).is_empty());
}

#[test]
fn a_box_holding_still_inside_a_target_faces_away_from_its_center() {
    // A box has no core to measure from, and physics answers back along the cast, which for a volume
    // holding still went nowhere in particular. The line between the centers says more.
    let path = path_through(&[Vec3::new(1.0, 1.0, 0.0); 2]);
    let cube = Shape::Box { half_extents: Vec3::splat(0.2) };
    let volume = swing(&path, cube, ticks(0), ticks(0));
    let thin = Shape::Sphere { radius: 0.05 };

    let mut world = empty_world();
    add_target(&mut world, 2, 0, thin, Vec3::new(1.1, 1.0, 0.0));
    let mut hits = HitQuery::new();
    let events = step(&mut hits, &world, &volume);
    assert_eq!(events.len(), 1);
    assert_near(events[0].normal, Vec3::NEG_X);

    // With the two centers together there's no such line, and it's still a direction.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, thin, Vec3::new(1.0, 1.0, 0.0));
    let mut hits = HitQuery::new();
    let events = step(&mut hits, &world, &volume);
    assert_eq!(events.len(), 1);
    assert!(events[0].normal.is_normalized());
}

#[test]
fn a_volume_holding_still_on_a_moving_clock_is_still_live() {
    // A clock running on a path with no motion: a target that walks into it gets hit.
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0); 4]);
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(5.0, 1.0, 0.0));
    let mut hits = HitQuery::new();
    assert!(step(&mut hits, &world, &swing(&path, SPHERE, ticks(0), ticks(1))).is_empty());

    let id = HurtboxId::new(actor(2), 0).unwrap();
    let here = PathSample { translation: [0.15, 1.0, 0.0], ..PathSample::IDENTITY };
    let target = Shape::Sphere { radius: 0.2 };
    id.set(&mut world, Some(target), ActorTransform::default(), here).unwrap();
    let events = step(&mut hits, &world, &swing(&path, SPHERE, ticks(1), ticks(2)));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].at, ticks(1));
}

// ------------------------------------------------------------------------------------------------
// Hurtboxes.
// ------------------------------------------------------------------------------------------------

#[test]
fn hurtbox_ids_say_who_owns_them() {
    for (raw, index) in [(1_u32, 0_u8), (1, 15), (2, 0), (77, 9), ((1 << 26) - 1, 15)] {
        let id = HurtboxId::new(actor(raw), index).unwrap();
        assert_eq!(id.actor(), actor(raw));
        assert_eq!(id.index(), index);
        assert_eq!(HurtboxId::from_body(id.body()), Some(id));
    }
    assert_eq!(HurtboxId::new(actor(1), MAX_HURTBOXES), None);
    assert_eq!(HurtboxId::new(actor(1 << 26), 0), None, "past what an ID holds");
    assert_eq!(HurtboxId::bodies_of(actor(1 << 26)), None);
    // A character's capsule, whose body ID is its actor's, isn't one.
    assert_eq!(HurtboxId::from_body(sapphire_physics::BodyId(1)), None);
    assert_eq!(HurtboxId::from_body(sapphire_physics::BodyId(1 << 31)), None, "actor 0 isn't one");

    // Actors' runs of IDs don't overlap, and each holds all of its own, its perfect-dodge volume last.
    let (first, last) = HurtboxId::bodies_of(actor(5)).unwrap();
    assert_eq!(first, HurtboxId::new(actor(5), 0).unwrap().body());
    assert!(HurtboxId::new(actor(5), 15).unwrap().body() < last);
    assert_eq!(last, dodge_body(actor(5)).unwrap());
    assert!(dodge_body(actor(4)).unwrap() < first);
    assert!(HurtboxId::new(actor(6), 0).unwrap().body() > last);
}

/// A perfect-dodge volume's body says whose it is, and is never taken for a hurtbox, or the reverse.
#[test]
fn a_perfect_dodge_volumes_id_says_whose_it_is() {
    for raw in [1_u32, 2, 77, (1 << 26) - 1] {
        let body = dodge_body(actor(raw)).unwrap();
        assert_eq!(dodger_of(body), Some(actor(raw)));
        assert_eq!(HurtboxId::from_body(body), None, "not a hurtbox");
        for index in [0, 7, 15] {
            assert_eq!(dodger_of(HurtboxId::new(actor(raw), index).unwrap().body()), None);
        }
    }
    assert_eq!(dodge_body(actor(1 << 26)), None, "past what an ID holds");
    assert_eq!(dodger_of(sapphire_physics::BodyId(1)), None, "a character's capsule");
    assert_eq!(dodger_of(sapphire_physics::BodyId(16)), None, "a capsule whose number looks like one");
    assert_eq!(dodger_of(sapphire_physics::BodyId((1 << 31) | 16)), None, "actor 0 isn't one");
}

#[test]
fn an_actors_own_hurtboxes_never_hit_it() {
    // Actor 3 has every hurtbox it can, all along the swing's way, and the actors whose IDs lie either
    // side of it have one each, further on. Their runs of body IDs touch actor 3's on both sides.
    let mut world = empty_world();
    for index in 0..MAX_HURTBOXES {
        add_target(&mut world, 3, index, Shape::Sphere { radius: 0.2 }, Vec3::new(1.0, 1.0, 0.0));
    }
    add_target(&mut world, 2, 15, Shape::Sphere { radius: 0.2 }, Vec3::new(2.0, 1.0, 0.0));
    add_target(&mut world, 4, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(3.0, 1.0, 0.0));
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(5.0, 1.0, 0.0)]);
    let targets = |attacker: u32| {
        let mut volume = swing(&path, SPHERE, ticks(0), ticks(1));
        volume.attacker = actor(attacker);
        let mut hits = HitQuery::new();
        let events = step(&mut hits, &world, &volume);
        events.iter().map(|e| e.target.raw()).collect::<Vec<_>>()
    };

    assert_eq!(targets(3), [2, 4], "none of its own, all of its neighbours'");
    assert_eq!(targets(2), [3, 4], "its neighbour's are just someone else's");
    assert_eq!(targets(4), [3, 2]);
    assert_eq!(targets(9), [3, 2, 4]);
}

#[test]
fn own_hurtboxes_take_no_room_from_targets() {
    // A cast's buffer is full of what it finds, nearest first, so an attacker standing in its own
    // swing must not push real targets out of it. Its own come first here, with others behind.
    let mut world = empty_world();
    for index in 0..MAX_HURTBOXES {
        add_target(&mut world, 1, index, Shape::Sphere { radius: 0.2 }, Vec3::new(1.0, 1.0, 0.0));
    }
    for owner in 0..MAX_CAST_RESULTS as u32 {
        add_target(&mut world, 2 + owner, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(2.0, 1.0, 0.0));
    }
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(3.0, 1.0, 0.0)]);
    let mut hits = HitQuery::new();
    let events = step(&mut hits, &world, &swing(&path, SPHERE, ticks(0), ticks(1)));
    assert_eq!(events.len(), MAX_CAST_RESULTS);
    assert_eq!(hits.stats().results_dropped, 0);
}

#[test]
fn a_sweep_sees_this_ticks_hurtbox() {
    // Who sets a hurtbox is whoever runs before `Hits` in the same tick: setting it, and nothing else,
    // is all a sweep needs to see it there.
    let mut world = empty_world();
    let id = HurtboxId::new(actor(2), 0).unwrap();
    let root = |x: f32| ActorTransform { position: Vec3::new(x, 0.0, 0.0), yaw: 0.0 };
    let chest = PathSample { translation: [0.0, 1.0, 0.0], ..PathSample::IDENTITY };
    let own = Shape::Sphere { radius: 0.2 };
    id.set(&mut world, Some(own), root(50.0), chest).unwrap();
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(2.0, 1.0, 0.0)]);
    let touched = |world: &PhysicsWorld| {
        let mut hits = HitQuery::new();
        step(&mut hits, world, &swing(&path, SPHERE, ticks(0), ticks(1)))
    };

    assert!(touched(&world).is_empty());
    id.set(&mut world, Some(own), root(1.0), chest).unwrap();
    let events = touched(&world);
    assert_eq!(events.len(), 1);
    assert_near(events[0].point, Vec3::new(0.8, 1.0, 0.0));
    assert_eq!(world.body_count(), 1, "it moved, and there's still one");

    // A move's track can change its shape. 0.5 around x = 1 is touched 0.3 sooner.
    let swollen = Shape::Sphere { radius: 0.5 };
    id.set(&mut world, Some(swollen), root(1.0), chest).unwrap();
    assert_eq!(world.body_shape(id.body()), Some(swollen));
    assert_eq!(world.body_count(), 1);
    assert_close(touched(&world)[0].travel, 0.4);

    // Or switch it off, which `HurtboxState::Off` does: then nothing touches it. Its own shape next
    // tick brings it back, wherever its root has got to.
    id.set(&mut world, None, root(1.0), chest).unwrap();
    assert_eq!(world.body_count(), 0);
    assert!(touched(&world).is_empty());
    id.set(&mut world, None, root(1.0), chest).unwrap();
    id.set(&mut world, Some(own), root(1.0), chest).unwrap();
    assert_eq!(touched(&world).len(), 1);

    // A hurtbox's own transform goes through its root's yaw, as a volume's does.
    let yawed = ActorTransform { position: Vec3::new(0.0, 0.0, 3.0), yaw: FRAC_PI_2 };
    let front = PathSample { translation: [0.0, 1.0, 1.0], ..PathSample::IDENTITY };
    id.set(&mut world, Some(own), yawed, front).unwrap();
    assert_near(world.body_pose(id.body()).unwrap().position, Vec3::new(1.0, 1.0, 3.0));

    // What the world refuses leaves it as it was.
    let bad = Shape::Sphere { radius: -1.0 };
    assert!(id.set(&mut world, Some(bad), yawed, front).is_err());
    assert_eq!(world.body_shape(id.body()), Some(own));
    let lost = ActorTransform { position: Vec3::NAN, yaw: 0.0 };
    assert!(id.set(&mut world, Some(swollen), lost, front).is_err());
    assert_eq!(world.body_shape(id.body()), Some(own));

    // A despawn takes them all, and no one else's.
    add_target(&mut world, 2, 9, SPHERE, Vec3::ZERO);
    add_target(&mut world, 7, 0, SPHERE, Vec3::ZERO);
    assert_eq!(remove_hurtboxes(&mut world, actor(2)), 2);
    assert_eq!(world.body_count(), 1, "another actor's stay");
}

#[test]
fn a_hit_credits_a_targets_nearest_hurtbox_once() {
    // A target with a chest at x = 2 and a head at x = 1.5: the swing reaches the head first, and hits
    // the target once for both.
    let mut world = empty_world();
    add_target(&mut world, 2, 7, Shape::Sphere { radius: 0.2 }, Vec3::new(2.0, 1.0, 0.0));
    add_target(&mut world, 2, 3, Shape::Sphere { radius: 0.2 }, Vec3::new(1.5, 1.0, 0.0));
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(3.0, 1.0, 0.0)]);
    let mut hits = HitQuery::new();

    let events = step(&mut hits, &world, &swing(&path, SPHERE, ticks(0), ticks(1)));
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].target, events[0].hurtbox), (actor(2), 3));
    assert_close(events[0].travel, 1.2);

    // And a target with two hurtboxes the same distance off credits the lower index.
    let mut world = empty_world();
    add_target(&mut world, 2, 7, Shape::Sphere { radius: 0.2 }, Vec3::new(1.5, 1.0, 0.0));
    add_target(&mut world, 2, 3, Shape::Sphere { radius: 0.2 }, Vec3::new(1.5, 1.0, 0.0));
    let mut hits = HitQuery::new();
    let events = step(&mut hits, &world, &swing(&path, SPHERE, ticks(0), ticks(1)));
    assert_eq!((events.len(), events[0].hurtbox), (1, 3));
}

#[test]
fn something_else_in_the_hurtbox_group_hits_nobody() {
    let scenery = StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::splat(0.1) }),
        pose: Pose::at(Vec3::new(1.0, 1.0, 0.0)),
        memberships: Group::Hurtbox.mask(),
    };
    let world = PhysicsWorld::new(vec![scenery], 4).unwrap();
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(2.0, 1.0, 0.0)]);
    let mut hits = HitQuery::new();
    assert!(step(&mut hits, &world, &swing(&path, SPHERE, ticks(0), ticks(1))).is_empty());
    assert_eq!(hits.stats().unowned_touches, 1);
}

// ------------------------------------------------------------------------------------------------
// World geometry.
// ------------------------------------------------------------------------------------------------

/// A wall across the swing's way at x = 2, 20 cm thick, and a target on each side.
fn walled_world() -> PhysicsWorld {
    let wall = StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(0.1, 2.0, 2.0) }),
        pose: Pose::at(Vec3::new(2.0, 1.0, 0.0)),
        memberships: Group::World.mask(),
    };
    let mut world = PhysicsWorld::new(vec![wall], 8).unwrap();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(1.0, 1.0, 0.0));
    add_target(&mut world, 3, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(3.0, 1.0, 0.0));
    world
}

#[test]
fn a_wall_stops_nothing_by_default() {
    let world = walled_world();
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(4.0, 1.0, 0.0)]);
    let mut hits = HitQuery::new();
    let events = step(&mut hits, &world, &swing(&path, SPHERE, ticks(0), ticks(1)));
    assert_eq!(events.iter().map(|e| e.target).collect::<Vec<_>>(), [actor(2), actor(3)]);
    assert_eq!(hits.stats().volumes_stopped, 0);
}

#[test]
fn a_wall_stops_a_volume_that_says_so_until_it_rearms() {
    // A metre a tick from x = 0: past the wall's face between ticks 1 and 2. A target before it is hit,
    // and one behind it is hit neither this tick nor when the path has gone through the wall.
    let world = walled_world();
    let points: Vec<Vec3> = (0..=6).map(|tick| Vec3::new(tick as f32, 1.0, 0.0)).collect();
    let path = path_through(&points);
    let run = |rearm: &[u32]| -> (Vec<(u64, ActorId)>, HitQuery) {
        let mut hits = HitQuery::new();
        let mut found = Vec::new();
        for tick in 0..6 {
            let mut volume = swing(&path, SPHERE, ticks(tick), ticks(tick + 1));
            volume.rearm = rearm;
            volume.blocked_by = Group::World.mask();
            found.extend(step(&mut hits, &world, &volume).into_iter().map(|e| (tick, e.target)));
        }
        (found, hits)
    };

    let (found, hits) = run(&[]);
    assert_eq!(found, [(0, actor(2))]);
    assert_eq!(hits.stats().volumes_stopped, 1);
    // Stops once, and makes no further casts: ticks 0 and 1, and none afterward.
    assert_eq!(hits.stats().sub_steps, 2);

    // A re-arm lets it start again, wherever the path is by then.
    let (found, hits) = run(&[4]);
    assert_eq!(found, [(0, actor(2))], "at tick 4 the volume is past both");
    assert_eq!(hits.stats().sub_steps, 2 + 2, "it stopped at tick 1 and started again at 4");
    let (found, _) = run(&[2]);
    // From tick 2 it's in the wall (touching at x = 2), so it stops again at once.
    assert_eq!(found, [(0, actor(2))]);
}

#[test]
fn a_stop_holds_for_the_rest_of_its_own_step() {
    // One step across three table intervals: x = 0 to 1.5, to 3, to 4.5. The wall's face stops the
    // volume in the second, and a target at x = 4 lies only in the third, past where it stopped.
    let mut world = walled_world();
    add_target(&mut world, 4, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(4.0, 1.0, 0.0));
    let points = [0.0, 1.5, 3.0, 4.5].map(|x| Vec3::new(x, 1.0, 0.0));
    let path = path_through(&points);
    let mut volume = swing(&path, SPHERE, ticks(0), ticks(3));
    volume.blocked_by = Group::World.mask();
    let mut hits = HitQuery::new();

    let events = step(&mut hits, &world, &volume);

    assert_eq!(events.iter().map(|e| e.target).collect::<Vec<_>>(), [actor(2)]);
    assert_eq!(hits.stats().sub_steps, 2, "no cast past the stop");
}

#[test]
fn a_stopped_volume_is_known_by_its_group_and_path() {
    // A volume's place in a step's list moves as other windows close, and it stays stopped. Another
    // path in the group isn't stopped with it, nor is a playing of the move that comes later.
    let world = walled_world();
    let points: Vec<Vec3> = (0..=6).map(|tick| Vec3::new(tick as f32, 1.0, 0.0)).collect();
    let path = path_through(&points);
    let other_path = BakedPath::new(OTHER, path.samples().to_vec());
    let stopping = |path, tick, volume| {
        let mut sweep = swing(path, SPHERE, ticks(tick), ticks(tick + 1));
        sweep.volume = volume;
        sweep.blocked_by = Group::World.mask();
        sweep
    };
    let mut hits = HitQuery::new();

    hits.sweep(&world, &stopping(&path, 1, 3));
    hits.finish_step();
    assert_eq!(hits.stats().volumes_stopped, 1);
    let casts = hits.stats().sub_steps;

    // It was third in the list, and now it's first. It makes no cast at all, past the wall or not.
    hits.sweep(&world, &stopping(&path, 2, 0));
    hits.finish_step();
    assert_eq!(hits.stats().sub_steps, casts);

    // Its group on another path sweeps, and stops at the wall for itself.
    hits.sweep(&world, &stopping(&other_path, 2, 1));
    hits.finish_step();
    assert_eq!(hits.stats().sub_steps, casts + 1);

    // A later playing starts clean.
    let mut next = stopping(&path, 2, 0);
    next.instance = 2;
    hits.sweep(&world, &next);
    hits.finish_step();
    assert_eq!(hits.stats().sub_steps, casts + 2);
}

#[test]
fn a_target_on_the_near_side_of_a_wall_in_the_same_cast_is_hit() {
    // Everything in one cast, the wall at 1.8 of a 4 m run: the near target counts, the far one doesn't.
    let world = walled_world();
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(4.0, 1.0, 0.0)]);
    let mut volume = swing(&path, SPHERE, ticks(0), ticks(1));
    volume.blocked_by = Group::World.mask();
    let mut hits = HitQuery::new();
    let events = step(&mut hits, &world, &volume);
    assert_eq!(events.iter().map(|e| e.target).collect::<Vec<_>>(), [actor(2)]);

    // A wall in a group the volume doesn't name doesn't stop it.
    let mut volume = swing(&path, SPHERE, ticks(0), ticks(1));
    volume.blocked_by = Group::CameraBlocker.mask();
    let mut hits = HitQuery::new();
    assert_eq!(step(&mut hits, &world, &volume).len(), 2);

    // Starting inside the wall stops it at once, and it finds nothing beyond.
    let inside = path_through(&[Vec3::new(2.0, 1.0, 0.0), Vec3::new(6.0, 1.0, 0.0)]);
    let mut volume = swing(&inside, SPHERE, ticks(0), ticks(1));
    volume.blocked_by = Group::World.mask();
    let mut hits = HitQuery::new();
    assert!(step(&mut hits, &world, &volume).is_empty());
    assert_eq!(hits.stats().volumes_stopped, 1);

    // What it touches right there still counts, moving or holding still.
    let mut world = walled_world();
    add_target(&mut world, 4, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(2.1, 1.0, 0.0));
    let mut hits = HitQuery::new();
    let events = step(&mut hits, &world, &volume);
    assert_eq!(events.iter().map(|e| e.target).collect::<Vec<_>>(), [actor(4)]);
    let mut instant = swing(&inside, SPHERE, ticks(0), ticks(0));
    instant.blocked_by = Group::World.mask();
    let mut hits = HitQuery::new();
    let events = step(&mut hits, &world, &instant);
    assert_eq!(events.iter().map(|e| e.target).collect::<Vec<_>>(), [actor(4)]);
    assert_eq!(hits.stats().volumes_stopped, 1);
}

// ------------------------------------------------------------------------------------------------
// Hit groups.
// ------------------------------------------------------------------------------------------------

/// A sphere running along x, with a target at x = 1 it overlaps from tick 0 to tick 2.
fn group_world() -> (PhysicsWorld, BakedPath) {
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 1.0 }, Vec3::new(1.0, 1.0, 0.0));
    let points: Vec<Vec3> = (0..=40).map(|tick| Vec3::new(tick as f32 * 0.05, 1.0, 0.0)).collect();
    (world, path_through(&points))
}

#[test]
fn a_target_is_hit_once_per_group_per_instance() {
    let (world, path) = group_world();
    let mut hits = HitQuery::new();
    let mut count = 0;
    for tick in 0..10 {
        count += step(&mut hits, &world, &swing(&path, SPHERE, ticks(tick), ticks(tick + 1))).len();
    }
    assert_eq!(count, 1, "touching for ten ticks is one hit");
    assert_eq!(hits.record_count(), 1);

    // A different group in the same move hits it again, once.
    let mut other = swing(&path, SPHERE, ticks(10), ticks(11));
    other.group = OTHER;
    assert_eq!(step(&mut hits, &world, &other).len(), 1);
    assert_eq!(step(&mut hits, &world, &other).len(), 0);
    assert_eq!(hits.record_count(), 2);

    // The same group on the next playing of the move hits it again, and retires the old records.
    let mut again = swing(&path, SPHERE, ticks(11), ticks(12));
    again.instance = 2;
    assert_eq!(step(&mut hits, &world, &again).len(), 1);
    assert_eq!(hits.record_count(), 1);

    // A different attacker's same group is its own.
    let mut third = swing(&path, SPHERE, ticks(12), ticks(13));
    third.attacker = actor(3);
    third.instance = 2;
    assert_eq!(step(&mut hits, &world, &third).len(), 1);
    assert_eq!(hits.record_count(), 2);
}

#[test]
fn the_last_stretch_of_a_cancelled_move_still_remembers_what_it_hit() {
    // A cancel inside a step publishes both instances' volumes that step. The old instance's touch of
    // a target it already hit isn't a new hit, even though the new instance retires its records in
    // the same step.
    let (world, path) = group_world();
    let mut hits = HitQuery::new();
    assert_eq!(step(&mut hits, &world, &swing(&path, SPHERE, ticks(0), ticks(1))).len(), 1);

    hits.sweep(&world, &swing(&path, SPHERE, ticks(1), ticks(2)));
    let mut next = swing(&path, SPHERE, ticks(0), ticks(1));
    next.instance = 2;
    hits.sweep(&world, &next);
    hits.finish_step();
    let events = hits.events();
    assert_eq!(events.len(), 1, "only the new instance's hit: {events:?}");
    assert_eq!(events[0].instance, 2);
}

#[test]
fn a_spin_rearms_on_its_authored_ticks_at_any_speed() {
    // Overlapping the whole way, re-armed at ticks 10 and 20: three hits, at 0, 10 and 20 exactly, on
    // the owner's clock, whichever speed it runs. Empty, one hit.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 2.0 }, Vec3::new(0.0, 1.0, 0.0));
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0); 31]);

    for per_mille in [100_u32, 300, 1000, 3000, 8000] {
        let run = |rearm: &[u32]| {
            let mut clock = MoveClock::at_scale(per_mille);
            let mut hits = HitQuery::new();
            let mut found = Vec::new();
            loop {
                let (previous, reading) = clock.tick();
                let mut volume = clocked(&path, SPHERE, previous, reading);
                volume.rearm = rearm;
                found.extend(step(&mut hits, &world, &volume));
                if reading >= ticks(30) {
                    return found;
                }
            }
        };
        let spin = run(&[10, 20]);
        let volleys: Vec<_> = spin.iter().map(|e| e.volley).collect();
        assert_eq!(volleys, [0, 1, 2], "at {per_mille}‰");
        assert_eq!(spin[1].at, ticks(10), "at {per_mille}‰");
        assert_eq!(spin[2].at, ticks(20), "at {per_mille}‰");
        assert_eq!(run(&[]).len(), 1, "at {per_mille}‰");
        // A re-arm past the end changes nothing.
        assert_eq!(run(&[31]).len(), 1, "at {per_mille}‰");
    }
}

#[test]
fn a_rearm_inside_one_step_lands_both_hits() {
    // One step across a re-arm, the target touching from the start: a hit either side of the tick.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 2.0 }, Vec3::new(0.0, 1.0, 0.0));
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0); 4]);
    let mut volume = swing(&path, SPHERE, ticks(0), ticks(3));
    volume.rearm = &[2];
    let mut hits = HitQuery::new();
    let events = step(&mut hits, &world, &volume);
    assert_eq!(events.iter().map(|e| (e.volley, e.at)).collect::<Vec<_>>(), [(0, ticks(0)), (1, ticks(2))]);
}

#[test]
fn volleys_count_rearm_ticks_reached() {
    let path = path_through(&[Vec3::ZERO; 2]);
    let mut volume = swing(&path, SPHERE, ticks(0), ticks(1));
    volume.rearm = &[3, 7];
    let volley = |subticks| volume.volley_at(sub(subticks));
    assert_eq!(volley(0), 0);
    assert_eq!(volley(2999), 0);
    assert_eq!(volley(3000), 1, "from its tick on");
    assert_eq!(volley(6999), 1);
    assert_eq!(volley(7000), 2);
    assert_eq!(volley(u64::MAX), 2);
}

#[test]
fn two_volumes_of_a_group_credit_the_nearer_then_the_lower_index() {
    // Two volumes in one group, the second running further from its target: the first touches it after
    // 0.5 and the second after 1.5, so the first gets the credit.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(3.0, 1.0, 0.0));
    let near = path_through(&[Vec3::new(2.2, 1.0, 0.0), Vec3::new(4.0, 1.0, 0.0)]);
    let far = path_through(&[Vec3::new(1.2, 1.0, 0.0), Vec3::new(4.0, 1.0, 0.0)]);

    let run = |first_path: &BakedPath, second_path: &BakedPath| {
        let mut hits = HitQuery::new();
        let mut a = swing(first_path, SPHERE, ticks(0), ticks(1));
        a.volume = 3;
        let mut b = swing(second_path, SPHERE, ticks(0), ticks(1));
        b.volume = 5;
        hits.sweep(&world, &a);
        hits.sweep(&world, &b);
        hits.finish_step();
        hits.events().to_vec()
    };

    let events = run(&near, &far);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].volume, 3);
    assert_close(events[0].travel, 0.5);
    let events = run(&far, &near);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].volume, 5, "the nearer, though it's the later volume");
    assert_close(events[0].travel, 0.5);
    // Equally near: the lower index.
    let events = run(&near, &near);
    assert_eq!((events.len(), events[0].volume), (1, 3));

    // Next tick neither touches it again.
    let mut hits = HitQuery::new();
    let a = swing(&near, SPHERE, ticks(0), ticks(1));
    assert_eq!(step(&mut hits, &world, &a).len(), 1);
    assert_eq!(step(&mut hits, &world, &a).len(), 0);
}

#[test]
fn a_nearer_contact_in_a_later_volley_outranks_a_farther_one_in_an_earlier() {
    // Two volumes of a group touch one target in one step, each in its own part of the move, either
    // side of a re-arm at tick 5. Order goes by nearness, so the later volley is written first, and a
    // contact counts only if its volley is later than the target's last: the earlier one doesn't.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(1.0, 1.0, 0.0));
    let along = |xs: [f32; 7]| path_through(&xs.map(|x| Vec3::new(x, 1.0, 0.0)));
    // The surfaces touch with the volume's center at x = 0.7: 0.1 along this one, from tick 5.
    let near = along([0.6, 0.6, 0.6, 0.6, 0.6, 0.6, 1.6]);
    // And 0.5 along this one, from tick 0.
    let far = along([0.2, 1.2, 1.2, 1.2, 1.2, 1.2, 1.2]);
    let mut late = swing(&near, SPHERE, ticks(5), ticks(6));
    late.rearm = &[5];
    let mut early = swing(&far, SPHERE, ticks(0), ticks(1));
    early.rearm = &[5];
    early.volume = 1;
    let mut hits = HitQuery::new();

    hits.sweep(&world, &late);
    hits.sweep(&world, &early);
    hits.finish_step();

    let events = hits.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!((events[0].volume, events[0].volley), (0, 1));
    assert_close(events[0].travel, 0.1);
}

#[test]
fn touching_one_target_again_and_again_takes_one_contact() {
    // Three volumes of a group, in a target for a whole capped sweep each: more touches than there are
    // contacts, all of them one target's. Only the nearest of them takes room.
    let mut world = empty_world();
    add_target(&mut world, 2, 0, Shape::Sphere { radius: 5.0 }, Vec3::new(0.0, 1.0, 0.0));
    let samples = MAX_SUB_STEPS as usize + 1;
    let points: Vec<Vec3> = (0..samples).map(|tick| Vec3::new(tick as f32 * 0.01, 1.0, 0.0)).collect();
    let path = path_through(&points);
    let mut hits = HitQuery::new();

    for volume in 0..3 {
        let mut sweep = swing(&path, SPHERE, ticks(0), ticks(MAX_SUB_STEPS));
        sweep.volume = volume;
        hits.sweep(&world, &sweep);
    }
    hits.finish_step();

    assert_eq!(hits.stats().touches, 3 * MAX_SUB_STEPS);
    assert!(hits.stats().touches > MAX_CONTACTS as u64);
    assert_eq!(hits.stats().contacts_dropped, 0);
    assert_eq!(hits.events().len(), 1);
    assert_eq!(hits.events()[0].volume, 0, "all start touching, so the lowest index");
}

// ------------------------------------------------------------------------------------------------
// Order.
// ------------------------------------------------------------------------------------------------

/// Three targets, two volumes in different groups and an attacker of each kind.
fn order_scene() -> (PhysicsWorld, BakedPath, BakedPath) {
    let mut world = empty_world();
    // Each of these is 0.3 + its gap from a path's start, so targets 5 and 6 tie, and so do 4 and 7.
    add_target(&mut world, 5, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(2.0, 1.0, 0.0));
    add_target(&mut world, 6, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(2.0, 1.0, 0.0));
    add_target(&mut world, 4, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(1.0, 1.0, 0.0));
    add_target(&mut world, 7, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(1.0, 1.0, 0.0));
    let forward = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(3.0, 1.0, 0.0)]);
    let backward = path_through(&[Vec3::new(3.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)]);
    (world, forward, backward)
}

/// The fields the stated order reads.
fn order_of(events: &[HitEvent]) -> Vec<(u32, u32, u16, u32)> {
    events.iter().map(|e| (e.travel.round() as u32, e.attacker.raw(), e.volume, e.target.raw())).collect()
}

#[test]
fn events_go_nearest_then_by_volume_then_by_target() {
    let (world, forward, backward) = order_scene();
    let volume = |attacker: u32, index: u16, group: Name, path| {
        let mut volume = swing(path, SPHERE, ticks(0), ticks(1));
        volume.attacker = actor(attacker);
        volume.volume = index;
        volume.group = group;
        volume
    };
    let run = |volumes: &[VolumeSweep<'_>]| {
        let mut hits = HitQuery::new();
        for v in volumes {
            hits.sweep(&world, v);
        }
        hits.finish_step();
        order_of(hits.events())
    };

    let forward_0 = volume(1, 0, SWING, &forward);
    let forward_1 = volume(1, 1, OTHER, &forward);
    let backward_2 = volume(2, 0, SWING, &backward);

    // Forward from 0: targets 4 and 7 at 0.7, 5 and 6 at 1.7. Backward from 3: 5 and 6 at 0.7, and
    // 4 and 7 at 1.7. Rounded, that's the first entry of each, with the order inside each tie read
    // left to right: attacker, volume, target.
    let expected = [
        (1, 1, 0, 4),
        (1, 1, 0, 7),
        (1, 1, 1, 4),
        (1, 1, 1, 7),
        (1, 2, 0, 5),
        (1, 2, 0, 6),
        (2, 1, 0, 5),
        (2, 1, 0, 6),
        (2, 1, 1, 5),
        (2, 1, 1, 6),
        (2, 2, 0, 4),
        (2, 2, 0, 7),
    ];
    assert_eq!(run(&[forward_0, forward_1, backward_2]), expected);
    // None of which is the order they were swept in, or the bodies were added in.
    assert_eq!(run(&[backward_2, forward_1, forward_0]), expected);
    assert_eq!(run(&[forward_1, backward_2, forward_0]), expected);
}

#[test]
fn the_order_bodies_were_added_in_changes_nothing() {
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(5.0, 1.0, 0.0)]);
    let targets = [(2, 1.0), (3, 2.0), (4, 2.0), (5, 3.0), (6, 3.0)];
    let run = |order: &[usize]| {
        let mut world = empty_world();
        for &index in order {
            let (owner, x) = targets[index];
            add_target(&mut world, owner, 0, Shape::Sphere { radius: 0.2 }, Vec3::new(x, 1.0, 0.0));
        }
        let mut hits = HitQuery::new();
        step(&mut hits, &world, &swing(&path, SPHERE, ticks(0), ticks(1)))
    };
    let expected = run(&[0, 1, 2, 3, 4]);
    assert_eq!(expected.len(), 5);
    assert_eq!(run(&[4, 3, 2, 1, 0]), expected);
    assert_eq!(run(&[2, 0, 4, 1, 3]), expected);
}

// ------------------------------------------------------------------------------------------------
// Overflow.
// ------------------------------------------------------------------------------------------------

/// `count` targets in a row along x, each a metre on from the last, and a path through them all.
fn crowd(count: u32) -> (PhysicsWorld, BakedPath) {
    let mut world = PhysicsWorld::new(Vec::new(), count as usize).unwrap();
    for index in 0..count {
        let at = Vec3::new(index as f32 + 1.0, 1.0, 0.0);
        add_target(&mut world, 2 + index, 0, Shape::Sphere { radius: 0.2 }, at);
    }
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(count as f32 + 2.0, 1.0, 0.0)]);
    (world, path)
}

#[test]
fn a_cast_with_too_little_room_counts_what_it_lost_and_keeps_the_nearest() {
    let over = MAX_CAST_RESULTS as u32 + 7;
    let (world, path) = crowd(over);
    let mut hits = HitQuery::new();

    let events = step(&mut hits, &world, &swing(&path, SPHERE, ticks(0), ticks(1)));

    assert_eq!(events.len(), MAX_CAST_RESULTS);
    assert_eq!(hits.stats().results_dropped, 7);
    assert_eq!(events.last().unwrap().target, actor(1 + MAX_CAST_RESULTS as u32), "the farthest went");
}

/// A step of one volume per attacker, each running through the whole crowd.
fn many_attackers(world: &PhysicsWorld, path: &BakedPath, hits: &mut HitQuery, attackers: u32) {
    for attacker in 0..attackers {
        let mut volume = swing(path, SPHERE, ticks(0), ticks(1));
        volume.attacker = actor(10_000 + attacker);
        hits.sweep(world, &volume);
    }
    hits.finish_step();
}

#[test]
fn a_full_event_queue_counts_and_the_hit_lands_next_tick() {
    // More new hits in a tick than the queue holds: the nearest go, the rest wait, and every target is
    // still hit exactly once, a tick late at most.
    let over = MAX_HIT_EVENTS as u32 + 5;
    let (world, path) = crowd(16);
    let mut hits = HitQuery::new();

    many_attackers(&world, &path, &mut hits, over / 16 + 1);
    let total = (over / 16 + 1) * 16;
    assert_eq!(hits.events().len(), MAX_HIT_EVENTS);
    let dropped = hits.stats().events_dropped;
    assert_eq!(dropped, u64::from(total) - MAX_HIT_EVENTS as u64);
    assert_eq!(hits.record_count(), MAX_HIT_EVENTS, "a dropped hit leaves no record");

    // The next tick lands the rest, and none of the first lot again.
    many_attackers(&world, &path, &mut hits, over / 16 + 1);
    assert_eq!(hits.events().len() as u64, dropped);
    assert_eq!(hits.record_count() as u32, total);
    many_attackers(&world, &path, &mut hits, over / 16 + 1);
    assert!(hits.events().is_empty());
}

#[test]
fn a_full_ledger_refuses_rather_than_hits_every_tick() {
    // Fill it, a few attackers at a time so no queue fills first.
    let (world, path) = crowd(16);
    let mut hits = HitQuery::new();
    let attackers = (MAX_HIT_RECORDS / 16) as u32;
    let batch = (MAX_HIT_EVENTS / 16) as u32;
    let mut landed = 0;
    for start in (0..attackers).step_by(batch as usize) {
        for attacker in start..(start + batch).min(attackers) {
            let mut volume = swing(&path, SPHERE, ticks(0), ticks(1));
            volume.attacker = actor(10_000 + attacker);
            hits.sweep(&world, &volume);
        }
        hits.finish_step();
        landed += hits.events().len();
    }
    assert_eq!(landed, MAX_HIT_RECORDS);
    assert_eq!(hits.record_count(), MAX_HIT_RECORDS);
    assert_eq!(hits.stats().hits_refused, 0);

    // One more attacker's hits: refused, every tick, and never a hit.
    let mut late = swing(&path, SPHERE, ticks(0), ticks(1));
    late.attacker = actor(20_000);
    for tick in 1..=3 {
        assert_eq!(step(&mut hits, &world, &late), []);
        assert_eq!(hits.stats().hits_refused, 16 * tick);
    }

    // And those on record stay on record: nothing hits twice for the book being full.
    let mut known = swing(&path, SPHERE, ticks(0), ticks(1));
    known.attacker = actor(10_000);
    assert_eq!(step(&mut hits, &world, &known), []);
    assert_eq!(hits.stats().hits_refused, 48);

    // A later instance frees the attacker's room, and it can hit again.
    let mut next = swing(&path, SPHERE, ticks(0), ticks(1));
    next.attacker = actor(10_000);
    next.instance = 2;
    assert_eq!(step(&mut hits, &world, &next).len(), 16);
    assert_eq!(hits.record_count(), MAX_HIT_RECORDS);

    // As does an actor's leaving, as attacker and as target.
    hits.forget_actor(actor(10_000));
    assert_eq!(hits.record_count(), MAX_HIT_RECORDS - 16);
    hits.forget_actor(actor(2));
    assert_eq!(hits.record_count(), MAX_HIT_RECORDS - 16 - (attackers as usize - 1));
    assert_eq!(step(&mut hits, &world, &late).len(), 16);
}

#[test]
fn contacts_past_their_limit_are_counted() {
    let per_attacker = 16;
    let (world, path) = crowd(per_attacker);
    let attackers = MAX_CONTACTS as u32 / per_attacker + 1;
    let mut hits = HitQuery::new();
    many_attackers(&world, &path, &mut hits, attackers);
    let extra = u64::from(attackers * per_attacker) - MAX_CONTACTS as u64;
    assert_eq!(hits.stats().contacts_dropped, extra);
    assert_eq!(hits.stats().events_dropped, (MAX_CONTACTS - MAX_HIT_EVENTS) as u64);
}

#[test]
fn a_volume_it_cant_use_is_counted_and_skipped() {
    let world = walled_world();
    let path = path_through(&[Vec3::new(0.0, 1.0, 0.0), Vec3::new(4.0, 1.0, 0.0)]);
    let mut hits = HitQuery::new();

    let mut bad_shape = swing(&path, Shape::Sphere { radius: 0.0 }, ticks(0), ticks(1));
    assert_eq!(step(&mut hits, &world, &bad_shape), []);
    bad_shape.shape = Shape::Sphere { radius: f32::NAN };
    assert_eq!(step(&mut hits, &world, &bad_shape), []);

    let backward = swing(&path, SPHERE, ticks(1), ticks(0));
    assert_eq!(step(&mut hits, &world, &backward), []);

    let mut lost = swing(&path, SPHERE, ticks(0), ticks(1));
    lost.owner.root_after.position.x = f32::NAN;
    assert_eq!(step(&mut hits, &world, &lost), []);
    lost.owner.root_after.position.x = 0.0;
    lost.owner.root_after.yaw = f32::INFINITY;
    assert_eq!(step(&mut hits, &world, &lost), []);

    assert_eq!(hits.stats().invalid_volumes, 5);
    assert_eq!(hits.stats().sub_steps, 0);
}

// ------------------------------------------------------------------------------------------------
// Determinism.
// ------------------------------------------------------------------------------------------------

/// A fight's worth of sweeps with no two actors alike. Everything comes from the tick, so two runs
/// are alike only if nothing here reads anything else.
fn fight(ticks_run: u64) -> Vec<HitEvent> {
    let blade = arc_path(1.4, 2.6, 20);
    let reverse = arc_path(1.1, -3.4, 20);
    let mut world = empty_world();
    let mut hits = HitQuery::new();
    let attackers = [(1_u32, 0.0_f32, 0.0_f32), (2, 2.5, 0.7), (3, -1.5, 2.2), (4, 0.5, -1.0)];
    for (attacker, x, yaw) in attackers {
        let root = ActorTransform { position: Vec3::new(x, 0.0, x * 0.5), yaw };
        for index in 0..3_u8 {
            let local =
                PathSample { translation: [0.0, 0.5 + 0.5 * f32::from(index), 0.0], ..PathSample::IDENTITY };
            let shape = Shape::Capsule { half_height: 0.3, radius: 0.25 + 0.05 * f32::from(index) };
            HurtboxId::new(actor(attacker), index)
                .unwrap()
                .set(&mut world, Some(shape), root, local)
                .unwrap();
        }
    }

    let mut all = Vec::new();
    for tick in 0..ticks_run {
        // Everyone walks a little circle, so their hurtboxes come and go.
        for (attacker, x, yaw) in attackers {
            let (sin, cos) = det::sin_cos(tick as f32 * 0.03 + x);
            let root = ActorTransform {
                position: Vec3::new(x + sin, 0.0, x * 0.5 + cos),
                yaw: yaw + tick as f32 * 0.01,
            };
            for index in 0..3_u8 {
                let local = PathSample {
                    translation: [0.0, 0.5 + 0.5 * f32::from(index), 0.0],
                    ..PathSample::IDENTITY
                };
                let shape = Shape::Capsule { half_height: 0.3, radius: 0.25 + 0.05 * f32::from(index) };
                HurtboxId::new(actor(attacker), index)
                    .unwrap()
                    .set(&mut world, Some(shape), root, local)
                    .unwrap();
            }
        }
        for (attacker, x, yaw) in attackers {
            let (sin, cos) = det::sin_cos(tick as f32 * 0.03 + x);
            let root = ActorTransform {
                position: Vec3::new(x + sin, 0.0, x * 0.5 + cos),
                yaw: yaw + tick as f32 * 0.01,
            };
            let elapsed = (tick + u64::from(attacker) * 3) % 21;
            if elapsed == 0 {
                continue;
            }
            let instance = (tick + u64::from(attacker) * 3) as u32 / 21;
            for (index, (path, shape, group)) in [
                (&blade, Shape::Capsule { half_height: 0.4, radius: 0.08 }, SWING),
                (&reverse, Shape::Box { half_extents: Vec3::new(0.1, 0.3, 0.05) }, OTHER),
            ]
            .into_iter()
            .enumerate()
            {
                let mut volume = swing(path, shape, ticks(elapsed - 1), ticks(elapsed));
                volume.attacker = actor(attacker);
                volume.instance = instance;
                volume.volume = index as u16;
                volume.group = group;
                volume.owner.root_before = root;
                volume.owner.root_after = root;
                if index == 0 {
                    volume.rearm = &[8, 15];
                }
                hits.sweep(&world, &volume);
            }
        }
        hits.finish_step();
        all.extend_from_slice(hits.events());
    }
    all
}

#[test]
fn the_same_fight_gives_the_same_hits_to_the_bit() {
    let first = fight(600);
    let second = fight(600);
    assert!(first.len() > 20, "the fight must do something: {} hits", first.len());
    assert!(first.iter().any(|e| e.volley > 0), "and its spins re-arm");
    assert_eq!(first.len(), second.len());
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a, b);
        assert_eq!(a.point.to_array().map(f32::to_bits), b.point.to_array().map(f32::to_bits));
        assert_eq!(a.normal.to_array().map(f32::to_bits), b.normal.to_array().map(f32::to_bits));
        assert_eq!(a.travel.to_bits(), b.travel.to_bits());
    }
}

/// A cast holds the rotation halfway through its sub-step, so its error is even on both sides of
/// where the volume truly is.
#[test]
fn a_cast_holds_the_rotation_halfway_through_its_sub_step() {
    // A box 1 m long along local X, turning about Z from 0 to 0.09 rad. Its corner is 1.0099 out, so it
    // moves 0.0909 m, inside the box's 0.1 m thickness: one cast, with the box standing still.
    let plate = Shape::Box { half_extents: Vec3::new(1.0, 0.1, 0.1) };
    let path = turning_path(Vec3::ZERO, 0.09, 2);
    let volume = swing(&path, plate, ticks(0), ticks(1));
    assert_eq!(sub_steps(&volume).len(), 1);

    // How high the box's top reaches near its far end: rotating the corner (1, 0.1) by `a` takes it to
    // y = sin(a) + 0.1 cos(a), which is 0.1 at the start, 0.14495 at 0.045 rad and 0.18955 at the end.
    // So a thin target over x = 0.994, the middle rotation's corner, tells them apart.
    let thin = Shape::Sphere { radius: 0.005 };
    let at_height = |y: f32| {
        let mut world = empty_world();
        add_target(&mut world, 2, 0, thin, Vec3::new(0.994, y, 0.0));
        let mut hits = HitQuery::new();
        !step(&mut hits, &world, &volume).is_empty()
    };
    assert!(at_height(0.14), "the start rotation doesn't reach this high");
    assert!(!at_height(0.155), "the end rotation reaches past this");
}

#[test]
fn a_turning_sub_step_counts_what_turning_moved() {
    // A capsule's axis end is 1 m out, turning 0.2 rad in a tick with its center still: 0.2 m at a
    // thickness of 0.1, so two casts of 0.1 m each, with no length to either.
    let path = turning_path(Vec3::Y, 0.2, 2);
    let blade = Shape::Capsule { half_height: 1.0, radius: 0.1 };
    let walked = sub_steps(&swing(&path, blade, ticks(0), ticks(1)));
    assert_eq!(walked.len(), 2);
    assert_close(walked[0].motion, 0.1);
    assert_close(walked[1].motion, 0.1);
}

// ---- Through a real step: what moves publish, joined by the adapter ----

const BODY: Name = Name::new("body");
const ARMOUR: Name = Name::new("armour");

/// Every actor's own hurtbox, as [`DUEL`] authors it.
const OWN: Shape = Shape::Sphere { radius: 0.1 };

/// Moves written to make distances easy to work out by hand.
const DUEL: &str = r#"(
    type: "sapphire::moves::MoveSetDocument",
    version: 1,
    data: (
        intents: [
            (name: "lunge", button: Light, buffer: 1),
            (name: "hold", button: Heavy, buffer: 1),
            (name: "duck", button: Dodge, buffer: 1),
            (name: "change", button: Skill, buffer: 1),
            (name: "pull", button: Ultimate, buffer: 1),
        ],
        neutral: [
            (intent: "lunge", move: "lunge"),
            (intent: "hold", move: "hold"),
            (intent: "duck", move: "duck"),
            (intent: "change", move: "change"),
            (intent: "pull", move: "pull"),
        ],
        // Every actor's own hurtbox: a sphere at chest height, a metre up its root.
        fighter: (
            health: 1000,
            hurtboxes: [(name: "body", offset: (0.0, 1.0, 0.0), shape: Sphere(radius: 0.1))],
        ),
        moves: [
            (
                // A blade held at z = 0.5 in the root's space, carried by the root 0.125 m a tick from
                // tick 4 to tick 12. Its arc stops at tick 11, with the root at 0.875.
                name: "lunge",
                startup: 4,
                active: 8,
                recovery: 8,
                motion: (
                    root: [(at: 4, offset: (0.0, 0.0, 0.0)), (at: 12, offset: (0.0, 0.0, 1.0))],
                    paths: [(
                        name: "blade",
                        keys: [(at: 0, offset: (0.0, 1.0, 0.5)), (at: 20, offset: (0.0, 1.0, 0.5))],
                    )],
                ),
                hitboxes: [(name: "cut", from: 4, to: 12, path: "blade", shape: Capsule(radius: 0.1, length: 0.4))],
            ),
            (
                // A sphere held still a metre in front, for two thirds of a second, and another out of
                // anyone's reach overhead, published first.
                name: "hold",
                startup: 2,
                active: 40,
                recovery: 2,
                motion: (
                    paths: [
                        (
                            name: "blade",
                            keys: [(at: 0, offset: (0.0, 1.0, 1.0)), (at: 44, offset: (0.0, 1.0, 1.0))],
                        ),
                        (
                            name: "overhead",
                            keys: [(at: 0, offset: (0.0, 9.0, 1.0)), (at: 44, offset: (0.0, 9.0, 1.0))],
                        ),
                    ],
                ),
                hitboxes: [
                    (name: "high", from: 2, to: 42, path: "overhead", shape: Sphere(radius: 0.2)),
                    (name: "poke", from: 2, to: 42, path: "blade", shape: Sphere(radius: 0.2)),
                ],
            ),
            (
                // Stands still, and can't be hit on ticks 0 to 6.
                name: "duck",
                startup: 1,
                active: 6,
                recovery: 30,
                invulnerable: [(from: 0, to: 7)],
            ),
            (
                // Its hurtbox goes on ticks 1 to 3, is a two-metre sphere on ticks 4 to 7, and is its
                // own again after.
                name: "change",
                startup: 1,
                active: 10,
                recovery: 10,
                hurtboxes: [
                    (name: "body", from: 1, to: 4, state: Off),
                    (name: "body", from: 4, to: 8, state: Shape(Sphere(radius: 2.0))),
                ],
            ),
            (
                // Comes in from z = 3 at 0.1 m a tick, with its window open only from z = 1.5, on tick
                // 15, to z = 1.1, on tick 19.
                name: "pull",
                startup: 15,
                active: 5,
                recovery: 10,
                motion: (
                    paths: [(
                        name: "blade",
                        keys: [(at: 0, offset: (0.0, 1.0, 3.0)), (at: 30, offset: (0.0, 1.0, 0.0))],
                    )],
                ),
                hitboxes: [(name: "tug", from: 15, to: 20, path: "blade", shape: Sphere(radius: 0.05))],
            ),
        ],
    ),
)"#;

fn duel_moves() -> MoveSet {
    MoveSetDocument::parse(DUEL).unwrap().compile().unwrap()
}

/// An attacker at the origin facing +Z, and a target `reach` metres in front of it, both able to play
/// [`DUEL`], the attacker's clock at this rate.
fn duel(reach: f32, per_mille: u32) -> (Simulation, ActorId, ActorId) {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = Some(sim.add_move_set(duel_moves()));
    let attacker = sim.spawn(ActorSpawn { moves, ..ActorSpawn::default() }).unwrap();
    let position = Vec3::new(0.0, 0.0, reach);
    let target = sim.spawn(ActorSpawn { position, moves, ..ActorSpawn::default() }).unwrap();
    sim.step(&TickActions::NEUTRAL);
    if per_mille != 1000 {
        sim.clock_mut(attacker).unwrap().0.push_scale(ScaleSource::Debug, per_mille).unwrap();
    }
    (sim, attacker, target)
}

/// One step of both actors pressing these.
fn press(attacker: (ActorId, Buttons), target: (ActorId, Buttons)) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    for (actor, buttons) in [attacker, target] {
        actions.set(actor, ActorActions { buttons, stick: Stick::CENTER }).unwrap();
    }
    actions
}

/// Everything hit in `steps` steps, after the attacker presses `button` on the first of them.
fn hits_of(
    sim: &mut Simulation,
    attacker: ActorId,
    target: ActorId,
    button: Buttons,
    steps: u32,
) -> Vec<HitEvent> {
    let mut found = Vec::new();
    sim.step(&press((attacker, button), (target, Buttons::NONE)));
    found.extend_from_slice(sim.hits().events());
    for _ in 1..steps {
        sim.step(&TickActions::NEUTRAL);
        found.extend_from_slice(sim.hits().events());
    }
    found
}

#[test]
fn a_published_swing_hits_once_where_its_root_carried_it() {
    // The blade's surface is at z = 0.6 plus the root, and the target's surface at 1.15 - 0.1 = 1.05,
    // out of the path's own reach: they meet with the root at 0.45, which is tick 4 + 0.45 / 0.125.
    let (mut sim, attacker, target) = duel(1.15, 1000);
    let found = hits_of(&mut sim, attacker, target, Buttons::LIGHT, 40);
    assert_eq!(found.len(), 1, "{found:?}");
    let hit = found[0];
    assert_eq!((hit.attacker, hit.target, hit.hurtbox), (attacker, target, 0));
    assert_eq!((hit.instance, hit.group, hit.volume, hit.volley), (1, Name::new("cut"), 0, 0));
    assert!(hit.at.as_subticks().abs_diff(7600) <= 2, "at {:?}", hit.at);
    assert_near(hit.point, Vec3::new(0.0, 1.0, 1.05));
    assert_near(hit.normal, Vec3::new(0.0, 0.0, -1.0));

    // The same run gives the same hits and the same state.
    let (mut again, attacker_again, target_again) = duel(1.15, 1000);
    assert_eq!(hits_of(&mut again, attacker_again, target_again, Buttons::LIGHT, 40), found);
    assert_eq!(again.checksum(), sim.checksum());
}

#[test]
fn each_playing_of_a_move_hits_anew() {
    // `hold` leaves the root alone, so its sphere rests on the target's chest for all its 44 ticks.
    // That's one hit for each playing, from the volume published second.
    let (mut sim, attacker, target) = duel(1.0, 1000);
    let first = hits_of(&mut sim, attacker, target, Buttons::HEAVY, 50);
    let second = hits_of(&mut sim, attacker, target, Buttons::HEAVY, 50);
    let summary =
        |hits: &[HitEvent]| hits.iter().map(|hit| (hit.instance, hit.group, hit.volume)).collect::<Vec<_>>();
    assert_eq!(summary(&first), [(1, Name::new("poke"), 1)]);
    assert_eq!(summary(&second), [(2, Name::new("poke"), 1)]);
}

#[test]
fn a_published_window_cant_reach_back() {
    for per_mille in [500, 1000, 3000, 7000] {
        // The path passes z = 2 at tick 10, before the window opens.
        let (mut sim, attacker, target) = duel(2.0, per_mille);
        assert_eq!(hits_of(&mut sim, attacker, target, Buttons::ULTIMATE, 60), [], "at {per_mille}‰");

        // It passes z = 1.3 at tick 17, inside it. The sphere touches the chest from 0.15 m away.
        let (mut sim, attacker, target) = duel(1.3, per_mille);
        let found = hits_of(&mut sim, attacker, target, Buttons::ULTIMATE, 60);
        assert_eq!(found.len(), 1, "at {per_mille}‰: {found:?}");
        assert!(found[0].at.as_subticks().abs_diff(15500) <= 3, "at {per_mille}‰: {:?}", found[0].at);
    }
}

#[test]
fn a_published_swing_reaches_as_far_at_any_speed() {
    // With the root at 0.45, on the even part of its motion: the same time into the move, as long as
    // the step that gets there lies wholly on that part. 7× doesn't, and hits a little later.
    for per_mille in [500, 1000, 2000, 3000, 4000, 5000] {
        let (mut sim, attacker, target) = duel(1.15, per_mille);
        let found = hits_of(&mut sim, attacker, target, Buttons::LIGHT, 60);
        assert_eq!(found.len(), 1, "at {per_mille}‰: {found:?}");
        assert!(found[0].at.as_subticks().abs_diff(7600) <= 3, "at {per_mille}‰: {:?}", found[0].at);
    }
    for per_mille in [7000, 11000] {
        let (mut sim, attacker, target) = duel(1.15, per_mille);
        assert_eq!(hits_of(&mut sim, attacker, target, Buttons::LIGHT, 60).len(), 1, "at {per_mille}‰");
    }

    // With the root at 0.85, near the arc's end. 5× blends across the root's stop at tick 12, which
    // is a straight line across a bend, and falls short of this: see `OwnerStep::root_at`.
    for per_mille in [500, 1000, 2000, 3000, 4000] {
        let (mut sim, attacker, target) = duel(1.55, per_mille);
        let found = hits_of(&mut sim, attacker, target, Buttons::LIGHT, 60);
        assert_eq!(found.len(), 1, "at {per_mille}‰: {found:?}");
        assert!(found[0].at.as_subticks().abs_diff(10800) <= 3, "at {per_mille}‰: {:?}", found[0].at);
    }

    // With the root at 0.9, past the arc's end at tick 11, though not past the root's own travel: a
    // step reading past the arc's end takes only its share of the root's motion.
    for per_mille in [500, 1000, 2000, 3000, 4000, 5000, 7000] {
        let (mut sim, attacker, target) = duel(1.6, per_mille);
        let found = hits_of(&mut sim, attacker, target, Buttons::LIGHT, 60);
        assert_eq!(found, [], "at {per_mille}‰");
    }
}

#[test]
fn an_invulnerable_target_takes_the_hit_when_its_window_closes() {
    // The attacker's held sphere and the target's chest overlap from the window's first tick. On its
    // own that's a hit at once.
    let (mut sim, attacker, target) = duel(1.0, 1000);
    let found = hits_of(&mut sim, attacker, target, Buttons::HEAVY, 10);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].at, ticks(2));
    assert_eq!(found[0].travel, 0.0);

    // Ducking on the same step, it isn't, for as long as the window lasts, and the hit it was spared
    // still lands: nothing spent it.
    let (mut sim, attacker, target) = duel(1.0, 1000);
    sim.step(&press((attacker, Buttons::HEAVY), (target, Buttons::DODGE)));
    let mut invulnerable_steps = 0;
    let mut landed = Vec::new();
    for _ in 0..30 {
        let view = sim.actor(target).unwrap().moves;
        if view.windows.invulnerable {
            invulnerable_steps += 1;
            assert_eq!(sim.hits().events(), [], "at {:?}", view.current.map(|mv| mv.elapsed));
        } else {
            landed.extend_from_slice(sim.hits().events());
            if !sim.hits().events().is_empty() {
                assert_eq!(view.current.unwrap().elapsed, ticks(7));
            }
        }
        sim.step(&TickActions::NEUTRAL);
    }
    assert_eq!(invulnerable_steps, 7);
    assert_eq!(landed.len(), 1, "{landed:?}");
    assert_eq!(landed[0].target, target);
}

#[test]
fn a_frozen_attacker_sweeps_nothing_it_published() {
    // The target comes out of its duck at tick 7 with the attacker's clock stopped from its tick 3.
    let (mut sim, attacker, target) = duel(1.0, 1000);
    sim.step(&press((attacker, Buttons::HEAVY), (target, Buttons::DODGE)));
    for _ in 0..3 {
        sim.step(&TickActions::NEUTRAL);
    }
    let reached = sim.actor(attacker).unwrap().moves.current.unwrap().elapsed;
    assert_eq!(reached, ticks(3));
    const FROZEN: u16 = 12;
    sim.clock_mut(attacker).unwrap().0.apply_hitstop(FROZEN);

    let swept = sim.hits().stats().frozen_skips;
    let mut still = 0;
    let mut landed = Vec::new();
    for _ in 0..30 {
        sim.step(&TickActions::NEUTRAL);
        landed.extend_from_slice(sim.hits().events());
        let view = sim.actor(attacker).unwrap().moves;
        if view.current.unwrap().elapsed == reached {
            still += 1;
            assert_eq!(view.volumes.len(), 2, "a frozen move goes on publishing");
            assert!(landed.is_empty(), "hit while frozen: {landed:?}");
        }
    }
    assert_eq!(still, u64::from(FROZEN));
    assert_eq!(sim.hits().stats().frozen_skips - swept, 2 * u64::from(FROZEN));
    assert_eq!(landed.len(), 1, "{landed:?}");
    assert_eq!(landed[0].at, reached, "it picks up where it stopped");
}

#[test]
fn what_moves_publish_is_what_a_hurtbox_becomes() {
    let (mut sim, attacker, target) = duel(5.0, 1000);
    let entity = sim.world().resource::<ActorTable>().entity(target).unwrap();
    let output = |sim: &Simulation| *sim.world().get::<MoveOutput>(entity).unwrap();

    assert_eq!(published_hurtbox(BODY, OWN, &output(&sim)), Some(OWN), "with no move playing");

    sim.step(&press((attacker, Buttons::NONE), (target, Buttons::SKILL)));
    let mut seen = Vec::new();
    for _ in 0..12 {
        let now = output(&sim);
        let tick = sim.actor(target).unwrap().moves.current.unwrap().tick();
        let expected = match tick {
            1..=3 => None,
            4..=7 => Some(Shape::Sphere { radius: 2.0 }),
            _ => Some(OWN),
        };
        assert_eq!(published_hurtbox(BODY, OWN, &now), expected, "tick {tick}");
        assert_eq!(published_hurtbox(ARMOUR, OWN, &now), Some(OWN), "another's, at tick {tick}");
        seen.push(tick);
        sim.step(&TickActions::NEUTRAL);
    }
    assert_eq!(seen, (0..12).collect::<Vec<_>>());

    // Invulnerable, none of its hurtboxes are there, whatever they're called.
    let (mut sim, attacker, target) = duel(5.0, 1000);
    let entity = sim.world().resource::<ActorTable>().entity(target).unwrap();
    sim.step(&press((attacker, Buttons::NONE), (target, Buttons::DODGE)));
    let now = *sim.world().get::<MoveOutput>(entity).unwrap();
    assert!(now.windows.invulnerable);
    assert_eq!(published_hurtbox(BODY, OWN, &now), None);
    assert_eq!(published_hurtbox(ARMOUR, OWN, &now), None);
}

#[test]
fn a_moves_shapes_become_a_querys_on_the_same_axis() {
    use sapphire_moves::Shape as MoveShape;
    assert_eq!(hit_shape(MoveShape::Sphere { radius: 0.3 }), Shape::Sphere { radius: 0.3 });
    // A move's capsule says how far apart its end spheres are, and a query's says half that.
    assert_eq!(
        hit_shape(MoveShape::Capsule { radius: 0.25, length: 0.9 }),
        Shape::Capsule { half_height: 0.45, radius: 0.25 }
    );
    assert_eq!(hit_shape(MoveShape::Capsule { radius: 0.25, length: 0.0 }), Shape::Sphere { radius: 0.25 });
    assert_eq!(
        hit_shape(MoveShape::Box { half_extents: [0.1, 0.2, 0.3] }),
        Shape::Box { half_extents: Vec3::new(0.1, 0.2, 0.3) }
    );
}
