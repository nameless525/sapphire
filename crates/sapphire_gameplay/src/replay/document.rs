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
//!         enemies: ["content/combat/enemy/warrior.enemy.ron"],
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
//! **A kind's name** is its file's, before `.moves.ron` for a set of moves or `.enemy.ron` for a kind of
//! enemy (M3 B3). That's what a cast member's `moves` and a zone's spawner's `spawns` say. A kind of enemy
//! plays the set its document names, which has to be one of `moves`: it's added beside that set, and a second
//! kind on the same moves adds them again, after the rest, so the sets `moves` lists keep their IDs. Who
//! thinks for themselves has to be a kind of enemy, whose brains they fight with.
//!
//! **Where someone stands:** `at` is one of the zone's spawners or checkpoints, in a scenario that has a
//! zone, and otherwise one of the arena's spawn points (`player`, `enemy`, `crowd_0`...).
//!
//! **A scenario can start where a save does** (M3 B4): when the first of its cast stands at a checkpoint, its
//! zone starts there, with `cleared`'s encounters already fought, through [`Simulation::start_at`], the call
//! a game's load makes. So a replay can start from what a save holds, and the walk from there is the walk a
//! loaded game takes.
//!
//! **A trip's zones** (M3 G1): `shelf` lists the zones a run can go to, each put on the simulation's shelf in
//! that order ([`Simulation::add_zone`]), so a run can go into one and back. Each has to be one the simulation
//! keeps a copy of ([`super::BAKED_ZONES`]): a zone on the shelf takes its navmesh, and a bake takes the
//! pipeline. Going back into the zone a run started in goes into its copy on the shelf, which starts that
//! zone over.
//!
//! **What a run can play and open** (M3 G1): `timelines` and `dialogues` list the cutscenes and conversations
//! a game's flow or a replay's commands can start, each loaded in its list's order, so the first is timeline 0
//! and dialogue 0: the timelines before the player's ultimate's cinematic, so that one comes after them.
//! [`ScenarioDocument::timelines_by_id`] and [`ScenarioDocument::dialogues_by_id`] say which is which by the
//! documents' IDs, which is how a flow names them. A dialogue's conditions are checked as its cook checks them,
//! by the pipeline: the simulation loads only its effects, cooked against the registry `flags` names.
//!
//! **A zone here is what the simulation reads of its document** ([`ZoneDocument::simulation`]) with no
//! navmesh and no presentation, archived as a cook archives it, so it goes in through the same door as a pack's
//! `.szone` ([`Simulation::set_zone`]), and `tools/sapphire_pipeline/tests/encounters.rs` holds the two to one
//! `tables_hash`. A scenario that wants paths on a zone takes its navmesh some other way, which nothing does
//! yet (P3 uses the arena's): a bake takes the pipeline.

use bevy_reflect::Reflect;
use sapphire_colliders::{ColliderDocument, ColliderSet};
use sapphire_dialogue::{DialogueDocument, FlagRegistry};
use sapphire_math::Vec3;
use sapphire_moves::{EnemyDocument, MoveSet, MoveSetDocument, MoveSetError};
use sapphire_physics::PhysicsWorld;
use sapphire_reflect::Versioned;
use sapphire_serialize::AssetId;
use sapphire_serialize::cooked::AlignedVec;
use sapphire_timeline::TimelineDocument;
use sapphire_zone::{EncounterDocument, Zone, ZoneDocument, ZonePresentation};
use serde::{Deserialize, Serialize};

use super::play::RunStart;
use super::scenario::ScenarioError;
use crate::actors::{ActorId, ActorSpawn};
use crate::arena::SpawnPoint;
use crate::checkpoint::{Place, ZoneStart};
use crate::controller::ControllerProfile;
use crate::dialogue::DialogueId;
use crate::encounter::ZoneCast;
use crate::faction::Faction;
use crate::moves::WarpTarget;
use crate::party::PartyTuning;
use crate::simulation::{SimConfig, Simulation};
use crate::stats::{BUDGET_DESKTOP_LOW, PHYSICS_BUDGET_DESKTOP_LOW};
use crate::timeline::TimelineId;

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
    /// Those of `encounters` fought before it starts, as a save says (see the [module docs](self)).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleared: Vec<String>,
    /// The documents its move sets come from, from the repository's root, in the order they're added.
    pub moves: Vec<String>,
    /// Its kinds of enemy: `.enemy.ron`s, from the repository's root, each on one of `moves`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enemies: Vec<String>,
    /// Who stands there at the start, in spawn order.
    pub cast: Vec<CastMember>,
    /// Makes some of the cast a party.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub party: Option<CastParty>,
    /// Each of the cast aims its warps at the next, round the ring.
    #[serde(default)]
    pub warps_round: bool,
    /// The zones a trip can go to (M3 W4, G1), `.zone.ron`s from the repository's root, each put on the shelf in
    /// this order: see the [module docs](self).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shelf: Vec<String>,
    /// The timelines a run can play, `.timeline.ron`s from the repository's root, loaded first and in this
    /// order (M3 G1): see the [module docs](self).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timelines: Vec<String>,
    /// The dialogues a run can open, `.dialogue.ron`s from the repository's root, loaded in this order (M3 G1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dialogues: Vec<String>,
    /// The flag registry `dialogues` cook against, a `.ron` from the repository's root: the game's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
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
    /// A kind's name, a set of moves' or a kind of enemy's: see the [module docs](self).
    pub moves: String,
    /// Where they stand: see the [module docs](self).
    pub at: String,
    /// Their side. 0, the default, is against everyone.
    #[serde(default)]
    pub faction: u8,
    /// They choose a target as a player does, and a zone's triggers watch them.
    #[serde(default)]
    pub soft_lock: bool,
    /// They think for themselves, as a kind of enemy.
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

/// A kind of enemy in a scenario's simulations: what a stage draws the fighters on `set` as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindOfEnemy {
    /// The set its fighters play.
    pub set: crate::moves::MoveSetId,
    /// Its `.enemy.ron`'s ID, which its cook has.
    pub kind: AssetId,
}

/// A script one of a scenario's kinds of enemy names (M3 C2), which this build holds: what binds it in a
/// fight is the script crate's, which gameplay can't see ([`super::ScenarioSetup::DocumentAnd`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentScript {
    /// From the repository's root: what hot reload watches.
    pub path: &'static str,
    /// Its file's name: what it's bound by, and what an edit of it goes by.
    pub file: &'static str,
    /// Its source, as this build holds it.
    pub source: &'static str,
}

/// One of a scenario's `enemies`, read, and where [`ScenarioDocument::set_up_from`] puts it.
struct PlannedEnemy {
    document: EnemyDocument,
    /// Its moves' place in `moves`.
    moves: usize,
    /// Another kind took those moves first, so it has a copy of them.
    again: bool,
    /// The number of the set it plays, in a simulation that had none before.
    set: u32,
}

/// The side a document's number means: 0 is none.
const fn side_of(number: u8) -> Option<Faction> {
    if number == 0 { Some(Faction::NONE) } else { Faction::new(number) }
}

/// A kind's name, from its document's path: see the [module docs](self).
pub(super) fn kind_of(document: &str) -> &str {
    let file = super::scenario::document_file_name(document);
    file.strip_suffix(".moves.ron").or_else(|| file.strip_suffix(".enemy.ron")).unwrap_or(file)
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
        for (index, path) in self.cleared.iter().enumerate() {
            if !self.encounters.contains(path) {
                return Err(invalid(format!("cleared[{index}]"), "that isn't one of `encounters`"));
            }
        }
        for (index, member) in self.cast.iter().enumerate() {
            let is_enemy = self.enemies.iter().any(|path| kind_of(path) == member.moves);
            if !is_enemy && !self.moves.iter().any(|path| kind_of(path) == member.moves) {
                let problem = format!("`{}` isn't one of `moves` or `enemies`", member.moves);
                return Err(invalid(format!("cast[{index}].moves"), problem));
            }
            if member.brain && !is_enemy {
                let problem = format!("`{}` isn't one of `enemies`, so it has no brain", member.moves);
                return Err(invalid(format!("cast[{index}].brain"), problem));
            }
            if side_of(member.faction).is_none() {
                return Err(invalid(format!("cast[{index}].faction"), "that's no side"));
            }
        }
        if !self.shelf.is_empty() && self.zone.is_none() {
            return Err(invalid("shelf", "a trip goes from a zone, and this has none"));
        }
        for (index, path) in self.shelf.iter().enumerate() {
            baked_zone(path).map_err(|problem| invalid(format!("shelf[{index}]"), problem))?;
        }
        if !self.dialogues.is_empty() && self.flags.is_none() {
            return Err(invalid("flags", "its dialogues cook against a flag registry, and it names none"));
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

    /// Each of its kinds of enemy, by its sidecar's ID, and the set its fighters play in a simulation this
    /// sets up: what draws them is that kind's look, which a stage reads from its cooked document (M3 B3b). The
    /// simulation never reads it.
    pub fn kinds_of_enemy(&self) -> Result<Vec<KindOfEnemy>, ScenarioError> {
        let planned = self.planned_enemies()?;
        let kinds = self.enemies.iter().zip(planned).map(|(path, plan)| {
            Ok(KindOfEnemy { set: crate::moves::MoveSetId(plan.set), kind: id_of(path)? })
        });
        kinds.collect()
    }

    /// Each of its timelines, by its sidecar's ID, and the timeline it is in a simulation this sets up: what a
    /// game's flow plays by ID (M3 G1).
    pub fn timelines_by_id(&self) -> Result<Vec<(AssetId, TimelineId)>, ScenarioError> {
        let ids = self.timelines.iter().enumerate().map(|(place, path)| {
            let place = u32::try_from(place).map_err(|_| invalid("timelines", "too many"))?;
            Ok((id_of(path)?, TimelineId::from_raw(place)))
        });
        ids.collect()
    }

    /// Each of its dialogues, by its sidecar's ID, and the dialogue it is in a simulation this sets up: what a
    /// game's flow opens by ID (M3 G1).
    pub fn dialogues_by_id(&self) -> Result<Vec<(AssetId, DialogueId)>, ScenarioError> {
        let ids = self.dialogues.iter().enumerate().map(|(place, path)| {
            let place = u16::try_from(place).map_err(|_| invalid("dialogues", "too many"))?;
            Ok((id_of(path)?, DialogueId::from_raw(place)))
        });
        ids.collect()
    }

    /// Loads its timelines, then its dialogues' effects, each in its list's order: so each has the ID its place
    /// says ([`Self::timelines_by_id`], [`Self::dialogues_by_id`]), which this holds it to.
    fn load_timelines_and_dialogues(&self, sim: &mut Simulation) -> Result<(), ScenarioError> {
        for ((_, id), path) in self.timelines_by_id()?.into_iter().zip(&self.timelines) {
            let timeline = TimelineDocument::from_ron(named(path)?)?.cook()?;
            if sim.add_timeline(timeline.simulation)? != id {
                return Err(invalid(format!("timelines: {path}"), "something else loaded a timeline first"));
            }
        }
        let Some(flags) = self.flags.as_deref().filter(|_| !self.dialogues.is_empty()) else { return Ok(()) };
        let registry =
            FlagRegistry::from_ron(named(flags)?).map_err(|error| invalid("flags", error.to_string()))?;
        for ((_, id), path) in self.dialogues_by_id()?.into_iter().zip(&self.dialogues) {
            let refused =
                |error: &dyn std::fmt::Display| invalid(format!("dialogues: {path}"), error.to_string());
            let document = DialogueDocument::from_ron(named(path)?).map_err(|error| refused(&error))?;
            // Its conditions' source is the pipeline's to check: this keeps only the effects.
            let dialogue = document.cook(&registry, |_| Ok(())).map_err(|error| refused(&error))?;
            if sim.add_dialogue(dialogue.simulation).map_err(|error| refused(&error))? != id {
                return Err(refused(&"something else loaded a dialogue first"));
            }
        }
        Ok(())
    }

    /// The document each of its move sets is loaded from, in set order: `moves`, then the one each copy
    /// for a second kind on the same moves is, so a live edit of a document reaches every set it plays.
    pub(super) fn set_documents(&self) -> Result<Vec<&str>, ScenarioError> {
        let copies = self.planned_enemies()?.into_iter().filter(|plan| plan.again);
        let copies: Vec<&str> = copies.map(|plan| self.moves[plan.moves].as_str()).collect();
        Ok(self.moves.iter().map(String::as_str).chain(copies).collect())
    }

    /// The scripts its kinds of enemy name, each once, in the order of `enemies`: what a fight on it binds.
    pub fn scripts(&self) -> Result<Vec<DocumentScript>, ScenarioError> {
        let mut scripts: Vec<DocumentScript> = Vec::new();
        for (path, plan) in self.enemies.iter().zip(self.planned_enemies()?) {
            let Some(id) = plan.document.script else { continue };
            let found = NAMED_DOCUMENTS
                .iter()
                .find(|(named, _)| named.ends_with(".luau") && id_of(named).ok() == Some(id));
            let (named, source) = found.ok_or_else(|| {
                invalid(format!("enemies: {path}"), format!("this build holds no script {id}"))
            })?;
            if !scripts.iter().any(|script| script.path == *named) {
                let file = super::scenario::document_file_name(named);
                scripts.push(DocumentScript { path: named, file, source });
            }
        }
        Ok(scripts)
    }

    /// Where each of `enemies` goes, in a simulation [`Self::set_up_from`] makes: on the set its moves have, the
    /// first kind on them, or on a copy added after everything before it.
    fn planned_enemies(&self) -> Result<Vec<PlannedEnemy>, ScenarioError> {
        let mut taken = vec![false; self.moves.len()];
        let mut next = self.moves.len() as u32;
        let mut planned = Vec::with_capacity(self.enemies.len());
        for path in &self.enemies {
            let document = EnemyDocument::parse(named(path)?)?;
            let moves = self.moves.iter().position(|moves| id_of(moves).ok() == Some(document.moves));
            let moves = moves
                .ok_or_else(|| invalid(format!("enemies: {path}"), "its moves aren't one of `moves`"))?;
            let again = std::mem::replace(&mut taken[moves], true);
            let set = match again {
                true => {
                    next += 1;
                    next - 1
                }
                false => moves as u32,
            };
            planned.push(PlannedEnemy { document, moves, again, set });
        }
        Ok(planned)
    }

    /// Where the stage stands it, if it stands in a zone of its own (M3 C2): one it names, on colliders that
    /// aren't the arena's, since a fight on the arena's colliders stands on the town's square whatever zone
    /// it names.
    pub(super) fn stands_in_zone(&self) -> Option<&str> {
        let zone = self.zone.as_deref()?;
        (self.colliders != crate::arena::COLLIDERS_PATH).then_some(zone)
    }

    /// The zone's stem, the name the app's content finds it by: its file's, before `.zone.ron`.
    pub(super) fn zone_stem(zone: &str) -> Result<&str, ScenarioError> {
        let file = super::scenario::document_file_name(zone);
        file.strip_suffix(".zone.ron").ok_or_else(|| invalid("zone", "a zone's document ends `.zone.ron`"))
    }

    /// What the stage needs of it to stand it in its zone (see [`Self::stands_in_zone`]): the player at the
    /// first of the cast who chooses a target as a player does, and as its other fighter whoever of the rest of
    /// the cast or its encounters' spawners stands nearest, then the next six for its crowd, which are only
    /// where the stage checks for ground. The simulation says where everyone is from its first step.
    pub(super) fn zone_fight(&'static self) -> Result<super::ZoneFight, ScenarioError> {
        let path =
            self.stands_in_zone().ok_or_else(|| invalid("zone", "it doesn't stand in a zone of its own"))?;
        let stem = Self::zone_stem(path)?;
        let (document, _) = zone_of(path)?;
        let point = |name: &str| {
            let spawner = document.spawners.iter().find(|spawner| spawner.name == name);
            let spawner =
                spawner.ok_or_else(|| invalid("cast", format!("the zone has no spawner `{name}`")))?;
            Ok::<_, ScenarioError>(SpawnPoint {
                position: spawner.position,
                yaw_degrees: spawner.yaw_degrees,
            })
        };
        let player_at = self.cast.iter().find(|member| member.soft_lock).unwrap_or(&self.cast[0]).at.as_str();
        let player = point(player_at)?;

        let mut others = Vec::new();
        for member in self.cast.iter().filter(|member| member.at != player_at) {
            others.push(point(&member.at)?);
        }
        for path in &self.encounters {
            for wave in EncounterDocument::from_ron(named(path)?)?.waves {
                for name in &wave.spawners {
                    others.push(point(name)?);
                }
            }
        }
        let away =
            |spawn: &SpawnPoint| Vec3::from_array(spawn.position).distance(Vec3::from_array(player.position));
        // Stable, so two as far away keep the order they're named in.
        others.sort_by(|a, b| away(a).total_cmp(&away(b)));
        let enemy = others.first().copied().unwrap_or(player);
        let crowd = std::array::from_fn(|at| others.get(at + 1).copied().unwrap_or(enemy));

        let colliders = ColliderDocument::from_ron(named(&self.colliders)?)?.cook()?.to_cooked()?;
        Ok(super::ZoneFight {
            stem,
            player_at,
            // A zone's patrols are its own document's (M3 P3), not the arena's.
            spawns: crate::arena::ArenaSpawns { player, enemy, crowd, patrols: Vec::new() },
            colliders,
        })
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
    /// party's members past the first. A scenario sets one up through [`Self::set_up_from`].
    #[cfg(test)]
    pub(super) fn set_up(&self, sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
        self.set_up_from(sim, &RunStart::default())
    }

    /// Makes a world of it, the same way every run, and returns who input drives, as a scenario sets one
    /// up; from a save's place in its zone (M3 G1) rather than its own `cast[0].at` and `cleared`, if
    /// `start` has one: the first of the cast stands at its checkpoint, or the zone's spawner
    /// for none, and its encounters stay cleared. One with no zone refuses a place in one.
    pub(super) fn set_up_from(
        &self,
        sim: &mut Simulation,
        run: &RunStart,
    ) -> Result<Vec<ActorId>, ScenarioError> {
        if run.zone.is_some() && self.zone.is_none() {
            return Err(invalid("zone", "a save's place needs a zone, and this names none"));
        }
        let colliders = ColliderDocument::from_ron(named(&self.colliders)?)?.cook()?.to_cooked()?;

        let zone = self.zone.as_deref().map(zone_of).transpose()?;
        let names_colliders = zone.as_ref().is_some_and(|(document, _)| document.colliders.is_some());
        if !names_colliders {
            let set = ColliderSet::read(&colliders)?;
            sim.set_physics_world(PhysicsWorld::from_cooked(set, sim.body_capacity())?)?;
        }
        self.load_timelines_and_dialogues(sim)?;
        // The player's moves name their ultimate's cinematic, which a fight loads before them, as `fight_on`
        // does (M3 Q3b).
        if self.moves.iter().any(|path| kind_of(path) == "player") {
            sim.add_cinematic(super::ULTIMATE_CINEMATIC, super::ultimate_cinematic()?)?;
        }
        let controller = Some(sim.add_controller_profile(&ControllerProfile::default())?);
        let mut sets = Vec::with_capacity(self.moves.len() + self.enemies.len());
        for path in &self.moves {
            sets.push((kind_of(path), sim.add_move_set(moves_of(path)?)));
        }
        let planned = self.planned_enemies()?;
        for (path, plan) in self.enemies.iter().zip(&planned) {
            let moves = &self.moves[plan.moves];
            let enemy = plan.document.compile(&MoveSetDocument::parse(named(moves)?)?)?;
            let set = match plan.again {
                false => sets[plan.moves].1,
                true => sim.add_move_set(moves_of(moves)?),
            };
            sim.add_enemy(set, enemy.mind)?;
            // A kind of enemy named as its moves are is those moves, already listed.
            if !sets.contains(&(kind_of(path), set)) {
                sets.push((kind_of(path), set));
            }
        }
        sim.set_zone_cast(ZoneCast { moves: &sets, controller });

        let mut start = None;
        if let Some((document, cooked)) = &zone {
            self.give_zone(sim, document, cooked, &colliders, ZoneCast { moves: &sets, controller })?;
            start = match &run.zone {
                Some(saved) => sim.start_at(saved)?,
                None => self.start_zone(sim, document)?,
            };
        }

        for path in &self.shelf {
            let baked = baked_zone(path).map_err(|problem| invalid("shelf", problem))?;
            sim.add_zone(baked.load()?)?;
        }

        let mut cast = Vec::with_capacity(self.cast.len());
        for (index, member) in self.cast.iter().enumerate() {
            let field = || format!("cast[{index}].at");
            let moves = sets.iter().find(|(kind, _)| *kind == member.moves).map(|&(_, id)| id);
            let (position, yaw) = match (zone.as_ref(), start.filter(|_| index == 0)) {
                (_, Some(place)) => (place.position.to_array(), place.yaw),
                (Some((document, _)), None) => {
                    let spawner = document.spawners.iter().find(|spawner| spawner.name == member.at);
                    let checkpoint =
                        document.checkpoints.iter().find(|checkpoint| checkpoint.name == member.at);
                    let place =
                        spawner.map(|spawner| (spawner.position, spawner.yaw_degrees)).or_else(|| {
                            checkpoint.map(|checkpoint| (checkpoint.position, checkpoint.yaw_degrees))
                        });
                    let (position, degrees) = place
                        .ok_or_else(|| invalid(field(), "the zone has no such spawner or checkpoint"))?;
                    (position, degrees.to_radians())
                }
                (None, None) => {
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

    /// Starts the zone as a save would, if the first of the cast stands at a checkpoint or anything's been
    /// cleared, and says where the first of the cast stands then.
    fn start_zone(
        &self,
        sim: &mut Simulation,
        document: &ZoneDocument,
    ) -> Result<Option<Place>, ScenarioError> {
        let first = self.cast.first().map(|member| member.at.as_str());
        let checkpoint =
            document.checkpoints.iter().find(|checkpoint| Some(checkpoint.name.as_str()) == first);
        if checkpoint.is_none() && self.cleared.is_empty() {
            return Ok(None);
        }
        let mut cleared = self.cleared.iter().map(|path| id_of(path)).collect::<Result<Vec<_>, _>>()?;
        cleared.sort_unstable();
        let start = ZoneStart { checkpoint: checkpoint.map(|checkpoint| checkpoint.name.clone()), cleared };
        Ok(sim.start_at(&start)?)
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

/// The simulation's copy of the zone at `path`, a `.zone.ron` from the repository's root: a trip's zone is one
/// of [`super::BAKED_ZONES`], since its navmesh takes a bake, which takes the pipeline.
fn baked_zone(path: &str) -> Result<&'static super::BakedZone, String> {
    let file = super::scenario::document_file_name(path);
    let stem = file.strip_suffix(".zone.ron").ok_or("a zone's document ends `.zone.ron`")?;
    super::BAKED_ZONES.iter().find(|baked| baked.stem == stem).ok_or_else(|| {
        let baked: Vec<&str> = super::BAKED_ZONES.iter().map(|baked| baked.stem).collect();
        format!("the simulation keeps a copy of {baked:?}, and not of `{stem}`")
    })
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
        let cases: [Spoiled; 8] = [
            ("about", |d| d.about.clear()),
            ("max_actors", |d| d.max_actors = 0),
            ("moves", |d| d.moves.clear()),
            ("cast", |d| d.cast.clear()),
            ("encounters", |d| d.encounters = vec!["x.encounter.ron".to_owned()]),
            ("cast[1].moves", |d| d.cast[1].moves = "dancer".to_owned()),
            ("cast[1].faction", |d| d.cast[1].faction = 17),
            ("cast[0].brain", |d| d.cast[0].brain = true),
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

    /// A document's timelines load first, in its order, so each has the ID its place says, and the player's
    /// ultimate's cinematic comes after them (M3 G1): what the court's gate scripts play as timeline 0.
    #[test]
    fn a_documents_timelines_are_its_first() {
        let court = SCENARIO_DOCUMENTS.iter().find(|(name, _)| *name == "ward-court").expect("held").1;
        let court: ScenarioDocument = sapphire_serialize::from_ron(court).expect("reads");
        let ids = court.timelines_by_id().expect("it says");
        let gate = id_of("content/zones/lantern_ward/court_gate.timeline.ron").expect("a sidecar");
        assert_eq!(ids, [(gate, TimelineId::from_raw(0))]);
        let mut sim = Simulation::new(court.config());
        court.set_up(&mut sim).expect("sets up");
        let gate = TimelineDocument::from_ron(named(&court.timelines[0]).expect("held")).expect("reads");
        let digest = gate.cook().expect("cooks").simulation.digest();
        assert_eq!(sim.timeline_digest(TimelineId::from_raw(0)), Some(digest));
        assert!(sim.timeline_digest(TimelineId::from_raw(1)).is_some(), "then the ultimate's cinematic");
        assert!(court.dialogues_by_id().expect("it says").is_empty());

        let mut document = court;
        document.dialogues = vec!["content/dialogue/ward_guide.dialogue.ron".to_owned()];
        let error = document.check().expect_err("no registry");
        assert!(error.to_string().contains("`flags`"), "{error}");
    }

    /// A trip's zones go on the shelf in the document's order (M3 G1), and only from a zone, and only zones
    /// the simulation keeps a copy of.
    #[test]
    fn a_shelf_holds_the_zones_a_run_can_go_to() {
        let trip = SCENARIO_DOCUMENTS.iter().find(|(name, _)| *name == "ground-trip").expect("held").1;
        let trip: ScenarioDocument = sapphire_serialize::from_ron(trip).expect("reads");
        let mut sim = Simulation::new(trip.config());
        trip.set_up(&mut sim).expect("sets up");
        let rows: Vec<&str> = (0..2)
            .map(|row| sim.zone_ground(crate::zone::ZoneId::from_raw(row)).expect("on the shelf").pack())
            .collect();
        assert_eq!(rows, ["zone_proving_hub", "zone_proving_ground"]);
        assert_eq!(sim.current_zone(), None, "it starts in the zone it names, which no row holds");

        let mut document = trip.clone();
        document.shelf.push("content/zones/town/town.zone.ron".to_owned());
        let error = document.check().expect_err("no copy of the town");
        assert!(error.to_string().contains("`shelf[2]`"), "{error}");
        let mut document = arena_fight();
        document.shelf = trip.shelf;
        let error = document.check().expect_err("no zone to go from");
        assert!(error.to_string().contains("`shelf`"), "{error}");
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
            for path in document.moves.iter().chain(&document.enemies).chain([&document.colliders]) {
                assert!(named(path).is_ok(), "{name}: {path}");
            }
        }
    }

    /// What a stage draws each fighter as is what the simulation fights it as: every kind of enemy's set is a
    /// set with that kind's mind, a set of its own, and a second kind on the same moves has a copy of them.
    #[test]
    fn each_kind_of_enemy_says_the_set_its_fighters_play() {
        for (name, text) in SCENARIO_DOCUMENTS {
            let document: ScenarioDocument = sapphire_serialize::from_ron(text).expect("reads");
            let mut sim = Simulation::new(document.config());
            document.set_up(&mut sim).unwrap_or_else(|error| panic!("{name}: {error}"));
            let kinds = document.kinds_of_enemy().unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(kinds.len(), document.enemies.len(), "{name}");
            for (kind, path) in kinds.iter().zip(&document.enemies) {
                let mind = sim.move_library().mind(kind.set).unwrap_or_else(|| panic!("{name}: {path}"));
                let compiled = EnemyDocument::parse(named(path).expect("held")).expect("reads");
                assert_eq!(
                    mind.boss(),
                    compiled.boss.as_ref().map(|boss| boss.name.as_str()),
                    "{name}: {path}"
                );
                assert_eq!(kind.kind, id_of(path).expect("a sidecar"), "{name}: {path}");
                assert_eq!(kinds.iter().filter(|other| other.set == kind.set).count(), 1, "{name}: {path}");
            }
        }
        let boss = SCENARIO_DOCUMENTS.iter().find(|(name, _)| *name == "arena-boss").expect("held").1;
        let boss: ScenarioDocument = sapphire_serialize::from_ron(boss).expect("reads");
        let captain = id_of("content/combat/enemy/captain.enemy.ron").expect("a sidecar");
        let kinds = boss.kinds_of_enemy().expect("it plans");
        assert_eq!(kinds, [KindOfEnemy { set: crate::moves::MoveSetId(1), kind: captain }]);
    }

    /// A second kind on moves another kind has goes on a copy of them, after every set `moves` lists.
    #[test]
    fn a_second_kind_on_the_same_moves_plays_a_copy_of_them() {
        let mut document = arena_fight();
        document.enemies.push("content/combat/enemy/captain.enemy.ron".to_owned());
        let mut sim = Simulation::new(document.config());
        document.set_up(&mut sim).expect("it starts");
        let kinds = document.kinds_of_enemy().expect("it plans");
        let sets: Vec<u32> = kinds.iter().map(|kind| kind.set.0).collect();
        assert_eq!(sets, [1, 2]);
        assert_eq!(sim.move_library().set_count(), 3);
        assert_eq!(sim.move_library().mind(kinds[1].set).and_then(|mind| mind.boss()), Some("boss-captain"));
    }
}
