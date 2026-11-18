//! The benchmark district's simulation half (M5 B2, B5): its spawners, the plaza's encounter volume, its
//! checkpoint, the plaza's fight, the colliders a walk and that fight touch, and a navmesh over the avenues and
//! the plaza, built here in code as the rest of this crate builds its zone.
//!
//! The district itself is generated: `sapphire_pipeline`'s `bench::district` writes its streets, buildings,
//! props, lights and tour from `bench/district/` into a zone document that cooks like any other. What a
//! simulation reads of it stays compiled in (m5-plan §3, "The simulation's data"), so a fight on the
//! district builds it from here rather than from a pack nothing ships: [`simulation`] is the cooked half,
//! and the generator writes [`spawners`], [`triggers`], [`checkpoints`], [`encounter`] and [`colliders`] into
//! its documents. `sapphire_pipeline`'s district tests hold the two to each other: the generated document's
//! simulation is [`simulation`], byte for byte, its streets are [`STREETS`], and everything here stands on open
//! ground the generator left clear. [`built`] is what a scenario on the district stands its simulation on
//! (`content/scenarios/district-encounter.scenario.ron`, M5 B5), handed to whoever sets that scenario up.
//!
//! # Where things are
//!
//! The district is `cells` × `cells` cells of [`CELL_SIZE`] about the origin, an even count. Its plaza is
//! the four cells that meet at the origin, [`PLAZA_HALF`] either way, with streets along its edges and an
//! avenue crossing it on each axis. The player starts at the south end of the north–south avenue facing
//! north ([`START`]), a walk of the avenue reaches the plaza from the south, and twelve enemies wait in it
//! in three rows facing the way the player comes ([`ENEMIES`]). Entering the plaza's volume
//! ([`PLAZA_EVENT`]) starts the fight, all twelve in one wave: §3.4's twelve active enemies.
//!
//! # What a body meets (M5 B5)
//!
//! Only what the walk up the avenue and the plaza's fight can touch, and the same for every preset, so one
//! replay plays on the `ci` district and the `budget` one alike: the inner four by four cells, the `ci`
//! district whole. [`colliders`] is the ground under those cells and a box for each block among them: the
//! block's ground inside the streets that edge it (on an outer side, the widest street that side has in any
//! preset), as high as the tallest building. Buildings, their frontage and courtyards are inside it; lantern
//! posts and the plaza's rim furniture, a metre into the paving, aren't collided, and nor is the smallest
//! district's boundary wall, which no walk reaches. [`navmesh`] is the two avenues and the plaza's paving, a
//! body's radius in from a block, as rectangles sharing whole edges: the lanes and streets between blocks,
//! which walls close in places, aren't on it. [`AVENUE_START`] is where a walk up the avenue starts, the same
//! on every preset ([`START`] moves with the district's size).

use sapphire_colliders::{ColliderDocument, ColliderEntry, ColliderShape, Group, Rotation};
use sapphire_navmesh::{NO_NEIGHBOUR, NavMesh};
use sapphire_serialize::AssetId;
use sapphire_zone::grid::Grid;
use sapphire_zone::{
    BuildZone, BuiltZone, Checkpoint, EncounterDocument, Spawner, Trigger, Volume, Wave, WaveStart,
    ZoneDocument, ZoneSimulation,
};

/// A cell's side, in metres: the district's grid.
pub const CELL_SIZE: f32 = 32.0;
/// Half the plaza's side, in metres: the four cells about the origin.
pub const PLAZA_HALF: f32 = CELL_SIZE;
/// The fewest cells a side: the plaza and a ring of blocks around it.
pub const MIN_CELLS: u32 = 4;
/// The spawner the player starts at.
pub const START: &str = "player_start";
/// Where a walk up the avenue starts (M5 B5): [`START`] in the smallest district, and on the avenue in any.
pub const AVENUE_START: &str = "avenue_start";
/// The event the plaza's volume raises, which starts its fight.
pub const PLAZA_EVENT: &str = "plaza_entered";
/// How many enemies the plaza's fight brings, at once.
pub const ENEMIES: usize = 12;
/// The streets' widths, in metres, which the generated district's document must have (M5 B5): the colliders and
/// the navmesh are cut by them.
pub const STREETS: Streets = Streets { avenue: 12.0, street: 8.0, lane: 5.0, edge: 2.5 };
/// The name a scenario's `built` gives this zone, which is also the stem a stage finds its pack by (`--zone
/// district`).
pub const ZONE_NAME: &str = "district";
/// How far in from the district's edge the player starts, in metres.
const START_INSET: f32 = 4.0;
/// Half the plaza volume's side: inside the streets that edge the plaza, so a walk up the avenue enters it
/// on the plaza's paving.
const VOLUME_HALF: f32 = 27.0;
/// The plaza fight's side.
const FACTION: u8 = 2;
/// How high a block's box stands, in metres: over the tallest building (three storeys of 3.4 m and a roof).
const BLOCK_HEIGHT: f32 = 12.0;
/// How far the navmesh keeps from a block, in metres: a body's radius and a little.
const BODY_RADIUS: f32 = 0.5;
/// How far along an avenue from the plaza's middle the navmesh reaches, in metres: inside the smallest
/// district's boundary wall, a body's radius off it.
const AVENUE_REACH: f32 = 63.0;
/// The navmesh's cell, in metres: every corner above lies on it.
const NAV_CELL: f32 = 0.5;
/// The benchmark's root ID, `sapphire_pipeline::bench::BENCH_ROOT`, from which its documents' IDs come: the
/// pipeline's district tests hold the two together.
const BENCH_ROOT: AssetId = AssetId::from_text("4b0c8e31-5d27-4f6a-9a83-2e61c07db5f4");

/// A street's width by its kind, in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Streets {
    /// Along each axis through the plaza.
    pub avenue: f32,
    /// On every other line between the avenue and the edge.
    pub street: f32,
    /// On the rest.
    pub lane: f32,
    /// The alley between the outer blocks and the boundary wall, whole.
    pub edge: f32,
}

/// Why a district can't be built at `cells` a side.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a district is an even number of cells a side, from {MIN_CELLS}, not {0}")]
pub struct CellsError(pub u32);

/// Checks that a district can be `cells` a side.
pub fn check_cells(cells: u32) -> Result<(), CellsError> {
    if cells < MIN_CELLS || !cells.is_multiple_of(2) {
        return Err(CellsError(cells));
    }
    Ok(())
}

/// Half the district's side, in metres.
#[must_use]
pub fn half_side(cells: u32) -> f32 {
    cells as f32 * CELL_SIZE * 0.5
}

/// The plaza's encounter, by the ID the generator gives it: `district/plaza.encounter.ron` under the benchmark.
#[must_use]
pub fn encounter_id() -> AssetId {
    BENCH_ROOT.sub_asset("district/plaza.encounter.ron")
}

/// The district's collider set, by the ID the generator gives it: `district/district.colliders.ron`.
#[must_use]
pub fn colliders_id() -> AssetId {
    BENCH_ROOT.sub_asset("district/district.colliders.ron")
}

/// The district's spawners: the player's start, where a fight starts the player at the plaza's edge, where a
/// walk up the avenue starts, and the plaza's twelve enemies, a warrior and a Lancer in turn.
#[must_use]
pub fn spawners(cells: u32) -> Vec<Spawner> {
    spawners_of(cells, &["warrior", "lancer"])
}

/// [`spawners`], the plaza's enemies spawning `kinds` in turn.
fn spawners_of(cells: u32, kinds: &[&str]) -> Vec<Spawner> {
    let spawner = |name: String, position: [f32; 3], yaw_degrees: f32, spawns: &str| Spawner {
        name,
        position,
        yaw_degrees,
        spawns: spawns.to_owned(),
        patrol: None,
    };
    // Facing north (-Z), up the avenue toward the plaza.
    let mut spawners = vec![
        spawner(START.to_owned(), [0.0, 0.0, half_side(cells) - START_INSET], 180.0, "player"),
        spawner("plaza_arrive".to_owned(), [0.0, 0.0, VOLUME_HALF + 1.0], 180.0, "player"),
        spawner(AVENUE_START.to_owned(), [0.0, 0.0, half_side(MIN_CELLS) - START_INSET], 180.0, "player"),
    ];
    // Three rows of four across the plaza's middle, facing south (+Z), the way the player comes.
    for (index, (x, z)) in
        [-18.0_f32, -6.0, 6.0, 18.0].iter().flat_map(|&x| [-14.0_f32, -4.0, 6.0].map(|z| (x, z))).enumerate()
    {
        spawners.push(spawner(enemy_spawner(index), [x, 0.0, z], 0.0, kinds[index % kinds.len()]));
    }
    spawners
}

/// The name of the plaza's `index`th enemy spawner.
#[must_use]
pub fn enemy_spawner(index: usize) -> String {
    format!("plaza_spawn_{:02}", index + 1)
}

/// The plaza's encounter volume: the plaza inside its edging streets, up to a character's height and a
/// quarter metre under the paving, since a trigger holds a character by its feet.
#[must_use]
pub fn triggers() -> Vec<Trigger> {
    vec![Trigger {
        name: "plaza".to_owned(),
        position: [0.0, 1.25, 0.0],
        volume: Volume::Box { half_extents: [VOLUME_HALF, 1.5, VOLUME_HALF] },
        event: PLAZA_EVENT.to_owned(),
    }]
}

/// Where a walk comes back to: the plaza's south edge, outside its volume.
#[must_use]
pub fn checkpoints() -> Vec<Checkpoint> {
    vec![Checkpoint {
        name: "plaza".to_owned(),
        position: [0.0, 0.0, VOLUME_HALF + 1.0],
        yaw_degrees: 180.0,
        volume: Volume::Box { half_extents: [4.0, 1.5, 1.0] },
    }]
}

/// The plaza's fight: its twelve enemies in one wave, a few ticks apart, with three tokens between them.
#[must_use]
pub fn encounter() -> EncounterDocument {
    EncounterDocument {
        starts_on: PLAZA_EVENT.to_owned(),
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

/// A rectangle on the ground, in metres: least X, least Z, most X, most Z.
type Rect = [f32; 4];

/// The inner four by four cells' lines on either axis, from the west (or the north): the outer line, the lane,
/// the avenue, the lane, the outer line.
const LINES: [f32; 5] = [-2.0 * CELL_SIZE, -CELL_SIZE, 0.0, CELL_SIZE, 2.0 * CELL_SIZE];

/// Half the width of the street along line `k` of [`LINES`]: the avenue through the middle, a lane either side,
/// and outside those the widest an outer line's street is in any district (the edge's alley in the smallest, a
/// street in a larger one), so a block's box stays inside the block in every preset.
fn half_street(k: usize) -> f32 {
    match k {
        2 => STREETS.avenue * 0.5,
        1 | 3 => STREETS.lane * 0.5,
        _ => STREETS.edge.max(STREETS.street * 0.5),
    }
}

/// The blocks among the inner four by four cells, each the ground inside the streets that edge it, by cell from
/// the north-west, row by row: every cell but the plaza's four.
fn blocks() -> Vec<([usize; 2], Rect)> {
    let mut blocks = Vec::new();
    for z in 0..4 {
        for x in 0..4 {
            if (1..=2).contains(&x) && (1..=2).contains(&z) {
                continue;
            }
            let rect = [
                LINES[x] + half_street(x),
                LINES[z] + half_street(z),
                LINES[x + 1] - half_street(x + 1),
                LINES[z + 1] - half_street(z + 1),
            ];
            blocks.push(([x, z], rect));
        }
    }
    blocks
}

/// The district's colliders (see the [module docs](self#what-a-body-meets-m5-b5)): the ground under the inner
/// four by four cells, its top at 0, and a box for each block among them.
#[must_use]
pub fn colliders() -> ColliderDocument {
    let entry = |name: String, half_extents: [f32; 3], position: [f32; 3]| ColliderEntry {
        name,
        shape: ColliderShape::Box { half_extents },
        position,
        rotation: Rotation::None,
        groups: vec![Group::World],
    };
    let reach = LINES[4];
    let mut colliders = vec![entry("ground".to_owned(), [reach, 0.5, reach], [0.0, -0.5, 0.0])];
    for ([x, z], [x0, z0, x1, z1]) in blocks() {
        colliders.push(entry(
            format!("block_{x}_{z}"),
            [(x1 - x0) * 0.5, BLOCK_HEIGHT * 0.5, (z1 - z0) * 0.5],
            [(x0 + x1) * 0.5, BLOCK_HEIGHT * 0.5, (z0 + z1) * 0.5],
        ));
    }
    ColliderDocument { colliders }
}

/// What a body can walk: the plaza's paving inside its lanes, and each avenue out to [`AVENUE_REACH`], a
/// body's radius in from the blocks either side.
fn walkable() -> [Rect; 3] {
    let paving = CELL_SIZE - half_street(1);
    let avenue = half_street(2) - BODY_RADIUS;
    [
        [-paving, -paving, paving, paving],
        [-avenue, -AVENUE_REACH, avenue, AVENUE_REACH],
        [-AVENUE_REACH, -avenue, AVENUE_REACH, avenue],
    ]
}

/// The district's navmesh (see the [module docs](self#what-a-body-meets-m5-b5)): the grid every walkable
/// rectangle's edges cut, keeping each piece inside one of them, so neighbours share whole edges. Every corner
/// lies on a whole cell of half a metre, so nothing in it is rounded.
#[must_use]
pub fn navmesh() -> NavMesh {
    let walkable = walkable();
    // Both axes cut alike, since what's walkable is symmetric about both.
    let mut cuts: Vec<f32> = walkable.iter().flat_map(|rect| [rect[0], rect[2]]).collect();
    cuts.sort_by(f32::total_cmp);
    cuts.dedup();
    let n = cuts.len() - 1;
    let inside = |x: usize, z: usize| {
        let (cx, cz) = ((cuts[x] + cuts[x + 1]) * 0.5, (cuts[z] + cuts[z + 1]) * 0.5);
        walkable.iter().any(|rect| cx > rect[0] && cx < rect[2] && cz > rect[1] && cz < rect[3])
    };
    let mut polygon = vec![None; n * n];
    let mut count = 0u32;
    for z in 0..n {
        for x in 0..n {
            if inside(x, z) {
                polygon[z * n + x] = Some(count);
                count += 1;
            }
        }
    }
    let at = |x: Option<usize>, z: Option<usize>| match (x, z) {
        (Some(x), Some(z)) if x < n && z < n => polygon[z * n + x].unwrap_or(NO_NEIGHBOUR),
        _ => NO_NEIGHBOUR,
    };
    let vertex = |x: usize, z: usize| (z * (n + 1) + x) as u32;
    let cell = |v: f32| (v / NAV_CELL) as i32;
    let mut mesh = NavMesh {
        origin: [0.0; 3],
        cell_size: NAV_CELL,
        cell_height: NAV_CELL,
        vertices: (0..=n)
            .flat_map(|z| (0..=n).map(move |x| (x, z)))
            .map(|(x, z)| [cell(cuts[x]), 0, cell(cuts[z])])
            .collect(),
        polygon_starts: vec![0],
        corners: Vec::new(),
        neighbours: Vec::new(),
    };
    for z in 0..n {
        for x in 0..n {
            if polygon[z * n + x].is_none() {
                continue;
            }
            // West, south (+Z), east, north: each corner's edge runs to the next, wound as the format asks.
            mesh.corners.extend([vertex(x, z), vertex(x, z + 1), vertex(x + 1, z + 1), vertex(x + 1, z)]);
            mesh.neighbours.extend([
                at(x.checked_sub(1), Some(z)),
                at(Some(x), Some(z + 1)),
                at(Some(x + 1), Some(z)),
                at(Some(x), z.checked_sub(1)),
            ]);
            mesh.polygon_starts.push(mesh.corners.len() as u32);
        }
    }
    mesh
}

/// The district's simulation half, as a cook of its generated document makes it at `cells` a side.
///
/// # Panics
///
/// If `cells` isn't a district's ([`check_cells`]).
#[must_use]
pub fn simulation(cells: u32) -> ZoneSimulation {
    document(cells, &["warrior", "lancer"])
        .simulation()
        .expect("the district's simulation half is a valid zone's")
}

/// The district's simulation half as a document at `cells` a side, the plaza's enemies spawning `kinds` in turn.
///
/// # Panics
///
/// If `cells` isn't a district's ([`check_cells`]).
fn document(cells: u32, kinds: &[&str]) -> ZoneDocument {
    check_cells(cells).expect("a district's cells");
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
        spawners: spawners_of(cells, kinds),
        triggers: triggers(),
        checkpoints: checkpoints(),
        splines: Vec::new(),
        navigation: None,
        encounters: vec![encounter_id()],
        exits: Vec::new(),
        ambient: None,
        water: Vec::new(),
        rooms: Vec::new(),
    }
}

/// The zones this crate builds for a scenario to stand on, by the name its `built` gives each: what the replay
/// runner and a development build's fight hand over (`sapphire_gameplay::replay::Scenario::zones`).
pub const ZONES: &[(&str, BuildZone)] = &[(ZONE_NAME, built)];

/// The district a scenario fights on (M5 B5), a [`sapphire_zone::BuildZone`]: its simulation half at the
/// smallest size, whose colliders and navmesh are every preset's, the plaza's twelve spawning `enemies` in turn.
///
/// # Errors
///
/// If `enemies` names no kind, or the colliders or the navmesh don't cook.
pub fn built(enemies: &[&str]) -> Result<BuiltZone, String> {
    if enemies.is_empty() {
        return Err(
            "the district's plaza brings enemies of the kinds its scenario names, and it names none".into()
        );
    }
    let colliders = colliders().cook().and_then(|set| set.to_cooked()).map_err(|error| error.to_string())?;
    let navmesh = navmesh().to_cooked().map_err(|error| error.to_string())?;
    Ok(BuiltZone {
        document: document(MIN_CELLS, enemies),
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
    fn the_plaza_holds_its_fight_and_the_start_lies_outside_it() {
        for cells in [MIN_CELLS, 8, 16] {
            let simulation = simulation(cells);
            let [trigger] = simulation.triggers.as_slice() else { panic!("one volume") };
            let CookedVolume::Box { half_extents } = trigger.volume else { panic!("a box") };
            let inside = |at: [f32; 3]| {
                (0..3).all(|axis| (at[axis] - trigger.position[axis]).abs() <= half_extents[axis])
            };
            for index in 0..ENEMIES {
                let name = enemy_spawner(index);
                let spawner = simulation.spawners.iter().find(|spawner| spawner.name == name).unwrap();
                assert!(inside(spawner.position), "{name} stands in the plaza");
            }
            for name in [START, AVENUE_START] {
                let start = simulation.spawners.iter().find(|spawner| spawner.name == name).unwrap().position;
                assert!(
                    !inside(start) && start[2] < half_side(cells),
                    "{name} is in the district, south of the plaza"
                );
            }
            assert!(
                half_extents[0] < PLAZA_HALF && half_extents[2] < PLAZA_HALF,
                "the volume is the plaza's"
            );
        }
    }

    #[test]
    fn the_fight_names_this_zones_spawners_and_its_volume() {
        let fight = encounter();
        fight.check().unwrap();
        let names: Vec<String> = spawners(8).into_iter().map(|spawner| spawner.name).collect();
        assert_eq!(fight.waves.iter().map(|wave| wave.spawners.len()).sum::<usize>(), ENEMIES);
        for name in fight.waves.iter().flat_map(|wave| &wave.spawners) {
            assert!(names.contains(name), "{name}");
        }
        assert_eq!(triggers()[0].event, fight.starts_on);
    }

    #[test]
    fn only_an_even_count_from_four_is_a_district() {
        assert!(check_cells(4).is_ok() && check_cells(8).is_ok());
        assert!(check_cells(2).is_err() && check_cells(5).is_err());
    }

    /// The colliders and the navmesh come out the same bytes every time, and the navmesh is one the simulation
    /// accepts: every polygon convex and wound one way, every shared edge shared from both sides.
    #[test]
    fn the_colliders_and_the_navmesh_are_the_same_every_time() {
        let cooked = || colliders().cook().unwrap().to_cooked().unwrap().into_vec();
        assert_eq!(cooked(), cooked());
        let mesh = navmesh().to_cooked().unwrap();
        assert_eq!(mesh.as_slice(), navmesh().to_cooked().unwrap().as_slice());
        let read = NavMesh::read(&mesh).expect("a navmesh the simulation accepts");
        // The plaza's three by three, and two pieces of each avenue past it.
        assert_eq!(read.polygon_count(), 13);
        let built = built(&["warrior"]).unwrap();
        assert_eq!(built.colliders.as_slice(), cooked().as_slice());
        assert_eq!(built.navmesh.as_deref(), Some(mesh.as_slice()));
    }

    /// Whoever the fight brings, and whoever starts or comes back, stands on the navmesh and clear of every
    /// block, with a body's half metre about them: at the smallest size, which a scenario builds.
    #[test]
    fn everyone_stands_on_the_navmesh_clear_of_the_blocks() {
        let mesh = navmesh().to_cooked().unwrap();
        let graph = NavGraph::new(NavMesh::read(&mesh).unwrap());
        let on =
            |at: [f32; 3]| graph.locate(Vec3::from_array(at), Reach { across: 0.0, vertical: 0.1 }).is_some();
        let boxes: Vec<Rect> = blocks().into_iter().map(|(_, rect)| rect).collect();
        let clear = |at: [f32; 3]| {
            boxes.iter().all(|rect| {
                at[0] + 0.5 <= rect[0]
                    || at[0] - 0.5 >= rect[2]
                    || at[2] + 0.5 <= rect[1]
                    || at[2] - 0.5 >= rect[3]
            })
        };
        let simulation = simulation(MIN_CELLS);
        let places = simulation.spawners.iter().map(|spawner| (spawner.name.as_str(), spawner.position));
        let places =
            places.chain(simulation.checkpoints.iter().map(|point| (point.name.as_str(), point.position)));
        for (name, at) in places {
            assert!(on(at), "{name} at {at:?} is off the navmesh");
            assert!(clear(at), "{name} at {at:?} stands in a block");
        }
        // The ground is under the whole navmesh.
        let ground = &colliders().colliders[0];
        let ColliderShape::Box { half_extents } = ground.shape else { panic!("a box") };
        assert!(half_extents[0] >= AVENUE_REACH && half_extents[2] >= AVENUE_REACH);
    }

    /// A walk up the avenue from its start into the plaza's middle stays on the navmesh at every step, and the
    /// navmesh says it walks straight.
    #[test]
    fn the_avenues_walk_never_leaves_the_navmesh() {
        let mesh = navmesh().to_cooked().unwrap();
        let graph = NavGraph::new(NavMesh::read(&mesh).unwrap());
        let reach = Reach { across: 0.0, vertical: 0.1 };
        let start = simulation(MIN_CELLS).spawners.iter().find(|s| s.name == AVENUE_START).unwrap().position;
        let mut last = None;
        for step in 0..=240 {
            let at = Vec3::new(0.0, 0.0, start[2] * (1.0 - step as f32 / 240.0));
            let here = graph.locate(at, reach).unwrap_or_else(|| panic!("{at} is off the navmesh"));
            if let Some(from) = last {
                assert!(graph.walks_straight(from, here), "{at}");
            }
            last = Some(here);
        }
        // The blocks either side of the avenue are off it.
        assert!(graph.locate(Vec3::new(20.0, 0.0, 50.0), reach).is_none());
    }

    /// A scenario's kinds spawn in turn at the plaza's twelve, and it has to name one.
    #[test]
    fn the_built_district_spawns_the_kinds_it_is_given() {
        let built = built(&["warrior", "lancer", "striker"]).unwrap();
        let kinds: Vec<&str> = (0..ENEMIES)
            .map(|index| {
                let name = enemy_spawner(index);
                built.document.spawners.iter().find(|spawner| spawner.name == name).unwrap().spawns.as_str()
            })
            .collect();
        assert_eq!(kinds[..4], ["warrior", "lancer", "striker", "warrior"]);
        assert_eq!(built.document.colliders, Some(colliders_id()));
        assert_eq!(built.encounters, [(encounter_id(), encounter())]);
        assert!(super::built(&[]).is_err());
    }
}
