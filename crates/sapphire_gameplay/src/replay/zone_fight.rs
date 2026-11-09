//! What the stage needs of a scenario that stands in a zone of its own (M3 Q3, C2): which zone, where its two
//! fighters stand, and the colliders its simulation stands on. Its document says all of it
//! (`ScenarioDocument::stands_in_zone`).

use super::scenario::ScenarioError;
use super::{SCENARIOS, find_scenario};
use crate::arena::ArenaSpawns;

/// What the stage needs of a fight that stands in a zone: which zone, where the scenario stands its two
/// fighters, and the colliders its simulation stands on, cooked, for the stage to hold its pack's to.
pub struct ZoneFight {
    pub stem: &'static str,
    /// The spawner the player stands at, by name, where the zone stage stands the character first.
    pub player_at: &'static str,
    pub spawns: ArenaSpawns,
    pub colliders: sapphire_serialize::cooked::AlignedVec,
}

/// [`ZoneFight`] for the scenario named `scenario`, if it stands in a zone: a scenario document whose zone
/// stands on colliders of its own, which says the rest.
pub fn zone_fight(scenario: &str) -> Option<Result<ZoneFight, ScenarioError>> {
    let document = find_scenario(&SCENARIOS, scenario)?.document()?;
    document.stands_in_zone()?;
    Some(document.zone_fight())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scenario document that walks a zone on its own ground stands there on a stage, so the zone bench can
    /// play it (M3 Z1): the zone by its stem, its first at their spawner, its colliders the zone's. One on the
    /// arena's colliders stands in the town, its zone the simulation's alone, and one that starts at a
    /// checkpoint is refused, naming it.
    #[test]
    fn a_walk_of_a_zone_stands_on_its_ground_and_one_on_the_arenas_doesnt() {
        let court = zone_fight("ward-court").expect("the court").expect("it stands");
        assert_eq!((court.stem, court.player_at), ("lantern_ward", "checkpoint_court"));
        let walk = zone_fight("lantern-walk").expect("it walks the ward").expect("it stands");
        assert_eq!((walk.stem, walk.player_at), ("lantern_ward", "player_start"));
        assert_eq!(walk.spawns.player.position, [0.0, 0.0, 14.0]);
        assert_eq!(walk.colliders.as_slice(), court.colliders.as_slice(), "the ward's own");
        let yard = zone_fight("cart-yard").expect("it walks the proving ground").expect("it stands");
        assert_eq!(yard.stem, "proving_ground");
        assert!(zone_fight("square-street").is_none(), "it's on the arena's colliders");
        assert!(zone_fight("arena-fight").is_none());
        assert!(zone_fight("no-such-scenario").is_none());
        let Some(Err(error)) = zone_fight("ground-platform") else { panic!("it starts at a checkpoint") };
        assert!(error.to_string().contains("no spawner"), "{error}");
    }
}
