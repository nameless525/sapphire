//! The benchmark nature zone's simulation half (M6 W1): its water, its spawners, the clearing's encounter
//! volume, its checkpoint, the clearing's fight, the colliders that fight touches and a navmesh over the
//! clearing, built here in code as [`super::district`]'s are.
//!
//! The zone itself is generated: `sapphire_pipeline`'s `bench::nature` writes its terrain, trees, grass, day,
//! weather plan, probes and walks from `bench/nature/` into a zone document that cooks like any other. What a
//! simulation reads of it stays compiled in (m5-plan §3, "The simulation's data"), so a fight there builds it
//! from here: the generator writes [`water`], [`spawners`], [`triggers`], [`checkpoints`], [`encounter`] and
//! [`colliders`] into its documents, and `sapphire_pipeline`'s nature tests hold the generated document's
//! simulation to [`simulation`], byte for byte, and its ground flat under [`CLEARING`]. [`built`] is what a
//! scenario on the zone stands its simulation on (`content/scenarios/nature-encounter.scenario.ron`).
//!
//! # Where things are
//!
//! The zone is [`HALF_EXTENT`] either way of the origin, in cells of [`CELL_SIZE`]: +X is east, +Z south. The
//! water is a lake in the meadow east of the middle, a river running into it from the north and a frozen pond
//! in the south-west ([`water`]), every surface at [`WATER_HEIGHT`]. The clearing is a flat square of ground
//! [`FLAT_HALF`] either way of [`CLEARING`], south-west of the lake. The player starts at its south edge
//! facing north ([`START`]); six enemies wait in it in two rows of three, facing the way the player comes;
//! entering its volume ([`CLEARING_EVENT`]) starts the fight, all six in one wave: the party of three against
//! six (m6-plan, W1).
//!
//! # What a body meets
//!
//! Only what the clearing's fight can touch: [`colliders`] is the flat ground under it, its top at 0, and
//! [`navmesh`] the clearing's square, a body's radius in from its edge. Terrain, trees and grass are
//! presentation, and nothing collides with them.

use sapphire_colliders::{ColliderDocument, ColliderEntry, ColliderShape, Group, Rotation};
use sapphire_navmesh::{NO_NEIGHBOUR, NavMesh};
use sapphire_serialize::AssetId;
use sapphire_zone::grid::Grid;
use sapphire_zone::water::{RiverPoint, WaterBody, WaterShape, WaterSurface};
use sapphire_zone::{
    BuiltZone, Checkpoint, EncounterDocument, Spawner, Trigger, Volume, Wave, WaveStart, ZoneDocument,
    ZoneSimulation,
};

/// A cell's side, in metres: the zone's grid.
pub const CELL_SIZE: f32 = 64.0;
/// Half the zone's extent along X and Z, in metres: 2,048 by 1,280.
pub const HALF_EXTENT: [f32; 2] = [1024.0, 640.0];
/// The clearing's middle, `(x, z)`, where the fight is.
pub const CLEARING: [f32; 2] = [320.0, 320.0];
/// Half the side of the square about [`CLEARING`] whose ground is flat at 0 and collided, in metres.
pub const FLAT_HALF: f32 = 48.0;
/// The height every water surface lies at, in metres.
pub const WATER_HEIGHT: f32 = -0.5;
/// The spawner the player starts at.
pub const START: &str = "player_start";
/// The event the clearing's volume raises, which starts its fight.
pub const CLEARING_EVENT: &str = "clearing_entered";
/// How many enemies the clearing's fight brings, at once.
pub const ENEMIES: usize = 6;
/// The name a scenario's `built` gives this zone, which is also the stem a stage finds its pack by (`--zone
/// nature`).
pub const ZONE_NAME: &str = "nature";
/// The lake's middle `(x, z)` and its radii along X and Z, in metres.
pub const LAKE: ([f32; 2], [f32; 2]) = ([480.0, 160.0], [72.0, 48.0]);
/// The frozen pond's middle `(x, z)` and its radii, in metres.
pub const POND: ([f32; 2], [f32; 2]) = ([-480.0, 512.0], [40.0, 32.0]);
/// How far south of the clearing's middle the player starts: outside the volume, on the flat ground.
const START_OUT: f32 = 30.0;
/// Half the clearing volume's side.
const VOLUME_HALF: f32 = 26.0;
/// The clearing fight's side.
const FACTION: u8 = 2;
/// How far the navmesh keeps from the clearing's edge, in metres: a body's radius.
const BODY_RADIUS: f32 = 0.5;
/// The half-side of the clearing's navmesh, in metres: the collided square's ([`FLAT_HALF`]) less a body's
/// radius, so a body that paths anywhere on it stands on the ground collider.
const NAV_HALF: f32 = FLAT_HALF - BODY_RADIUS;
/// The navmesh's cell, in metres: every corner lies on it.
const NAV_CELL: f32 = 0.5;
/// The river's points, `(x, z, width, speed)`, from the zone's north edge south into the lake.
const RIVER: [[f32; 4]; 7] = [
    [180.0, -640.0, 10.0, 1.4],
    [200.0, -480.0, 10.0, 1.4],
    [260.0, -360.0, 12.0, 1.2],
    [300.0, -220.0, 12.0, 1.2],
    [380.0, -60.0, 14.0, 1.0],
    [420.0, 60.0, 16.0, 0.8],
    [440.0, 130.0, 16.0, 0.6],
];

/// The clearing's encounter, by the ID the generator gives it: `nature/clearing.encounter.ron` under the benchmark.
#[must_use]
pub fn encounter_id() -> AssetId {
    crate::BENCH_ROOT.sub_asset("nature/clearing.encounter.ron")
}

/// The zone's collider set, by the ID the generator gives it: `nature/nature.colliders.ron`.
#[must_use]
pub fn colliders_id() -> AssetId {
    crate::BENCH_ROOT.sub_asset("nature/nature.colliders.ron")
}

/// An ellipse's outline about `middle` with `radii`, its sixteen points from +X.
fn ellipse((middle, radii): ([f32; 2], [f32; 2])) -> Vec<[f32; 2]> {
    crate::DIRECTIONS.iter().map(|[c, s]| [middle[0] + c * radii[0], middle[1] + s * radii[1]]).collect()
}

/// The zone's water: the lake, the river into it, and the frozen pond, which nobody swims in.
/// Their surfaces are presentation: the generator names each one's reflection probe, which no simulation reads.
#[must_use]
pub fn water() -> Vec<WaterBody> {
    let body = |name: &str, shape: WaterShape, surface: WaterSurface| WaterBody {
        name: name.to_owned(),
        shape,
        surface,
        material: None,
        swim: surface == WaterSurface::Liquid,
        probe: None,
        shore_margin: 1.0,
    };
    let river = RIVER
        .iter()
        .map(|&[x, z, width, speed]| RiverPoint { at: [x, WATER_HEIGHT, z], width, speed })
        .collect();
    vec![
        body("lake", WaterShape::Lake { outline: ellipse(LAKE), height: WATER_HEIGHT }, WaterSurface::Liquid),
        body("river", WaterShape::River { points: river }, WaterSurface::Liquid),
        body(
            "frozen_pond",
            WaterShape::Lake { outline: ellipse(POND), height: WATER_HEIGHT },
            WaterSurface::Frozen,
        ),
    ]
}

/// The zone's spawners: the player's start at the clearing's south edge, facing north, and the clearing's six
/// enemies, spawning `kinds` in turn.
fn spawners_of(kinds: &[&str]) -> Vec<Spawner> {
    let spawner = |name: String, position: [f32; 3], yaw_degrees: f32, spawns: &str| Spawner {
        name,
        position,
        yaw_degrees,
        spawns: spawns.to_owned(),
        patrol: None,
    };
    let [x, z] = CLEARING;
    let mut spawners = vec![spawner(START.to_owned(), [x, 0.0, z + START_OUT], 180.0, "player")];
    // Two rows of three across the clearing's middle, facing south (+Z), the way the player comes.
    for (index, (dx, dz)) in
        [-10.0_f32, 0.0, 10.0].iter().flat_map(|&dx| [-12.0_f32, -2.0].map(|dz| (dx, dz))).enumerate()
    {
        spawners.push(spawner(enemy_spawner(index), [x + dx, 0.0, z + dz], 0.0, kinds[index % kinds.len()]));
    }
    spawners
}

/// The zone's spawners, the clearing's enemies a warrior and a Lancer in turn: what the generated document
/// holds (a scenario's `enemies` decides what a fight brings).
#[must_use]
pub fn spawners() -> Vec<Spawner> {
    spawners_of(&["warrior", "lancer"])
}

/// The name of the clearing's `index`th enemy spawner.
#[must_use]
pub fn enemy_spawner(index: usize) -> String {
    format!("clearing_spawn_{:02}", index + 1)
}

/// The clearing's encounter volume, up to a character's height and a quarter metre under the ground.
#[must_use]
pub fn triggers() -> Vec<Trigger> {
    vec![Trigger {
        name: "clearing".to_owned(),
        position: [CLEARING[0], 1.25, CLEARING[1]],
        volume: Volume::Box { half_extents: [VOLUME_HALF, 1.5, VOLUME_HALF] },
        event: CLEARING_EVENT.to_owned(),
    }]
}

/// Where a walk comes back to: the player's start, outside the clearing's volume.
#[must_use]
pub fn checkpoints() -> Vec<Checkpoint> {
    vec![Checkpoint {
        name: "clearing".to_owned(),
        position: [CLEARING[0], 0.0, CLEARING[1] + START_OUT],
        yaw_degrees: 180.0,
        volume: Volume::Box { half_extents: [4.0, 1.5, 1.0] },
    }]
}

/// The clearing's fight: its six enemies in one wave, a few ticks apart, with three tokens between them.
#[must_use]
pub fn encounter() -> EncounterDocument {
    EncounterDocument {
        starts_on: CLEARING_EVENT.to_owned(),
        faction: FACTION,
        tokens: 3,
        token_rest: 30,
        waves: vec![Wave {
            when: WaveStart::Start,
            gap: 6,
            spawners: (0..ENEMIES).map(enemy_spawner).collect(),
        }],
    }
}

/// The zone's colliders: the clearing's flat ground, its top at 0.
#[must_use]
pub fn colliders() -> ColliderDocument {
    ColliderDocument {
        colliders: vec![ColliderEntry {
            name: "clearing_ground".to_owned(),
            shape: ColliderShape::Box { half_extents: [FLAT_HALF, 0.5, FLAT_HALF] },
            position: [CLEARING[0], -0.5, CLEARING[1]],
            rotation: Rotation::None,
            groups: vec![Group::World],
        }],
    }
}

/// The zone's navmesh: the clearing's square, a body's radius in from its edge, as one polygon.
#[must_use]
pub fn navmesh() -> NavMesh {
    let cell = |v: f32| (v / NAV_CELL) as i32;
    let ([x0, z0], [x1, z1]) =
        ([CLEARING[0] - NAV_HALF, CLEARING[1] - NAV_HALF], [CLEARING[0] + NAV_HALF, CLEARING[1] + NAV_HALF]);
    NavMesh {
        origin: [0.0; 3],
        cell_size: NAV_CELL,
        cell_height: NAV_CELL,
        vertices: vec![
            [cell(x0), 0, cell(z0)],
            [cell(x0), 0, cell(z1)],
            [cell(x1), 0, cell(z1)],
            [cell(x1), 0, cell(z0)],
        ],
        polygon_starts: vec![0, 4],
        // West, south (+Z), east, north, as the district's are wound.
        corners: vec![0, 1, 2, 3],
        neighbours: vec![NO_NEIGHBOUR; 4],
    }
}

/// The zone's simulation half, as a cook of its generated document makes it.
#[must_use]
pub fn simulation() -> ZoneSimulation {
    document(&["warrior", "lancer"])
        .simulation()
        .expect("the nature zone's simulation half is a valid zone's")
}

/// The zone's simulation half as a document, the clearing's enemies spawning `kinds` in turn.
fn document(kinds: &[&str]) -> ZoneDocument {
    ZoneDocument {
        grid: Grid { cell_size: CELL_SIZE },
        placements: Vec::new(),
        instances: Vec::new(),
        lights: Vec::new(),
        probes: None,
        reflections: Vec::new(),
        day: None,
        rigs: None,
        sky: None,
        wind: None,
        weather: None,
        fog: None,
        colliders: Some(colliders_id()),
        spawners: spawners_of(kinds),
        triggers: triggers(),
        checkpoints: checkpoints(),
        splines: Vec::new(),
        navigation: None,
        encounters: vec![encounter_id()],
        exits: Vec::new(),
        ambient: None,
        water: water(),
        rooms: Vec::new(),
        weather_plan: None,
    }
}

/// The nature zone a scenario fights on, a [`sapphire_zone::BuildZone`]: its simulation half, the clearing's
/// six spawning `enemies` in turn.
///
/// # Errors
///
/// If `enemies` names no kind, or the colliders or the navmesh don't cook.
pub fn built(enemies: &[&str]) -> Result<BuiltZone, String> {
    if enemies.is_empty() {
        return Err(
            "the nature zone's clearing brings enemies of the kinds its scenario names, and it names none"
                .into(),
        );
    }
    let colliders = colliders().cook().and_then(|set| set.to_cooked()).map_err(|error| error.to_string())?;
    let navmesh = navmesh().to_cooked().map_err(|error| error.to_string())?;
    Ok(BuiltZone {
        document: document(enemies),
        encounters: vec![(encounter_id(), encounter())],
        colliders,
        navmesh: Some(navmesh),
    })
}

#[cfg(test)]
mod tests {
    use sapphire_math::Vec3;
    use sapphire_nav::{NavGraph, Reach};
    use sapphire_zone::CookedVolume;

    use super::*;

    #[test]
    fn the_clearing_holds_its_fight_and_the_start_lies_outside_it() {
        let simulation = simulation();
        let [trigger] = simulation.triggers.as_slice() else { panic!("one volume") };
        let CookedVolume::Box { half_extents } = trigger.volume else { panic!("a box") };
        let inside =
            |at: [f32; 3]| (0..3).all(|axis| (at[axis] - trigger.position[axis]).abs() <= half_extents[axis]);
        for index in 0..ENEMIES {
            let name = enemy_spawner(index);
            let spawner = simulation.spawners.iter().find(|spawner| spawner.name == name).unwrap();
            assert!(inside(spawner.position), "{name} stands in the clearing");
        }
        let start = simulation.spawners.iter().find(|spawner| spawner.name == START).unwrap().position;
        assert!(!inside(start), "the player starts outside the volume");
        assert!(
            half_extents[0] < FLAT_HALF && half_extents[2] < FLAT_HALF,
            "the volume is on the flat ground"
        );
    }

    #[test]
    fn the_fight_names_this_zones_spawners_and_its_volume() {
        let fight = encounter();
        fight.check().unwrap();
        let names: Vec<String> = spawners().into_iter().map(|spawner| spawner.name).collect();
        assert_eq!(fight.waves.iter().map(|wave| wave.spawners.len()).sum::<usize>(), ENEMIES);
        for name in fight.waves.iter().flat_map(|wave| &wave.spawners) {
            assert!(names.contains(name), "{name}");
        }
        assert_eq!(triggers()[0].event, fight.starts_on);
    }

    /// The water is three bodies the zone check accepts, every one clear of the clearing's flat ground, inside
    /// the zone, and the river ends in the lake.
    #[test]
    fn the_water_is_three_bodies_clear_of_the_clearing() {
        let bodies = water();
        assert_eq!(bodies.len(), 3);
        let simulation = simulation();
        assert_eq!(simulation.water.len(), 3);
        let clear = |[x, z]: [f32; 2], reach: f32| {
            (x - CLEARING[0]).abs() > FLAT_HALF + reach || (z - CLEARING[1]).abs() > FLAT_HALF + reach
        };
        for (middle, radii) in [LAKE, POND] {
            for point in ellipse((middle, radii)) {
                assert!(clear(point, 0.0), "{point:?}");
                assert!(point[0].abs() < HALF_EXTENT[0] && point[1].abs() < HALF_EXTENT[1]);
            }
        }
        for [x, z, width, _] in RIVER {
            assert!(clear([x, z], width * 0.5), "the river at ({x}, {z})");
            assert!(x.abs() <= HALF_EXTENT[0] && z.abs() <= HALF_EXTENT[1]);
        }
        let [x, z, ..] = RIVER[RIVER.len() - 1];
        let (middle, radii) = LAKE;
        let (dx, dz) = ((x - middle[0]) / radii[0], (z - middle[1]) / radii[1]);
        assert!(dx * dx + dz * dz < 1.0, "the river ends inside the lake");
    }

    /// The colliders and the navmesh come out the same bytes every time, the navmesh is one the simulation
    /// accepts, and everyone the fight brings, and whoever starts or comes back, stands on it.
    #[test]
    fn everyone_stands_on_the_navmesh_over_the_ground() {
        let cooked = || colliders().cook().unwrap().to_cooked().unwrap().into_vec();
        assert_eq!(cooked(), cooked());
        let mesh = navmesh().to_cooked().unwrap();
        assert_eq!(mesh.as_slice(), navmesh().to_cooked().unwrap().as_slice());
        let read = NavMesh::read(&mesh).expect("a navmesh the simulation accepts");
        assert_eq!(read.polygon_count(), 1);
        let graph = NavGraph::new(read);
        let reach = Reach { across: 0.0, vertical: 0.1 };
        let simulation = simulation();
        let places = simulation.spawners.iter().map(|spawner| (spawner.name.as_str(), spawner.position));
        let places =
            places.chain(simulation.checkpoints.iter().map(|point| (point.name.as_str(), point.position)));
        for (name, at) in places {
            assert!(
                graph.locate(Vec3::from_array(at), reach).is_some(),
                "{name} at {at:?} is off the navmesh"
            );
            let (dx, dz) = (at[0] - CLEARING[0], at[2] - CLEARING[1]);
            assert!(dx.abs() < FLAT_HALF && dz.abs() < FLAT_HALF, "{name} stands on the collided ground");
        }
        // The navmesh reaches the collided square's edge less a body's radius, and no further.
        let edge = FLAT_HALF - BODY_RADIUS;
        for [sx, sz] in [[1.0, 1.0], [-1.0, 1.0], [1.0, -1.0], [-1.0, -1.0]] {
            let inside = Vec3::new(CLEARING[0] + sx * (edge - 0.05), 0.0, CLEARING[1] + sz * (edge - 0.05));
            assert!(graph.locate(inside, reach).is_some(), "{inside:?} is off the navmesh");
            let outside = Vec3::new(CLEARING[0] + sx * (edge + 0.05), 0.0, CLEARING[1] + sz * (edge + 0.05));
            assert!(graph.locate(outside, reach).is_none(), "{outside:?} is on the navmesh");
        }
        let built = built(&["warrior"]).unwrap();
        assert_eq!(built.colliders.as_slice(), cooked().as_slice());
        assert_eq!(built.navmesh.as_deref(), Some(mesh.as_slice()));
    }

    /// A scenario's kinds spawn in turn at the clearing's six, and it has to name one.
    #[test]
    fn the_built_zone_spawns_the_kinds_it_is_given() {
        let built = built(&["warrior", "lancer", "striker"]).unwrap();
        let kinds: Vec<&str> = (0..ENEMIES)
            .map(|index| {
                let name = enemy_spawner(index);
                built.document.spawners.iter().find(|spawner| spawner.name == name).unwrap().spawns.as_str()
            })
            .collect();
        assert_eq!(kinds, ["warrior", "lancer", "striker", "warrior", "lancer", "striker"]);
        assert_eq!(built.document.colliders, Some(colliders_id()));
        assert_eq!(built.encounters, [(encounter_id(), encounter())]);
        assert!(super::built(&[]).is_err());
    }
}
