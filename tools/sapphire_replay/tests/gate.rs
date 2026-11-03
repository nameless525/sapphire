//! Gate 4 through the suite (spec §9): a scenario whose world allocates in a step fails its replay,
//! naming the first tick, even under `--update`. Installs the counting allocator, as the binary does.

use std::alloc::System;
use std::fs;
use std::path::PathBuf;

use sapphire_core::alloc::CountingAllocator;
use sapphire_ecs::prelude::*;
use sapphire_gameplay::replay::{ARCHITECTURE, SCENARIOS, Scenario, ScenarioError, ScenarioSetup, melee};
use sapphire_gameplay::{ActorId, CombatSet, Simulation};
use sapphire_replay::{Record, Suite, check, record};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// Allocates from its 40th run on, as a new system that forgot rule R6 might.
fn allocates_late(mut runs: Local<'_, u32>) {
    *runs += 1;
    if *runs >= 40 {
        std::hint::black_box(Vec::<u8>::with_capacity(64));
    }
}

fn allocating(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = melee(sim)?;
    sim.add_combat_system(CombatSet::Resolve, allocates_late);
    Ok(cast)
}

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_step_that_allocates_fails_the_suite() {
    let root = Dir(std::env::temp_dir().join(format!("sapphire_replay_gate_{}", std::process::id())));
    let _ = fs::remove_dir_all(&root.0);
    let (dir, out) = (root.0.join("replays"), root.0.join("out"));
    record(&Record {
        dir: &dir,
        out: &out,
        name: "duel",
        scenario: "melee",
        script: "melee-sweep",
        ticks: Some(100),
        seed: None,
        scenarios: &SCENARIOS,
        architecture: ARCHITECTURE,
    })
    .expect("records");

    let clean = Suite {
        dir: &dir,
        out: &out,
        update: false,
        only: None,
        scenarios: &SCENARIOS,
        architecture: ARCHITECTURE,
    };
    let outcome = check(&clean).expect("runs");
    assert_eq!(outcome.failures, Vec::<String>::new());
    assert_eq!(outcome.played, 1);

    // The same world and the same actions, with an allocation in its step.
    let scenarios = [Scenario { setup: ScenarioSetup::Code(allocating), ..SCENARIOS[0] }];
    let outcome = check(&Suite { scenarios: &scenarios, ..clean }).expect("runs");
    assert_eq!(outcome.failures.len(), 1, "{:?}", outcome.failures);
    let failure = &outcome.failures[0];
    assert!(failure.starts_with("duel:"), "{failure}");
    assert!(failure.contains("rule R6") && failure.contains("first at tick 40"), "{failure}");

    // An update doesn't launder it: the failure stands, and what's recorded stays.
    let recorded = fs::read_to_string(dir.join(format!("checksums.{ARCHITECTURE}.txt"))).expect("there");
    let outcome = check(&Suite { scenarios: &scenarios, update: true, ..clean }).expect("runs");
    assert_eq!(outcome.failures.len(), 1, "{:?}", outcome.failures);
    let after = fs::read_to_string(dir.join(format!("checksums.{ARCHITECTURE}.txt"))).expect("there");
    assert_eq!(after, recorded);
}
