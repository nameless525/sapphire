//! A real replay with a host on it: the `arena-fight` scenario with one system more. A scenario's setup
//! is a plain `fn`, so the host waits in a static for it.

#![allow(dead_code, reason = "each test binary uses its share")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use sapphire_gameplay::Simulation;
use sapphire_gameplay::replay::{Played, Player, Replay, SCENARIOS, Scenario, arena_fight, find_scenario};
use sapphire_script::bindings::Frame;
use sapphire_script::{HookEnd, Limits, ScriptHost, ScriptVm, SharedHost, attach, cook};

static WAITING: Mutex<Option<SharedHost>> = Mutex::new(None);

fn setup(
    sim: &mut Simulation,
) -> Result<Vec<sapphire_gameplay::ActorId>, sapphire_gameplay::replay::ScenarioError> {
    let cast = arena_fight(sim)?;
    let host = WAITING.lock().unwrap().take().expect("a host is waiting");
    attach(sim, &host);
    Ok(cast)
}

/// A recorded replay's file.
pub fn replay_file(name: &str) -> Replay {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays").join(format!("{name}.sreplay"));
    Replay::decode(&std::fs::read(path).expect("the replay is there")).expect("reads")
}

/// `arena-fight`, with `host` attached. Only one may start at a time, since they share the static.
pub fn player_with(host: &SharedHost, replay: Replay) -> Player {
    let plain = find_scenario(SCENARIOS, "arena-fight").expect("listed");
    let scenarios = [Scenario { setup, ..*plain }];
    *WAITING.lock().unwrap() = Some(Arc::clone(host));
    Player::new(replay, &scenarios).expect("starts")
}

/// A host running `source`.
pub fn host_running(source: &str, limits: Limits) -> SharedHost {
    let mut host = ScriptHost::new(limits).expect("the VM starts");
    host.bind(&cook(source).expect("cooks"), "=test").expect("binds");
    Arc::new(Mutex::new(host))
}

/// Plays a whole replay with `host` on it, a collection step after every tick, as a frame's would be.
pub fn play(host: &SharedHost, replay: Replay) -> Played {
    let mut player = player_with(host, replay);
    while player.step() {
        host.lock().unwrap().gc_step().expect("collects");
    }
    player.finish()
}

/// One hook named `hook` on a fresh VM, with a frame the test can read afterwards.
pub fn run_hook(source: &str) -> Result<(HookEnd, Frame), String> {
    let vm = ScriptVm::new(Limits::default()).expect("the VM starts");
    let cooked = cook(source).map_err(|error| error.to_string())?;
    let hooks = vm.load(&cooked, "=test").map_err(|error| error.to_string())?;
    let hook: mlua::Function = hooks.get("hook").map_err(|error| error.to_string())?;
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
