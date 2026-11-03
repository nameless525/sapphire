//! The Lantern Ward (M3 C1) against what it says of itself. Its documents are written by hand now, and
//! nothing else connects them: `.zone.ron` says what's drawn and where people go, `.colliders.ron` what
//! a body walks on and what the navmesh bakes from, and `.probes.ron` what the probe grid sees.
//!
//! - **Everywhere it names is a walk from the start,** through `sapphire_nav` on what a cook bakes, on
//!   every OS this runs on. It prints the bake's digest, as `nav_baked` does, for comparing them.
//! - **A body makes that walk,** in the real controller with the same stick anyone has.
//! - **The walls hold:** the bake reaches nothing outside them.
//! - **Nothing drawn at a character's height stands on the navmesh, and no collider stands where nothing's
//!   drawn,** so moving a building leaves neither a way through it nor an invisible wall.
//! - **The probe grid sees where things are.**

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use glam::{Mat4, Vec3};
use sapphire_assets::AssetId;
use sapphire_colliders::{ColliderDocument, ColliderSet as CookedColliders};
use sapphire_gameplay::{
    ActorActions, ActorSpawn, Buttons, SimConfig, Simulation, Stick, TickActions, arena,
};
use sapphire_nav::{NavGraph, PathSearch, Reach, RouteEnd, Steer};
use sapphire_navmesh::NavMesh;
use sapphire_physics::{ColliderSet, PhysicsWorld};
use sapphire_pipeline::assembly::{self, Detail};
use sapphire_pipeline::import::gltf;
use sapphire_pipeline::meta::{self, ImportSettings, SourceTree};
use sapphire_pipeline::nav::{BakeSettings, Seed, bake_colliders};
use sapphire_pipeline::probes;
use sapphire_serialize::cooked::AlignedVec;
use sapphire_zone::ZoneDocument;

mod support;

use support::{Triangle, Volume, clip_plane, in_band, volumes};

/// How far a collider's face may stand from what's drawn there: a little under half a character's
/// radius, past which an invisible wall starts to show (`arena.rs`).
const TOLERANCE: f32 = 0.12;

/// What's in reach of a body's capsule, over the height of the mesh it stands on: over the steps' treads
/// and the terrace's edge, which the mesh's ramp cuts under, and under a character's head.
const BODY: [f32; 2] = [0.35, 1.6];

/// What bounds the search for things drawn at a body's height, above the ground: past the terrace's
/// 1.2 m, by a body.
const DRAWN_BAND: [f32; 2] = [0.2, 2.9];

/// How close to its target a walker has to get, on the XZ plane, in metres: `Steer::Arrived`'s, with a
/// little room.
const ARRIVED: f32 = 1.0;

/// How long a leg may take against how long it should at a run: corners and queues of one.
const PATIENCE: f32 = 3.0;

/// Places outside the ward's walls: past the gate, beyond the market's far wall, the garden's, the
/// terrace's back and two corners behind the rows, none of which a walk may reach.
const OUTSIDE: [[f32; 3]; 6] = [
    [0.0, 0.0, 26.0],
    [90.0, 0.0, 0.0],
    [-90.0, 0.0, 0.0],
    [0.0, 0.0, -122.0],
    [32.0, 0.0, 26.0],
    [-32.0, 0.0, -32.0],
];

fn ward_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/zones/lantern_ward")
}

fn document() -> ZoneDocument {
    let text = fs::read_to_string(ward_dir().join("lantern_ward.zone.ron")).expect("the document");
    ZoneDocument::from_ron(&text).expect("it reads")
}

fn cooked_colliders() -> AlignedVec {
    collider_set().to_cooked().expect("it writes")
}

fn collider_set() -> CookedColliders {
    let text = fs::read_to_string(ward_dir().join("lantern_ward.colliders.ron")).expect("the document");
    ColliderDocument::from_ron(&text).expect("it reads").cook().expect("it cooks")
}

/// What a cook bakes for the zone: its colliders, its document's area, and every spawner a seed.
fn baked() -> AlignedVec {
    let document = document();
    let navigation = document.navigation.as_ref().expect("the ward bakes");
    let seeds: Vec<Seed> = document
        .spawners
        .iter()
        .map(|spawner| Seed { field: spawner.name.clone(), position: spawner.position })
        .collect();
    let (baked, islands) = bake_colliders(
        &cooked_colliders(),
        (navigation.min, navigation.max),
        &seeds,
        &BakeSettings::CHARACTER,
        None,
    )
    .expect("it bakes");
    let cooked = baked.mesh.to_cooked().expect("it writes");
    writeln!(
        std::io::stdout(),
        "NAV-DIGEST {} lantern_ward: baked {}, {} bytes, {} polygons, {islands:?}",
        std::env::consts::ARCH,
        blake3::hash(&cooked),
        cooked.len(),
        baked.mesh.polygon_count(),
    )
    .expect("it prints");
    cooked
}

fn graph() -> NavGraph {
    NavGraph::new(NavMesh::read(&baked()).expect("it reads"))
}

fn start(document: &ZoneDocument) -> Vec3 {
    let start = document.spawners.iter().find(|spawner| spawner.name == "player_start");
    Vec3::from(start.expect("where exit test 1 starts").position)
}

/// Every spawner, trigger and point of every spline is on one piece of ground with the start.
#[test]
fn everywhere_the_ward_names_is_a_walk_from_the_start() {
    let document = document();
    let graph = graph();
    let start = graph.locate(start(&document), Reach::CHARACTER).expect("the start's on the mesh");
    let mut corridor = vec![0; graph.polygon_count()];
    let spawners =
        document.spawners.iter().map(|spawner| (spawner.name.clone(), Vec3::from(spawner.position)));
    let triggers = document.triggers.iter().map(|trigger| {
        // A trigger stands round where someone would be: its foot's on the ground it watches.
        let on_ground = Vec3::from(trigger.position) * Vec3::new(1.0, 0.0, 1.0);
        (trigger.name.clone(), on_ground)
    });
    let splines = document.splines.iter().filter(|spline| spline.name == "tour").flat_map(|spline| {
        spline
            .points
            .iter()
            .enumerate()
            .map(|(index, &point)| (format!("{}[{index}]", spline.name), Vec3::from(point)))
    });
    let mut count = 0;
    for (name, position) in spawners.chain(triggers).chain(splines) {
        let end = graph
            .locate(position, Reach::CHARACTER)
            .unwrap_or_else(|| panic!("{name} at {position} isn't on the navmesh"));
        let route = PathSearch::new(&graph, u32::MAX).find(&graph, start, end, &mut corridor);
        assert_eq!(route.end, RouteEnd::Reached, "{name} at {position} is no walk from the start");
        count += 1;
    }
    assert!(count > 40, "the ward names more than {count} places");
    let spawns = |what: &str| document.spawners.iter().filter(|spawner| spawner.spawns == what).count();
    assert_eq!(spawns("boss"), 1, "where a boss stands");
    assert_eq!(spawns("checkpoint"), 1, "where a checkpoint stands");
    assert!(spawns("warrior") >= 12, "three rooms' fights");
}

/// The walls hold: the bake keeps nothing outside them, as an island no seed reaches is dropped and a gap
/// would join it to the rest.
#[test]
fn the_walls_close_the_ward() {
    let graph = graph();
    for point in OUTSIDE {
        let located = graph.locate(Vec3::from(point), Reach::CHARACTER);
        assert!(located.is_none(), "{point:?} is outside the walls, yet on the mesh: {located:?}");
    }
}

/// A body walks to every spawner in the document's order, from the start up the terrace's steps to the
/// boss, in the real controller on the colliders, steering by the navmesh through a stick.
#[test]
fn a_body_walks_the_ward_from_the_start_to_the_boss() {
    let document = document();
    let colliders = cooked_colliders();
    let mut sim = Simulation::new(SimConfig::default());
    let set = ColliderSet::read(&colliders).expect("it reads");
    let capacity = sim.body_capacity();
    sim.set_physics_world(PhysicsWorld::from_cooked(set, capacity).expect("it builds")).expect("empty");
    sim.set_navigation(&baked()).expect("it reads");
    let profile = arena::profile();
    let speed = profile.run_speed;
    let controller = Some(sim.add_controller_profile(&profile).expect("a good profile"));
    let walker = sim
        .spawn(ActorSpawn {
            position: start(&document),
            yaw: 0.0,
            moves: None,
            controller,
            ..ActorSpawn::default()
        })
        .expect("room for one");
    let path = sim.navigation_mut().expect("just given").add_path();

    for spawner in &document.spawners {
        let target = Vec3::from(spawner.position);
        let from = sim.actor(walker).expect("alive").position;
        let ticks = (PATIENCE * from.distance(target) / speed * sapphire_time::TICK_HZ as f32) as u32 + 120;
        let mut arrived = false;
        for _ in 0..ticks {
            let at = sim.actor(walker).expect("alive").position;
            let mut navigation = sim.navigation_mut().expect("it has one");
            navigation.begin_tick();
            let stick = match navigation.follow(path, at, target) {
                Steer::Walk(direction) => Stick::from_axes(direction.x, direction.z),
                Steer::Arrived => {
                    arrived = true;
                    break;
                }
                Steer::NoPath => panic!("no path from {at} to {} at {target}", spawner.name),
            };
            let mut actions = TickActions::NEUTRAL;
            actions.set(walker, ActorActions { buttons: Buttons::NONE, stick }).expect("room for one");
            sim.step(&actions);
        }
        let at = sim.actor(walker).expect("alive").position;
        let across = (at - target) * Vec3::new(1.0, 0.0, 1.0);
        assert!(
            arrived && across.length() <= ARRIVED,
            "a body walking from {from} to {} at {target} got to {at} in {ticks} ticks",
            spawner.name
        );
        // Standing where the spawner is, on what's under it, not on or under the ground.
        assert!((at.y - target.y).abs() < 0.2, "{} stands at {target}, but a body at {at}", spawner.name);
    }
}

/// One mesh the document places, at each of its places.
struct Drawn {
    label: String,
    triangles: Vec<Triangle>,
    /// Round all of its triangles, to skip them quickly.
    low: Vec3,
    high: Vec3,
}

/// A prefab's triangles, once for each document's mesh: the assembly's own code reads them, as a cook does.
struct Prefabs {
    /// The assemblies under the zone's directory, by ID.
    sources: BTreeMap<AssetId, PathBuf>,
    /// What's been assembled.
    triangles: BTreeMap<AssetId, Vec<Triangle>>,
}

impl Prefabs {
    fn new() -> Self {
        let tree = SourceTree::scan(&ward_dir()).expect("the directory");
        let resolved = meta::resolve(&tree, true).expect("every source has its sidecar");
        let sources = resolved
            .assets
            .iter()
            .filter(|asset| matches!(asset.meta.importer, ImportSettings::Assembly(_)))
            .map(|asset| (asset.meta.id, asset.path.clone()))
            .collect();
        Self { sources, triangles: BTreeMap::new() }
    }

    /// The prefab's triangles, or nothing for a mesh that isn't an assembly: a plant, which a body brushes
    /// through but for its trunk, or a mountain.
    fn triangles(&mut self, id: AssetId) -> Option<&[Triangle]> {
        let path = self.sources.get(&id)?;
        let triangles = self.triangles.entry(id).or_insert_with(|| {
            let document = assembly::read(&fs::read(path).expect("the document")).expect("it reads");
            let dir = path.parent().expect("a file").to_owned();
            let mesh = assembly::assemble(&document, Detail::Full, &mut |file: &str| {
                let path = dir.join(file);
                let bytes = fs::read(&path).expect("the piece");
                Ok(gltf::import_slice(&bytes, path.parent()).expect("it imports"))
            })
            .expect("it assembles");
            let (corners, _) = mesh.indices.as_chunks::<3>();
            corners
                .iter()
                .map(|corners| corners.map(|corner| Vec3::from(mesh.vertices[corner as usize].position)))
                .collect()
        });
        Some(triangles)
    }
}

/// Everything the zone's documents place that's solid and near: its instances but the backdrop, and its
/// placements.
fn drawn(document: &ZoneDocument) -> Vec<Drawn> {
    let mut prefabs = Prefabs::new();
    let instances = document
        .instances
        .iter()
        .filter(|instance| !instance.backdrop)
        .map(|instance| (instance.mesh, instance.at, instance.turn, instance.scale));
    let placements = document
        .placements
        .iter()
        .filter(|placement| !placement.backdrop)
        .map(|placement| (placement.prefab, placement.at, placement.turn, placement.scale));
    instances
        .chain(placements)
        .filter_map(|(id, at, turn, scale)| {
            let transform = Mat4::from_translation(Vec3::from(at))
                * Mat4::from_rotation_y(turn.to_radians())
                * Mat4::from_scale(Vec3::from(scale));
            let triangles: Vec<Triangle> = prefabs
                .triangles(id)?
                .iter()
                .map(|triangle| triangle.map(|corner| transform.transform_point3(corner)))
                .collect();
            let (mut low, mut high) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
            for corner in triangles.iter().flatten() {
                low = low.min(*corner);
                high = high.max(*corner);
            }
            Some(Drawn { label: format!("{id} at {at:?}"), triangles, low, high })
        })
        .collect()
}

/// Points over a polygon no more than half a metre apart: a wall's whole length, not its corners.
fn over_polygon(polygon: &[Vec3]) -> impl Iterator<Item = Vec3> + '_ {
    (1..polygon.len().saturating_sub(1)).flat_map(move |fan| {
        let (a, b, c) = (polygon[0], polygon[fan], polygon[fan + 1]);
        let longest = a.distance(b).max(b.distance(c)).max(c.distance(a));
        let steps = (longest / 0.5).ceil().max(1.0) as u32;
        (0..=steps).flat_map(move |i| {
            (0..=steps - i).map(move |j| {
                let (u, v) = (i as f32 / steps as f32, j as f32 / steps as f32);
                a + (b - a) * u + (c - a) * v
            })
        })
    })
}

/// **Nothing to walk through:** no point drawn at a body's height stands over the navmesh, which keeps a
/// body's radius from every collider. What's drawn on the far side of a wall isn't over it either.
#[test]
fn nothing_drawn_at_a_bodys_height_stands_on_the_navmesh() {
    let document = document();
    let graph = graph();
    let over = Reach { across: 0.0, vertical: BODY[1] + 0.1 };
    let drawn = drawn(&document);
    assert!(drawn.len() > 100, "the ward draws more than {} things", drawn.len());
    let mut failures = Vec::new();
    for placed in &drawn {
        let mut worst: Option<(f32, Vec3)> = None;
        for triangle in &placed.triangles {
            for point in over_polygon(&in_band(*triangle, DRAWN_BAND)) {
                let Some(on) = graph.locate(point, over) else { continue };
                let height = point.y - on.position.y;
                if (BODY[0]..=BODY[1]).contains(&height) && worst.is_none_or(|(most, _)| height > most) {
                    worst = Some((height, point));
                }
            }
        }
        if let Some((height, point)) = worst {
            failures.push(format!("{}: {point} is drawn {height:.2} m over the navmesh", placed.label));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// **No invisible walls:** each side of each collider has something drawn within the tolerance of it,
/// inside the box or just past it, along X and Z, which is the way a body meets one. The ground's is the
/// paving, a tree's is its trunk (a plant isn't an assembly), and a ramp's is the steps'.
#[test]
fn every_side_of_every_collider_has_something_drawn_at_it() {
    let document = document();
    let volumes: Vec<Volume> = volumes(&collider_set());
    let drawn = drawn(&document);
    let held = |volume: &&Volume| {
        !["ground", "court_steps"].contains(&volume.name.as_str()) && !volume.name.contains("tree")
    };
    assert!(volumes.iter().filter(held).count() > 100);
    let mut failures = Vec::new();
    for volume in volumes.iter().filter(held) {
        let reach = volume.half + Vec3::splat(TOLERANCE);
        let radius = reach.length();
        let (mut least, mut most) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for placed in &drawn {
            if (volume.center - placed.low.max(volume.center.min(placed.high))).length() > radius {
                continue;
            }
            for triangle in &placed.triangles {
                let mut polygon: Vec<Vec3> = triangle.iter().map(|&point| volume.local(point)).collect();
                for axis in 0..3 {
                    polygon = clip_plane(&polygon, |p| p[axis] + reach[axis]);
                    polygon = clip_plane(&polygon, |p| reach[axis] - p[axis]);
                }
                for point in polygon {
                    least = least.min(point);
                    most = most.max(point);
                }
            }
        }
        for (axis, letter) in [(0, "x"), (2, "z")] {
            let (low, high) = (least[axis] + volume.half[axis], volume.half[axis] - most[axis]);
            if low > TOLERANCE {
                failures.push(format!(
                    "{} at {}: -{letter} is {low:.3} m from anything drawn",
                    volume.name, volume.center
                ));
            }
            if high > TOLERANCE {
                failures.push(format!(
                    "{} at {}: +{letter} is {high:.3} m from anything drawn",
                    volume.name, volume.center
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The probe grid bakes what the zone draws, where it draws it: everything in its list is an instance at
/// that place, and every building, wall and gate of the zone's own is in its list, or its light moves off
/// them when one's moved.
#[test]
fn the_probe_grid_sees_what_the_zone_draws() {
    let document = document();
    let grid = probes::read(&fs::read(ward_dir().join("lantern_ward.probes.ron")).expect("the document"))
        .expect("it reads");
    let is_at = |id: AssetId, at: [f32; 3], turn: f32| {
        grid.meshes.iter().any(|mesh| mesh.mesh == id && mesh.translation == at && mesh.yaw == turn)
    };
    for mesh in &grid.meshes {
        let placed = document.instances.iter().any(|instance| {
            instance.mesh == mesh.mesh && instance.at == mesh.translation && instance.turn == mesh.yaw
        });
        assert!(
            placed,
            "the grid bakes {} at {:?}, which the zone doesn't draw",
            mesh.mesh, mesh.translation
        );
    }
    let prefabs = Prefabs::new();
    for instance in &document.instances {
        // A plant or a mountain isn't an assembly; what a grid bakes has no scale.
        let held = prefabs.sources.contains_key(&instance.mesh) && instance.scale == [1.0; 3];
        assert!(
            !held || is_at(instance.mesh, instance.at, instance.turn),
            "the zone draws {} at {:?}, which the grid doesn't bake",
            instance.mesh,
            instance.at
        );
    }
}
