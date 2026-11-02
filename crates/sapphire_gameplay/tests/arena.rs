//! The arena's two documents against each other and against a character (M2 O5): the collider document
//! as the scenario builds it, the spawn points it starts from, and a character walking and jumping
//! between them through the simulation, by the stick alone, as a player would.
//!
//! Whether the colliders stand where the town draws things is another test's, in the crate that can
//! read the town: `tools/sapphire_pipeline/tests/arena.rs`.

use std::collections::{BTreeSet, VecDeque};

use sapphire_colliders::{ArchivedColliderSet, ColliderDocument, ColliderSet};
use sapphire_gameplay::arena::{cooked_colliders, profile, spawns};
use sapphire_gameplay::replay::{
    ARENA_WALK_JUMP, ARENA_WALK_LEGS, Recording, SCENARIOS, arena_walk, find_scenario, find_script,
};
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, GroundMode, ResolvedProfile, SimConfig, Simulation, Stick,
    TickActions, actor_body,
};
use sapphire_math::rng::Pcg32;
use sapphire_math::{Vec2, Vec3};
use sapphire_physics::{BodyId, Group, GroupMask, HitTarget, PhysicsWorld, Pose, QueryFilter, Shape};
use sapphire_serialize::cooked::AlignedVec;

const DOCUMENT: &str = include_str!("../../../content/arena/square.colliders.ron");

/// The paving's edge and the back row's fronts: what a character can reach, at the widest.
const BOUNDS_X: f32 = 18.6;
const BOUNDS_FRONT_Z: f32 = 20.3;
const BOUNDS_BACK_Z: f32 = -17.75;

/// The duel's clear floor: a rectangle holding nothing but one lantern post, which both spawn points
/// stand in. The test that holds it is [`the_duels_floor_is_clear`].
const FLOOR_X: [f32; 2] = [-0.6, 10.6];
const FLOOR_Z: [f32; 2] = [-7.0, 12.0];

/// The highest a character's feet get: a jump from the tallest things it can climb, and nowhere near
/// the rows' roofs. [`a_character_jumping_about_stays_in`] holds it.
const HIGHEST_FEET: f32 = 2.5;

fn resolved() -> ResolvedProfile {
    ResolvedProfile::new(&profile()).expect("the arena's profile is good")
}

fn capsule() -> Shape {
    resolved().capsule()
}

fn cooked() -> AlignedVec {
    cooked_colliders().expect("the document cooks")
}

fn set(bytes: &AlignedVec) -> &ArchivedColliderSet {
    ColliderSet::read(bytes).expect("the cooked set reads")
}

fn world(bytes: &AlignedVec) -> PhysicsWorld {
    PhysicsWorld::from_cooked(set(bytes), 2).expect("the world builds")
}

/// What a character collides with, of the statics.
fn character_filter() -> GroupMask {
    Group::World.mask() | Group::CharacterBlocker.mask()
}

/// The capsule's origin with its feet at `feet`, a skin over them as the controller stands it.
fn capsule_pose(feet: Vec3) -> Pose {
    Pose::at(feet + resolved().feet_to_center())
}

/// A collider's name, for failures.
fn name_of(set: &ArchivedColliderSet, target: HitTarget) -> String {
    match target {
        HitTarget::Static(id) => set.colliders[id.0 as usize].name.to_string(),
        HitTarget::Body(id) => format!("body {}", id.0),
    }
}

/// Whether a capsule with its feet at `feet` is within the paving, by its radius.
fn in_bounds(feet: Vec3) -> bool {
    let r = profile().radius;
    feet.x.abs() <= BOUNDS_X - r + 1e-3
        && (BOUNDS_BACK_Z + r - 1e-3..=BOUNDS_FRONT_Z - r + 1e-3).contains(&feet.z)
}

/// One character alone in the arena, moved by the stick and the jump button as a player moves it, with
/// what must hold on every tick checked as it goes.
struct Walker {
    sim: Simulation,
    id: ActorId,
}

impl Walker {
    fn new(feet: Vec3) -> Self {
        let mut sim = Simulation::new(SimConfig { max_actors: 2, ..SimConfig::default() });
        let bytes = cooked();
        let physics = PhysicsWorld::from_cooked(set(&bytes), sim.body_capacity()).expect("the world builds");
        sim.set_physics_world(physics).expect("it has room");
        let controller = Some(sim.add_controller_profile(&profile()).expect("a good profile"));
        let id = sim.spawn(ActorSpawn { position: feet, controller, ..ActorSpawn::default() }).expect("room");
        let mut walker = Self { sim, id };
        walker.tick(Vec2::ZERO, Buttons::NONE);
        assert_eq!(walker.mode(), GroundMode::Grounded, "at {feet}");
        walker
    }

    fn feet(&self) -> Vec3 {
        self.sim.actor(self.id).expect("alive").position
    }

    fn mode(&self) -> GroundMode {
        self.sim.actor(self.id).expect("alive").controller.state.mode
    }

    fn tick(&mut self, stick: Vec2, buttons: Buttons) {
        let mut actions = TickActions::NEUTRAL;
        let stick = Stick::from_axes(stick.x, stick.y);
        actions.set(self.id, ActorActions { buttons, stick }).expect("room");
        self.sim.step(&actions);
        assert_nothing_wrong(&self.sim, self.id);
    }

    /// Runs at `waypoint` until it's there, and says where it got if it can't.
    fn run_to(&mut self, waypoint: Vec3) {
        for _ in 0..1200 {
            let toward = Vec2::new(waypoint.x - self.feet().x, waypoint.z - self.feet().z);
            if toward.length() < 0.15 {
                return;
            }
            self.tick(toward.normalize(), Buttons::NONE);
        }
        panic!("stuck at {} on the way to {waypoint}", self.feet());
    }
}

/// What holds on every tick of anyone's run in the arena: inside it, on or over the ground, and in
/// nothing, by more than a sweep's rounding.
fn assert_nothing_wrong(sim: &Simulation, id: ActorId) {
    let view = sim.actor(id).expect("alive");
    let feet = view.position;
    assert!(feet.is_finite(), "{feet}");
    assert!(feet.y >= -2e-4, "under the ground at {feet}");
    assert!(feet.y <= HIGHEST_FEET, "up on something at {feet}");
    assert!(in_bounds(feet), "out at {feet}");
    let pose = sim.physics().body_pose(actor_body(id)).expect("its body");
    let solid = QueryFilter::new(character_filter());
    if let Some(found) = sim.physics().penetration(&capsule(), Pose::at(pose.position), &solid) {
        assert!(found.depth < 2e-3, "{} m inside something at {feet}", found.depth);
    }
}

/// The arena's collision world, as one replay file holds it for every architecture: the `Test` job
/// holds this one constant on Linux, Windows and macOS. If only some of them fail, something between
/// the document and the world has reached glam's SIMD types or a C math library (see
/// `sapphire_math::det`), and that's the bug: don't record a value per architecture. If all of them
/// fail, the document changed: update this and run `cargo xtask replay --update`.
#[test]
fn the_arenas_world_is_the_same_on_every_architecture() {
    let digest = world(&cooked()).statics_digest();
    assert_eq!(digest, ARENA_WORLD_DIGEST, "the arena's world's digest is {digest:#x}");
}

const ARENA_WORLD_DIGEST: u64 = 0x98e6_1174_9c60_6459;

#[test]
fn the_document_parses_cooks_and_builds_a_world() {
    let document = ColliderDocument::from_ron(DOCUMENT).expect("the document reads");
    let bytes = cooked();
    let cooked = set(&bytes);
    assert_eq!(cooked.colliders.len(), document.colliders.len());
    // The scenario stands on this world, whole.
    let scenario = find_scenario(SCENARIOS, "arena").expect("listed");
    let (sim, _) = scenario.start(scenario.config).expect("sets up");
    assert_eq!(sim.physics().static_count(), document.colliders.len());
    assert_eq!(sim.physics().statics_digest(), world(&bytes).statics_digest());
    let world = world(&bytes);
    assert_eq!(world.static_count(), document.colliders.len());

    // The ground is what's under the middle of the square, its top at y 0.
    let hit = world
        .cast_ray(Vec3::new(3.0, 3.0, 8.0), Vec3::NEG_Y, 10.0, &QueryFilter::new(Group::World.mask()))
        .expect("the ground");
    assert_eq!(name_of(cooked, hit.target), "ground");
    assert!((hit.distance - 3.0).abs() < 1e-5, "{}", hit.distance);
}

/// Blockers keep characters in, and nothing else: a blocker in the world group would stop hits and
/// cameras at an edge nothing draws. Each collider says by its name which it is.
#[test]
fn only_blockers_are_out_of_the_world_group() {
    let document = ColliderDocument::from_ron(DOCUMENT).expect("the document reads");
    let mut blockers = 0;
    for entry in &document.colliders {
        let expected = if entry.name.starts_with("block_") {
            blockers += 1;
            Group::CharacterBlocker
        } else if entry.name.starts_with("camera_") {
            Group::CameraBlocker
        } else {
            Group::World
        };
        assert_eq!(entry.groups, [expected], "{}", entry.name);
    }
    assert!(blockers > 0);
}

#[test]
fn each_spawn_point_stands_on_the_ground() {
    let bytes = cooked();
    let cooked = set(&bytes);
    let world = world(&bytes);
    let filter = QueryFilter::new(Group::World.mask());
    let spawns = spawns();
    for (who, spawn) in [("player", spawns.player), ("enemy", spawns.enemy)] {
        let over = spawn.position() + Vec3::Y * 2.0;
        let hit = world
            .cast_ray(over, Vec3::NEG_Y, 4.0, &filter)
            .unwrap_or_else(|| panic!("{who}: nothing under it"));
        assert_eq!(name_of(cooked, hit.target), "ground", "{who}");
        assert!((hit.distance - 2.0).abs() < 1e-3, "{who} stands {} m over the ground", 2.0 - hit.distance);
    }
}

#[test]
fn a_capsule_of_the_characters_size_fits_at_each_spawn_point() {
    let bytes = cooked();
    let cooked = set(&bytes);
    let world = world(&bytes);
    let filter = QueryFilter::new(character_filter());
    let spawns = spawns();
    for (who, spawn) in [("player", spawns.player), ("enemy", spawns.enemy)] {
        let mut hits = [HitTarget::Body(BodyId(0)); 8];
        let found = world.overlap(&capsule(), capsule_pose(spawn.position()), &filter, &mut hits);
        let names: Vec<String> = hits[..found.written].iter().map(|&hit| name_of(cooked, hit)).collect();
        assert_eq!(found.found, 0, "{who} overlaps {names:?}");
    }
    // And they stand apart from each other, with room to dodge.
    let apart = spawns.player.position().distance(spawns.enemy.position());
    assert!(apart > 4.0 * profile().radius, "{apart}");
}

#[test]
fn the_walk_from_the_player_to_the_enemy_is_clear() {
    let bytes = cooked();
    let cooked = set(&bytes);
    let world = world(&bytes);
    let spawns = spawns();
    let (from, to) = (spawns.player.position(), spawns.enemy.position());
    let filter = QueryFilter::new(character_filter());
    let hit = world.cast_shape(&capsule(), capsule_pose(from), to - from, from.distance(to), &filter);
    assert!(hit.is_none(), "{:?}", hit.map(|hit| name_of(cooked, hit.target)));
}

/// Nothing on the duel's floor but the post of the lantern at (8, −2.5): a body anywhere on this
/// rectangle, away from that post, overlaps nothing.
#[test]
fn the_duels_floor_is_clear() {
    let bytes = cooked();
    let cooked = set(&bytes);
    let world = world(&bytes);
    let filter = QueryFilter::new(character_filter());
    let lantern = Vec3::new(8.0, 0.0, -2.5);
    let spot = |index: u32, from: f32| from + index as f32 * 0.25;
    let (columns, rows) =
        (((FLOOR_X[1] - FLOOR_X[0]) / 0.25) as u32, ((FLOOR_Z[1] - FLOOR_Z[0]) / 0.25) as u32);
    for column in 0..=columns {
        for row in 0..=rows {
            let feet = Vec3::new(spot(column, FLOOR_X[0]), 0.0, spot(row, FLOOR_Z[0]));
            if feet.distance(lantern) <= 0.19 * std::f32::consts::SQRT_2 + profile().radius {
                continue;
            }
            let mut hits = [HitTarget::Body(BodyId(0)); 4];
            let found = world.overlap(&capsule(), capsule_pose(feet), &filter, &mut hits);
            let names: Vec<String> = hits[..found.written].iter().map(|&hit| name_of(cooked, hit)).collect();
            assert_eq!(found.found, 0, "at {feet}: {names:?}");
        }
    }
    for spawn in [spawns().player, spawns().enemy] {
        let feet = spawn.position();
        assert!((FLOOR_X[0]..=FLOOR_X[1]).contains(&feet.x), "{feet}");
        assert!((FLOOR_Z[0]..=FLOOR_Z[1]).contains(&feet.z), "{feet}");
    }
}

/// The replay's own path, through the scenario: it walks, runs, is stopped by the lantern's post short
/// of where its stick asks, jumps there, and comes back to about where it started, with the enemy
/// standing throughout.
#[test]
fn the_arena_walk_collides_with_the_post_and_comes_back() {
    let scenario = find_scenario(SCENARIOS, "arena").expect("listed");
    let script = find_script("arena-walk").expect("listed");
    let mut recording = Recording::new(scenario, scenario.config).expect("sets up");
    let (player, enemy) = (recording.cast()[0], recording.cast()[1]);
    let mut source = arena_walk;
    let spawns = spawns();

    // The post's face, less the capsule and the skin it keeps.
    let stop = 8.0 - 0.19 - profile().radius - profile().skin;
    let at_post = u64::from(ARENA_WALK_LEGS[0].2 + ARENA_WALK_LEGS[1].2 + ARENA_WALK_LEGS[2].2);
    let mut walked = 0.0;
    let mut highest: f32 = 0.0;
    let mut last = spawns.player.position();
    for tick in 0..u64::from(script.ticks) {
        recording.step(&mut source).expect("short");
        let sim = recording.simulation();
        assert_nothing_wrong(sim, player);
        let view = sim.actor(player).expect("alive");
        let feet = view.position;
        assert!(!view.controller.state.events.pushed_out, "tick {tick}: began inside something at {feet}");
        walked += (feet - last).length();
        last = feet;
        highest = highest.max(feet.y);
        if tick == ARENA_WALK_JUMP - 1 {
            assert!((feet.x - stop).abs() < 1e-2, "the post stops it at x = {stop}: {feet}");
            assert!((feet.z + 2.5).abs() < 0.49, "it's against the post, not past it: {feet}");
            assert!(feet.y.abs() < 2e-4, "{feet}");
        }
        if tick + 1 == at_post {
            assert_eq!(view.controller.state.mode, GroundMode::Grounded, "landed: {feet}");
        }
        assert_eq!(sim.actor(enemy).expect("alive").position, spawns.enemy.position(), "tick {tick}");
    }
    assert!(walked > 18.0, "{walked}");
    let jump = profile().jump_speed * profile().jump_speed / (2.0 * profile().gravity);
    assert!(highest > jump * 0.9 && highest <= jump, "the jump reached {highest} of {jump}");
    assert!(last.distance(spawns.player.position()) < 0.5, "back about where it started: {last}");
}

/// A player's lap: a character runs round the inside of the arena by the stick, along lanes that are
/// clear, and reaches each corner in turn.
#[test]
fn a_character_runs_round_the_arena() {
    let waypoints = [
        Vec3::new(16.4, 0.0, 12.0),
        Vec3::new(16.4, 0.0, -13.0),
        Vec3::new(0.0, 0.0, -13.0),
        Vec3::new(0.0, 0.0, -11.5),
        Vec3::new(-16.4, 0.0, -11.5),
        Vec3::new(-16.4, 0.0, 19.5),
        Vec3::new(16.4, 0.0, 19.5),
        Vec3::new(16.4, 0.0, 12.0),
        spawns().player.position(),
    ];
    let mut walker = Walker::new(spawns().player.position());
    for waypoint in waypoints {
        walker.run_to(waypoint);
        assert_eq!(walker.mode(), GroundMode::Grounded, "at {waypoint}");
        assert!(walker.feet().y.abs() < 2e-4, "the lanes are flat: {}", walker.feet());
    }
}

/// A character run at each wall and each way out, from clear floor beside it: what stops it is what
/// should, and a jump with the stick still held doesn't get it over.
#[test]
fn a_character_run_and_jumped_at_each_edge_stays_in() {
    /// Where a run starts, which way it goes, and what stops it.
    const RUNS: [([f32; 2], [f32; 2], &str); 19] = [
        // The ways out.
        ([3.2, -13.0], [0.0, -1.0], "block_gate_street"),
        ([-3.6, -13.0], [0.0, -1.0], "block_gate_courtyard"),
        ([7.4, -13.0], [0.0, -1.0], "block_back_east"),
        ([16.0, -7.8], [1.0, 0.0], "block_east_alley_back"),
        ([16.0, 2.2], [1.0, 0.0], "block_east_alley_front"),
        ([16.6, -16.6], [1.0, 0.0], "block_east_corner"),
        ([-16.0, -5.45], [-1.0, 0.0], "block_west_alley_back"),
        ([-16.0, 3.45], [-1.0, 0.0], "block_west_alley_front"),
        ([-16.3, -16.2], [-1.0, 0.0], "block_west_corner"),
        ([0.0, 15.0], [0.0, 1.0], "block_edge_front"),
        ([-16.0, 15.0], [-1.0, 0.0], "block_edge_west"),
        ([16.0, 15.0], [1.0, 0.0], "block_edge_east"),
        // The rows.
        ([-0.4, -15.0], [0.0, -1.0], "gate_wall_middle"),
        ([11.5, -16.7], [0.0, -1.0], "shop3_back"),
        ([-14.8, -17.0], [0.0, -1.0], "hall_back"),
        ([16.0, -2.8], [1.0, 0.0], "shop_right"),
        ([-16.0, -10.4], [-1.0, 0.0], "shop_left"),
        ([-16.0, 7.4], [-1.0, 0.0], "house_left"),
        ([16.0, 6.4], [1.0, 0.0], "house_right_front"),
    ];
    let bytes = cooked();
    let cooked = set(&bytes);
    let filter = QueryFilter::new(character_filter());
    for (start, direction, stopper) in RUNS {
        let mut walker = Walker::new(Vec3::new(start[0], 0.0, start[1]));
        let stick = Vec2::new(direction[0], direction[1]);
        for _ in 0..120 {
            walker.tick(stick, Buttons::NONE);
        }
        let feet = walker.feet();
        assert!(walker.feet().y.abs() < 2e-4, "from {start:?}: {feet}");
        let ahead = Vec3::new(direction[0], 0.0, direction[1]);
        let hit = walker
            .sim
            .physics()
            .cast_shape(&capsule(), capsule_pose(feet), ahead, 0.1, &filter)
            .unwrap_or_else(|| panic!("from {start:?}: nothing stops it at {feet}"));
        assert_eq!(name_of(cooked, hit.target), stopper, "from {start:?}, at {feet}");

        // A jump, and the stick still held, past the top of it and down again.
        walker.tick(stick, Buttons::JUMP);
        assert_ne!(walker.mode(), GroundMode::Grounded, "from {start:?}, at {feet}");
        for _ in 0..60 {
            walker.tick(stick, Buttons::NONE);
        }
        assert!(walker.feet().distance(feet) < 2e-2, "from {start:?}: {feet}, then {}", walker.feet());
    }
}

/// What a player mashing about would find, on the real controller: a run in a random direction every
/// half second or so and a jump at random, from each spot, for a minute. No tick leaves the arena, or
/// is inside anything, or stands much higher than a jump from what can be climbed.
#[test]
fn a_character_jumping_about_stays_in() {
    let spots = [
        spawns().player.position(),
        Vec3::new(-14.5, 0.0, -7.0),
        Vec3::new(14.0, 0.0, -11.0),
        Vec3::new(-6.5, 0.0, -13.0),
        Vec3::new(12.0, 0.0, -13.0),
        Vec3::new(-15.0, 0.0, 15.0),
        Vec3::new(15.0, 0.0, 15.0),
        Vec3::new(0.5, 0.0, -14.5),
    ];
    for (index, spot) in spots.into_iter().enumerate() {
        let mut rng = Pcg32::new(7, index as u64);
        let mut walker = Walker::new(spot);
        let mut stick = Vec2::ZERO;
        for tick in 0..3600u32 {
            if tick % 30 == 0 {
                let turn = rng.unit_f32() * std::f32::consts::TAU;
                stick = Vec2::new(sapphire_math::det::sin(turn), sapphire_math::det::cos(turn));
            }
            let buttons = if rng.unit_f32() < 0.05 { Buttons::JUMP } else { Buttons::NONE };
            walker.tick(stick, buttons);
        }
    }
}

/// Everywhere a capsule can get to from the player's spawn point, by moves of half a metre each way
/// that a sweep finds clear: all of it in bounds, so no gap in the rows leaks, and as much of the
/// square as the arena says it holds.
#[test]
fn no_way_out_of_the_arena_fits_a_capsule() {
    const STEP: f32 = 0.5;
    let bytes = cooked();
    let world = world(&bytes);
    let filter = QueryFilter::new(character_filter());
    let start = spawns().player.position();
    let cell = |x: i32, z: i32| start + Vec3::new(x as f32 * STEP, 0.0, z as f32 * STEP);
    let mut seen = BTreeSet::from([(0, 0)]);
    let mut queue = VecDeque::from([(0, 0)]);
    while let Some((x, z)) = queue.pop_front() {
        assert!(in_bounds(cell(x, z)), "reached {} from the player's spawn point", cell(x, z));
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let next = (x + dx, z + dz);
            if seen.contains(&next) {
                continue;
            }
            let direction = Vec3::new(dx as f32, 0.0, dz as f32);
            if world.cast_shape(&capsule(), capsule_pose(cell(x, z)), direction, STEP, &filter).is_none() {
                seen.insert(next);
                queue.push_back(next);
            }
        }
    }
    // The paving between the rows is about 1,300 m², less what stands on it.
    let area = seen.len() as f32 * STEP * STEP;
    assert!((900.0..=1300.0).contains(&area), "{area} m² reachable");
}
