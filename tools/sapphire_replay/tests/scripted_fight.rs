//! A runner's scenarios can carry a script: a recorded fight with a hook on it, under gate 4, in
//! the binary's own build, with no GPU in the tree. `cargo xtask check-layers` holds that tree, since
//! `sapphire_script` is in `[simulation]` and this runner in its `runners`.

use std::alloc::System;
use std::path::PathBuf;
use std::sync::Mutex;

use sapphire_core::alloc::CountingAllocator;
use sapphire_gameplay::replay::{
    Player, Replay, SCENARIOS, Scenario, ScenarioError, arena_fight, find_scenario,
};
use sapphire_gameplay::{ActorId, Simulation};
use sapphire_script::{Limits, ScriptHost, attach, cook, host_of};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

static HOST: Mutex<Option<ScriptHost>> = Mutex::new(None);

fn scripted_fight(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = arena_fight(sim)?;
    attach(sim, HOST.lock().unwrap().take().expect("the test set it"));
    Ok(cast)
}

#[test]
fn a_runner_plays_a_recorded_fight_with_a_hook_on_it() {
    let mut host = ScriptHost::new(Limits::default()).unwrap();
    let chunk = "return { on_phase = function(actor, phase) sapphire.set_board(actor, phase) end }";
    host.bind(&cook(chunk).unwrap(), "=test").unwrap();
    *HOST.lock().unwrap() = Some(host);

    let plain = find_scenario(SCENARIOS, "arena-fight").expect("listed");
    let scenarios = [Scenario { setup: scripted_fight, ..*plain }];
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/arena-fight-won.sreplay");
    let replay = Replay::decode(&std::fs::read(path).unwrap()).unwrap();
    let mut player = Player::new(replay, &scenarios).unwrap();
    player.run_to_end();
    let stats = host_of(player.simulation()).stats();
    let played = player.finish();

    played.allocation_gate().expect("a hook's VM isn't gate 4's");
    assert!(stats.hooks_fired >= 3, "{stats:?}");
    assert_eq!(stats.hooks_failed, 0, "{stats:?}");
}
