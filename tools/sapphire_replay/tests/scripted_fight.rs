//! A runner's scenarios can carry a script: a recorded fight with a hook on it, under gate 4, in
//! the binary's own build, with no GPU in the tree. `cargo xtask check-layers` holds that tree, since
//! `sapphire_script` is in `[simulation]` and this runner in its `runners`.

use std::alloc::System;
use std::path::PathBuf;
use std::sync::Mutex;

use sapphire_core::alloc::CountingAllocator;
use sapphire_gameplay::replay::{
    Player, Replay, SCENARIOS, Scenario, ScenarioError, ScenarioSetup, arena_fight, find_scenario,
};
use sapphire_gameplay::{ActorId, Simulation};
use sapphire_script::scenarios::SCRIPTED;
use sapphire_script::{Limits, ScriptHost, attach, cook, host_of};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

static HOST: Mutex<Option<ScriptHost>> = Mutex::new(None);

fn scripted_fight(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = arena_fight(sim)?;
    attach(sim, HOST.lock().unwrap().take().expect("the test set it"));
    Ok(cast)
}

fn committed(name: &str) -> Replay {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays").join(format!("{name}.sreplay"));
    Replay::decode(&std::fs::read(path).unwrap()).unwrap()
}

/// The suite's scripted replay, as the suite plays it: gate 4 holds a whole fight with commands going into
/// the checksum, and the fight really has them. A chunk that no longer answers anything here would still
/// reproduce, so this is what says the replay is worth keeping.
#[test]
fn the_scripted_replay_plays_every_hook_clean_under_the_gate() {
    assert!(sapphire_core::alloc::is_installed());
    let mut player =
        Player::new(committed("arena-scripted"), &SCRIPTED).expect("its chunk's what was recorded");
    player.run_to_end();
    let sim = player.simulation();
    let stats = host_of(sim).stats();
    let scripts = sim.scripts().expect("attached");
    let board = *scripts.board();
    let applied = scripts.commands_applied();
    let played = player.finish();

    played.allocation_gate().expect("nothing but the VM allocated in a step");
    assert_eq!(
        (stats.hooks_failed, stats.hooks_over_budget, stats.hooks_skipped, stats.commands_discarded),
        (0, 0, 0, 0),
        "{stats:?}"
    );
    assert_eq!(scripts_dropped(&committed("arena-scripted")), 0, "every event was answered");
    assert!(stats.hooks_fired > 100, "{stats:?}");
    assert!(applied > 10, "{applied} commands");
    // The chunk's own slots: moves it listed, hits, damage, defeats, a side, a spread, a target, beats.
    for (slot, value) in board.iter().take(8).enumerate() {
        assert!(*value != 0.0, "slot {slot} never moved: {:?}", &board[..8]);
    }
}

fn scripts_dropped(replay: &Replay) -> u64 {
    let mut player = Player::new(replay.clone(), &SCRIPTED).unwrap();
    player.run_to_end();
    player.simulation().scripts().expect("attached").events_dropped()
}

/// Where the collector's steps fall can't reach a checksum: it's outside the step, and a hook can't see it.
#[test]
fn the_collectors_steps_reach_no_checksum() {
    let run = |collect: bool| {
        let mut player = Player::new(committed("arena-scripted"), &SCRIPTED).unwrap();
        let mut ends = Vec::new();
        while player.step() {
            ends.push(player.simulation().checksum());
            if collect {
                host_of(player.simulation()).gc_step().unwrap();
            }
        }
        ends
    };
    assert_eq!(run(true), run(false));
}

#[test]
fn a_runner_plays_a_recorded_fight_with_a_hook_on_it() {
    let mut host = ScriptHost::new(Limits::default()).unwrap();
    let chunk = "return { on_phase = function(actor, phase) sapphire.set_board(actor, phase) end }";
    host.bind(&cook(chunk).unwrap(), "=test").unwrap();
    *HOST.lock().unwrap() = Some(host);

    let plain = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
    let scenarios = [Scenario { setup: ScenarioSetup::Code(scripted_fight), ..*plain }];
    let replay = committed("arena-fight-won");
    let mut player = Player::on_current_tables(replay, &scenarios).unwrap();
    player.run_to_end();
    let stats = host_of(player.simulation()).stats();
    let played = player.finish();

    played.allocation_gate().expect("a hook's VM isn't gate 4's");
    assert!(stats.hooks_fired >= 3, "{stats:?}");
    assert_eq!(stats.hooks_failed, 0, "{stats:?}");
}
