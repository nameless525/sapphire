//! Scenarios: the named starting worlds replays run from.
//!
//! A replay records the name of one, and running it means setting up the same world. **To add one,**
//! write its setup and add a row to [`SCENARIOS`]. A setup does the same things in the same order
//! every run: tables first, then spawns.

use sapphire_math::{Vec3, det};
use sapphire_moves::{BakedTables, RootSample, TablesError};

use crate::actors::{ActorId, ActorSpawn, SpawnError};
use crate::simulation::{SimConfig, Simulation};
use crate::stats::BUDGET_DESKTOP_LOW;

/// Why a scenario couldn't set its world up, as with a replay that asks for too small a pool.
#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    #[error(transparent)]
    Tables(#[from] TablesError),
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
    /// Loads the tables and spawns the actors, and returns the actors input drives, the player
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

/// Every scenario. G2 and G3 add the fights the replay suite exists for here.
pub const SCENARIOS: &[Scenario] = &[Scenario {
    name: "drill",
    about: "three actors on two of D1's drill tables: the drill's buttons, clocks and spawns",
    config: SimConfig { seed: 7, max_actors: 16, step_budget: BUDGET_DESKTOP_LOW },
    setup: drill,
}];

/// The scenario named `name`, among `scenarios`.
#[must_use]
pub fn find_scenario<'a>(scenarios: &'a [Scenario], name: &str) -> Option<&'a Scenario> {
    scenarios.iter().find(|scenario| scenario.name == name)
}

/// A table with no two readings alike: forward by the tick, sideways by its square, turning slowly,
/// with a pinned sine in it, so the reading depends on `libm`.
///
/// # Panics
///
/// Never for a tick count that fits a table: the samples are finite and start at zero.
#[must_use]
pub fn drill_curve(ticks: u32) -> BakedTables {
    let root = (0..=ticks)
        .map(|tick| {
            let t = tick as f32;
            RootSample {
                translation: [t * t * 0.01, det::sin(t * 0.3) - det::sin(0.0), t * 0.25],
                yaw: t * 0.02,
            }
        })
        .collect();
    BakedTables::new(root, vec![]).expect("the samples are finite and start at zero")
}

/// Three actors that stand apart and follow tables of two lengths, so none can stand in for another.
pub fn drill(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let short = sim.add_tables(drill_curve(20));
    let long = sim.add_tables(drill_curve(90));
    let mut cast = Vec::with_capacity(3);
    for (table, x) in [(short, -2.0), (long, 0.0), (short, 2.0)] {
        cast.push(sim.spawn(ActorSpawn {
            position: Vec3::new(x, 0.0, 0.0),
            yaw: x * 0.5,
            table: Some(table),
            transient: false,
        })?);
    }
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
