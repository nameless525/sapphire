//! Scenarios: the named starting worlds replays run from.
//!
//! A replay records the name of one, and running it means setting up the same world. **To add one,**
//! write a document in `content/scenarios/` ([`document`](super::document)), and no Rust changes. A world only
//! code can say gets its setup and a row in [`CODE_SCENARIOS`]. A setup does the same things in the same
//! order every run: the collision world and controller profiles, move sets, then spawns.
//!
//! One replay file holds one [`tables_hash`](Simulation::tables_hash) for every architecture, so a
//! scenario's move sets must come out byte for byte the same on every architecture: from a document,
//! as [`test_moves`] does, or from committed cooked bytes, and in scalars and `sapphire_math::det` if
//! a setup computes anything, never through glam's SIMD types. Anything else a run depends on belongs
//! in that hash too. Its collision world is already there: `arena`'s comes from a compiled-in
//! document, whose cook goes through `det` for that reason, and the hash takes its statics' digest.

use std::sync::LazyLock;

use sapphire_colliders::{ColliderError, ColliderSet};
use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_moves::{MoveSet, MoveSetDocument, MoveSetError};
use sapphire_physics::{
    ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape, WorldError,
};
use sapphire_serialize::AssetId;
use sapphire_serialize::cooked::AlignedVec;
use sapphire_time::ScaleSource;
use sapphire_timeline::{TimelineDocument, TimelineError, TimelineSimulation};
use sapphire_zone::{Zone, ZoneError};

use super::document::ScenarioDocument;
use crate::actions::{Buttons, Controls};
use crate::actors::{ActorId, ActorRequests, ActorSpawn, ActorTable, SpawnError};
use crate::controller::{ControllerProfile, PhysicsWorldError, ProfileError, ProfileId};
use crate::encounter::ZoneStartError;
use crate::faction::Faction;
use crate::movement::ActorTransform;
use crate::moves::{MoveRunner, MoveSetId, WarpTarget};
use crate::party::{PARTY_SLOTS, Party, PartyError, PartyTuning};
use crate::resolve::SlowMotion;
use crate::simulation::{CombatSet, SimConfig, SimRng, Simulation};
use crate::stats::{BUDGET_DESKTOP_LOW, PHYSICS_BUDGET_DESKTOP_LOW};
use crate::zone::{ZoneGround, ZoneLoadError};

/// The test content's document: see that file for its numbers. The real moves came beside it (M2 O3,
/// `arena-fight`), and the older scenarios stay on it, so tuning those moves moves none of their replays.
const TEST_MOVES: &str = include_str!("../../../sapphire_moves/test_content/slice.moves.ron");

/// Synthetic timelines, for the scripts that hold a skip to the same checksums as a watch.
const CINEMATIC: &str = include_str!("../../../sapphire_timeline/test_content/cinematic.timeline.ron");
const CUTSCENE: &str = include_str!("../../../sapphire_timeline/test_content/cutscene.timeline.ron");

/// That document, from the repository's root.
const TEST_MOVES_PATH: &str = "crates/sapphire_moves/test_content/slice.moves.ron";

/// The enemy's document (M2 O4), brain and all, as this build holds it: a replay needs no assets.
pub const WARRIOR_MOVES: &str = include_str!("../../../../content/combat/enemy/warrior.moves.ron");

/// The player's and the warrior's documents, from the repository's root: what [`PreparedFight`] finds a live
/// edit of either by.
const PLAYER_MOVES_PATH: &str = "content/combat/player/player.moves.ron";
const WARRIOR_MOVES_PATH: &str = "content/combat/enemy/warrior.moves.ron";

/// The cinematic the player's ultimate names (M3 Q3b), as this build holds it: every fight on the player's
/// moves loads it under [`ULTIMATE_CINEMATIC`], wherever it is, a scenario document's among them.
const ULTIMATE: &str = include_str!("../../../../content/combat/player/ultimate.timeline.ron");

/// The name the player's `ultimate` move gives its cinematic.
pub const ULTIMATE_CINEMATIC: &str = "ultimate";

/// A zone as a cook made it, its simulation's half: see [`zone_trip`].
#[derive(Clone, Copy, Debug)]
pub struct BakedZone {
    /// The zone's source, by its sidecar's ID.
    pub asset: AssetId,
    /// The source's stem, from which its pack and its documents' names come.
    pub stem: &'static str,
    pub zone: &'static [u8],
    pub colliders: &'static [u8],
    pub navmesh: &'static [u8],
}

impl BakedZone {
    /// What a simulation runs on while it's there.
    pub fn load(&self) -> Result<ZoneGround, ZoneLoadError> {
        let aligned = |bytes: &[u8]| {
            let mut aligned = AlignedVec::<16>::with_capacity(bytes.len());
            aligned.extend_from_slice(bytes);
            aligned
        };
        let (zone, colliders, navmesh) = (aligned(self.zone), aligned(self.colliders), aligned(self.navmesh));
        let simulation = &Zone::read(&zone)?.simulation;
        let (collider_id, navmesh_id) =
            (simulation.colliders.as_ref().copied(), simulation.navigation.as_ref().copied());
        ZoneGround::from_cooked(self.asset, &zone, |id| {
            if Some(id) == collider_id {
                Some(&colliders[..])
            } else if Some(id) == navmesh_id {
                Some(&navmesh[..])
            } else {
                None
            }
        })
    }
}

/// What the `zone-trip` scenario's shelf holds, in the script's order.
pub const BAKED_ZONES: [BakedZone; 2] = [
    BakedZone {
        asset: AssetId::from_text("5f3ac6de-0b7c-4f21-9d3e-7a1e64c2b805"),
        stem: "proving_hub",
        zone: include_bytes!("../../baked/zones/proving_hub.szone"),
        colliders: include_bytes!("../../baked/zones/proving_hub.scol"),
        navmesh: include_bytes!("../../baked/zones/proving_hub.snav"),
    },
    BakedZone {
        asset: AssetId::from_text("e8c21831-7d98-4a79-b592-912b806cf3e2"),
        stem: "proving_ground",
        zone: include_bytes!("../../baked/zones/proving_ground.szone"),
        colliders: include_bytes!("../../baked/zones/proving_ground.scol"),
        navmesh: include_bytes!("../../baked/zones/proving_ground.snav"),
    },
];

/// What the party's tests add to [`TEST_MOVES`]: a swap and three assists.
const PARTY_MOVES: &str = include_str!("../../../sapphire_moves/test_content/party.moves.ron");

/// Why a scenario couldn't set its world up, as with a replay that asks for too small a pool.
#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    #[error(transparent)]
    Moves(#[from] MoveSetError),
    #[error(transparent)]
    Colliders(#[from] WorldError),
    #[error(transparent)]
    ColliderDocument(#[from] ColliderError),
    #[error(transparent)]
    World(#[from] PhysicsWorldError),
    #[error(transparent)]
    Profile(#[from] ProfileError),
    #[error(transparent)]
    Party(#[from] PartyError),
    #[error(transparent)]
    NavMesh(#[from] sapphire_navmesh::NavMeshError),
    #[error(transparent)]
    Timeline(#[from] TimelineError),
    #[error(transparent)]
    Cinematic(#[from] crate::CinematicError),
    #[error("the zone has no spawner named `{0}`")]
    MissingSpawner(&'static str),
    #[error("the arena couldn't bind its runtime profiles: {0}")]
    RuntimeProfiles(String),
    #[error(transparent)]
    ZoneDocument(#[from] ZoneError),
    #[error(transparent)]
    Zone(#[from] ZoneStartError),
    /// A scenario's document names something it can't have.
    #[error("`{field}`: {problem}")]
    Document { field: String, problem: String },
    /// The script a setup binds didn't cook or load.
    #[error("script {name}: {problem}")]
    Script { name: String, problem: String },
    #[error(transparent)]
    ZoneLoad(#[from] ZoneLoadError),
}

/// A named starting world.
#[derive(Clone, Copy, Debug)]
pub struct Scenario {
    /// Its name in replay files: see [`super::is_valid_name`].
    pub name: &'static str,
    /// A line on what it holds, for listings.
    pub about: &'static str,
    /// What a new recording starts from. A replay keeps its own copy, so changing this moves
    /// nothing recorded.
    pub config: SimConfig,
    /// The documents its move sets come from, from the repository's root, in the order its setup adds
    /// them: what live tuning goes by (M2 T2), and what `link watch` watches. A test holds them to
    /// what the setup loads.
    pub documents: &'static [&'static str],
    /// Loads the move sets and spawns the actors.
    pub setup: ScenarioSetup,
}

/// How a scenario makes its world.
#[derive(Clone, Copy, Debug)]
pub enum ScenarioSetup {
    /// By a function, for a world only code can say.
    Code(fn(&mut Simulation) -> Result<Vec<ActorId>, ScenarioError>),
    /// By what a document says.
    Document(&'static ScenarioDocument),
}

impl Scenario {
    /// A new simulation set up as this scenario says, with `config`, and the actors input drives.
    pub fn start(&self, config: SimConfig) -> Result<(Simulation, Vec<ActorId>), ScenarioError> {
        let mut sim = Simulation::new(config);
        let cast = self.set_up(&mut sim)?;
        sim.check_cinematics()?;
        Ok((sim, cast))
    }

    /// What it was written from, if it's a document.
    #[must_use]
    pub const fn document(&self) -> Option<&'static ScenarioDocument> {
        match self.setup {
            ScenarioSetup::Code(_) => None,
            ScenarioSetup::Document(document) => Some(document),
        }
    }

    /// Loads its move sets and spawns its actors in `sim`, and returns the actors input drives, the player
    /// first, for an [`ActionSource`](super::ActionSource).
    pub fn set_up(&self, sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
        match self.setup {
            ScenarioSetup::Code(setup) => setup(sim),
            ScenarioSetup::Document(document) => document.set_up(sim),
        }
    }

    /// The set that plays the document with this file name, in this scenario's simulations.
    #[must_use]
    pub fn set_of(&self, file_name: &str) -> Option<MoveSetId> {
        let index = self.documents.iter().position(|path| document_file_name(path) == file_name)?;
        Some(MoveSetId(u32::try_from(index).ok()?))
    }

    /// The zone it stands in, by the stem the app's content finds it by, if it stands in one: the stage
    /// opens that zone around the fight and stands its ground on the zone's colliders.
    #[must_use]
    pub fn zone(&self) -> Option<&'static str> {
        (self.name == super::ward::WARD_COURT).then_some(super::ward::WARD_ZONE_STEM)
    }
}

/// A document's file name, from its path from the repository's root: what a push names it by.
#[must_use]
pub fn document_file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The side the arena's player is on: a party is all of it.
pub const PLAYERS: Faction = Faction::new(1).expect("a faction's number");
/// The side the arena's warriors are on, who leave each other alone.
pub const WARRIORS: Faction = Faction::new(2).expect("a faction's number");
/// A second side of warriors in `arena-skirmish`, who fight the first as readily as the player.
pub const RIVALS: Faction = Faction::new(3).expect("a faction's number");

/// The scenarios that need code: a setup adds a system, or computes who stands where. The rest are documents
/// ([`document`](super::document)), and a new one should be.
const CODE_SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "melee",
        about: "three characters on flat ground within reach of each other on the test moves, warps aimed round \
                the ring, and buttons that slow clocks and spawn actors",
        config: SimConfig {
            seed: 7,
            max_actors: 16,
            step_budget: BUDGET_DESKTOP_LOW,
            physics_budget: PHYSICS_BUDGET_DESKTOP_LOW,
        },
        documents: &[TEST_MOVES_PATH],
        setup: ScenarioSetup::Code(melee),
    },
    Scenario {
        name: "duel",
        about: "a player and an opponent on flat ground and the test moves, 4 m apart and facing, each aiming \
                warps at the other",
        config: SimConfig {
            seed: 7,
            max_actors: 2,
            step_budget: BUDGET_DESKTOP_LOW,
            physics_budget: PHYSICS_BUDGET_DESKTOP_LOW,
        },
        documents: &[TEST_MOVES_PATH],
        setup: ScenarioSetup::Code(duel),
    },
    Scenario {
        name: "nav-chase",
        about: "the arena's square on its real navmesh: input drives the player, and 64 chasers follow paths \
                to them on the budget of 1024 nodes a tick, through the stick and the controller",
        config: SimConfig {
            seed: 11,
            max_actors: 1 + super::chase::CHASERS as u32,
            step_budget: BUDGET_DESKTOP_LOW,
            physics_budget: PHYSICS_BUDGET_DESKTOP_LOW,
        },
        documents: &[TEST_MOVES_PATH],
        setup: ScenarioSetup::Code(super::chase::nav_chase),
    },
    Scenario {
        name: "timelines",
        about: "`duel`, with a synthetic cinematic and a synthetic cutscene loaded, for the scripts that hold a \
                skip to the same checksums as a watch",
        config: SimConfig {
            seed: 7,
            max_actors: 2,
            step_budget: BUDGET_DESKTOP_LOW,
            physics_budget: PHYSICS_BUDGET_DESKTOP_LOW,
        },
        documents: &[TEST_MOVES_PATH],
        setup: ScenarioSetup::Code(timelines),
    },
    Scenario {
        name: super::ward::WARD_COURT,
        about: "`arena-fight` in the Lantern Ward's temple court: the zone's colliders, the player at the court's \
                checkpoint and the warrior at its far end, with the court's gate cutscene loaded to open on",
        config: SimConfig {
            seed: 7,
            max_actors: 8,
            step_budget: BUDGET_DESKTOP_LOW,
            physics_budget: PHYSICS_BUDGET_DESKTOP_LOW,
        },
        documents: &[PLAYER_MOVES_PATH, WARRIOR_MOVES_PATH],
        setup: ScenarioSetup::Code(super::ward::ward_court),
    },
    Scenario {
        name: "zone-trip",
        about: "`arena-fight`, with the proving hub and the proving ground on the shelf: the warrior stays behind \
                at the first entry, and the player goes on alone",
        config: SimConfig {
            seed: 7,
            max_actors: 8,
            step_budget: BUDGET_DESKTOP_LOW,
            physics_budget: PHYSICS_BUDGET_DESKTOP_LOW,
        },
        documents: &[PLAYER_MOVES_PATH, WARRIOR_MOVES_PATH],
        setup: ScenarioSetup::Code(zone_trip),
    },
];

/// The scenario documents this build holds, read. A bad one stops here, naming itself and the field.
static DOCUMENTS: LazyLock<Vec<(&'static str, ScenarioDocument)>> = LazyLock::new(super::document::read_all);

/// Each document's move sets' paths, for [`Scenario::documents`].
static DOCUMENT_PATHS: LazyLock<Vec<Vec<&'static str>>> = LazyLock::new(|| {
    let paths = |(_, document): &'static (&str, ScenarioDocument)| {
        document.moves.iter().map(String::as_str).collect()
    };
    DOCUMENTS.iter().map(paths).collect()
});

/// Every scenario: those that take code, then the documents by name.
pub static SCENARIOS: LazyLock<Vec<Scenario>> = LazyLock::new(|| {
    let documents = DOCUMENTS.iter().zip(DOCUMENT_PATHS.iter()).map(|((name, document), paths)| Scenario {
        name,
        about: &document.about,
        config: document.config(),
        documents: paths,
        setup: ScenarioSetup::Document(document),
    });
    CODE_SCENARIOS.iter().copied().chain(documents).collect()
});

/// The scenario named `name`, among `scenarios`.
#[must_use]
pub fn find_scenario<'a>(scenarios: &'a [Scenario], name: &str) -> Option<&'a Scenario> {
    scenarios.iter().find(|scenario| scenario.name == name)
}

/// The test content's moves: a three-hit string, a dash attack, a dodge and a skill.
pub fn test_moves() -> Result<MoveSet, MoveSetError> {
    MoveSetDocument::parse(TEST_MOVES)?.compile()
}

/// The real enemy's moves, fighter and brain: `content/combat/enemy/warrior.moves.ron`.
pub fn warrior_moves() -> Result<MoveSet, MoveSetError> {
    MoveSetDocument::parse(WARRIOR_MOVES)?.compile()
}

/// The player's moves, from the simulation's cooked copy of them.
pub fn player_moves() -> Result<MoveSet, MoveSetError> {
    super::document::baked_moves("player").expect("`arena-fight` names them, so this build holds them")
}

/// The simulation half of the player's ultimate's cinematic, which a fight on the player's moves loads
/// before them under [`ULTIMATE_CINEMATIC`].
pub fn ultimate_cinematic() -> Result<TimelineSimulation, ScenarioError> {
    Ok(TimelineDocument::from_ron(ULTIMATE)?.cook()?.simulation)
}

/// What a party's swap takes in a test: a second on the field, and two and a half off it.
pub const PARTY_TUNING: PartyTuning = PartyTuning { swap_cooldown: 60, rest: 150 };

/// Each [`party_document`] member's health: little enough that a test's fight reaches a defeat soon, and
/// different, so the three can't stand in for each other in a checksum.
const PARTY_HEALTH: [u32; PARTY_SLOTS] = [60, 90, 120];

/// A party member who differs from the others, for tests and benches, where identical members would hide an
/// order leak: the test content, with `party.moves.ron`'s swap, its assist as that member's `swap_in`,
/// and its own health. No scenario plays these, because a scenario's sets are each one document (M2 T2).
pub fn party_document(member: usize) -> Result<MoveSetDocument, MoveSetError> {
    let mut document = MoveSetDocument::parse(TEST_MOVES)?;
    let party = MoveSetDocument::parse(PARTY_MOVES)?;
    document.intents.extend(party.intents);
    let mut assist = party.moves[member % PARTY_SLOTS].clone();
    assist.name = "swap_in".to_owned();
    document.moves.push(assist);
    if let Some(fighter) = &mut document.fighter {
        fighter.health = PARTY_HEALTH[member % PARTY_SLOTS];
    }
    Ok(document)
}

/// A [`party_document`] member's moves.
pub fn party_moves(member: usize) -> Result<MoveSet, MoveSetError> {
    party_document(member)?.compile()
}

/// Steps a [`Buttons::ULTIMATE`] press slows the world for, in [`melee`], and to how many thousandths.
const MELEE_SLOW_STEPS: u16 = 45;
const MELEE_SLOW_PER_MILLE: u32 = 200;

/// The first three actors' buttons that do what no move in the test content does yet: [`Buttons::SWAP`]
/// toggles slowing the actor, [`Buttons::ULTIMATE`] slows the world for a while, as a perfect dodge
/// does, and [`Buttons::JUMP`] jumps and spawns another actor at a random yaw, which plays a skill and
/// leaves, with no capsule, as what a fight spawns that isn't a body has none. Hits freeze clocks on
/// their own.
pub fn melee_buttons(
    table: Res<'_, ActorTable>,
    mut requests: ResMut<'_, ActorRequests>,
    mut rng: ResMut<'_, SimRng>,
    mut slow: ResMut<'_, SlowMotion>,
    mut actors: Query<'_, '_, (&Controls, &mut EntityClock, &mut MoveRunner, &ActorTransform)>,
) {
    for (id, entity) in table.iter() {
        let Ok((controls, mut clock, mut runner, transform)) = actors.get_mut(entity) else { continue };
        // Actors past the first three are the ones spawned here.
        if id.raw() > 3 {
            if runner.started() == 0 {
                runner.force(Name::new("skill"));
            } else if !runner.is_running() {
                requests.despawn(id, &table);
            }
            continue;
        }
        let pressed = controls.pressed();
        if pressed.contains(Buttons::SWAP) && clock.0.remove_source(ScaleSource::Gameplay) == 0 {
            // Toggled, so the stack never fills.
            let _ = clock.0.push_scale(ScaleSource::Gameplay, 500);
        }
        if pressed.contains(Buttons::ULTIMATE) {
            slow.request(MELEE_SLOW_PER_MILLE, MELEE_SLOW_STEPS);
        }
        if pressed.contains(Buttons::JUMP) {
            let yaw = rng.0.unit_f32() * std::f32::consts::TAU;
            // A full pool refuses and counts it.
            let spawn = ActorSpawn {
                position: transform.position,
                yaw,
                moves: runner.set(),
                controller: None,
                ..ActorSpawn::default()
            };
            let _ = requests.spawn(spawn);
        }
    }
}

/// Each of `cast` aims its warps at the next, round the ring.
fn aim_round(sim: &mut Simulation, cast: &[ActorId]) {
    for (index, &actor) in cast.iter().enumerate() {
        sim.set_warp_target(actor, WarpTarget::Actor(cast[(index + 1) % cast.len()]));
    }
}

/// Flat ground for the test fights, 400 m across with its top at y = 0, and a profile of default tuning
/// for everyone on it. M2 O5's arena is the real stage.
fn flat_ground(sim: &mut Simulation) -> Result<ProfileId, ScenarioError> {
    let floor = StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(200.0, 0.5, 200.0) }),
        pose: Pose::at(Vec3::new(0.0, -0.5, 0.0)),
        memberships: Group::World.mask(),
    };
    sim.set_physics_world(PhysicsWorld::new(vec![floor], sim.body_capacity())?)?;
    Ok(sim.add_controller_profile(&ControllerProfile::default())?)
}

/// Adds [`melee_buttons`] to `sim`, after the hits and perfect dodges of [`CombatSet::Resolve`]: it forces a
/// skill on an actor it spawned, as a hit can force a reaction, and so the later of the two is the one an
/// actor plays. Without the order, which was later fell to how the schedule happened to sort them, and a
/// new set of systems elsewhere (M3 Q3b's) or a body attached could swap them.
pub fn add_melee_buttons(sim: &mut Simulation) {
    sim.add_combat_system(CombatSet::Resolve, melee_buttons.after(crate::resolve::fire_perfect_dodges));
}

/// Three characters that stand apart and turned differently, with [`melee_buttons`]: everything a step
/// can do to a move, on one world.
pub fn melee(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let controller = Some(flat_ground(sim)?);
    let moves = sim.add_move_set(test_moves()?);
    add_melee_buttons(sim);
    let mut cast = Vec::with_capacity(3);
    for x in [-2.0, 0.0, 2.0] {
        cast.push(sim.spawn(ActorSpawn {
            position: Vec3::new(x, 0.0, 0.0),
            yaw: x * 0.5,
            moves: Some(moves),
            controller,
            ..ActorSpawn::default()
        })?);
    }
    aim_round(sim, &cast);
    Ok(cast)
}

/// A player at the origin and an opponent 4 m ahead of it, facing it: the string, its cancels and the
/// dash attack as a fight has them.
pub fn duel(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    duel_on(sim, test_moves()?)
}

/// [`duel`], with `set` for the test moves.
pub(super) fn duel_on(sim: &mut Simulation, set: MoveSet) -> Result<Vec<ActorId>, ScenarioError> {
    let controller = Some(flat_ground(sim)?);
    let moves = sim.add_move_set(set);
    let player = sim.spawn(ActorSpawn {
        position: Vec3::ZERO,
        yaw: 0.0,
        moves: Some(moves),
        controller,
        ..ActorSpawn::default()
    })?;
    let opponent = sim.spawn(ActorSpawn {
        position: Vec3::new(0.0, 0.0, 4.0),
        yaw: std::f32::consts::PI,
        moves: Some(moves),
        controller,
        ..ActorSpawn::default()
    })?;
    let cast = vec![player, opponent];
    aim_round(sim, &cast);
    Ok(cast)
}

/// [`duel`], with the synthetic timelines loaded: first the cinematic, then the cutscene. Their simulation
/// halves are decimal text cooked to whole numbers, so they come out the same on every architecture.
pub fn timelines(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    for document in [CINEMATIC, CUTSCENE] {
        let timeline = TimelineDocument::from_ron(document)?.cook()?;
        sim.add_timeline(timeline.simulation)?;
    }
    duel(sim)
}

/// Immutable inputs for repeated arena fights. Each start builds a fresh simulation and collision
/// world; preparation belongs to the combat session, so a restart never reparses its built-in content.
pub struct PreparedFight {
    player: MoveSet,
    ultimate: TimelineSimulation,
    warrior: MoveSet,
    colliders: AlignedVec,
    spawns: crate::arena::ArenaSpawns,
    party: bool,
}

impl PreparedFight {
    pub fn new(party: bool) -> Result<Self, ScenarioError> {
        Ok(Self {
            player: player_moves()?,
            ultimate: ultimate_cinematic()?,
            warrior: warrior_moves()?,
            colliders: crate::arena::cooked_colliders()?,
            spawns: crate::arena::spawns(),
            party,
        })
    }

    /// Starts at revision zero of the latest edits, before spawning or drawing a brain's first wait.
    pub fn start(
        &self,
        config: SimConfig,
        live: &super::LiveMoves,
    ) -> Result<(Simulation, Vec<ActorId>), ScenarioError> {
        let mut sim = Simulation::new(config);
        let player = live.latest(document_file_name(PLAYER_MOVES_PATH)).unwrap_or(&self.player);
        let warrior = live.latest(document_file_name(WARRIOR_MOVES_PATH)).unwrap_or(&self.warrior);
        let cast = fight_on(
            &mut sim,
            player.clone(),
            &self.ultimate,
            warrior.clone(),
            &self.colliders,
            self.spawns,
            self.party,
        )?;
        sim.check_cinematics()?;
        Ok((sim, cast))
    }
}

/// A built-in fight with an explicitly resettable lifetime. It exposes only stepping, move tuning
/// and immutable simulation views: arbitrary systems, resources and world edits cannot enter this
/// reset contract. Use a plain Simulation for custom scenarios instead.
///
/// Arbitrary simulation mutation cannot enter the reset boundary:
/// ```compile_fail
/// use sapphire_gameplay::{SimConfig, replay::{LiveMoves, PreparedFight}};
/// let prepared = PreparedFight::new(false).unwrap();
/// let mut fight = prepared.start_resettable(SimConfig::default(), &LiveMoves::default()).unwrap();
/// fight.world_clock_mut(); // mutable Simulation access is deliberately unavailable
/// ```
pub struct FightWorld {
    simulation: Simulation,
    cast: [ActorId; 2],
    config: SimConfig,
    actors: FightActors,
    initial_party: Party,
}

impl std::ops::Deref for FightWorld {
    type Target = Simulation;

    fn deref(&self) -> &Simulation {
        &self.simulation
    }
}

impl FightWorld {
    pub fn cast(&self) -> &[ActorId] {
        &self.cast
    }

    pub fn step(&mut self, actions: &crate::TickActions) {
        self.simulation.step(actions);
    }

    pub fn tune_move_set(&mut self, id: MoveSetId, set: MoveSet) -> Option<crate::Tuned> {
        self.simulation.tune_move_set(id, set)
    }

    /// Makes `tuning` the weights a soft lock is chosen by, between steps. A restart keeps them, as it keeps
    /// tuned moves. Says whether that changed anything ([`Simulation::tune_targeting`]).
    pub fn tune_targeting(&mut self, tuning: crate::targeting::TargetingTuning) -> bool {
        self.simulation.tune_targeting(tuning)
    }

    /// Start again at revision zero of the current moves, allocating no new storage. A live edit
    /// that no longer supports this party is refused before changing any state, allowing recovery.
    pub fn restart(&mut self) -> Result<(), ScenarioError> {
        let party = self.initial_party.is_formed();
        if party {
            crate::party::check_member_set(
                self.cast[0],
                self.simulation.move_library().current(self.actors.player),
            )?;
        }
        self.simulation.restart_builtin_fight(self.config);
        self.cast = self
            .actors
            .spawn(&mut self.simulation, party, Some(self.initial_party))
            .expect("the configured pool and preflighted move sets can spawn this fight");
        Ok(())
    }
}

impl PreparedFight {
    /// Retains immutable inputs as above, and seals the built-in world's mutation boundary so reset
    /// can preserve capacities without retaining gameplay state.
    pub fn start_resettable(
        &self,
        config: SimConfig,
        live: &super::LiveMoves,
    ) -> Result<FightWorld, ScenarioError> {
        self.start_resettable_with_setup(config, live, |_, _| Ok(()))
    }

    /// Configures the new tick-zero arena before sealing its resettable mutation boundary.
    pub fn start_resettable_with_setup(
        &self,
        config: SimConfig,
        live: &super::LiveMoves,
        setup: impl FnOnce(&mut Simulation, &[ActorId]) -> Result<(), ScenarioError>,
    ) -> Result<FightWorld, ScenarioError> {
        let (mut simulation, cast) = self.start(config, live)?;
        setup(&mut simulation, &cast)?;
        let initial_party = *simulation.party();
        let player = simulation.actor(cast[0]).expect("the player was spawned");
        let warrior = simulation.actor(cast[1]).expect("the warrior was spawned");
        let actors = FightActors {
            spawns: self.spawns,
            player: player.moves.set.expect("the player has a set"),
            warrior: warrior.moves.set.expect("the warrior has a set"),
            controller: player.controller.profile().expect("the player has a controller"),
        };
        Ok(FightWorld {
            simulation,
            cast: cast.try_into().expect("a built-in fight's cast has two actors"),
            config,
            actors,
            initial_party,
        })
    }
}

/// A fight, as `arena-fight`'s document says it: for a test that wants one with something of its own added
/// to it, by a setup of its own.
pub fn arena_fight(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    find_scenario(&SCENARIOS, "arena-fight").expect("this build holds its document").set_up(sim)
}

/// [`arena_fight`], with the zones it can go to on the shelf, in the order the `zone-trip` script names them.
///
/// **Copies, as the arena's navmesh and the player's moves are:** a replay needs no assets, so the simulation
/// keeps what a cook made of each, and `tools/sapphire_pipeline/tests/zone_baked.rs` holds those to the
/// documents. A row's digest is what a show beside a run holds the pack's own zone to.
pub fn zone_trip(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = arena_fight(sim)?;
    for baked in &BAKED_ZONES {
        sim.add_zone(baked.load()?);
    }
    Ok(cast)
}

/// The player's moves after the cinematic their ultimate names, so the scenario's check finds it loaded.
pub(super) fn fight_on(
    sim: &mut Simulation,
    player: MoveSet,
    ultimate: &TimelineSimulation,
    warrior: MoveSet,
    cooked: &[u8],
    spawns: crate::arena::ArenaSpawns,
    party: bool,
) -> Result<Vec<ActorId>, ScenarioError> {
    let set = ColliderSet::read(cooked)?;
    sim.set_physics_world(PhysicsWorld::from_cooked(set, sim.body_capacity())?)?;
    sim.add_cinematic(ULTIMATE_CINEMATIC, ultimate.clone())?;
    let actors = FightActors {
        spawns,
        controller: sim.add_controller_profile(&crate::arena::profile())?,
        player: sim.add_move_set(player),
        warrior: sim.add_move_set(warrior),
    };
    Ok(actors.spawn(sim, party, None)?.to_vec())
}

#[derive(Clone, Copy)]
struct FightActors {
    spawns: crate::arena::ArenaSpawns,
    player: MoveSetId,
    warrior: MoveSetId,
    controller: ProfileId,
}

impl FightActors {
    fn spawn(
        self,
        sim: &mut Simulation,
        party: bool,
        initial_party: Option<Party>,
    ) -> Result<[ActorId; 2], ScenarioError> {
        let Self { spawns, player, warrior, controller } = self;
        let (moves, warrior, controller) = (Some(player), Some(warrior), Some(controller));
        let member = ActorSpawn {
            position: spawns.player.position(),
            yaw: spawns.player.yaw(),
            moves,
            controller,
            faction: PLAYERS,
        };
        let first = sim.spawn(member)?;
        let enemy = sim.spawn(ActorSpawn {
            position: spawns.enemy.position(),
            yaw: spawns.enemy.yaw(),
            moves: warrior,
            controller,
            faction: WARRIORS,
        })?;
        // A built-in fight starts with the player's ultimate ready (M3 Q3c): every fight recorded before
        // the meter was, and a full meter stays out of the checksum.
        sim.fill_energy(first);
        if party {
            let members = [first, sim.spawn(member)?, sim.spawn(member)?];
            for &member in &members[1..] {
                sim.fill_energy(member);
            }
            if let Some(formation) = initial_party {
                sim.restore_builtin_party(formation);
            } else {
                sim.form_party(&members, PARTY_TUNING)?;
            }
            for member in members {
                sim.give_soft_lock(member);
            }
        } else {
            sim.prepare_standing_jump(first);
            sim.give_soft_lock(first);
        }
        // An edit can take the brain away, in which case the enemy stands there.
        sim.give_brain(enemy);
        Ok([first, enemy])
    }
}

#[cfg(test)]
mod tests {
    use super::super::is_valid_name;
    use super::*;

    #[test]
    fn reset_preflights_party_edits_and_restarts_from_the_latest_valid_revisions() {
        let prepared = PreparedFight::new(true).expect("prepares");
        let scenario = find_scenario(&SCENARIOS, "party").expect("listed");
        let mut live = super::super::LiveMoves::default();
        let mut world = prepared.start_resettable(scenario.config, &live).expect("starts");
        for tick in 0..120 {
            let actions = super::super::script::fight_back(tick, world.cast());
            world.step(&actions);
        }
        let running_player = player_moves().expect("loads");
        // Removing an assist is supported live; forming the next party must still refuse it.
        let mut document =
            MoveSetDocument::parse(include_str!("../../../../content/combat/player/player.moves.ron"))
                .expect("parses");
        document.moves.retain(|mv| mv.name != "swap_in");
        let no_assist = document.compile_over(&running_player).expect("compiles without the assist");
        assert!(matches!(world.tune_move_set(MoveSetId(0), no_assist), Some(crate::Tuned::Replaced { .. })));
        let before = world.checksum();
        let tick = world.tick();
        assert!(matches!(world.restart(), Err(ScenarioError::Party(PartyError::NoSwapInMove { .. }))));
        assert_eq!(world.checksum(), before, "failed restart is atomic");
        assert_eq!(world.tick(), tick);

        let mut warrior = MoveSetDocument::parse(WARRIOR_MOVES).expect("parses");
        warrior.brain = None;
        let warrior = warrior.compile().expect("compiles");
        world.tune_move_set(MoveSetId(1), warrior.clone()).expect("warrior's set");
        live.remember("warrior.moves.ron", warrior);
        let player = player_moves().expect("loads");
        world.tune_move_set(MoveSetId(0), player).expect("restored player's set");
        world.restart().expect("the repair is taken");
        let (mut fresh, cast) = live.start(scenario, scenario.config).expect("independent fresh setup");
        for tick in 0..300 {
            assert_eq!(world.checksum(), fresh.checksum(), "tick={tick}");
            let actions = super::super::script::fight_back(tick, &cast);
            world.step(&actions);
            fresh.step(&actions);
        }
        world.restart().expect("starts over again");
        let (fresh, _) = live.start(scenario, scenario.config).expect("fresh");
        assert_eq!(world.checksum(), fresh.checksum());
        assert_eq!(world.move_library().current_revision(MoveSetId(0)), Some(0));
        assert_eq!(world.move_library().current_revision(MoveSetId(1)), Some(0));
    }

    #[test]
    fn prepared_fights_make_independent_worlds_on_the_latest_edits() {
        for (name, party) in [("arena-fight", false), ("party", true)] {
            let scenario = find_scenario(&SCENARIOS, name).expect("listed");
            let prepared = PreparedFight::new(party).expect("prepares");
            let mut live = super::super::LiveMoves::default();
            for edited in [false, true] {
                if edited {
                    let mut document = MoveSetDocument::parse(WARRIOR_MOVES).expect("parses");
                    document.brain.as_mut().expect("brain").patience =
                        sapphire_moves::TickRange { min: 500, max: 500 };
                    live.remember("warrior.moves.ron", document.compile().expect("compiles"));
                }
                let (mut expected, cast) = live.start(scenario, scenario.config).expect("fresh setup");
                let (mut actual, prepared_cast) = prepared.start(scenario.config, &live).expect("starts");
                assert_eq!(prepared_cast, cast);
                for tick in 0..300 {
                    assert_eq!(actual.checksum(), expected.checksum(), "{name} edited={edited} tick={tick}");
                    let actions = super::super::script::fight_back(tick, &cast);
                    expected.step(&actions);
                    actual.step(&actions);
                }
                let (again, _) = prepared.start(scenario.config, &live).expect("starts again");
                let (fresh, _) = live.start(scenario, scenario.config).expect("fresh setup");
                assert_eq!(again.tick(), 0);
                assert_eq!(
                    again.checksum(),
                    fresh.checksum(),
                    "the previous mutable world carries nothing over"
                );
            }
        }
    }

    #[test]
    fn every_scenario_has_a_good_unique_name_and_sets_up_from_its_own_config() {
        for (index, scenario) in SCENARIOS.iter().enumerate() {
            assert!(is_valid_name(scenario.name), "{}", scenario.name);
            assert!(!scenario.about.is_empty(), "{}", scenario.name);
            let found = find_scenario(&SCENARIOS, scenario.name).expect("listed");
            assert!(std::ptr::eq(found, &SCENARIOS[index]), "{} is listed twice", scenario.name);
            let (sim, cast) = scenario.start(scenario.config).expect("sets up");
            assert_eq!(sim.tick(), 0);
            assert!(!cast.is_empty(), "{} has no one for input to drive", scenario.name);
        }
        assert!(find_scenario(&SCENARIOS, "no-such-scenario").is_none());
    }

    /// A fight stands on the ground: every one of its cast has a capsule, and lands on its first tick.
    #[test]
    fn the_fights_stand_on_the_ground() {
        for scenario in SCENARIOS.iter() {
            let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
            sim.step(&crate::actions::TickActions::NEUTRAL);
            for actor in cast {
                let view = sim.actor(actor).expect("alive");
                assert!(view.controller.is_active(), "{}", scenario.name);
                assert_eq!(view.controller.state.mode, crate::controller::GroundMode::Grounded);
                assert!(view.position.y.abs() < 1e-5, "{}: {}", scenario.name, view.position.y);
            }
        }
    }

    /// The arena's cast stands where the document says, the player first, each facing the other.
    #[test]
    fn the_arena_spawns_its_two_actors_where_the_document_says() {
        let scenario = find_scenario(&SCENARIOS, "arena").expect("listed");
        let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
        let spawns = crate::arena::spawns();
        assert_eq!(cast.len(), 2);
        sim.step(&crate::actions::TickActions::NEUTRAL);
        for (actor, spot) in cast.iter().zip([spawns.player, spawns.enemy]) {
            let view = sim.actor(*actor).expect("alive");
            assert_eq!(view.position, spot.position());
            assert_eq!(view.yaw, spot.yaw());
        }
    }

    /// One collision world is told from another by a replay's hash: moving a collider says so, rather
    /// than playing on and diverging at the first tick.
    #[test]
    fn the_tables_hash_holds_the_collision_world() {
        let hash_on = |top: f32| {
            let mut sim = Simulation::new(SimConfig::default());
            let floor = StaticCollider {
                id: ColliderId(0),
                shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(200.0, 0.5, 200.0) }),
                pose: Pose::at(Vec3::new(0.0, top - 0.5, 0.0)),
                memberships: Group::World.mask(),
            };
            sim.set_physics_world(PhysicsWorld::new(vec![floor], sim.body_capacity()).expect("builds"))
                .expect("room");
            sim.tables_hash()
        };
        assert_eq!(hash_on(0.0), hash_on(0.0));
        assert_ne!(hash_on(0.0), hash_on(0.25));
    }

    #[test]
    fn too_small_a_pool_is_an_error() {
        let config = SimConfig { max_actors: 1, ..SCENARIOS[0].config };
        assert!(matches!(
            SCENARIOS[0].start(config),
            Err(ScenarioError::Spawn(SpawnError::PoolFull { capacity: 1 }))
        ));
    }
}
