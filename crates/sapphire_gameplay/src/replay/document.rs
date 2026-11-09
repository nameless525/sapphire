//! Scenarios as documents (M3 B2): `content/scenarios/<name>.scenario.ron`.
//!
//! ```ron
//! (
//!     type: "sapphire::gameplay::ScenarioDocument",
//!     version: 1,
//!     data: (
//!         about: "the player at the proving ground's cart yard, where three waves come at it",
//!         max_actors: 8,
//!         colliders: "content/zones/proving_ground/proving_ground.colliders.ron",
//!         zone: Some("content/zones/proving_ground/proving_ground.zone.ron"),
//!         encounters: ["content/zones/proving_ground/cart_yard.encounter.ron"],
//!         moves: ["content/combat/player/player.moves.ron", "content/combat/enemy/warrior.moves.ron"],
//!         cast: [(moves: "player", at: "player_start", faction: 1, soft_lock: true)],
//!     ),
//! )
//! ```
//!
//! **To add one,** write the document. `build.rs` compiles in each of them and all they name, so a replay
//! needs no assets and a headless runner on any OS has them, and no Rust changes. A replay records a scenario's
//! name, and its `tables_hash` holds what that name meant: a moved spawner or a saved encounter says so rather
//! than playing on. A bad document fails [`super::SCENARIOS`]' first reader, naming the file and the field.
//!
//! **A set of moves' name** is its file's, before `.moves.ron`. That's what a cast member's `moves` and a
//! zone's spawner's `spawns` say.
//!
//! **Where someone stands:** `at` is one of the zone's spawners, in a scenario that has a zone, and otherwise
//! one of the arena's spawn points (`player`, `enemy`, `crowd_0`...).
//!
//! **A zone here is what the simulation reads of its document** ([`ZoneDocument::simulation`]) with no
//! navmesh and no presentation, archived as a cook archives it, so it goes in through the same door as a pack's
//! `.szone` ([`Simulation::set_zone`]), and `tools/sapphire_pipeline/tests/encounters.rs` holds the two to one
//! `tables_hash`. A scenario that wants paths on a zone takes its navmesh some other way, which nothing does
//! yet (P3 uses the arena's): a bake takes the pipeline.

use bevy_reflect::Reflect;
use sapphire_colliders::{ColliderDocument, ColliderSet};
use sapphire_math::Vec3;
use sapphire_moves::{MoveSet, MoveSetDocument, MoveSetError};
use sapphire_physics::PhysicsWorld;
use sapphire_reflect::Versioned;
use sapphire_serialize::AssetId;
use sapphire_serialize::cooked::AlignedVec;
use sapphire_zone::{EncounterDocument, Zone, ZoneDocument, ZonePresentation};
use serde::{Deserialize, Serialize};

use super::scenario::ScenarioError;
use crate::actors::{ActorId, ActorSpawn};
use crate::arena::SpawnPoint;
use crate::controller::ControllerProfile;
use crate::encounter::ZoneCast;
use crate::faction::Faction;
use crate::moves::WarpTarget;
use crate::party::PartyTuning;
use crate::simulation::{SimConfig, Simulation};
use crate::stats::{BUDGET_DESKTOP_LOW, PHYSICS_BUDGET_DESKTOP_LOW};

include!(concat!(env!("OUT_DIR"), "/scenario_documents.rs"));

/// A scenario document (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::gameplay"]
#[serde(deny_unknown_fields)]
pub struct ScenarioDocument {
    /// A line on what it holds, for listings.
    pub about: String,
    /// What a new recording seeds with. A replay keeps its own.
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// The pool a new recording makes.
    pub max_actors: u32,
    /// Its collision world: a `.colliders.ron`, from the repository's root. If its zone names colliders, these
    /// must be those.
    pub colliders: String,
    /// A `.zone.ron`, from the repository's root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
    /// The `.encounter.ron`s the zone lists, from the repository's root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub encounters: Vec<String>,
    /// The documents its move sets come from, from the repository's root, in the order they're added.
    pub moves: Vec<String>,
    /// Who stands there at the start, in spawn order.
    pub cast: Vec<CastMember>,
    /// Makes some of the cast a party.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub party: Option<CastParty>,
    /// Each of the cast aims its warps at the next, round the ring.
    #[serde(default)]
    pub warps_round: bool,
}

fn default_seed() -> u64 {
    7
}

impl Versioned for ScenarioDocument {
    /// 1: M3 B2.
    const VERSION: u32 = 1;
}

/// Someone a scenario starts with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::gameplay"]
#[serde(deny_unknown_fields)]
pub struct CastMember {
    /// A set of moves' name: see the [module docs](self).
    pub moves: String,
    /// Where they stand: see the [module docs](self).
    pub at: String,
    /// Their side. 0, the default, is against everyone.
    #[serde(default)]
    pub faction: u8,
    /// They choose a target as a player does, and a zone's triggers watch them.
    #[serde(default)]
    pub soft_lock: bool,
    /// They think for themselves.
    #[serde(default)]
    pub brain: bool,
    /// A jump from standing waits out its preparation (`Simulation::prepare_standing_jump`), as the arena's
    /// player's does.
    #[serde(default)]
    pub standing_jump: bool,
}

/// The cast's party, by places in the cast, the first on the field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::gameplay"]
#[serde(deny_unknown_fields)]
pub struct CastParty {
    pub members: Vec<usize>,
    pub swap_cooldown: u32,
    pub rest: u32,
}

/// `document`, as this build holds it.
fn named(document: &str) -> Result<&'static str, ScenarioError> {
    NAMED_DOCUMENTS
        .iter()
        .find(|(path, _)| *path == document)
        .map(|(_, text)| *text)
        .ok_or_else(|| invalid(document, "this build doesn't hold it"))
}

fn invalid(field: impl Into<String>, problem: impl Into<String>) -> ScenarioError {
    ScenarioError::Document { field: field.into(), problem: problem.into() }
}

/// The ID in the sidecar beside `document`. A sidecar's importer is the cooker's business, so this reads the
/// one line and nothing else of it.
fn id_of(document: &str) -> Result<AssetId, ScenarioError> {
    let path = format!("{document}.meta");
    id_in(named(&path)?, &path)
}

/// Encounter `id`, cooked as a pack holds it, from the document whose sidecar names it: a zone that lists it
/// stands a fight there, and a scenario that names it is how this build holds it (M3 W4's trip into B2's yard).
pub(super) fn cooked_encounter(id: AssetId) -> Result<AlignedVec, ScenarioError> {
    for (path, text) in NAMED_DOCUMENTS {
        let Some(document) = path.strip_suffix(".meta") else { continue };
        if document.ends_with(".encounter.ron") && id_in(text, path)? == id {
            return Ok(EncounterDocument::from_ron(named(document)?)?.cook()?.to_cooked()?);
        }
    }
    Err(invalid(format!("encounter {id}"), "no encounter document this build holds has that ID"))
}

/// The ID in a sidecar's `text`, from `path`.
fn id_in(text: &str, path: &str) -> Result<AssetId, ScenarioError> {
    let id = text.split("id:").nth(1).and_then(|rest| rest.split('"').nth(1));
    let id = id.ok_or_else(|| invalid(path, "it has no `id`"))?;
    id.parse().map_err(|_| invalid(path, format!("`{id}` isn't an ID")))
}

/// The pack of the zone whose sidecar holds `id`, as a cook names it, among the zones this build holds.
fn zone_pack(id: AssetId) -> Result<Option<String>, ScenarioError> {
    for (stem, sidecar) in ZONE_SIDECARS {
        if id_in(sidecar, stem)? == id {
            return Ok(Some(sapphire_zone::pack_name(stem)));
        }
    }
    Ok(None)
}

/// The side a document's number means: 0 is none.
const fn side_of(number: u8) -> Option<Faction> {
    if number == 0 { Some(Faction::NONE) } else { Faction::new(number) }
}

/// A set of moves' name, from its document's path: see the [module docs](self).
pub(super) fn kind_of(document: &str) -> &str {
    let file = super::scenario::document_file_name(document);
    file.strip_suffix(".moves.ron").unwrap_or(file)
}

/// The set of moves named `kind`, if this build holds it cooked: a cook bakes tables from clips, which takes
/// the pipeline, and nothing that runs a replay links that, so the simulation keeps `baked/<kind>.smoves`.
/// `tools/sapphire_pipeline/tests/player_moves.rs` holds the player's to its document.
pub(super) fn baked_moves(kind: &str) -> Option<Result<MoveSet, MoveSetError>> {
    let (_, bytes) = BAKED_MOVES.iter().find(|(name, _)| *name == kind)?;
    let mut aligned = AlignedVec::<16>::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    Some(MoveSet::from_cooked(&aligned))
}

/// The moves in `document`, cooked in advance if this build holds them that way.
fn moves_of(document: &str) -> Result<MoveSet, ScenarioError> {
    match baked_moves(kind_of(document)) {
        Some(set) => Ok(set?),
        None => Ok(MoveSetDocument::parse(named(document)?)?.compile()?),
    }
}

/// Every scenario document this build holds, read: its name and what it says. A bad one stops here, saying
/// which and why, the first time anyone asks for a scenario.
pub(super) fn read_all() -> Vec<(&'static str, ScenarioDocument)> {
    SCENARIO_DOCUMENTS
        .iter()
        .map(|(name, text)| {
            let document: ScenarioDocument = sapphire_serialize::from_ron(text)
                .unwrap_or_else(|error| panic!("content/scenarios/{name}.scenario.ron: {error}"));
            document.check().unwrap_or_else(|error| panic!("content/scenarios/{name}.scenario.ron: {error}"));
            (*name, document)
        })
        .collect()
}

impl ScenarioDocument {
    /// Checks everything that needs nothing outside the document, naming the field.
    pub fn check(&self) -> Result<(), ScenarioError> {
        if self.about.is_empty() {
            return Err(invalid("about", "say what it holds"));
        }
        if self.max_actors == 0 {
            return Err(invalid("max_actors", "there's nobody for it to hold"));
        }
        if self.moves.is_empty() {
            return Err(invalid("moves", "a fight has moves"));
        }
        if self.cast.is_empty() {
            return Err(invalid("cast", "input has to have someone to drive"));
        }
        if self.zone.is_none() && !self.encounters.is_empty() {
            return Err(invalid("encounters", "an encounter stands in a zone, and this has none"));
        }
        for (index, member) in self.cast.iter().enumerate() {
            if !self.moves.iter().any(|path| kind_of(path) == member.moves) {
                let problem = format!("`{}` isn't one of `moves`", member.moves);
                return Err(invalid(format!("cast[{index}].moves"), problem));
            }
            if side_of(member.faction).is_none() {
                return Err(invalid(format!("cast[{index}].faction"), "that's no side"));
            }
        }
        if let Some(party) = &self.party {
            for (index, &member) in party.members.iter().enumerate() {
                if member >= self.cast.len() {
                    return Err(invalid(format!("party.members[{index}]"), "there's nobody at that place"));
                }
            }
        }
        Ok(())
    }

    /// What a new recording of it starts from.
    pub(super) fn config(&self) -> SimConfig {
        SimConfig {
            seed: self.seed,
            max_actors: self.max_actors,
            step_budget: BUDGET_DESKTOP_LOW,
            physics_budget: PHYSICS_BUDGET_DESKTOP_LOW,
        }
    }

    /// Makes a world of it, the same way every run, and returns who input drives: the cast, without a
    /// party's members past the first.
    pub(super) fn set_up(&self, sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
        let colliders = ColliderDocument::from_ron(named(&self.colliders)?)?.cook()?.to_cooked()?;

        let zone = self.zone.as_deref().map(zone_of).transpose()?;
        let names_colliders = zone.as_ref().is_some_and(|(document, _)| document.colliders.is_some());
        if !names_colliders {
            let set = ColliderSet::read(&colliders)?;
            sim.set_physics_world(PhysicsWorld::from_cooked(set, sim.body_capacity())?)?;
        }
        // The player's moves name their ultimate's cinematic, which a fight loads before them, as `fight_on`
        // does (M3 Q3b).
        if self.moves.iter().any(|path| kind_of(path) == "player") {
            sim.add_cinematic(super::ULTIMATE_CINEMATIC, super::ultimate_cinematic()?)?;
        }
        let controller = Some(sim.add_controller_profile(&ControllerProfile::default())?);
        let mut sets = Vec::with_capacity(self.moves.len());
        for path in &self.moves {
            sets.push((kind_of(path), sim.add_move_set(moves_of(path)?)));
        }

        if let Some((document, cooked)) = &zone {
            self.give_zone(sim, document, cooked, &colliders, ZoneCast { moves: &sets, controller })?;
        }

        let mut cast = Vec::with_capacity(self.cast.len());
        for (index, member) in self.cast.iter().enumerate() {
            let field = || format!("cast[{index}].at");
            let moves = sets.iter().find(|(kind, _)| *kind == member.moves).map(|&(_, id)| id);
            let (position, yaw) = match zone.as_ref() {
                Some((document, _)) => {
                    let spawner = document.spawners.iter().find(|spawner| spawner.name == member.at);
                    let spawner = spawner.ok_or_else(|| invalid(field(), "the zone has no such spawner"))?;
                    (spawner.position, spawner.yaw_degrees.to_radians())
                }
                None => {
                    let point =
                        arena_point(&member.at).ok_or_else(|| invalid(field(), "no such spawn point"))?;
                    (point.position, point.yaw())
                }
            };
            let actor = sim.spawn(ActorSpawn {
                position: Vec3::from_array(position),
                yaw,
                moves,
                controller,
                faction: side_of(member.faction).expect("`check` looked at it"),
            })?;
            // A scenario's fight starts with every meter full, as a built-in one does (M3 Q3c), so a full
            // meter stays out of the checksum. A set with none has nothing to fill.
            sim.fill_energy(actor);
            cast.push(actor);
        }

        if let Some(party) = &self.party {
            let members: Vec<ActorId> = party.members.iter().map(|&place| cast[place]).collect();
            let tuning = PartyTuning { swap_cooldown: party.swap_cooldown, rest: party.rest };
            sim.form_party(&members, tuning)?;
        }
        for (member, &actor) in self.cast.iter().zip(&cast) {
            if member.soft_lock {
                sim.give_soft_lock(actor);
            }
            if member.standing_jump {
                sim.prepare_standing_jump(actor);
            }
        }
        // An edit may have taken a brain away (`LiveMoves::start`), and then that one stands there.
        for (member, &actor) in self.cast.iter().zip(&cast) {
            if member.brain {
                sim.give_brain(actor);
            }
        }
        if self.warps_round {
            for (index, &actor) in cast.iter().enumerate() {
                sim.set_warp_target(actor, WarpTarget::Actor(cast[(index + 1) % cast.len()]));
            }
        }

        let past_the_first = self.party.iter().flat_map(|party| party.members.iter().skip(1));
        let left_out: Vec<usize> = past_the_first.copied().collect();
        let driven = cast.iter().enumerate().filter(|(place, _)| !left_out.contains(place));
        Ok(driven.map(|(_, &actor)| actor).collect())
    }

    /// Hands a simulation the zone: its cooked encounters by ID, and the colliders, as a pack would.
    fn give_zone(
        &self,
        sim: &mut Simulation,
        document: &ZoneDocument,
        cooked: &AlignedVec,
        colliders: &AlignedVec,
        cast: ZoneCast<'_>,
    ) -> Result<(), ScenarioError> {
        let mut bytes: Vec<(AssetId, AlignedVec)> = Vec::with_capacity(self.encounters.len() + 1);
        if let Some(id) = document.colliders {
            if id_of(&self.colliders)? != id {
                return Err(invalid("colliders", "that isn't the set the zone names"));
            }
            bytes.push((id, colliders.clone()));
        }
        for path in &self.encounters {
            let id = id_of(path)?;
            if !document.encounters.contains(&id) {
                return Err(invalid(format!("encounters: {path}"), "the zone doesn't list it"));
            }
            let encounter = EncounterDocument::from_ron(named(path)?)?;
            bytes.push((id, encounter.cook()?.to_cooked()?));
        }
        for id in &document.encounters {
            if !bytes.iter().any(|(held, _)| held == id) {
                return Err(invalid("encounters", format!("the zone lists {id}, and this doesn't")));
            }
        }
        let zone = Zone::read(cooked)?;
        sim.set_zone(zone, cast, |id| {
            bytes.iter().find(|(held, _)| *held == id).map(|(_, bytes)| bytes.as_slice())
        })?;
        Ok(())
    }
}

/// The arena's spawn point by name.
fn arena_point(name: &str) -> Option<SpawnPoint> {
    let spawns = crate::arena::spawns();
    match name {
        "player" => Some(spawns.player),
        "enemy" => Some(spawns.enemy),
        _ => spawns.crowd.get(name.strip_prefix("crowd_")?.parse::<usize>().ok()?).copied(),
    }
}

/// A zone's document, and what the simulation reads of it archived as a cook archives it.
fn zone_of(path: &str) -> Result<(ZoneDocument, AlignedVec), ScenarioError> {
    let document = ZoneDocument::from_ron(named(path)?)?;
    let mut simulation = document.simulation()?;
    // An exit's pack is the pack of the zone it leads to, which a cook finds by its ID (M3 W4).
    for exit in &mut simulation.exits {
        let pack = zone_pack(exit.to)?;
        exit.pack =
            pack.ok_or_else(|| invalid(format!("exits `{}`.to", exit.name), "no zone has that ID"))?;
    }
    let zone = Zone {
        simulation,
        presentation: ZonePresentation {
            cell_size: document.grid.cell_size,
            backdrop: None,
            cells: Vec::new(),
            instances: Vec::new(),
            lights: Vec::new(),
            probes: None,
            day: None,
            rigs: None,
            splines: Vec::new(),
        },
    };
    let cooked = zone.to_cooked()?;
    Ok((document, cooked))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field's name, and what spoils it.
    type Spoiled = (&'static str, fn(&mut ScenarioDocument));

    fn arena_fight() -> ScenarioDocument {
        sapphire_serialize::from_ron(
            SCENARIO_DOCUMENTS.iter().find(|(name, _)| *name == "arena-fight").expect("held").1,
        )
        .expect("reads")
    }

    fn cant_start(document: &ScenarioDocument) -> String {
        let mut sim = Simulation::new(document.config());
        document.set_up(&mut sim).expect_err("it can't").to_string()
    }

    /// Each thing that can be wrong says where, and these are what a document's author has to go on.
    #[test]
    fn a_bad_document_is_refused_naming_the_field() {
        let cases: [Spoiled; 7] = [
            ("about", |d| d.about.clear()),
            ("max_actors", |d| d.max_actors = 0),
            ("moves", |d| d.moves.clear()),
            ("cast", |d| d.cast.clear()),
            ("encounters", |d| d.encounters = vec!["x.encounter.ron".to_owned()]),
            ("cast[1].moves", |d| d.cast[1].moves = "dancer".to_owned()),
            ("cast[1].faction", |d| d.cast[1].faction = 17),
        ];
        for (field, spoil) in cases {
            let mut document = arena_fight();
            spoil(&mut document);
            let error = document.check().err().unwrap_or_else(|| panic!("{field} passed"));
            assert!(error.to_string().contains(&format!("`{field}`")), "{field}: {error}");
        }
        let mut document = arena_fight();
        document.party = Some(CastParty { members: vec![9], swap_cooldown: 1, rest: 1 });
        let error = document.check().expect_err("nobody's there");
        assert!(error.to_string().contains("`party.members[0]`"), "{error}");
    }

    #[test]
    fn what_it_names_has_to_be_there() {
        let mut document = arena_fight();
        document.colliders = "nowhere.colliders.ron".to_owned();
        assert!(cant_start(&document).contains("`nowhere.colliders.ron`"));

        let mut document = arena_fight();
        document.cast[1].at = "nowhere".to_owned();
        assert!(cant_start(&document).contains("`cast[1].at`"));

        // A zone's spawners, in a scenario with a zone, rather than the arena's points.
        let waves = SCENARIO_DOCUMENTS.iter().find(|(name, _)| *name == "arena-waves").expect("held").1;
        let waves: ScenarioDocument = sapphire_serialize::from_ron(waves).expect("reads");
        let mut document = waves.clone();
        document.cast[0].at = "player".to_owned();
        assert!(cant_start(&document).contains("`cast[0].at`"));

        // A zone lists its encounters, and a scenario has to bring just those.
        let mut document = waves;
        document.encounters.clear();
        assert!(cant_start(&document).contains("`encounters`"));
    }

    #[test]
    fn a_field_nobody_knows_is_refused() {
        let text = r#"(type: "sapphire::gameplay::ScenarioDocument", version: 1, data: (
            about: "a", max_actors: 2, colliders: "c", moves: [], cast: [], tokens: 3,
        ))"#;
        let error = sapphire_serialize::from_ron::<ScenarioDocument>(text).expect_err("no such field");
        assert!(error.to_string().contains("tokens"), "{error}");
    }

    /// Everything a scenario names comes out of the one list, so `build.rs` found it, and every set of moves
    /// with a cooked copy reads as the same set from it as from `SCENARIOS`' reader.
    #[test]
    fn the_build_holds_what_every_scenario_names() {
        for (name, text) in SCENARIO_DOCUMENTS {
            let document: ScenarioDocument = sapphire_serialize::from_ron(text).expect("reads");
            document.check().unwrap_or_else(|error| panic!("{name}: {error}"));
            for path in document.moves.iter().chain([&document.colliders]) {
                assert!(named(path).is_ok(), "{name}: {path}");
            }
        }
    }
}
