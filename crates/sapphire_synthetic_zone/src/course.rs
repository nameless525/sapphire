//! The traversal course's simulation half (M6 A14): a flat yard of boxes for the traversal rows to prove on,
//! built here in code as [`super::nature`]'s is. A15's mantle and vault, A16's climbing, A17's glide and A19's
//! swim each have a station, and a scenario on the zone (`content/scenarios/course.scenario.ron`) stands the
//! player at its start; [`built`] is what that scenario's simulation stands on.
//!
//! Nothing here enters a traversal mode: the course is the data the modes will be proved on, and today a
//! character walks it, jumps on its low ledges and falls into its pool like any other ground.
//!
//! # Where things are
//!
//! +X is east, +Z south, the ground's top at 0. The player starts at the south end ([`START`]) facing north,
//! and meets, going north:
//!
//! - **The ledges** ([`LEDGES`]): four blocks in a row across the course, 0.5, 1.0, 1.5 and 2.0 m tall, the
//!   heights A15 mantles. East of them, **the vault** ([`VAULT`]): a wall 1 m tall and 0.4 m deep.
//!   West of them, **the ledge with no room** ([`NO_ROOM`]): a 1 m ledge under a slab 1.2 m over its top, too
//!   low to stand under, where a mantle is refused and the press jumps.
//! - **The climbing wall** ([`CLIMB`]): a block [`TOWER_HEIGHT`] tall in `Climbable` scenery, whose south face
//!   is the wall and whose top is a platform to top out on. East of it, **the plain wall** ([`PLAIN`]), as tall
//!   and only `World`: a face climbing must refuse, since only tagged scenery climbs.
//! - **The glide drop**: the platform's north edge, [`TOWER_HEIGHT`] over **the pool** ([`POOL`]), a basin
//!   [`POOL_FLOOR`] down whose water lies at [`WATER_HEIGHT`], deep enough for the default `swim_depth` of
//!   any capsule a document may hold. North of the pool, the ground goes on to the course's end.
//!
//! # What a body meets
//!
//! Every block is a box collider ([`colliders`]): the ground around the pool's hole, the pool's floor, the
//! ledges, the vault, the two walls. The water is the zone's own ([`water`], W2's volumes), which nothing
//! collides with. There is no fight and no navmesh: nobody here walks by a brain.

use sapphire_colliders::{ColliderDocument, ColliderEntry, ColliderShape, Group, Rotation};
use sapphire_serialize::AssetId;
use sapphire_zone::grid::Grid;
use sapphire_zone::water::{WaterBody, WaterShape, WaterSurface};
use sapphire_zone::{BuiltZone, Checkpoint, Spawner, Volume, ZoneDocument, ZoneSimulation};

/// The name a scenario's `built` gives this zone.
pub const ZONE_NAME: &str = "course";
/// A cell's side, in metres: the zone's grid.
pub const CELL_SIZE: f32 = 32.0;
/// The spawner the player starts at.
pub const START: &str = "player_start";
/// Half the course's extent along X and Z, in metres: 48 by 96, the ground's whole top.
pub const HALF_EXTENT: [f32; 2] = [24.0, 48.0];
/// Where the player starts, `(x, z)`, facing north.
pub const START_AT: [f32; 2] = [0.0, 40.0];
/// The ledges' middles `(x, z)` and their heights, in metres.
pub const LEDGES: [([f32; 2], f32); 4] =
    [([-9.0, 24.0], 0.5), ([-5.0, 24.0], 1.0), ([-1.0, 24.0], 1.5), ([3.0, 24.0], 2.0)];
/// A ledge's half extents across and along the course, in metres: 1.5 m wide and 2 m deep.
pub const LEDGE_HALF: [f32; 2] = [1.5, 1.0];
/// The ledge with no room (M6 A15): its middle `(x, z)` and its height, west of [`LEDGES`] and as big as one, under
/// a slab [`NO_ROOM_GAP`] over its top, lower than the default 1.6 m capsule stands, so a mantle there is refused.
pub const NO_ROOM: ([f32; 2], f32) = ([-15.0, 24.0], 1.0);
/// How far over the no-room ledge's top its slab's underside is, and how thick the slab is, in metres.
pub const NO_ROOM_GAP: (f32, f32) = (1.2, 0.3);
/// The vault's middle `(x, z)`, its half extents across and along, and its height, in metres.
pub const VAULT: ([f32; 2], [f32; 2], f32) = ([12.0, 24.0], [2.0, 0.2], 1.0);
/// The climbing wall's block: its south face's span along X, and its extent along Z, in metres.
pub const CLIMB: ([f32; 2], [f32; 2]) = ([-6.0, 4.0], [0.0, 8.0]);
/// The plain wall's block, as [`CLIMB`]: the same height, beside it to the east.
pub const PLAIN: ([f32; 2], [f32; 2]) = ([8.0, 14.0], [0.0, 8.0]);
/// How tall both walls stand, in metres: the climb's height and the glide's drop.
pub const TOWER_HEIGHT: f32 = 8.0;
/// The pool's span along X and along Z, in metres: the basin cut in the ground north of the walls.
pub const POOL: ([f32; 2], [f32; 2]) = ([-12.0, 12.0], [-36.0, -8.0]);
/// The pool's floor, in metres.
pub const POOL_FLOOR: f32 = -3.0;
/// The pool's surface, in metres: a hand under the ground's top, 2.75 m over its floor.
pub const WATER_HEIGHT: f32 = -0.25;
/// How thick the pool's floor is, in metres. The ground's boxes reach down to its underside, so the hole's
/// sides are solid from under the floor to the ground's top, and nothing on the floor walks out under them.
const GROUND_DEPTH: f32 = 1.0;

/// The course's collider set, by the ID its generated document gives it: `course/course.colliders.ron`.
#[must_use]
pub fn colliders_id() -> AssetId {
    crate::BENCH_ROOT.sub_asset("course/course.colliders.ron")
}

/// A box collider in [`Group::World`] (and `extra`), from its corners `(x, y, z)`.
fn block(name: &str, min: [f32; 3], max: [f32; 3], extra: &[Group]) -> ColliderEntry {
    let middle = [0, 1, 2].map(|axis| (min[axis] + max[axis]) * 0.5);
    let half_extents = [0, 1, 2].map(|axis| (max[axis] - min[axis]) * 0.5);
    let mut groups = vec![Group::World];
    groups.extend_from_slice(extra);
    ColliderEntry {
        name: name.to_owned(),
        shape: ColliderShape::Box { half_extents },
        position: middle,
        rotation: Rotation::None,
        groups,
    }
}

/// The course's colliders: the ground around the pool's hole, the pool's floor, the ledges, the vault and the
/// two walls, the climbing wall's block alone `Climbable`.
#[must_use]
pub fn colliders() -> ColliderDocument {
    let [half_x, half_z] = HALF_EXTENT;
    let ([pool_x0, pool_x1], [pool_z0, pool_z1]) = POOL;
    let under = POOL_FLOOR - GROUND_DEPTH;
    let mut colliders = vec![
        // The ground in four pieces about the pool's hole: south and north of it the whole width, then west
        // and east of it along its length, each as deep as the pool's floor reaches, so they are its sides.
        block("ground_south", [-half_x, under, pool_z1], [half_x, 0.0, half_z], &[]),
        block("ground_north", [-half_x, under, -half_z], [half_x, 0.0, pool_z0], &[]),
        block("ground_west", [-half_x, under, pool_z0], [pool_x0, 0.0, pool_z1], &[]),
        block("ground_east", [pool_x1, under, pool_z0], [half_x, 0.0, pool_z1], &[]),
        block("pool_floor", [pool_x0, under, pool_z0], [pool_x1, POOL_FLOOR, pool_z1], &[]),
    ];
    for (index, ([x, z], height)) in LEDGES.into_iter().enumerate() {
        let [across, along] = LEDGE_HALF;
        colliders.push(block(
            &format!("ledge_{}", index + 1),
            [x - across, 0.0, z - along],
            [x + across, height, z + along],
            &[],
        ));
    }
    let ([x, z], height) = NO_ROOM;
    let [across, along] = LEDGE_HALF;
    let (gap, slab) = NO_ROOM_GAP;
    colliders.push(block(
        "ledge_no_room",
        [x - across, 0.0, z - along],
        [x + across, height, z + along],
        &[],
    ));
    let under = height + gap;
    colliders.push(block(
        "no_room_slab",
        [x - across, under, z - along],
        [x + across, under + slab, z + along],
        &[],
    ));
    let ([x, z], [across, along], height) = VAULT;
    colliders.push(block("vault", [x - across, 0.0, z - along], [x + across, height, z + along], &[]));
    for (name, ([x0, x1], [z0, z1]), extra) in
        [("climbing_wall", CLIMB, &[Group::Climbable][..]), ("plain_wall", PLAIN, &[][..])]
    {
        // A wall's block reaches north from its south face, the one a climb meets.
        colliders.push(block(name, [x0, 0.0, -z1], [x1, TOWER_HEIGHT, -z0], extra));
    }
    ColliderDocument { colliders }
}

/// The course's water: the pool, a rectangle over its basin, swum in.
#[must_use]
pub fn water() -> Vec<WaterBody> {
    let ([x0, x1], [z0, z1]) = POOL;
    vec![WaterBody {
        name: "pool".to_owned(),
        shape: WaterShape::Lake {
            outline: vec![[x0, z0], [x1, z0], [x1, z1], [x0, z1]],
            height: WATER_HEIGHT,
        },
        surface: WaterSurface::Liquid,
        material: None,
        swim: true,
        probe: None,
        shore_margin: 0.5,
    }]
}

/// The player's start, at the course's south end facing north.
#[must_use]
pub fn spawners() -> Vec<Spawner> {
    vec![Spawner {
        name: START.to_owned(),
        position: [START_AT[0], 0.0, START_AT[1]],
        yaw_degrees: 180.0,
        spawns: "player".to_owned(),
        patrol: None,
    }]
}

/// Where a walk of the course comes back to: its start, the climbing wall's top, and the pool's far side.
#[must_use]
pub fn checkpoints() -> Vec<Checkpoint> {
    let point = |name: &str, position: [f32; 3]| Checkpoint {
        name: name.to_owned(),
        position,
        yaw_degrees: 180.0,
        volume: Volume::Box { half_extents: [2.0, 1.5, 1.0] },
    };
    let ([x0, x1], [z0, z1]) = CLIMB;
    let ([_, _], [pool_north, _]) = POOL;
    vec![
        point("start", [START_AT[0], 0.0, START_AT[1]]),
        point("wall_top", [(x0 + x1) * 0.5, TOWER_HEIGHT, -(z0 + z1) * 0.5]),
        point("pool_far", [0.0, 0.0, pool_north - 4.0]),
    ]
}

/// The course's simulation half as a document.
#[must_use]
pub fn document() -> ZoneDocument {
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
        spawners: spawners(),
        triggers: Vec::new(),
        checkpoints: checkpoints(),
        splines: Vec::new(),
        navigation: None,
        encounters: Vec::new(),
        exits: Vec::new(),
        ambient: None,
        water: water(),
        rooms: Vec::new(),
        weather_plan: None,
        foliage: None,
    }
}

/// The course's simulation half, as a cook of its document makes it.
#[must_use]
pub fn simulation() -> ZoneSimulation {
    document().simulation().expect("the course's simulation half is a valid zone's")
}

/// The course a scenario stands on, a [`sapphire_zone::BuildZone`]: it has no fight, so a scenario on it names
/// no enemies.
///
/// # Errors
///
/// If `enemies` names any, or the colliders don't cook.
pub fn built(enemies: &[&str]) -> Result<BuiltZone, String> {
    if !enemies.is_empty() {
        return Err(format!(
            "the traversal course has no fight, and its scenario names enemies: {}",
            enemies.join(", ")
        ));
    }
    let colliders = colliders().cook().and_then(|set| set.to_cooked()).map_err(|error| error.to_string())?;
    Ok(BuiltZone { document: document(), encounters: Vec::new(), colliders, navmesh: None })
}

#[cfg(test)]
mod tests {
    use sapphire_colliders::{ColliderSet, GroupMask};
    use sapphire_zone::{Zone, ZoneGeometry};

    use super::*;

    /// The course's simulation digest, its colliders' cooked bytes in it: what a replay on the course rests
    /// on, generated the same every time and pinned. Search a job's log for `course digest`; a change to the
    /// course sets this to what it prints.
    #[test]
    fn the_course_is_generated_byte_for_byte_and_pinned() {
        use std::io::Write as _;

        let digest = || {
            let built = built(&[]).unwrap();
            let zone = built.document.cook(ZoneGeometry::default()).expect("cooks");
            let bytes = zone.to_cooked().expect("archives");
            let colliders = built.colliders;
            let digest = Zone::read(&bytes)
                .expect("reads")
                .simulation_digest(|id| (id == colliders_id()).then_some(&colliders[..]))
                .expect("digests");
            (digest, bytes.into_vec(), colliders.into_vec())
        };
        let (first, zone, colliders) = digest();
        let (again, zone_again, colliders_again) = digest();
        assert_eq!(
            (first, &zone, &colliders),
            (again, &zone_again, &colliders_again),
            "the same bytes twice"
        );
        let line =
            format!("sapphire_synthetic_zone course digest {first:#018x} on {}\n", std::env::consts::ARCH);
        std::io::stderr().write_all(line.as_bytes()).expect("stderr takes it");
        assert_eq!(first, 0x6434_99d5_72ab_976e, "the course's digest moved; {line}");
    }

    /// Only the climbing wall's block is climbable, and every block is scenery.
    #[test]
    fn only_the_climbing_wall_climbs() {
        let cooked = built(&[]).unwrap().colliders;
        let set = ColliderSet::read(&cooked).expect("reads");
        let climbable: Vec<&str> = set
            .colliders
            .iter()
            .filter(|collider| {
                GroupMask::from_bits(collider.memberships.to_native()).intersects(Group::Climbable.mask())
            })
            .map(|collider| collider.name.as_str())
            .collect();
        assert_eq!(climbable, ["climbing_wall"]);
        assert!(set.colliders.iter().all(|collider| {
            GroupMask::from_bits(collider.memberships.to_native()).intersects(Group::World.mask())
        }));
    }

    /// The pool is deep enough to swim at the default share of the tallest capsule a document may have, its
    /// water lies under the ground's top, the ledges are A15's heights and the drop is over a glide's least.
    #[test]
    fn the_pool_swims_and_the_ledges_span_a_mantles_heights() {
        let simulation = simulation();
        assert_eq!(simulation.water.len(), 1);
        const { assert!(WATER_HEIGHT < 0.0 && WATER_HEIGHT - POOL_FLOOR > 0.75 * 3.0, "deep enough") };
        let heights = LEDGES.map(|(_, height)| height);
        assert_eq!(heights, [0.5, 1.0, 1.5, 2.0], "A15's 0.5 to 2 m");
        const { assert!(TOWER_HEIGHT > 3.0, "over the default glide's least height") };
        let ([x0, x1], [z0, z1]) = POOL;
        assert!(x0 > -HALF_EXTENT[0] && x1 < HALF_EXTENT[0] && z0 > -HALF_EXTENT[1] && z1 < HALF_EXTENT[1]);
        // The walls' north edge is the pool's south edge: the glide drops off the platform over the water.
        assert_eq!(-CLIMB.1[1], z1);
        // The ledge with no room is a ledge a mantle takes but for the slab, which the default capsule can't stand
        // under, west of the others.
        let ([x, _], height) = NO_ROOM;
        assert!(height == 1.0 && NO_ROOM_GAP.0 < 1.6 && x + LEDGE_HALF[0] < LEDGES[0].0[0] - LEDGE_HALF[0]);
    }

    #[test]
    fn the_course_has_no_fight() {
        assert!(built(&["warrior"]).is_err());
        let built = built(&[]).unwrap();
        assert!(built.encounters.is_empty() && built.navmesh.is_none());
        assert_eq!(built.document.colliders, Some(colliders_id()));
        let start = &built.document.spawners[0];
        assert_eq!((start.name.as_str(), start.spawns.as_str()), (START, "player"));
    }
}
