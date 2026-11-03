//! A real replay with a host on it: the `arena-fight` scenario with one system more. A scenario's setup
//! is a plain `fn`, so the host waits in a static for it.

#![allow(dead_code, reason = "each test binary uses its share")]

use std::path::PathBuf;
use std::sync::Mutex;

use sapphire_gameplay::Simulation;
use sapphire_gameplay::replay::{
    Played, Player, Replay, SCENARIOS, Scenario, ScenarioSetup, arena_fight, find_scenario,
};
use sapphire_script::bindings::{BOARD_SLOTS, Frame};
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

/// `arena-fight`, with `host` in it. Only one may start at a time, since they share the static.
pub fn player_with(host: ScriptHost, replay: Replay) -> Player {
    let plain = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
    let scenarios = [Scenario { setup: ScenarioSetup::Code(setup), ..*plain }];
    *WAITING.lock().unwrap() = Some(host);
    Player::new(replay, &scenarios).expect("starts")
}

/// A host running `source`.
pub fn host_running(source: &str, limits: Limits) -> ScriptHost {
    let mut host = ScriptHost::new(limits).expect("the VM starts");
    host.bind(&cook(source).expect("cooks"), "=test").expect("binds");
    host
}

/// A run that's over, and what its host ended with.
pub struct Finished {
    pub played: Played,
    pub stats: ScriptStats,
    pub digest: u64,
    pub board: [f64; BOARD_SLOTS],
    pub used_memory: usize,
    pub last_error: Option<String>,
}

/// Ends a run, reading its host before the simulation goes.
pub fn finish(player: Player) -> Finished {
    let host = host_of(player.simulation());
    let (stats, digest, board) = (host.stats(), host.digest(), *host.board());
    let (used_memory, last_error) = (host.vm().used_memory(), host.last_error().map(str::to_owned));
    Finished { played: player.finish(), stats, digest, board, used_memory, last_error }
}

/// Plays a whole replay with `host` on it, a collection step after every tick, as a frame's would be.
pub fn play(host: ScriptHost, replay: Replay) -> Finished {
    let mut player = player_with(host, replay);
    while player.step() {
        host_of(player.simulation()).gc_step().expect("collects");
    }
    finish(player)
}

/// One hook named `hook` on a fresh VM, with a frame the test can read afterwards.
pub fn run_hook(source: &str) -> Result<(HookEnd, Frame), String> {
    let vm = ScriptVm::new(Limits::default()).expect("the VM starts");
    let cooked = cook(source).map_err(|error| error.to_string())?;
    let hooks = vm.load(&cooked, "=test").map_err(|error| error.to_string())?;
    let hook = hooks.hook("hook").map_err(|error| error.to_string())?.expect("has a hook");
    let mut frame = Frame::new();
    let end = vm.fire(&mut frame, &hook, ());
    Ok((end, frame))
}

/// What a hook left as `sapphire.mark`, in order.
pub fn marks(frame: &Frame) -> Vec<f64> {
    frame.queue[..frame.queued]
        .iter()
        .filter_map(|command| match command {
            sapphire_script::Command::Mark(value) => Some(*value),
            _ => None,
        })
        .collect()
}
