//! Scenarios: the named starting worlds replays run from.
//!
//! A replay records the name of one, and running it means setting up the same world. **To add one,**
//! write its setup and add a row to [`SCENARIOS`]. A setup does the same things in the same order
//! every run: the collision world and controller profiles, move sets, then spawns.
//!
//! One replay file holds one [`tables_hash`](Simulation::tables_hash) for every architecture, so a
//! scenario's move sets must come out byte for byte the same on every architecture: from a document,
//! as [`test_moves`] does, or from committed cooked bytes, and in scalars and `sapphire_math::det` if
//! a setup computes anything, never through glam's SIMD types. Anything else a run depends on belongs
//! in that hash too. Its collision world is already there: [`arena`]'s comes from a compiled-in
//! document, whose cook goes through `det` for that reason, and the hash takes its statics' digest.

use sapphire_colliders::ColliderError;
use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_moves::{MoveSet, MoveSetDocument, MoveSetError};
use sapphire_physics::{
    ColliderId, ColliderSet, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape, WorldError,
};
use sapphire_time::ScaleSource;

use crate::actions::{Buttons, Controls};
use crate::actors::{ActorId, ActorRequests, ActorSpawn, ActorTable, SpawnError};
use crate::controller::{ControllerProfile, PhysicsWorldError, ProfileError, ProfileId};
use crate::movement::ActorTransform;
use crate::moves::{MoveRunner, WarpTarget};
use crate::resolve::SlowMotion;
use crate::simulation::{CombatSet, SimConfig, SimRng, Simulation};
use crate::stats::{BUDGET_DESKTOP_LOW, PHYSICS_BUDGET_DESKTOP_LOW};

/// The test content's document: see that file for its numbers. M2 O3 replaces the content, and the
/// suite is re-recorded then.
const TEST_MOVES: &str = include_str!("../../../sapphire_moves/test_content/slice.moves.ron");

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
    /// Loads the move sets and spawns the actors, and returns the actors input drives, the player
    /// first, for an [`ActionSource`](super::ActionSource).
    pub setup: fn(&mut Simulation) -> Result<Vec<ActorId>, ScenarioError>,
}

impl Scenario {
    /// A new simulation set up as this scenario says, with `config`, and the actors input drives.
    pub fn start(&self, config: SimConfig) -> Result<(Simulation, Vec<ActorId>), ScenarioError> {
        let mut sim = Simulation::new(config);
        let cast = (self.setup)(&mut sim)?;
        Ok((sim, cast))
    }
}

/// Every scenario.
pub const SCENARIOS: &[Scenario] = &[
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
        setup: melee,
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
        setup: duel,
    },
    Scenario {
        name: "arena",
        about: "a player and an opponent at the look-dev town's spawn points, on its square's colliders, with \
                the test moves and each aiming warps at the other",
        config: SimConfig {
            seed: 7,
            max_actors: 8,
            step_budget: BUDGET_DESKTOP_LOW,
            physics_budget: PHYSICS_BUDGET_DESKTOP_LOW,
        },
        setup: arena,
    },
];

/// The scenario named `name`, among `scenarios`.
#[must_use]
pub fn find_scenario<'a>(scenarios: &'a [Scenario], name: &str) -> Option<&'a Scenario> {
    scenarios.iter().find(|scenario| scenario.name == name)
}

/// The test content's moves: a three-hit string, a dash attack, a dodge and a skill.
pub fn test_moves() -> Result<MoveSet, MoveSetError> {
    MoveSetDocument::parse(TEST_MOVES)?.compile()
}

/// Steps a [`Buttons::ULTIMATE`] press slows the world for, in [`melee`], and to how many thousandths.
const MELEE_SLOW_STEPS: u16 = 45;
const MELEE_SLOW_PER_MILLE: u32 = 200;

/// The first three actors' buttons that do what no move in the test content does yet: [`Buttons::SWAP`]
/// toggles slowing the actor, [`Buttons::ULTIMATE`] slows the world for a while, as M2 G4's perfect dodge
/// will, and [`Buttons::JUMP`] jumps and spawns another actor at a random yaw, which plays a skill and
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
            let spawn =
                ActorSpawn { position: transform.position, yaw, moves: runner.set(), controller: None };
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

/// Three characters that stand apart and turned differently, with [`melee_buttons`]: everything a step
/// can do to a move, on one world.
pub fn melee(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let controller = Some(flat_ground(sim)?);
    let moves = sim.add_move_set(test_moves()?);
    sim.add_combat_system(CombatSet::Resolve, melee_buttons);
    let mut cast = Vec::with_capacity(3);
    for x in [-2.0, 0.0, 2.0] {
        cast.push(sim.spawn(ActorSpawn {
            position: Vec3::new(x, 0.0, 0.0),
            yaw: x * 0.5,
            moves: Some(moves),
            controller,
        })?);
    }
    aim_round(sim, &cast);
    Ok(cast)
}

/// A player at the origin and an opponent 4 m ahead of it, facing it: the string, its cancels and the
/// dash attack as a fight has them.
pub fn duel(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let controller = Some(flat_ground(sim)?);
    let moves = sim.add_move_set(test_moves()?);
    let player = sim.spawn(ActorSpawn { position: Vec3::ZERO, yaw: 0.0, moves: Some(moves), controller })?;
    let opponent = sim.spawn(ActorSpawn {
        position: Vec3::new(0.0, 0.0, 4.0),
        yaw: std::f32::consts::PI,
        moves: Some(moves),
        controller,
    })?;
    let cast = vec![player, opponent];
    aim_round(sim, &cast);
    Ok(cast)
}

/// The arena (`crate::arena`): the town's square from its collider document, and a player and an
/// opponent at the document's spawn points. Input drives both, the player first.
pub fn arena(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cooked = crate::arena::cooked_colliders()?;
    let set = ColliderSet::read(&cooked)?;
    sim.set_physics_world(PhysicsWorld::from_cooked(set, sim.body_capacity())?)?;
    let controller = Some(sim.add_controller_profile(&crate::arena::profile())?);
    let moves = Some(sim.add_move_set(test_moves()?));
    let spawns = crate::arena::spawns();
    let mut cast = Vec::with_capacity(2);
    for spot in [spawns.player, spawns.enemy] {
        cast.push(sim.spawn(ActorSpawn { position: spot.position(), yaw: spot.yaw(), moves, controller })?);
    }
    aim_round(sim, &cast);
    Ok(cast)
}

#[cfg(test)]
mod tests {
    use super::super::is_valid_name;
    use super::*;

    #[test]
    fn every_scenario_has_a_good_unique_name_and_sets_up_from_its_own_config() {
        for (index, scenario) in SCENARIOS.iter().enumerate() {
            assert!(is_valid_name(scenario.name), "{}", scenario.name);
            assert!(!scenario.about.is_empty(), "{}", scenario.name);
            let found = find_scenario(SCENARIOS, scenario.name).expect("listed");
            assert!(std::ptr::eq(found, &SCENARIOS[index]), "{} is listed twice", scenario.name);
            let (sim, cast) = scenario.start(scenario.config).expect("sets up");
            assert_eq!(sim.tick(), 0);
            assert!(!cast.is_empty(), "{} has no one for input to drive", scenario.name);
        }
        assert!(find_scenario(SCENARIOS, "no-such-scenario").is_none());
    }

    /// A fight stands on the ground: every one of its cast has a capsule, and lands on its first tick.
    #[test]
    fn the_fights_stand_on_the_ground() {
        for scenario in SCENARIOS {
            let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
            sim.step(&crate::actions::TickActions::NEUTRAL);
            for actor in cast {
                let view = sim.actor(actor).expect("alive");
                assert!(view.controller.is_active(), "{}", scenario.name);
                assert_eq!(view.controller.state.mode, crate::controller::GroundMode::Grounded);
                assert_eq!(view.position.y, 0.0, "{}", scenario.name);
            }
        }
    }

    /// The arena's cast stands where the document says, the player first, each facing the other.
    #[test]
    fn the_arena_spawns_its_two_actors_where_the_document_says() {
        let scenario = find_scenario(SCENARIOS, "arena").expect("listed");
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
