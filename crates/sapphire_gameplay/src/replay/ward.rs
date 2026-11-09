//! The Lantern Ward's court (M3 Q3): the zone's own collision world, the player at the court's checkpoint and
//! the warrior who keeps it at the far end, and the cutscene at its gate loaded for the fight to open on.
//!
//! Everything comes from the ward's documents as this build holds them, read the way a cook reads them, so a
//! replay needs no assets and moving a spawner or a wall in the zone moves this scenario with it. It's code
//! for now because scenarios are; it says what a scenario document would (the colliders, the zone, its
//! cast at the zone's spawners, and its timelines), so it becomes one when they're documents.

use sapphire_colliders::ColliderDocument;
use sapphire_timeline::TimelineDocument;
use sapphire_zone::ZoneDocument;

use super::scenario::{
    ScenarioError, fight_on, player_moves, ultimate_cinematic, warrior_enemy, warrior_moves,
};
use crate::actors::ActorId;
use crate::arena::{ArenaSpawns, SpawnPoint};
use crate::simulation::Simulation;
use crate::timeline::TimelineId;

/// The scenario's name in replay files.
pub const WARD_COURT: &str = "ward-court";

/// The zone it stands in, by the stem the app's content finds it by.
pub const WARD_ZONE_STEM: &str = "lantern_ward";

/// The zone's document, from the repository's root, and as this build holds it.
pub const WARD_ZONE_PATH: &str = "content/zones/lantern_ward/lantern_ward.zone.ron";
const WARD_ZONE: &str = include_str!("../../../../content/zones/lantern_ward/lantern_ward.zone.ron");

/// The zone's collision world, from the repository's root, and as this build holds it.
pub const WARD_COLLIDERS_PATH: &str = "content/zones/lantern_ward/lantern_ward.colliders.ron";
const WARD_COLLIDERS: &str =
    include_str!("../../../../content/zones/lantern_ward/lantern_ward.colliders.ron");

/// The court's gate cutscene, from the repository's root, and as this build holds it.
pub const COURT_GATE_PATH: &str = "content/zones/lantern_ward/court_gate.timeline.ron";
const COURT_GATE: &str = include_str!("../../../../content/zones/lantern_ward/court_gate.timeline.ron");

/// The court's gate cutscene, the only timeline the scenario loads.
pub const COURT_GATE_ID: TimelineId = TimelineId::from_raw(0);

/// The step the court's gate cutscene comes before: half a second in, with the player standing at the
/// checkpoint and the warrior not yet near. A stand-in for the zone's trigger at the court's threshold, until
/// a trigger can start a timeline; what plays the fight live and what its scripts record both go by it.
pub const COURT_GATE_TICK: u32 = 30;

/// The zone's spawner the player stands at, and the one the warrior does.
pub const PLAYER_SPAWNER: &str = "checkpoint_court";
pub const WARRIOR_SPAWNER: &str = "court_spawn_e";

/// Where the ward's court puts its two fighters, from the zone's spawners.
pub fn ward_spawns() -> Result<ArenaSpawns, ScenarioError> {
    let zone = ZoneDocument::from_ron(WARD_ZONE)?;
    let point = |name: &'static str| {
        zone.spawners
            .iter()
            .find(|spawner| spawner.name == name)
            .map(|spawner| SpawnPoint { position: spawner.position, yaw_degrees: spawner.yaw_degrees })
            .ok_or(ScenarioError::MissingSpawner(name))
    };
    let enemy = point(WARRIOR_SPAWNER)?;
    // The court fights one warrior, so it has no crowd: any would stand where that one does.
    Ok(ArenaSpawns { player: point(PLAYER_SPAWNER)?, enemy, crowd: [enemy; 6], patrols: Vec::new() })
}

/// The ward's colliders, cooked as a pack holds them.
pub fn ward_colliders() -> Result<sapphire_serialize::cooked::AlignedVec, ScenarioError> {
    Ok(ColliderDocument::from_ron(WARD_COLLIDERS)?.cook()?.to_cooked()?)
}

/// What the stage needs of a fight that stands in a zone: which zone, where the scenario stands its two
/// fighters, and the colliders its simulation stands on, cooked, for the stage to hold its pack's to.
pub struct ZoneFight {
    pub stem: &'static str,
    /// The spawner the player stands at, by name, where the zone stage stands the character first.
    pub player_at: &'static str,
    pub spawns: ArenaSpawns,
    pub colliders: sapphire_serialize::cooked::AlignedVec,
}

/// [`ZoneFight`] for the scenario named `scenario`, if it stands in a zone.
pub fn zone_fight(scenario: &str) -> Option<Result<ZoneFight, ScenarioError>> {
    (scenario == WARD_COURT).then(|| {
        Ok(ZoneFight {
            stem: WARD_ZONE_STEM,
            player_at: PLAYER_SPAWNER,
            spawns: ward_spawns()?,
            colliders: ward_colliders()?,
        })
    })
}

/// The scenario's setup: the cutscene first, then the fight as `arena-fight` sets one up, on the ward's
/// ground and at its spawners. The gate is loaded first, so it's timeline 0, before the ultimate's cinematic.
pub fn ward_court(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let gate = TimelineDocument::from_ron(COURT_GATE)?.cook()?;
    sim.add_timeline(gate.simulation)?;
    let cooked = ward_colliders()?;
    let (player, ultimate, warrior) = (player_moves()?, ultimate_cinematic()?, warrior_moves()?);
    fight_on(sim, player, &ultimate, warrior, &warrior_enemy()?.mind, &cooked, ward_spawns()?, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::{SCENARIOS, find_scenario};

    #[test]
    fn the_court_stands_its_fighters_at_the_zones_spawners_facing_each_other() {
        let spawns = ward_spawns().expect("the ward names both spawners");
        assert_eq!(spawns.player.position, [0.0, 0.0, -63.0]);
        assert_eq!(spawns.enemy.position, [0.0, 0.0, -90.0]);
        // 0 faces +Z: the player faces up the court, the warrior back down it.
        assert_eq!((spawns.player.yaw_degrees, spawns.enemy.yaw_degrees), (180.0, 0.0));
    }

    #[test]
    fn the_scenario_loads_the_gate_and_says_its_zone() {
        let scenario = find_scenario(&SCENARIOS, WARD_COURT).expect("listed");
        assert_eq!(scenario.zone(), Some(WARD_ZONE_STEM));
        let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
        assert_eq!(cast.len(), 2);
        sim.play_timeline(COURT_GATE_ID).expect("the gate is the first timeline");
        assert!(sim.timeline().is_some());
    }
}
