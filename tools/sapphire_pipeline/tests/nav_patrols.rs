//! A patrol from a zone document to someone walking it (M3 P3): the proving ground's spline, through the
//! zone's cooked bytes, walked by a brain on that zone's own navmesh, with no Rust naming a point. And what
//! a cook refuses that a document can't know for itself.

mod nav_support;

use std::collections::{BTreeMap, HashMap};

use nav_support::{content, proving_ground};
use sapphire_assets::AssetId;
use sapphire_colliders::{ColliderDocument, ColliderSet};
use sapphire_core::Name;
use sapphire_gameplay::replay::WARRIORS;
use sapphire_gameplay::{ActorSpawn, BrainState, ControllerProfile, SimConfig, Simulation, TickActions};
use sapphire_math::Vec3;
use sapphire_moves::MoveSetDocument;
use sapphire_physics::PhysicsWorld;
use sapphire_pipeline::zone::{ZoneCookError, ZonePlan, cook_navigation};
use sapphire_zone::{Zone, ZoneDocument, ZoneGeometry};

const NAVIGATION: AssetId = AssetId::from_text("6d7b7a5c-8c5e-4d7f-9b0e-3a1f2c4d5e6f");

/// The proving ground's `.szone`, as a cook writes its simulation's half, with what its placements draw
/// left out: nothing here looks at them.
fn proving_ground_zone() -> sapphire_serialize::cooked::AlignedVec {
    let dir = content().join("zones/proving_ground");
    let text = std::fs::read_to_string(dir.join("proving_ground.zone.ron")).expect("the zone's file");
    let mut document = ZoneDocument::from_ron(&text).expect("the zone reads");
    document.placements.clear();
    document.instances.clear();
    document.day = None;
    document.rigs = None;
    document.exits.clear();
    let geometry = ZoneGeometry {
        meshes: BTreeMap::new(),
        instances: Vec::new(),
        day: None,
        rigs: None,
        navigation: Some(NAVIGATION),
        exit_packs: Vec::new(),
    };
    document.cook(geometry).expect("it cooks").to_cooked().expect("it writes")
}

/// What a document says is what's walked: the spawner names the spline, the spline says where, and a brain
/// goes round it in order on the zone's own mesh, past each of its points.
#[test]
fn the_proving_grounds_guard_walks_the_spline_its_spawner_names() {
    let bytes = proving_ground_zone();
    let zone = Zone::read(&bytes).expect("it reads back");

    let (colliders, baked, _) = proving_ground();
    let mut sim = Simulation::new(SimConfig { seed: 3, max_actors: 2, ..SimConfig::default() });
    let world = PhysicsWorld::from_cooked(
        ColliderSet::read(&colliders.to_cooked().expect("it writes")).expect("it reads"),
        sim.body_capacity(),
    )
    .expect("a good world");
    sim.set_physics_world(world).expect("before any spawn");
    sim.set_navigation(&baked.mesh.to_cooked().expect("it writes")).expect("a good navmesh");
    let controller = Some(sim.add_controller_profile(&ControllerProfile::default()).expect("a good one"));
    let text = std::fs::read_to_string(content().join("combat/enemy/warrior.moves.ron")).expect("its file");
    let moves = MoveSetDocument::parse(&text).expect("it parses").compile().expect("it compiles");
    let moves = Some(sim.add_move_set(moves));

    let spawner = zone.simulation.spawner(Name::new("far_gate")).expect("the zone's");
    let name = spawner.patrol().expect("it names one");
    let patrol = zone.simulation.patrol(name).expect("checked");
    let points: Vec<[f32; 3]> = patrol.points().collect();
    let walk = sim.add_patrol(name, &points, patrol.closed.into()).expect("a good one");
    let guard = sim
        .spawn(ActorSpawn {
            position: Vec3::from_array(spawner.position()),
            moves,
            controller,
            faction: WARRIORS,
            ..ActorSpawn::default()
        })
        .expect("room");
    assert!(sim.give_brain(guard));
    assert!(sim.give_patrol(guard, walk));

    let mut visited = Vec::new();
    let mut nearest = vec![f32::MAX; points.len()];
    for _ in 0..12_000 {
        sim.step(&TickActions::NEUTRAL);
        let view = sim.actor(guard).expect("alive");
        assert_eq!(view.brain.state(), BrainState::Watching, "there's nobody to notice");
        for (point, near) in points.iter().zip(&mut nearest) {
            let (x, z) = (view.position.x - point[0], view.position.z - point[2]);
            *near = near.min((x * x + z * z).sqrt());
        }
        let next = view.brain.patrol().expect("it's on one").next();
        if visited.last() != Some(&next) {
            visited.push(next);
        }
        if visited.len() > points.len() {
            break;
        }
    }
    assert_eq!(visited, [1, 2, 3, 0, 1], "from the first, where it stands, round in its order");
    assert!(nearest.iter().all(|near| *near < 1.0), "and it went to each: {nearest:?}");
}

/// Two floors with nothing between them, and a spawner on each so both are real.
const TWO_FLOORS: &str = r#"(type: "sapphire::colliders::ColliderDocument", version: 1, data: (colliders: [
    (name: "west", shape: Box(half_extents: (10.0, 0.5, 10.0)), position: (0.0, -0.5, 0.0)),
    (name: "east", shape: Box(half_extents: (10.0, 0.5, 10.0)), position: (40.0, -0.5, 0.0)),
]))"#;

fn cook(points: &str) -> Result<Vec<u8>, ZoneCookError> {
    let text = format!(
        r#"(
            type: "sapphire::zone::ZoneDocument",
            version: 1,
            data: (
                grid: (cell_size: 32.0),
                colliders: Some("0aec06e8-ef8a-4f60-8dce-1e159cc277fa"),
                navigation: Some((min: (-16.0, -16.0), max: (56.0, 16.0))),
                spawners: [
                    (name: "west", position: (0.0, 0.0, 0.0), spawns: "warrior", patrol: "walk"),
                    (name: "east", position: (40.0, 0.0, 0.0), spawns: "warrior"),
                ],
                splines: [(name: "rail", points: [(0.0, 9.0, 0.0), (99.0, 9.0, 0.0)]), (name: "walk", points: [{points}])],
            ),
        )"#
    );
    let plan = ZonePlan::new(NAVIGATION, text.as_bytes(), &HashMap::new(), None)?;
    let colliders = ColliderDocument::from_ron(TWO_FLOORS).expect("it reads").cook().expect("it cooks");
    cook_navigation(&plan, &colliders.to_cooked().expect("it writes"), None)
}

/// Only a spline someone walks is held to the mesh, as the `rail` here isn't. One that's walked names the
/// point that's wrong, whichever way it's wrong.
#[test]
fn a_cook_refuses_a_patrol_point_off_the_mesh_or_cut_off_and_names_it() {
    assert!(cook("(0.0, 0.0, 0.0), (5.0, 0.0, 5.0)").is_ok());

    let message = |points: &str| cook(points).expect_err(points).to_string();
    let off = message("(0.0, 0.0, 0.0), (5.0, 0.0, 5.0), (20.0, 0.0, 0.0)");
    assert!(off.contains("`splines[1].points[2]`") && off.contains("isn't on the navmesh"), "{off}");
    let high = message("(0.0, 4.0, 0.0), (5.0, 0.0, 5.0)");
    assert!(high.contains("`splines[1].points[0]`"), "{high}");
    let across = message("(5.0, 0.0, 5.0), (40.0, 0.0, 5.0)");
    assert!(across.contains("`splines[1].points[1]`") && across.contains("no way to it"), "{across}");
}

/// A moved point has to be checked again, so a navmesh's key has to see it: and a spline nobody walks doesn't
/// rebake anything.
#[test]
fn a_navmeshs_key_sees_what_is_patrolled_and_only_that() {
    let recipe = |rail: &str, walk: &str| {
        let text = format!(
            r#"(
                type: "sapphire::zone::ZoneDocument",
                version: 1,
                data: (
                    grid: (cell_size: 32.0),
                    colliders: Some("0aec06e8-ef8a-4f60-8dce-1e159cc277fa"),
                    navigation: Some((min: (-16.0, -16.0), max: (16.0, 16.0))),
                    spawners: [(name: "west", position: (0.0, 0.0, 0.0), spawns: "warrior", patrol: "walk")],
                    splines: [(name: "rail", points: [{rail}]), (name: "walk", points: [{walk}])],
                ),
            )"#
        );
        ZonePlan::new(NAVIGATION, text.as_bytes(), &HashMap::new(), None)
            .expect("it plans")
            .navigation_recipe()
    };
    let (rail, walk) = ("(0.0, 9.0, 0.0), (9.0, 9.0, 0.0)", "(0.0, 0.0, 0.0), (5.0, 0.0, 5.0)");
    let base = recipe(rail, walk);
    assert_eq!(recipe("(0.0, 8.0, 0.0), (9.0, 9.0, 0.0)", walk), base, "nobody walks the rail");
    assert_ne!(recipe(rail, "(0.0, 0.0, 0.0), (5.0, 0.0, 4.0)"), base);
}
