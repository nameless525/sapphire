//! Brains going somewhere (M3 P3) through the public API: straight across a room, round a wall through
//! [`sapphire_nav`], along a patrol from a document, and after a foe they notice, on a floor made here with
//! a wall to hold each to. `replay::chase` has the same on the arena's real navmesh with a crowd.

use std::alloc::System;

use sapphire_core::Name;
use sapphire_core::alloc::CountingAllocator;
use sapphire_gameplay::replay::{PLAYERS, WARRIORS};
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, BrainState, Buttons, ControllerProfile, MoveSetId, PatrolError,
    PatrolId, ProfileId, SimConfig, Simulation, Stick, TickActions,
};
use sapphire_math::Vec3;
use sapphire_moves::{BrainWatch, MoveSet, MoveSetDocument, TickRange};
use sapphire_navmesh::{NO_NEIGHBOUR, NavMesh};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};
use serde::Deserialize;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const CHASER: &str = include_str!("../../sapphire_moves/test_content/chaser.moves.ron");

/// The floor's side, in metres, from the origin along +X and +Z.
const SIDE: i32 = 40;

/// The cells a wall takes up across x, and how far along z it runs from the floor's edge: it leaves a
/// way round past z = 30. A cell either side of the wall's body is what a bake's cut for a capsule comes
/// to, as the wall's box stands 0.3 m inside its cells.
const WALL_CELLS: std::ops::Range<i32> = 19..22;
const WALL_LENGTH: i32 = 30;

const WATCH: BrainWatch = BrainWatch { sight: 8.0, lose: 14.0, walk: 0.3 };

/// A navmesh of metre squares over the floor, with the wall's cells left out if `walled`.
fn navmesh(walled: bool) -> Vec<u8> {
    let vertex = |x: i32, z: i32| (x + z * (SIDE + 1)) as u32;
    let open = |x: i32, z: i32| {
        (0..SIDE).contains(&x)
            && (0..SIDE).contains(&z)
            && !(walled && WALL_CELLS.contains(&x) && z < WALL_LENGTH)
    };
    let mut ids = vec![NO_NEIGHBOUR; (SIDE * SIDE) as usize];
    let mut count = 0;
    for z in 0..SIDE {
        for x in 0..SIDE {
            if open(x, z) {
                ids[(x + z * SIDE) as usize] = count;
                count += 1;
            }
        }
    }
    let across = |x: i32, z: i32| if open(x, z) { ids[(x + z * SIDE) as usize] } else { NO_NEIGHBOUR };
    let mut mesh =
        NavMesh { cell_size: 1.0, cell_height: 1.0, polygon_starts: vec![0], ..NavMesh::default() };
    for z in 0..=SIDE {
        for x in 0..=SIDE {
            mesh.vertices.push([x, 0, z]);
        }
    }
    for z in 0..SIDE {
        for x in 0..SIDE {
            if open(x, z) {
                mesh.corners.extend([vertex(x, z), vertex(x, z + 1), vertex(x + 1, z + 1), vertex(x + 1, z)]);
                mesh.neighbours.extend([
                    across(x - 1, z),
                    across(x, z + 1),
                    across(x + 1, z),
                    across(x, z - 1),
                ]);
                mesh.polygon_starts.push(mesh.corners.len() as u32);
            }
        }
    }
    mesh.to_cooked().expect("it writes").to_vec()
}

fn aligned(bytes: &[u8]) -> sapphire_serialize::cooked::AlignedVec {
    let mut aligned = sapphire_serialize::cooked::AlignedVec::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    aligned
}

fn collider(id: u32, centre: Vec3, half_extents: Vec3) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Convex(Shape::Box { half_extents }),
        pose: Pose::at(centre),
        memberships: Group::World.mask(),
    }
}

/// The chaser's document, with whatever a test wants of it.
fn moves(edit: impl FnOnce(&mut MoveSetDocument)) -> MoveSet {
    let mut document = MoveSetDocument::parse(CHASER).expect("it parses");
    edit(&mut document);
    document.compile().expect("it compiles")
}

/// One that watches, and never finds the time to do anything else.
fn guard_moves() -> MoveSet {
    moves(|document| document.brain.as_mut().expect("it has one").watch = Some(WATCH))
}

/// One that presses as soon as anything's in reach of 9 m: nothing but a wall would stop it.
fn eager_moves() -> MoveSet {
    moves(|document| {
        let brain = document.brain.as_mut().expect("it has one");
        brain.patience = TickRange { min: 1, max: 1 };
        brain.attacks[0].reach.far = 9.0;
        brain.attacks[0].reach.best = 1.0;
    })
}

/// A floor, with a wall if it has one, a navmesh of the same, and someone for the brains to be after.
struct Room {
    sim: Simulation,
    foe: ActorId,
    moves: Option<MoveSetId>,
    controller: Option<ProfileId>,
}

impl Room {
    fn new(walled: bool, navigated: bool, set: MoveSet) -> Self {
        Self::at(walled, navigated, set, Vec3::new(30.0, 0.0, 10.0))
    }

    fn at(walled: bool, navigated: bool, set: MoveSet, foe: Vec3) -> Self {
        let mut sim = Simulation::new(SimConfig { seed: 3, max_actors: 8, ..SimConfig::default() });
        let mut colliders = vec![collider(0, Vec3::new(20.0, -0.5, 20.0), Vec3::new(60.0, 0.5, 60.0))];
        if walled {
            let length = WALL_LENGTH as f32 - 0.3;
            colliders.push(collider(
                1,
                Vec3::new(20.5, 1.5, length / 2.0 - 2.0),
                Vec3::new(1.2, 1.5, length / 2.0 + 2.0),
            ));
        }
        let physics = PhysicsWorld::new(colliders, sim.body_capacity()).expect("a good world");
        sim.set_physics_world(physics).expect("before any spawn");
        if navigated {
            sim.set_navigation(&aligned(&navmesh(walled))).expect("a good navmesh");
        }
        let controller = Some(sim.add_controller_profile(&ControllerProfile::default()).expect("a good one"));
        let target = Some(sim.add_move_set(moves(|_| {})));
        let moves = Some(sim.add_move_set(set));
        let foe = sim
            .spawn(ActorSpawn {
                position: foe,
                moves: target,
                controller,
                faction: PLAYERS,
                ..ActorSpawn::default()
            })
            .expect("room");
        sim.step(&TickActions::NEUTRAL);
        Self { sim, foe, moves, controller }
    }

    /// Someone with a brain.
    fn brain_at(&mut self, x: f32, z: f32) -> ActorId {
        let brain = self.spawned(x, z);
        assert!(self.sim.give_brain(brain));
        brain
    }

    fn spawned(&mut self, x: f32, z: f32) -> ActorId {
        let position = Vec3::new(x, 0.0, z);
        let (moves, controller) = (self.moves, self.controller);
        self.sim
            .spawn(ActorSpawn { position, moves, controller, faction: WARRIORS, ..ActorSpawn::default() })
            .expect("room")
    }

    fn at_foe(&self, brain: ActorId) -> f32 {
        ground(self.place(self.foe), self.place(brain))
    }

    fn place(&self, actor: ActorId) -> Vec3 {
        self.sim.actor(actor).expect("alive").position
    }

    fn searches(&mut self) -> u32 {
        self.sim.navigation_mut().expect("it has one").take_stats().searches
    }

    /// One tick with the foe's stick where `stick` puts it.
    fn step(&mut self, stick: Stick) {
        let mut actions = TickActions::NEUTRAL;
        let foe = ActorActions { buttons: Buttons::NONE, stick };
        actions.set(self.foe, foe).expect("one fits");
        self.sim.step(&actions);
    }

    fn steps(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.step(Stick::CENTER);
        }
    }
}

fn ground(a: Vec3, b: Vec3) -> f32 {
    ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

/// What §5.19 asks: in the same room as its foe a brain steers straight at it and costs no search, every
/// tick of the way, as it does on no navmesh at all.
#[test]
fn a_clear_line_steers_straight_with_no_search() {
    let mut room = Room::new(true, true, moves(|_| {}));
    let brain = room.brain_at(26.0, 14.0);
    room.steps(1);
    for tick in 0..30 {
        let (here, foe) = (room.place(brain), room.place(room.foe));
        let stick = room.sim.actor(brain).expect("alive").brain.stick().to_vec2();
        let length = (stick.x * stick.x + stick.y * stick.y).sqrt();
        let (x, z) = (foe.x - here.x, foe.z - here.z);
        let metres = (x * x + z * z).sqrt();
        assert!((stick.x / length - x / metres).abs() < 0.02, "tick {tick}: {stick} toward {x}, {z}");
        assert!((stick.y / length - z / metres).abs() < 0.02, "tick {tick}: {stick} toward {x}, {z}");
        room.steps(1);
    }
    assert_eq!(room.searches(), 0, "a clear line needs no route");
    assert!(room.sim.path_of(brain).expect("alive").route_end().is_none());
    room.steps(120);
    assert!(room.at_foe(brain) < 2.2, "and it gets there: {}", room.at_foe(brain));
}

/// With a wall in the way it walks the route round, and a brain on the same floor with no navmesh just
/// walks into the wall, which is what the mesh is for.
#[test]
fn a_wall_is_walked_round_and_once_past_it_the_line_is_clear() {
    let mut room = Room::new(true, true, moves(|_| {}));
    let brain = room.brain_at(10.0, 10.0);
    let start = room.place(brain);
    room.steps(1);
    let first = room.sim.actor(brain).expect("alive").brain.stick().to_vec2();
    // Along the corridor's stair of squares, which is why it isn't aimed at the corner itself.
    assert!(first.y > 0.1, "it heads for the way round, past z = 30: {first}");
    assert!(room.sim.path_of(brain).expect("alive").route_end().is_some(), "with a route");

    let mut furthest_along = 0.0_f32;
    for _ in 0..1500 {
        room.steps(1);
        furthest_along = furthest_along.max(room.place(brain).z);
        if room.at_foe(brain) < 2.2 {
            break;
        }
    }
    assert!(furthest_along > 25.0, "it went round: {furthest_along}");
    assert!(room.at_foe(brain) < 2.2, "and got there: {} from {start}", room.at_foe(brain));
    assert!(room.searches() < 10, "without looking for it again and again");
    assert!(room.sim.path_of(brain).expect("alive").route_end().is_none(), "and has no route now");

    let mut blind = Room::new(true, false, moves(|_| {}));
    let lost = blind.brain_at(10.0, 10.0);
    blind.steps(600);
    assert!(blind.place(lost).x < 20.0, "it's at the wall: {}", blind.place(lost));
}

/// Nobody swings at a wall, however near what's behind it, and the same brain swings as soon as it's round.
#[test]
fn it_presses_nothing_through_a_wall() {
    // 4 m apart through the wall, in reach of an attack that presses as soon as it can.
    let mut room = Room::at(true, true, eager_moves(), Vec3::new(22.5, 0.0, 26.0));
    let brain = room.brain_at(18.5, 26.0);
    let mut pressed_at = None;
    for tick in 0..900 {
        room.steps(1);
        let view = room.sim.actor(brain).expect("alive");
        let state = view.brain.state();
        if matches!(state, BrainState::Attacking { .. }) {
            pressed_at = Some((tick, view.position));
            break;
        }
    }
    let (tick, place) = pressed_at.expect("it swings in the end");
    assert!(tick > 30, "it had to walk round first: tick {tick}");
    assert!(place.z > 26.0, "and it's near the wall's end by then: {place}");

    let mut open = Room::at(false, true, eager_moves(), Vec3::new(22.5, 0.0, 26.0));
    let brain = open.brain_at(18.5, 26.0);
    open.steps(3);
    let state = open.sim.actor(brain).expect("alive").brain.state();
    assert!(matches!(state, BrainState::Attacking { .. }), "with no wall, at once: {state:?}");
}

/// A foe with no way to it isn't walked into the wall: it goes as near as the mesh says, which on a room of
/// one polygon is that polygon's middle, and stands.
#[test]
fn with_no_way_to_its_foe_it_stands() {
    // A wall right across, so the far side's another island.
    let mut sim = Simulation::new(SimConfig { seed: 3, max_actors: 4, ..SimConfig::default() });
    let floor = collider(0, Vec3::new(20.0, -0.5, 20.0), Vec3::new(60.0, 0.5, 60.0));
    let wall = collider(1, Vec3::new(20.5, 1.5, 20.0), Vec3::new(1.2, 1.5, 60.0));
    sim.set_physics_world(PhysicsWorld::new(vec![floor, wall], sim.body_capacity()).expect("good"))
        .expect("before any spawn");
    let mut mesh =
        NavMesh { cell_size: 1.0, cell_height: 1.0, polygon_starts: vec![0], ..NavMesh::default() };
    // Two rooms, side by side, ten metres across, with no edge between them.
    for (first, x) in [(0, 8), (4, 24)] {
        mesh.vertices.extend([[x, 0, 8], [x, 0, 18], [x + 10, 0, 18], [x + 10, 0, 8]]);
        mesh.corners.extend([first, first + 1, first + 2, first + 3]);
        mesh.neighbours.extend([NO_NEIGHBOUR; 4]);
        mesh.polygon_starts.push(first + 4);
    }
    sim.set_navigation(&mesh.to_cooked().expect("it writes")).expect("a good navmesh");
    let controller = Some(sim.add_controller_profile(&ControllerProfile::default()).expect("a good one"));
    let set = Some(sim.add_move_set(moves(|_| {})));
    let foe_at = Vec3::new(28.0, 0.0, 12.0);
    let spawn =
        |position, faction| ActorSpawn { position, moves: set, controller, faction, ..ActorSpawn::default() };
    sim.spawn(spawn(foe_at, PLAYERS)).expect("room");
    let brain = sim.spawn(spawn(Vec3::new(12.0, 0.0, 12.0), WARRIORS)).expect("room");
    assert!(sim.give_brain(brain));
    for _ in 0..60 {
        sim.step(&TickActions::NEUTRAL);
    }
    let view = sim.actor(brain).expect("alive");
    assert_eq!(view.brain.stick(), Stick::CENTER);
    assert!(ground(view.position, Vec3::new(13.0, 0.0, 13.0)) < 0.2, "{}", view.position);
    assert!(view.brain.target().is_some(), "it knows who it's after");
}

/// A patrol's a document: this file names a new one, no Rust knows it, and a brain walks it in order.
#[test]
fn a_new_patrol_is_a_document_and_a_brain_walks_it_round_a_wall() {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Doc {
        name: String,
        points: Vec<[f32; 3]>,
        closed: bool,
    }
    const DOC: &str = r#"(
        name: "round_the_wall",
        points: [(5.0, 0.0, 5.0), (5.0, 0.0, 20.0), (35.0, 0.0, 20.0), (35.0, 0.0, 5.0)],
        closed: true,
    )"#;
    let doc: Doc = ron::from_str(DOC).expect("it parses");
    let bad = r#"(name: "typo", points: [], closed: false, speed: 3.0)"#;
    let error = ron::from_str::<Doc>(bad).err().expect("no such field").to_string();
    assert!(error.contains("speed"), "it names the field: {error}");

    // Far from the foe, so nothing distracts it, past the wall from most of what it walks.
    let mut room = Room::at(true, true, guard_moves(), Vec3::new(38.0, 0.0, 38.0));
    let patrol = room.sim.add_patrol(Name::intern(&doc.name), &doc.points, doc.closed).expect("a good one");
    assert_eq!(room.sim.patrol_named(Name::intern("round_the_wall")), Some(patrol));
    let guard = room.spawned(4.0, 4.0);
    assert!(room.sim.give_brain(guard));
    assert!(room.sim.give_patrol(guard, patrol));

    let mut visited = Vec::new();
    for _ in 0..12_000 {
        room.steps(1);
        let view = room.sim.actor(guard).expect("alive");
        assert_eq!(view.brain.state(), BrainState::Watching);
        let next = view.brain.patrol().expect("it's on one").next();
        if visited.last() != Some(&next) {
            visited.push(next);
        }
        if visited.len() >= 6 {
            break;
        }
    }
    assert_eq!(visited, [0, 1, 2, 3, 0, 1], "in its order, and round again");
    assert!(room.searches() > 0, "the second leg isn't a straight line");
    assert!(
        room.sim.tables_hash()
            != Room::at(true, true, guard_moves(), Vec3::new(38.0, 0.0, 38.0)).sim.tables_hash()
    );
}

fn patrol_here(room: &mut Room, name: &str, points: &[[f32; 3]]) -> PatrolId {
    room.sim.add_patrol(Name::intern(name), points, false).expect("a good one")
}

/// A guard notices who's within sight with a clear line, and nobody else: not past `sight` and not through
/// a wall. It gives up past `lose`, and goes back to what it was doing.
#[test]
fn a_guard_notices_what_it_can_see_and_gives_up_on_what_it_cant_catch() {
    // 6 m off through the wall, then in the open.
    let mut room = Room::at(true, true, guard_moves(), Vec3::new(23.0, 0.0, 10.0));
    let post = patrol_here(&mut room, "post", &[[17.0, 0.0, 10.0]]);
    let guard = room.spawned(17.0, 10.0);
    assert!(room.sim.give_brain(guard));
    assert!(room.sim.give_patrol(guard, post));
    room.steps(120);
    let view = room.sim.actor(guard).expect("alive");
    assert_eq!(view.brain.state(), BrainState::Watching, "a wall's between them");
    assert_eq!(view.brain.target(), None);
    assert_eq!(view.brain.stick(), Stick::CENTER, "and it stands at its post");

    // No wall: 12 m off is past sight, and then they walk in.
    let mut room = Room::at(false, true, guard_moves(), Vec3::new(29.0, 0.0, 10.0));
    let post = patrol_here(&mut room, "post", &[[17.0, 0.0, 10.0]]);
    let guard = room.spawned(17.0, 10.0);
    assert!(room.sim.give_brain(guard));
    assert!(room.sim.give_patrol(guard, post));
    room.steps(120);
    assert_eq!(room.sim.actor(guard).expect("alive").brain.target(), None, "12 m is past its sight");
    let mut walked = 0;
    while room.sim.actor(guard).expect("alive").brain.target().is_none() {
        room.step(Stick::new(-127, 0));
        walked += 1;
        assert!(walked < 240, "it never noticed, at {} m", room.at_foe(guard));
    }
    assert!((room.at_foe(guard) - WATCH.sight).abs() < 0.5, "noticed at {} m", room.at_foe(guard));
    assert_eq!(room.sim.actor(guard).expect("alive").brain.target(), Some(room.foe));

    // Now they run: 6 m/s against its 4, so they get past `lose`, and it goes back.
    let mut ran = 0;
    while room.sim.actor(guard).expect("alive").brain.state() != BrainState::Watching {
        room.step(Stick::new(127, 0));
        ran += 1;
        assert!(ran < 1200, "it never gave up, at {} m", room.at_foe(guard));
    }
    assert!(room.at_foe(guard) > WATCH.lose - 0.5, "gave up at {} m", room.at_foe(guard));
    assert_eq!(room.sim.actor(guard).expect("alive").brain.target(), None);
    assert!(room.place(guard).x > 17.5, "it had gone after them: {}", room.place(guard));
    room.steps(1500);
    assert!(
        ground(room.place(guard), Vec3::new(17.0, 0.0, 10.0)) < 1.0,
        "back at its post: {}",
        room.place(guard)
    );
}

/// Only a brain whose document says how can walk one, and a bad patrol says what's wrong with it.
#[test]
fn a_patrol_asks_for_a_brain_that_can_walk_one() {
    let mut room = Room::new(false, true, moves(|_| {}));
    let patrol = patrol_here(&mut room, "ok", &[[0.0; 3]]);
    let no_brain = room.spawned(1.0, 1.0);
    assert!(!room.sim.give_patrol(no_brain, patrol), "it has no brain");
    let no_watch = room.brain_at(3.0, 3.0);
    assert!(!room.sim.give_patrol(no_watch, patrol), "its document has no `watch`");
    let name = Name::intern("bad");
    assert_eq!(room.sim.add_patrol(name, &[], true), Err(PatrolError::Points { name }));
}

/// On no navmesh a patrol's walked in straight lines, as a brain does anything there.
#[test]
fn on_no_navmesh_a_patrol_goes_straight() {
    let mut room = Room::at(false, false, guard_moves(), Vec3::new(38.0, 0.0, 38.0));
    let patrol = patrol_here(&mut room, "line", &[[4.0, 0.0, 4.0], [14.0, 0.0, 4.0]]);
    let guard = room.spawned(4.0, 4.0);
    assert!(room.sim.give_brain(guard));
    assert!(room.sim.give_patrol(guard, patrol));
    room.steps(20);
    let stick = room.sim.actor(guard).expect("alive").brain.stick().to_vec2();
    assert!(stick.x > 0.25 && stick.x < 0.35 && stick.y.abs() < 0.02, "at its walk's tilt: {stick}");
    assert!(room.place(guard).x > 4.0);
}

/// Two on one patrol, or two who start in the same place, part rather than stand on each other.
#[test]
fn two_at_one_spot_part() {
    let mut room = Room::at(false, true, moves(|_| {}), Vec3::new(38.0, 0.0, 38.0));
    let first = room.brain_at(10.0, 10.0);
    room.steps(1);
    let second = room.brain_at(room.place(first).x, room.place(first).z);
    room.steps(60);
    assert!(
        ground(room.place(first), room.place(second)) > 0.6,
        "{} {}",
        room.place(first),
        room.place(second)
    );
}

/// No heap in a step, whoever's walking, with every kind of thing to ask of a route going on at once.
#[test]
fn walking_allocates_nothing() {
    let mut room = Room::new(true, true, guard_moves());
    let patrol = patrol_here(&mut room, "walk", &[[5.0, 0.0, 5.0], [5.0, 0.0, 20.0], [35.0, 0.0, 20.0]]);
    let walkers: Vec<ActorId> = (0..6).map(|index| room.spawned(4.0 + index as f32, 4.0)).collect();
    for walker in &walkers {
        assert!(room.sim.give_brain(*walker));
        assert!(room.sim.give_patrol(*walker, patrol));
    }
    room.steps(10);
    let before = room.sim.stats().allocations;
    room.steps(1200);
    assert_eq!(room.sim.stats().allocations, before, "rule R6");
}
