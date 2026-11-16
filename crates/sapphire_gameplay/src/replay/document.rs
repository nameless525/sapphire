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
//! **What a zone can spawn is the zone's** (M3 C2): its spawners' `spawns`, by the kind's name, as its cook
//! keeps them. So a scenario on a zone lists in `enemies` only the kinds its cast needs, and every kind its
//! encounters' spawners name that `moves` and `enemies` don't is added after them, with its moves if `moves`
//! hasn't got those, from the kind of that name this build holds: `build.rs` compiles in every `.enemy.ron`
//! under `content/combat/`, the moves each names and its script. That holds for an encounter `cleared` lists too,
//! so a save's start loads what it would have loaded fresh. A kind listed anyway keeps its place, and so its
//! set's number.
//!
//! **Who a player draws as** (M5 C8): a cast member's `playable` names a playable kind by its file's name,
//! before `.playable.ron`, as `moves` names a kind; `build.rs` compiles in every one under `content/`. The kind
//! has to play the member's set, and a kind of enemy draws as itself, so it names none. The simulation never
//! reads it ([`ScenarioDocument::playable_kinds`]); a game draws a member that names none as its default kind.
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
//! that order ([`Simulation::add_zone`]), so a run can go into one and back. The simulation keeps a copy of
//! each one's cook ([`super::BAKED_ZONES`]), since a zone on the shelf takes its navmesh, and a bake takes the
//! pipeline. `build.rs` finds every zone a scenario shelves, and the encounters beside it (M4 R3): its copies
//! are taken once, by `tools/sapphire_pipeline/tests/zone_baked.rs`, and no Rust changes. Going back into the
//! zone a run started in goes into its copy on the shelf, which starts that zone over.
//!
//! **What a run can play and open** (M3 G1): `timelines` and `dialogues` list the cutscenes and conversations
//! a game's flow or a replay's commands can start, each loaded in its list's order, so the first is timeline 0
//! and dialogue 0: the timelines before the cinematics the sets' moves name (`super::cinematics`), so those
//! come after them.
//! [`ScenarioDocument::timelines_by_id`] and [`ScenarioDocument::dialogues_by_id`] say which is which by the
//! documents' IDs, which is how a flow names them. A dialogue's conditions are checked as its cook checks them,
//! by the pipeline: the simulation loads only its effects, cooked against the registry `flags` names.
//!
//! **A zone here is what the simulation reads of its document** ([`ZoneDocument::simulation`]) with no
//! navmesh and no presentation, archived as a cook archives it, so it goes in through the same door as a pack's
//! `.szone` ([`Simulation::set_zone`]), and `tools/sapphire_pipeline/tests/encounters.rs` holds the two to one
//! `tables_hash`. A scenario that wants paths on a zone takes its navmesh some other way: a bake takes the
//! pipeline, so only a zone code builds brings one.
//!
//! **A zone code builds** (M5 B5): `built: Some("<name>")` stands the scenario on what that zone's builder
//! makes ([`sapphire_zone::BuiltZone`]): its simulation half, its encounters, its colliders and its navmesh.
//! The benchmark district is the one (`sapphire_synthetic_zone::district`), in a crate no shipped build links, so
//! the builders are a value whoever sets a scenario up hands over ([`super::Scenario::zones`]): the replay runner
//! and a development build's fight do, and a scenario naming a zone nobody handed over can't start, saying so.
//! Its encounters' spawners spawn the kinds of enemy `enemies` lists, in turn, so those are the document's names.

use bevy_reflect::Reflect;
use sapphire_colliders::{ColliderDocument, ColliderSet};
use sapphire_dialogue::{DialogueDocument, FlagRegistry};
use sapphire_math::Vec3;
use sapphire_moves::{EnemyDocument, MoveSet, MoveSetDocument, MoveSetError, PlayableDocument};
use sapphire_physics::PhysicsWorld;
use sapphire_reflect::Versioned;
use sapphire_serialize::AssetId;
use sapphire_serialize::cooked::AlignedVec;
use sapphire_timeline::TimelineDocument;
use sapphire_zone::{BuiltZone, EncounterDocument, Zone, ZoneDocument, ZonePresentation, ZoneSimulation};
use serde::{Deserialize, Serialize};

use super::play::RunStart;
use super::scenario::{ScenarioError, ZoneBuilders};
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
    /// must be those. A scenario on a zone code builds names none: the zone brings its own.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub colliders: String,
    /// A `.zone.ron`, from the repository's root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
    /// A zone code builds rather than a document (M5 B5), by the name its builder goes by: see the [module
    /// docs](self). It brings its own colliders, encounters and navmesh, so the scenario names no `colliders`,
    /// `zone`, `encounters` or `cleared`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<String>,
    /// The `.encounter.ron`s the zone lists, from the repository's root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub encounters: Vec<String>,
    /// Those of `encounters` fought before it starts, as a save says (see the [module docs](self)).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleared: Vec<String>,
    /// The documents its move sets come from, from the repository's root, in the order they're added.
    pub moves: Vec<String>,
    /// Its kinds of enemy: `.enemy.ron`s, from the repository's root, each on one of `moves`. Those its
    /// zone's encounters spawn needn't be here (see the [module docs](self)).
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
    /// The playable kind they draw as (M5 C8): its `.playable.ron`'s name, before that suffix, which has to
    /// play `moves`. Left out, a game draws them as its own default kind. Presentation: no step reads it or
    /// anything its document holds, and no checksum holds it (a body a fight binds takes its numbers from the
    /// fight's own code, M5 Z3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playable: Option<String>,
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
    let document = encounter_path(id).ok_or_else(|| {
        invalid(format!("encounter {id}"), "no encounter document this build holds has that ID")
    })?;
    Ok(EncounterDocument::from_ron(named(document)?)?.cook()?.to_cooked()?)
}

/// The `.encounter.ron` whose sidecar names `id`, from the repository's root, among the documents this build
/// holds: what a playtest's report calls an encounter a run met (M4 F2).
#[must_use]
pub fn encounter_path(id: AssetId) -> Option<&'static str> {
    NAMED_DOCUMENTS.iter().find_map(|(path, text)| {
        let document = path.strip_suffix(".meta")?;
        (document.ends_with(".encounter.ron") && id_in(text, path).ok()? == id).then_some(document)
    })
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

/// A kind of enemy in a scenario's simulations: what a stage draws the fighters on `set` as, and what a HUD
/// calls them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindOfEnemy {
    /// The set its fighters play.
    pub set: crate::moves::MoveSetId,
    /// Its `.enemy.ron`'s ID, which its cook has.
    pub kind: AssetId,
    /// The key of its name in the game's words, as its document names it (M4 X2): presentation, which no
    /// step reads.
    pub name: String,
}

/// A playable kind one of a scenario's cast draws as (M5 C8): presentation, which no step reads, nor anything
/// its document holds (M5 Z3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindOfPlayable {
    /// Its `.playable.ron`'s ID, which its cook has.
    pub kind: AssetId,
    /// The key of its name in the game's words, as its document names it: what a party's slot calls it.
    pub name: String,
}

/// The playable kind named `name` that this build holds: the `.playable.ron` of that file name.
fn playable_named(name: &str) -> Option<&'static str> {
    let file = format!("{name}.playable.ron");
    let found = NAMED_DOCUMENTS.iter().find(|(path, _)| super::scenario::document_file_name(path) == file);
    found.map(|(path, _)| *path)
}

/// The playable kind this build holds as `{name}.playable.ron` (M5 C4): what `--playable` stands in look-dev,
/// by its sidecar's ID and the key of its name.
///
/// # Errors
///
/// If this build holds no such kind, or it doesn't read.
pub fn playable_kind_named(name: &str) -> Result<KindOfPlayable, ScenarioError> {
    let path = playable_named(name)
        .ok_or_else(|| invalid("playable", format!("this build holds no `{name}.playable.ron`")))?;
    let document = PlayableDocument::parse(named(path)?)
        .map_err(|error| invalid("playable", format!("{path}: {error}")))?;
    Ok(KindOfPlayable { kind: id_of(path)?, name: document.name })
}

/// Every kind of enemy this build holds, which `build.rs` compiles in from `content/combat/`, by its
/// document's path from the repository's root, with the path of the moves it plays: what a scenario's
/// `moves` and `enemies` would list to stand one (M4 X2).
pub fn kinds_held() -> Result<Vec<(&'static str, &'static str)>, ScenarioError> {
    let kinds = NAMED_DOCUMENTS.iter().filter(|(path, _)| path.ends_with(".enemy.ron"));
    let kinds = kinds.map(|&(path, text)| {
        let moves = moves_with_id(EnemyDocument::parse(text)?.moves)?;
        Ok((path, moves.ok_or_else(|| invalid(path, "this build holds no moves for it"))?))
    });
    kinds.collect()
}

/// The script one of a scenario's kinds of enemy names (M3 C2), which this build holds, and the set that
/// kind's fighters play, whose events alone it hears ([`crate::ScriptScope::Kind`]): what binds it in a fight
/// is the script crate's, which gameplay can't see ([`super::ScenarioSetup::DocumentAnd`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentScript {
    /// The set its kind's fighters play, in a simulation the scenario sets up.
    pub set: crate::moves::MoveSetId,
    /// From the repository's root: what hot reload watches.
    pub path: &'static str,
    /// Its file's name: what it's bound by, and what an edit of it goes by.
    pub file: &'static str,
    /// Its source, as this build holds it.
    pub source: &'static str,
}

/// What a scenario plays: its `moves` and `enemies`, then what its zone's encounters spawn that those don't
/// say (see the [module docs](self)). Paths from the repository's root.
struct Roster<'a> {
    moves: Vec<&'a str>,
    enemies: Vec<&'a str>,
}

impl Roster<'_> {
    /// Where each of its kinds of enemy goes, in a simulation [`ScenarioDocument::set_up_from`] makes: on the set
    /// its moves have, the first kind on them, or on a copy added after everything before it.
    fn planned(&self) -> Result<Vec<PlannedEnemy>, ScenarioError> {
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
}

/// The kind of enemy named `name` that this build holds: the `.enemy.ron` of that file name.
fn kind_named(name: &str) -> Option<&'static str> {
    let file = format!("{name}.enemy.ron");
    let found = NAMED_DOCUMENTS.iter().find(|(path, _)| super::scenario::document_file_name(path) == file);
    found.map(|(path, _)| *path)
}

/// The set of moves with this ID that this build holds.
fn moves_with_id(id: AssetId) -> Result<Option<&'static str>, ScenarioError> {
    for (path, text) in NAMED_DOCUMENTS {
        let Some(document) = path.strip_suffix(".meta") else { continue };
        if document.ends_with(".moves.ron") && id_in(text, path)? == id {
            return Ok(Some(document));
        }
    }
    Ok(None)
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
/// which and why, the first time anyone asks for a scenario. The playable kinds its cast draws as are read as it
/// sets up (`set_up_from`), so a bad `.playable.ron` is that set-up's error rather than every scenario's stop.
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
        match (&self.built, self.colliders.is_empty()) {
            (Some(_), false) => return Err(invalid("colliders", "a zone code builds brings its own")),
            (None, true) => {
                return Err(invalid(
                    "colliders",
                    "a fight stands on a `.colliders.ron`, or on a zone code builds",
                ));
            }
            _ => {}
        }
        if self.built.is_some() {
            if self.zone.is_some() || !self.encounters.is_empty() || !self.cleared.is_empty() {
                let problem = "a zone code builds is the scenario's zone, with its own encounters: name no `zone`, \
                               `encounters` or `cleared`";
                return Err(invalid("built", problem));
            }
            if self.enemies.is_empty() {
                let problem = "a zone code builds brings the kinds of enemy this lists, and it lists none";
                return Err(invalid("enemies", problem));
            }
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
        if !self.shelf.is_empty() && self.zone.is_none() && self.built.is_none() {
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

    /// Each of its kinds of enemy, by its sidecar's ID, the set its fighters play in a simulation this
    /// sets up, and its name's key: what draws them is that kind's look, which a stage reads from its cooked
    /// document (M3 B3b), and what a HUD calls them is its name (M4 X2). The simulation never reads either.
    pub fn kinds_of_enemy(&self) -> Result<Vec<KindOfEnemy>, ScenarioError> {
        let roster = self.roster()?;
        let planned = roster.planned()?;
        let kinds = roster.enemies.iter().zip(planned).map(|(path, plan)| {
            let set = crate::moves::MoveSetId(plan.set);
            Ok(KindOfEnemy { set, kind: id_of(path)?, name: plan.document.name })
        });
        kinds.collect()
    }

    /// The playable kind each of its cast draws as, in cast order (M5 C8): `None` for one that names none,
    /// whom a game draws as its default kind. Each named kind is one this build holds, playing the set its member
    /// does. Presentation, as [`Self::kinds_of_enemy`] is: the simulation never reads it.
    pub fn playable_kinds(&self) -> Result<Vec<Option<KindOfPlayable>>, ScenarioError> {
        let kinds = self.cast.iter().enumerate().map(|(index, member)| {
            let Some(name) = &member.playable else { return Ok(None) };
            let field = format!("cast[{index}].playable");
            let path = playable_named(name)
                .ok_or_else(|| invalid(&field, format!("this build holds no `{name}.playable.ron`")))?;
            let document = PlayableDocument::parse(named(path)?)
                .map_err(|error| invalid(&field, format!("{path}: {error}")))?;
            let set = self.moves.iter().find(|path| kind_of(path) == member.moves).ok_or_else(|| {
                invalid(
                    &field,
                    format!("`{}` isn't one of `moves`, and a kind of enemy draws as itself", member.moves),
                )
            })?;
            if id_of(set)? != document.moves {
                return Err(invalid(&field, format!("`{name}` plays other moves than `{}`", member.moves)));
            }
            Ok(Some(KindOfPlayable { kind: id_of(path)?, name: document.name }))
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

    /// The document each of its move sets is loaded from, in set order: `moves` and those its zone's kinds
    /// bring, then the one each copy for a second kind on the same moves is, so a live edit of a document
    /// reaches every set it plays.
    pub(super) fn set_documents(&self) -> Result<Vec<&str>, ScenarioError> {
        let roster = self.roster()?;
        let copies = roster.planned()?.into_iter().filter(|plan| plan.again);
        let copies: Vec<&str> = copies.map(|plan| roster.moves[plan.moves]).collect();
        Ok(roster.moves.iter().copied().chain(copies).collect())
    }

    /// The script each of its kinds of enemy names, kind by kind in the order of `enemies` and then those
    /// its zone brings, with the set that kind plays: what a fight on it binds, each for its own kind's
    /// fighters. Two kinds that name one file have it twice, a set each.
    pub fn scripts(&self) -> Result<Vec<DocumentScript>, ScenarioError> {
        let roster = self.roster()?;
        let mut scripts: Vec<DocumentScript> = Vec::new();
        for (path, plan) in roster.enemies.iter().zip(roster.planned()?) {
            let Some(id) = plan.document.script else { continue };
            let found = NAMED_DOCUMENTS
                .iter()
                .find(|(named, _)| named.ends_with(".luau") && id_of(named).ok() == Some(id));
            let (named, source) = found.ok_or_else(|| {
                invalid(format!("enemies: {path}"), format!("this build holds no script {id}"))
            })?;
            let file = super::scenario::document_file_name(named);
            let set = crate::moves::MoveSetId(plan.set);
            scripts.push(DocumentScript { set, path: named, file, source });
        }
        Ok(scripts)
    }

    /// What it plays: `moves` and `enemies`, then each kind its zone's encounters spawn that those don't
    /// say, and that kind's moves if they're not there yet (see the [module docs](self)). A spawner of a
    /// name this build holds no kind of is left for giving the zone to refuse, by the spawner's name.
    fn roster(&self) -> Result<Roster<'_>, ScenarioError> {
        let mut moves: Vec<&str> = self.moves.iter().map(String::as_str).collect();
        let mut enemies: Vec<&str> = self.enemies.iter().map(String::as_str).collect();
        let Some(zone) = self.zone.as_deref() else { return Ok(Roster { moves, enemies }) };
        let zone = ZoneDocument::from_ron(named(zone)?)?;
        for path in &self.encounters {
            for wave in EncounterDocument::from_ron(named(path)?)?.waves {
                for name in &wave.spawners {
                    let Some(spawner) = zone.spawners.iter().find(|spawner| spawner.name == *name) else {
                        continue;
                    };
                    let spawns = spawner.spawns.as_str();
                    if moves.iter().chain(&enemies).any(|path| kind_of(path) == spawns) {
                        continue;
                    }
                    let Some(kind) = kind_named(spawns) else { continue };
                    let document = EnemyDocument::parse(named(kind)?)?;
                    let its_moves = moves_with_id(document.moves)?.ok_or_else(|| {
                        invalid(
                            format!("encounters: {path}"),
                            format!("this build holds no moves for {kind}"),
                        )
                    })?;
                    if !moves.contains(&its_moves) {
                        moves.push(its_moves);
                    }
                    enemies.push(kind);
                }
            }
        }
        Ok(Roster { moves, enemies })
    }

    /// Where the stage stands it, if it stands in a zone of its own (M3 C2): one it names, on colliders that
    /// aren't the arena's, since a fight on the arena's colliders stands on the town's square whatever zone
    /// it names.
    pub(super) fn stands_in_zone(&self) -> Option<&str> {
        let zone = self.zone.as_deref()?;
        (self.colliders != crate::arena::COLLIDERS_PATH).then_some(zone)
    }

    /// The name the app's content finds the zone it stands in by, if it stands in one: a document zone's stem,
    /// or a built zone's name, which is its pack's stem too.
    pub(super) fn zone_name(&self) -> Option<&str> {
        match self.built.as_deref() {
            Some(name) => Some(name),
            None => Self::zone_stem(self.stands_in_zone()?).ok(),
        }
    }

    /// What the builder of the zone it names in `built` makes, from `zones`, its encounters spawning the kinds of
    /// enemy it lists; `None` for a scenario on no built zone.
    fn built_zone(&self, zones: ZoneBuilders) -> Result<Option<BuiltZone>, ScenarioError> {
        let Some(name) = self.built.as_deref() else { return Ok(None) };
        let (_, build) = zones.iter().find(|(held, _)| *held == name).ok_or_else(|| {
            invalid(
                "built",
                format!(
                    "this build builds no zone `{name}`: a development build's fight and the replay runner hand \
                     its builder over"
                ),
            )
        })?;
        let kinds: Vec<&str> = self.enemies.iter().map(|path| kind_of(path)).collect();
        build(&kinds).map(Some).map_err(|problem| invalid("built", format!("zone `{name}`: {problem}")))
    }

    /// Its zone as the simulation reads it, with the collider set it stands on and the encounters the zone lists,
    /// cooked: its document's, or what its builder in `zones` makes. `None` for no zone, with the colliders alone.
    fn zone_with_ground(&self, zones: ZoneBuilders) -> Result<ZoneWithGround, ScenarioError> {
        let spawned = |encounter: &EncounterDocument| -> Vec<String> {
            encounter.waves.iter().flat_map(|wave| wave.spawners.iter().cloned()).collect()
        };
        if let Some(built) = self.built_zone(zones)? {
            let cooked = archived(built.document.simulation()?, built.document.grid.cell_size)?;
            let (mut encounters, mut fought) = (Vec::with_capacity(built.encounters.len()), Vec::new());
            for (id, encounter) in &built.encounters {
                encounters.push((*id, encounter.cook()?.to_cooked()?));
                fought.extend(spawned(encounter));
            }
            return Ok(ZoneWithGround {
                zone: Some((built.document, cooked)),
                colliders: built.colliders,
                encounters,
                fought,
                navmesh: built.navmesh,
            });
        }
        let colliders = ColliderDocument::from_ron(named(&self.colliders)?)?.cook()?.to_cooked()?;
        let zone = self.zone.as_deref().map(zone_of).transpose()?;
        let (mut encounters, mut fought) = (Vec::with_capacity(self.encounters.len()), Vec::new());
        for path in &self.encounters {
            let encounter = EncounterDocument::from_ron(named(path)?)?;
            encounters.push((id_of(path)?, encounter.cook()?.to_cooked()?));
            fought.extend(spawned(&encounter));
        }
        if let Some((document, _)) = &zone {
            if let Some(id) = document.colliders
                && id_of(&self.colliders)? != id
            {
                return Err(invalid("colliders", "that isn't the set the zone names"));
            }
            for (path, (id, _)) in self.encounters.iter().zip(&encounters) {
                if !document.encounters.contains(id) {
                    return Err(invalid(format!("encounters: {path}"), "the zone doesn't list it"));
                }
            }
        }
        Ok(ZoneWithGround { zone, colliders, encounters, fought, navmesh: None })
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
    pub(super) fn zone_fight(&'static self, zones: ZoneBuilders) -> Result<super::ZoneFight, ScenarioError> {
        let stem =
            self.zone_name().ok_or_else(|| invalid("zone", "it doesn't stand in a zone of its own"))?;
        let ZoneWithGround { zone, colliders, fought, .. } = self.zone_with_ground(zones)?;
        let (document, _) = zone.ok_or_else(|| invalid("zone", "it doesn't stand in a zone of its own"))?;
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
        for name in &fought {
            others.push(point(name)?);
        }
        let away =
            |spawn: &SpawnPoint| Vec3::from_array(spawn.position).distance(Vec3::from_array(player.position));
        // Stable, so two as far away keep the order they're named in.
        others.sort_by(|a, b| away(a).total_cmp(&away(b)));
        let enemy = others.first().copied().unwrap_or(player);
        let crowd = others.iter().skip(1).copied().collect();

        Ok(super::ZoneFight {
            stem,
            player_at,
            // A zone's patrols are its own document's (M3 P3), not the arena's.
            spawns: crate::arena::ArenaSpawns { player, enemy, crowd, patrols: Vec::new() },
            colliders,
            exits: document.exits.len(),
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
    pub(super) fn set_up(
        &self,
        sim: &mut Simulation,
        zones: ZoneBuilders,
    ) -> Result<Vec<ActorId>, ScenarioError> {
        self.set_up_from(sim, &RunStart::default(), zones)
    }

    /// Makes a world of it, the same way every run, and returns who input drives, as a scenario sets one
    /// up; from a save's place in its zone (M3 G1) rather than its own `cast[0].at` and `cleared`, if
    /// `start` has one: the first of the cast stands at its checkpoint, or the zone's spawner
    /// for none, and its encounters stay cleared. One with no zone refuses a place in one. A zone it names
    /// in `built` is what its builder in `zones` makes.
    pub(super) fn set_up_from(
        &self,
        sim: &mut Simulation,
        run: &RunStart,
        zones: ZoneBuilders,
    ) -> Result<Vec<ActorId>, ScenarioError> {
        if run.zone.is_some() && self.zone.is_none() && self.built.is_none() {
            return Err(invalid("zone", "a save's place needs a zone, and this names none"));
        }
        // Who the cast draws as is presentation, which no step reads; but nothing should record or play a
        // scenario no game can draw, so a kind that doesn't read, or plays another set, refuses the set-up here,
        // naming the member, the document and what is wrong.
        self.playable_kinds()?;
        let ZoneWithGround { zone, colliders, encounters, navmesh, .. } = self.zone_with_ground(zones)?;
        let names_colliders = zone.as_ref().is_some_and(|(document, _)| document.colliders.is_some());
        if !names_colliders {
            let set = ColliderSet::read(&colliders)?;
            sim.set_physics_world(PhysicsWorld::from_cooked(set, sim.body_capacity())?)?;
        }
        self.load_timelines_and_dialogues(sim)?;
        // Every cinematic the sets' moves name loads before them, in their order, as `fight_on`'s do (M3 Q3b,
        // Z5a): a second kind on the same moves names nothing they don't.
        let roster = self.roster()?;
        let read = roster.moves.iter().map(|&path| Ok((path, moves_of(path)?)));
        let read = read.collect::<Result<Vec<_>, ScenarioError>>()?;
        super::cinematics::add_cinematics(sim, read.iter().map(|(path, set)| (*path, set)))?;
        let controller = Some(sim.add_controller_profile(&ControllerProfile::default())?);
        let mut sets = Vec::with_capacity(roster.moves.len() + roster.enemies.len());
        for (path, set) in read {
            sets.push((kind_of(path), sim.add_move_set(set)));
        }
        let planned = roster.planned()?;
        for (path, plan) in roster.enemies.iter().zip(&planned) {
            let moves = roster.moves[plan.moves];
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
            give_zone(sim, document, cooked, &colliders, encounters, ZoneCast { moves: &sets, controller })?;
            if let Some(navmesh) = &navmesh {
                sim.set_navigation(navmesh)?;
            }
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

    /// The actors a run needs room for to meet every fighter its zone's encounters bring, those it starts
    /// with `cleared` aside, and its cast (M3 G1c). A fallen fighter keeps its place in the pool, so a pool
    /// smaller than this refuses the last arrivals of a run that walks the whole zone, and an encounter whose
    /// last wave came to nobody is cleared without a fight: the ward's run held 12 for 15, and its flow won
    /// with no boss. A scenario of one room can hold less, since the rest never start.
    /// A built zone's encounters are what its builder in `zones` makes.
    pub fn room_for_everyone(&self, zones: ZoneBuilders) -> Result<usize, ScenarioError> {
        let mut room = self.cast.len();
        let spawns = |encounter: &EncounterDocument| {
            encounter.waves.iter().map(|wave| wave.spawners.len()).sum::<usize>()
        };
        if let Some(built) = self.built_zone(zones)? {
            room += built.encounters.iter().map(|(_, encounter)| spawns(encounter)).sum::<usize>();
        }
        for path in self.encounters.iter().filter(|path| !self.cleared.contains(path)) {
            room += spawns(&EncounterDocument::from_ron(named(path)?)?);
        }
        Ok(room)
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
}

/// A scenario's zone as its simulation reads it ([`ScenarioDocument::zone_with_ground`]).
struct ZoneWithGround {
    /// The zone's document and what the simulation reads of it, archived, if it has one.
    zone: Option<(ZoneDocument, AlignedVec)>,
    /// The collider set it stands on, cooked.
    colliders: AlignedVec,
    /// Each encounter the zone lists that the scenario brings, by ID, cooked.
    encounters: Vec<(AssetId, AlignedVec)>,
    /// Every spawner those encounters' waves name, in order.
    fought: Vec<String>,
    /// Its navmesh, cooked, if it brings one: a built zone's.
    navmesh: Option<AlignedVec>,
}

/// Hands a simulation the zone: its cooked encounters by ID, and the colliders, as a pack would.
fn give_zone(
    sim: &mut Simulation,
    document: &ZoneDocument,
    cooked: &AlignedVec,
    colliders: &AlignedVec,
    mut bytes: Vec<(AssetId, AlignedVec)>,
    cast: ZoneCast<'_>,
) -> Result<(), ScenarioError> {
    for id in &document.encounters {
        if !bytes.iter().any(|(held, _)| held == id) {
            return Err(invalid("encounters", format!("the zone lists {id}, and this doesn't")));
        }
    }
    if let Some(id) = document.colliders {
        bytes.push((id, colliders.clone()));
    }
    let zone = Zone::read(cooked)?;
    sim.set_zone(zone, cast, |id| {
        bytes.iter().find(|(held, _)| *held == id).map(|(_, bytes)| bytes.as_slice())
    })?;
    Ok(())
}

/// The simulation's copy of the zone at `path`, a `.zone.ron` from the repository's root: a trip's zone is one
/// of [`super::BAKED_ZONES`], since its navmesh takes a bake, which takes the pipeline.
fn baked_zone(path: &str) -> Result<&'static super::BakedZone, String> {
    let file = super::scenario::document_file_name(path);
    let stem = file.strip_suffix(".zone.ron").ok_or("a zone's document ends `.zone.ron`")?;
    super::baked_zone(stem).ok_or_else(|| {
        format!(
            "this build keeps no copy of `{stem}`'s cook: `cargo xtask content`, then `SAPPHIRE_BAKE_ZONES=1 cargo \
             test -p sapphire_pipeline --test zone_baked` takes one into crates/sapphire_gameplay/baked/zones/"
        )
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
    let cooked = archived(document.simulation()?, document.grid.cell_size)?;
    Ok((document, cooked))
}

/// What the simulation reads of a zone, archived as a cook archives it, with no presentation.
fn archived(mut simulation: ZoneSimulation, cell_size: f32) -> Result<AlignedVec, ScenarioError> {
    // An exit's pack is the pack of the zone it leads to, which a cook finds by its ID (M3 W4).
    for exit in &mut simulation.exits {
        let pack = zone_pack(exit.to)?;
        exit.pack =
            pack.ok_or_else(|| invalid(format!("exits `{}`.to", exit.name), "no zone has that ID"))?;
    }
    let zone = Zone {
        simulation,
        presentation: ZonePresentation {
            cell_size,
            backdrop: None,
            cells: Vec::new(),
            instances: Vec::new(),
            lights: Vec::new(),
            probes: None,
            reflections: None,
            day: None,
            rigs: None,
            splines: Vec::new(),
            sky: None,
            wind: None,
            weather: None,
            fog: None,
        },
    };
    Ok(zone.to_cooked()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActorActions, Buttons, Stick, TickActions};
    use sapphire_core::Name;

    /// The zones code builds, as the replay runner hands them over: the benchmark district (M5 B5).
    const ZONES: ZoneBuilders = sapphire_synthetic_zone::district::ZONES;

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
        document.set_up(&mut sim, ZONES).expect_err("it can't").to_string()
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

    /// What a run walking every encounter needs room for (M3 G1c): its cast, and every fighter the encounters
    /// it hasn't fought bring, as the zone a simulation stands on it counts them. A fallen fighter keeps its
    /// place, so a pool smaller than this refuses the last to come, and a game's flow refuses such a scenario
    /// (`Game::new`): the ward's run once held 12 for 15, and its boss came to nobody. Held for every scenario
    /// this build has, so a fighter more in a zone's encounters is the documents' to count, and for a run
    /// built from one of them with what it starts having fought changed.
    #[test]
    fn room_for_everyone_counts_the_cast_and_every_fighter_to_come() {
        let to_come = |document: &ScenarioDocument| -> usize {
            let mut sim = Simulation::new(document.config());
            document.set_up(&mut sim, ZONES).expect("sets up");
            let encounters = sim.zone().map_or(&[][..], crate::encounter::SimZone::encounters);
            let waiting = encounters.iter().filter(|held| held.phase() != crate::EncounterPhase::Cleared);
            waiting.map(crate::encounter::SimEncounter::planned).sum()
        };
        let documents: Vec<(&str, ScenarioDocument)> = SCENARIO_DOCUMENTS
            .iter()
            .map(|(name, text)| (*name, sapphire_serialize::from_ron(text).expect("reads")))
            .collect();
        for (name, document) in &documents {
            let room = document.room_for_everyone(ZONES).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(room, document.cast.len() + to_come(document), "{name}: as the zone counts them");
        }

        // A run with two encounters or more and none fought: one fought takes its fighters off, and all of
        // them leave its cast alone.
        let (name, run) = documents
            .iter()
            .find(|(_, document)| document.encounters.len() >= 2 && document.cleared.is_empty())
            .expect("a run through a zone of several fights");
        let whole = run.room_for_everyone(ZONES).expect("reads");
        assert!(whole > run.cast.len(), "{name}: its encounters bring someone");
        let mut fought = run.clone();
        fought.cleared = vec![run.encounters[0].clone()];
        let fewer = fought.room_for_everyone(ZONES).expect("reads");
        assert!(fewer < whole, "{name}: {fewer} with its first encounter fought, of {whole}");
        assert_eq!(fewer, fought.cast.len() + to_come(&fought), "{name}: as the zone counts them");
        fought.cleared.clone_from(&run.encounters);
        let room = fought.room_for_everyone(ZONES).expect("reads");
        assert_eq!(room, run.cast.len(), "{name}: every encounter fought: its cast alone");
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
        court.set_up(&mut sim, ZONES).expect("sets up");
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
        trip.set_up(&mut sim, ZONES).expect("sets up");
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
            // A zone code builds names no colliders (M5 B5).
            let colliders = (!document.colliders.is_empty()).then_some(&document.colliders);
            for path in document.moves.iter().chain(&document.enemies).chain(colliders) {
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
            document.set_up(&mut sim, ZONES).unwrap_or_else(|error| panic!("{name}: {error}"));
            let kinds = document.kinds_of_enemy().unwrap_or_else(|error| panic!("{name}: {error}"));
            let roster = document.roster().expect("it plans");
            assert_eq!(kinds.len(), roster.enemies.len(), "{name}");
            for (kind, path) in kinds.iter().zip(&roster.enemies) {
                let mind = sim.move_library().mind(kind.set).unwrap_or_else(|| panic!("{name}: {path}"));
                let compiled = EnemyDocument::parse(named(path).expect("held")).expect("reads");
                assert_eq!(mind.boss(), compiled.boss.then_some(compiled.name.as_str()), "{name}: {path}");
                assert_eq!(kind.kind, id_of(path).expect("a sidecar"), "{name}: {path}");
                assert_eq!(kind.name, compiled.name, "{name}: {path}");
                assert_eq!(kinds.iter().filter(|other| other.set == kind.set).count(), 1, "{name}: {path}");
            }
        }
        let boss = SCENARIO_DOCUMENTS.iter().find(|(name, _)| *name == "arena-boss").expect("held").1;
        let boss: ScenarioDocument = sapphire_serialize::from_ron(boss).expect("reads");
        let captain = id_of("content/combat/enemy/captain.enemy.ron").expect("a sidecar");
        let kinds = boss.kinds_of_enemy().expect("it plans");
        let set = crate::moves::MoveSetId(1);
        assert_eq!(kinds, [KindOfEnemy { set, kind: captain, name: "boss-captain".to_owned() }]);
    }

    /// A cast member's playable kind is read as its scenario sets up, and one it can't draw as is that set-up's
    /// error, naming the member and what is wrong, never a panic: `sapphire_replay record` stopped on a bad
    /// `.playable.ron` before it said anything of its own (M5 Z2's rehearsal).
    #[test]
    fn a_kind_the_cast_cannot_draw_as_fails_its_set_up_naming_it() {
        let text = SCENARIO_DOCUMENTS.iter().find(|(name, _)| *name == "party-three-kinds").expect("held").1;
        let document: ScenarioDocument = sapphire_serialize::from_ron(text).expect("reads");
        let place = document.cast.iter().position(|member| member.playable.is_some()).expect("a kind");
        let field = format!("cast[{place}].playable");
        let set_up = |document: &ScenarioDocument| {
            let mut sim = Simulation::new(document.config());
            document.set_up(&mut sim, ZONES).map(drop).map_err(|error| error.to_string())
        };
        set_up(&document).expect("sets up as it ships");

        let mut nobody = document.clone();
        nobody.cast[place].playable = Some("nobody".to_owned());
        let error = set_up(&nobody).expect_err("no such kind");
        assert!(error.contains(&field) && error.contains("nobody.playable.ron"), "{error}");

        // A kind this build holds, on a member playing another kind's set.
        let other = document
            .cast
            .iter()
            .filter_map(|member| member.playable.clone())
            .find(|name| Some(name) != document.cast[place].playable.as_ref())
            .expect("a second kind");
        let mut swapped = document.clone();
        swapped.cast[place].playable = Some(other.clone());
        let error = set_up(&swapped).expect_err("another kind's moves");
        assert!(error.contains(&field) && error.contains(&format!("`{other}` plays other moves")), "{error}");
    }

    /// A playable kind is found by its file's stem, as `--playable` names it (M5 C4), and one this build
    /// doesn't hold is refused by name.
    #[test]
    fn a_playable_kind_is_found_by_its_name() {
        let aster = playable_kind_named("hero").expect("Aster");
        let id = id_of("content/combat/hero/hero.playable.ron").expect("a sidecar");
        assert_eq!(aster, KindOfPlayable { kind: id, name: "character-name-aster".to_owned() });
        let error = playable_kind_named("nobody").expect_err("not held").to_string();
        assert!(error.contains("nobody.playable.ron"), "{error}");
    }

    /// Every kind of enemy under `content/combat/` is held, with the moves it plays.
    #[test]
    fn every_kind_of_enemy_is_held_with_its_moves() {
        let kinds = kinds_held().expect("they read");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut on_disk = Vec::new();
        let mut folders = vec![root.join("content/combat")];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(folder).expect("a folder") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    folders.push(path);
                } else if path.to_string_lossy().ends_with(".enemy.ron") {
                    let relative = path.strip_prefix(&root).expect("under the root");
                    on_disk.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        on_disk.sort();
        let mut held: Vec<String> = kinds.iter().map(|(kind, _)| (*kind).to_owned()).collect();
        held.sort();
        assert_eq!(held, on_disk);
        for (kind, moves) in kinds {
            let document = EnemyDocument::parse(named(kind).expect("held")).expect("reads");
            assert_eq!(id_of(moves).ok(), Some(document.moves), "{kind}");
        }
    }

    /// A scenario on a zone needn't list what its encounters spawn, cleared or not (M3 C2): the zone's
    /// spawners say, and the kinds of those names come after what it lists, with their moves and scripts.
    /// One that lists them gets the same kinds. Held for every scenario this build has on a zone with
    /// encounters, with only the kinds its cast plays listed, and again with every encounter fought.
    #[test]
    fn a_zones_encounters_bring_the_kinds_they_spawn() {
        let ids = |document: &ScenarioDocument| {
            let mut ids: Vec<AssetId> =
                document.kinds_of_enemy().expect("it plans").iter().map(|kind| kind.kind).collect();
            ids.sort_unstable();
            ids
        };
        let mut zones = 0;
        for (name, text) in SCENARIO_DOCUMENTS {
            let listed: ScenarioDocument = sapphire_serialize::from_ron(text).expect("reads");
            let Some(zone) = listed.zone.as_deref().filter(|_| !listed.encounters.is_empty()) else {
                continue;
            };
            zones += 1;
            let zone = ZoneDocument::from_ron(named(zone).expect("held")).expect("reads");
            let cast: Vec<&str> = listed.cast.iter().map(|member| member.moves.as_str()).collect();
            let plays = |path: &&String| cast.contains(&kind_of(path));

            // The kinds its cast plays, and the kind each spawner its encounters call names.
            let mut kinds: Vec<&str> = listed.enemies.iter().filter(plays).map(String::as_str).collect();
            for path in &listed.encounters {
                for wave in EncounterDocument::from_ron(named(path).expect("held")).expect("reads").waves {
                    for called in &wave.spawners {
                        let spawner = zone.spawners.iter().find(|spawner| spawner.name == *called);
                        let spawner = spawner.unwrap_or_else(|| panic!("{name}: {path} calls {called}"));
                        kinds.extend(kind_named(&spawner.spawns));
                    }
                }
            }
            let mut expected: Vec<AssetId> =
                kinds.iter().map(|path| id_of(path).expect("a sidecar")).collect();
            expected.sort_unstable();
            expected.dedup();
            let all = ids(&listed);
            assert!(expected.iter().all(|kind| all.contains(kind)), "{name}: listing them changes nothing");

            let mut bare = listed.clone();
            bare.enemies.retain(|path| plays(&path));
            bare.moves.retain(|path| plays(&path));
            let mut fought = bare.clone();
            fought.cleared.clone_from(&fought.encounters);
            for document in [&bare, &fought] {
                let mut sim = Simulation::new(document.config());
                document.set_up(&mut sim, ZONES).unwrap_or_else(|error| panic!("{name}: {error}"));
                assert_eq!(ids(document), expected, "{name}: the kinds it spawns, and its cast's");
                let roster = document.roster().expect("it plans");
                let lists: Vec<&str> = document.moves.iter().map(String::as_str).collect();
                assert!(roster.moves.starts_with(&lists), "{name}: what it lists comes first");

                // Each kind with its moves, and its script on its set: exit test 2 holds that no code names a
                // boss's.
                let scripts = document.scripts().expect("it can say");
                for kind in document.kinds_of_enemy().expect("it plans") {
                    let path = roster.enemies.iter().find(|path| id_of(path).ok() == Some(kind.kind));
                    let path = path.expect("planned");
                    let enemy = EnemyDocument::parse(named(path).expect("held")).expect("reads");
                    let moves = moves_with_id(enemy.moves).expect("reads").expect("held");
                    assert!(roster.moves.contains(&moves), "{name}: {path}'s moves");
                    let bound = scripts.iter().filter(|script| script.set == kind.set);
                    let bound: Vec<AssetId> =
                        bound.map(|script| id_of(script.path).expect("a sidecar")).collect();
                    assert_eq!(bound, Vec::from_iter(enemy.script), "{name}: {path}'s script");
                }
                for script in &scripts {
                    assert!(script.file.ends_with(".luau"), "{name}: {script:?}");
                    assert!(sim.move_library().mind(script.set).is_some(), "{name}: {script:?}");
                }
            }
        }
        assert!(zones > 0, "a scenario on a zone with encounters");
    }

    /// A second kind on moves another kind has goes on a copy of them, after every set `moves` lists.
    #[test]
    fn a_second_kind_on_the_same_moves_plays_a_copy_of_them() {
        let mut document = arena_fight();
        document.enemies.push("content/combat/enemy/captain.enemy.ron".to_owned());
        let mut sim = Simulation::new(document.config());
        document.set_up(&mut sim, ZONES).expect("it starts");
        let kinds = document.kinds_of_enemy().expect("it plans");
        let sets: Vec<u32> = kinds.iter().map(|kind| kind.set.0).collect();
        assert_eq!(sets, [1, 2]);
        assert_eq!(sim.move_library().set_count(), 3);
        assert_eq!(sim.move_library().mind(kinds[1].set).and_then(|mind| mind.boss()), Some("boss-captain"));
    }

    /// The benchmark district's fight, as the replay runner holds it (M5 B5).
    fn district() -> ScenarioDocument {
        let text = SCENARIO_DOCUMENTS.iter().find(|(name, _)| *name == "district-encounter").expect("held").1;
        sapphire_serialize::from_ron(text).expect("reads")
    }

    /// The district's fight is what its document says: the party of three its cast names standing at the
    /// avenue's start with the first on the field, the zone the builder makes with the navmesh beside it, and
    /// one encounter of twelve to come, of the kind it names. With no builder it's refused, saying who hands one
    /// over, which is what a game's build does (M5 B5).
    #[test]
    fn the_district_stands_its_party_and_brings_its_twelve() {
        use sapphire_synthetic_zone::district::{AVENUE_START, PLAZA_EVENT};
        let document = district();
        let mut sim = Simulation::new(document.config());
        let driven = document.set_up(&mut sim, ZONES).expect("it starts");
        let cast: Vec<ActorId> = sim.party().slots().map(|slot| slot.actor).collect();
        assert_eq!(cast.len(), 3);
        assert_eq!(driven, [cast[0]], "input names the party's first");
        assert_eq!(sim.party().active(), Some(cast[0]));
        let zone = sim.zone().expect("in the district");
        let start =
            zone.checkpoint("plaza").and_then(|id| zone.checkpoint_place(id)).expect("its checkpoint");
        assert_eq!(start.position, Vec3::new(0.0, 0.0, 28.0));
        let lead = sim.actors().find(|actor| actor.id == cast[0]).expect("there");
        assert_eq!(lead.position, Vec3::new(0.0, 0.0, 60.0), "at `{AVENUE_START}`");
        assert!(sim.navigation().is_some(), "the builder's navmesh");
        assert_eq!(zone.encounters().len(), 1);
        assert_eq!(zone.planned(), 12);
        assert_eq!(zone.fires(Name::new(PLAZA_EVENT)), 0);
        assert_eq!(document.room_for_everyone(ZONES).expect("reads"), 15);
        assert_eq!(document.kinds_of_enemy().expect("it plans").len(), 1, "warriors alone");

        let mut bare = Simulation::new(document.config());
        let error = document.set_up(&mut bare, &[]).expect_err("no builder").to_string();
        assert!(error.contains("`built`") && error.contains("builds no zone `district`"), "{error}");
        assert!(error.contains("hand its builder over"), "{error}");
    }

    /// A party in a zone (M5 B5): a swap in the plaza's trigger volume starts its encounter once, since the
    /// trigger watches whoever's on the field and the one coming in stands where the one leaving did; and a return
    /// to a checkpoint takes whoever's on the field there, not the party's first.
    #[test]
    fn a_swap_in_a_trigger_starts_its_encounter_once_and_a_return_takes_whos_on_the_field() {
        use sapphire_synthetic_zone::district::PLAZA_EVENT;
        let document = district();
        let mut sim = Simulation::new(document.config());
        document.set_up(&mut sim, ZONES).expect("it starts");
        let cast: Vec<ActorId> = sim.party().slots().map(|slot| slot.actor).collect();
        // Into the plaza's volume, pressing swap on the step that finds it there.
        sim.teleport(cast[0], Place { position: Vec3::new(0.0, 0.0, 20.0), yaw: std::f32::consts::PI })
            .expect("it moves");
        let mut swap = TickActions::NEUTRAL;
        swap.set(cast[0], ActorActions { buttons: Buttons::SWAP, stick: Stick::CENTER }).expect("room");
        sim.step(&swap);
        assert_eq!(sim.party().active(), Some(cast[1]), "swapped in the volume");
        for _ in 0..240 {
            sim.step(&TickActions::NEUTRAL);
        }
        let zone = sim.zone().expect("in the district");
        let stats = zone.stats();
        assert_eq!((stats.triggers_fired, stats.encounters_started), (1, 1));
        assert_eq!(zone.fires(Name::new(PLAZA_EVENT)), 1);
        assert_eq!(zone.encounters()[0].phase(), crate::EncounterPhase::Running);
        assert_eq!(stats.spawned, 12, "the whole wave");

        let plaza = zone.checkpoint("plaza").expect("its checkpoint");
        let place = zone.checkpoint_place(plaza).expect("a place");
        assert_eq!(sim.return_to_checkpoint(plaza).expect("someone's playing"), cast[1]);
        let back = sim.actors().find(|actor| actor.id == cast[1]).expect("there");
        assert_eq!(back.position, place.position);
    }
}
