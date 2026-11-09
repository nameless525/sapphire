//! A real replay with a host on it: the `arena-fight` scenario with one system more. A scenario's setup
//! is a plain `fn`, so the host waits in a static for it.

#![allow(dead_code, reason = "each test binary uses its share")]

use std::path::PathBuf;
use std::sync::Mutex;

use sapphire_gameplay::replay::{
    Played, Player, Replay, SCENARIOS, Scenario, ScenarioSetup, arena_fight, find_scenario,
};
use sapphire_gameplay::script::BOARD_SLOTS;
use sapphire_gameplay::{HookFrame, ScriptCommand, Simulation};
use sapphire_script::{HookEnd, Limits, ScriptHost, ScriptStats, ScriptVm, attach, cook, host_of};

static WAITING: Mutex<Option<ScriptHost>> = Mutex::new(None);

fn setup(
    sim: &mut Simulation,
) -> Result<Vec<sapphire_gameplay::ActorId>, sapphire_gameplay::replay::ScenarioError> {
    let cast = arena_fight(sim)?;
    attach(sim, WAITING.lock().unwrap().take().expect("a host is waiting"));
    Ok(cast)
}

/// A recorded replay's file.
pub fn replay_file(name: &str) -> Replay {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays").join(format!("{name}.sreplay"));
    Replay::decode(&std::fs::read(path).expect("the replay is there")).expect("reads")
}

/// `arena-fight`, with `host` in it, playing the replay's actions whatever its chunk does to the tables'
/// hash. Only one may start at a time, since they share the static.
pub fn player_with(host: ScriptHost, replay: Replay) -> Player {
    let plain = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
    let scenarios = [Scenario { setup: ScenarioSetup::Code(setup), ..*plain }];
    *WAITING.lock().unwrap() = Some(host);
    Player::on_current_tables(replay, &scenarios).expect("starts")
}

/// A host running `source`.
pub fn host_running(source: &str, limits: Limits) -> ScriptHost {
    let mut host = ScriptHost::new(limits).expect("the VM starts");
    host.bind(&cook(source).expect("cooks"), "=test.luau").expect("binds");
    host
}

/// A run that's over, and what its host ended with.
pub struct Finished {
    pub played: Played,
    pub stats: ScriptStats,
    pub commands_applied: u64,
    pub digest: u64,
    pub board: [f64; BOARD_SLOTS],
    pub used_memory: usize,
    pub last_error: Option<String>,
}

/// Ends a run, reading its host before the simulation goes.
pub fn finish(player: Player) -> Finished {
    let host = host_of(player.simulation());
    let scripts = player.simulation().scripts().expect("a host enables them");
    let (commands_applied, digest, board) = (scripts.commands_applied(), scripts.digest(), *scripts.board());
    let (stats, used_memory) = (host.stats(), host.vm().used_memory());
    let last_error = host.last_error().map(str::to_owned);
    Finished { played: player.finish(), stats, commands_applied, digest, board, used_memory, last_error }
}

/// Plays a whole replay with `host` on it, which collects after every tick, as a game's does.
pub fn play(host: ScriptHost, replay: Replay) -> Finished {
    let mut player = player_with(host, replay);
    while player.step() {}
    finish(player)
}

/// One hook named `hook` on a fresh VM, with a frame the test can read afterwards.
pub fn run_hook(source: &str) -> Result<(HookEnd, HookFrame), String> {
    let vm = ScriptVm::new(Limits::default()).expect("the VM starts");
    let cooked = cook(source).map_err(|error| error.to_string())?;
    let hooks = vm.load(&cooked, "=test").map_err(|error| error.to_string())?;
    let hook = hooks.hook("hook").map_err(|error| error.to_string())?.expect("has a hook");
    let mut frame = HookFrame::new(0);
    let end = vm.fire(&mut frame, &hook, ());
    Ok((end, frame))
}

/// What a hook left as `sapphire.mark`, in order.
pub fn marks(frame: &HookFrame) -> Vec<f64> {
    frame
        .queued()
        .iter()
        .filter_map(|command| match command {
            ScriptCommand::Mark(value) => Some(*value),
            _ => None,
        })
        .collect()
}
