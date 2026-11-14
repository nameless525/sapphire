//! The benchmark district's simulation half (M5 B2): its spawners, the plaza's encounter volume, its
//! checkpoint and the plaza's fight, built here in code as the rest of this crate builds its zone.
//!
//! The district itself is generated: `sapphire_pipeline`'s `bench::district` writes its streets, buildings,
//! props, lights and tour from `bench/district/` into a zone document that cooks like any other. What a
//! simulation reads of it stays compiled in (m5-plan §3, "The simulation's data"), so a fight on the
//! district builds it from here rather than from a pack nothing ships: [`simulation`] is the cooked half,
//! and the generator writes [`spawners`], [`triggers`], [`checkpoints`] and [`encounter`] into its documents.
//! `sapphire_pipeline`'s district tests hold the two to each other: the generated document's simulation is
//! [`simulation`], byte for byte, and everything here stands on open ground the generator left clear.
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
//! What this doesn't build: colliders and a navmesh, which the zone names none of, as this crate's other
//! zone doesn't. A fight on the district needs ground under it; that's the fight's package to add.

use sapphire_serialize::AssetId;
use sapphire_zone::grid::Grid;
use sapphire_zone::{
    Checkpoint, EncounterDocument, Spawner, Trigger, Volume, Wave, WaveStart, ZoneDocument, ZoneSimulation,
};

/// A cell's side, in metres: the district's grid.
pub const CELL_SIZE: f32 = 32.0;
/// Half the plaza's side, in metres: the four cells about the origin.
pub const PLAZA_HALF: f32 = CELL_SIZE;
/// The fewest cells a side: the plaza and a ring of blocks around it.
pub const MIN_CELLS: u32 = 4;
/// The spawner the player starts at.
pub const START: &str = "player_start";
/// The event the plaza's volume raises, which starts its fight.
pub const PLAZA_EVENT: &str = "plaza_entered";
/// How many enemies the plaza's fight brings, at once.
pub const ENEMIES: usize = 12;
/// How far in from the district's edge the player starts, in metres.
const START_INSET: f32 = 4.0;
/// Half the plaza volume's side: inside the streets that edge the plaza, so a walk up the avenue enters it
/// on the plaza's paving.
const VOLUME_HALF: f32 = 27.0;
/// The plaza fight's side.
const FACTION: u8 = 2;

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

/// The district's spawners: the player's start, where a fight starts the player at the plaza's edge, and
/// the plaza's twelve enemies, a warrior and a Lancer in turn.
#[must_use]
pub fn spawners(cells: u32) -> Vec<Spawner> {
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
    ];
    // Three rows of four across the plaza's middle, facing south (+Z), the way the player comes.
    for (index, (x, z)) in
        [-18.0_f32, -6.0, 6.0, 18.0].iter().flat_map(|&x| [-14.0_f32, -4.0, 6.0].map(|z| (x, z))).enumerate()
    {
        let kind = if index.is_multiple_of(2) { "warrior" } else { "lancer" };
        spawners.push(spawner(enemy_spawner(index), [x, 0.0, z], 0.0, kind));
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

/// The district's simulation half, as a cook of its generated document makes it: `encounter` is the
/// plaza fight's asset.
///
/// # Panics
///
/// If `cells` isn't a district's ([`check_cells`]).
#[must_use]
pub fn simulation(cells: u32, encounter: AssetId) -> ZoneSimulation {
    check_cells(cells).expect("a district's cells");
    let document = ZoneDocument {
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
        colliders: None,
        spawners: spawners(cells),
        triggers: triggers(),
        checkpoints: checkpoints(),
        splines: Vec::new(),
        navigation: None,
        encounters: vec![encounter],
        exits: Vec::new(),
    };
    document.simulation().expect("the district's simulation half is a valid zone's")
}

#[cfg(test)]
mod tests {
    use sapphire_zone::CookedVolume;

    use super::*;

    #[test]
    fn the_plaza_holds_its_fight_and_the_start_lies_outside_it() {
        for cells in [MIN_CELLS, 8, 16] {
            let simulation = simulation(cells, AssetId::from_text("4b0c8e31-5d27-4f6a-9a83-2e61c07db5f5"));
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
            let start = simulation.spawners.iter().find(|spawner| spawner.name == START).unwrap().position;
            assert!(
                !inside(start) && start[2] < half_side(cells),
                "the start is in the district, south of the plaza"
            );
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
}
