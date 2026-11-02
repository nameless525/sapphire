//! Scenarios: the named starting worlds replays run from.
//!
//! A replay records the name of one, and running it means setting up the same world. **To add one,**
//! write its setup and add a row to [`SCENARIOS`]. A setup does the same things in the same order
//! every run: move sets first, then spawns.
//!
//! One replay file holds one [`tables_hash`](Simulation::tables_hash) for every architecture, so a
//! scenario's move sets must come out byte for byte the same on every architecture: from a document,
//! as [`test_moves`] does, or from committed cooked bytes, and in scalars and `sapphire_math::det` if
//! a setup computes anything, never through glam's SIMD types. Anything else a run depends on belongs
//! in that hash too.

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_moves::{MoveSet, MoveSetDocument, MoveSetError};
use sapphire_time::ScaleSource;

use crate::actions::{Buttons, Controls};
use crate::actors::{ActorId, ActorRequests, ActorSpawn, ActorTable, SpawnError};
use crate::movement::ActorTransform;
use crate::moves::{MoveRunner, WarpTarget};
use crate::simulation::{CombatSet, SimConfig, SimRng, Simulation};
use crate::stats::BUDGET_DESKTOP_LOW;

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

/// Every scenario. G3 adds the fights with hits in them here.
pub const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "melee",
        about: "three actors on the test moves, warps aimed round the ring, and buttons that freeze and slow \
                clocks and spawn actors",
        config: SimConfig { seed: 7, max_actors: 16, step_budget: BUDGET_DESKTOP_LOW },
        setup: melee,
    },
    Scenario {
        name: "duel",
        about: "a player and an opponent on the test moves, 4 m apart and facing, each aiming warps at the other",
        config: SimConfig { seed: 7, max_actors: 2, step_budget: BUDGET_DESKTOP_LOW },
        setup: duel,
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

/// Ticks a [`Buttons::HEAVY`] press freezes its own actor's clock, in [`melee`].
pub const MELEE_HITSTOP_TICKS: u16 = 6;

/// The first three actors' buttons that do to clocks and the pool what hits will, until M2 G3:
/// [`Buttons::HEAVY`] freezes the actor, [`Buttons::SWAP`] toggles slowing it, [`Buttons::ULTIMATE`]
/// toggles slowing the world, and [`Buttons::JUMP`] spawns another actor at a random yaw, which plays
/// a skill and leaves. None of these buttons has an intent in the test content.
pub fn melee_buttons(
    table: Res<'_, ActorTable>,
    mut requests: ResMut<'_, ActorRequests>,
    mut rng: ResMut<'_, SimRng>,
    mut world_clock: ResMut<'_, WorldClock>,
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
        if pressed.contains(Buttons::HEAVY) {
            clock.0.apply_hitstop(MELEE_HITSTOP_TICKS);
        }
        if pressed.contains(Buttons::SWAP) && clock.0.remove_source(ScaleSource::Gameplay) == 0 {
            // Toggled, so the stack never fills.
            let _ = clock.0.push_scale(ScaleSource::Gameplay, 500);
        }
        if pressed.contains(Buttons::ULTIMATE) && world_clock.0.remove_source(ScaleSource::PerfectDodge) == 0
        {
            let _ = world_clock.0.push_scale(ScaleSource::PerfectDodge, 200);
        }
        if pressed.contains(Buttons::JUMP) {
            let yaw = rng.0.unit_f32() * std::f32::consts::TAU;
            // A full pool refuses and counts it.
            let _ = requests.spawn(ActorSpawn { position: transform.position, yaw, moves: runner.set() });
        }
    }
}

/// Each of `cast` aims its warps at the next, round the ring.
fn aim_round(sim: &mut Simulation, cast: &[ActorId]) {
    for (index, &actor) in cast.iter().enumerate() {
        sim.set_warp_target(actor, WarpTarget::Actor(cast[(index + 1) % cast.len()]));
    }
}

/// Three actors that stand apart and turned differently, with [`melee_buttons`]: everything a step
/// can do to a move, on one world.
pub fn melee(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let moves = sim.add_move_set(test_moves()?);
    sim.add_combat_system(CombatSet::Resolve, melee_buttons);
    let mut cast = Vec::with_capacity(3);
    for x in [-2.0, 0.0, 2.0] {
        cast.push(sim.spawn(ActorSpawn {
            position: Vec3::new(x, 0.0, 0.0),
            yaw: x * 0.5,
            moves: Some(moves),
        })?);
    }
    aim_round(sim, &cast);
    Ok(cast)
}

/// A player at the origin and an opponent 4 m ahead of it, facing it: the string, its cancels and the
/// dash attack as a fight has them.
pub fn duel(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let moves = sim.add_move_set(test_moves()?);
    let player = sim.spawn(ActorSpawn { position: Vec3::ZERO, yaw: 0.0, moves: Some(moves) })?;
    let opponent = sim.spawn(ActorSpawn {
        position: Vec3::new(0.0, 0.0, 4.0),
        yaw: std::f32::consts::PI,
        moves: Some(moves),
    })?;
    let cast = vec![player, opponent];
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

    #[test]
    fn too_small_a_pool_is_an_error() {
        let config = SimConfig { max_actors: 1, ..SCENARIOS[0].config };
        assert!(matches!(
            SCENARIOS[0].start(config),
            Err(ScenarioError::Spawn(SpawnError::PoolFull { capacity: 1 }))
        ));
    }
}
