//! Replay scenarios with a script in them. They're here rather than in `sapphire_gameplay`'s list because
//! gameplay can't see this crate, so whoever plays them chains this list to that one.

use std::sync::LazyLock;

use sapphire_gameplay::replay::{SCENARIOS, Scenario, ScenarioError, ScenarioSetup, find_scenario};
use sapphire_gameplay::{ActorId, Simulation};

use crate::{Limits, ScriptHost, attach, cook};

/// The chunk `arena-scripted` binds, in source. Its bytecode is in the replay's `tables_hash`, so changing
/// what this cooks to refuses the replay until it's recorded again.
pub const ARENA_SCRIPTED: &str = include_str!("../test_content/arena-scripted.luau");

/// How often `arena-scripted`'s `on_beat` fires. A game's wouldn't.
const ARENA_SCRIPTED_BEAT: u64 = 60;

/// Every scenario with a script in it. `arena-scripted` is `arena-skirmish`'s document with a script
/// bound after it: its config, its documents and its cast, so tuning those moves `arena-scripted` too.
pub static SCRIPTED: LazyLock<Vec<Scenario>> = LazyLock::new(|| {
    vec![Scenario {
        name: "arena-scripted",
        about: "arena-skirmish with a boss's sort of script on it: every hook, query and command on a real fight",
        setup: ScenarioSetup::Code(arena_scripted),
        ..*skirmish()
    }]
});

/// `arena-skirmish`, which this build holds as a document.
fn skirmish() -> &'static Scenario {
    find_scenario(&SCENARIOS, "arena-skirmish")
        .expect("arena-skirmish is a scenario document this build holds")
}

fn arena_scripted(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = skirmish().set_up(sim)?;
    let cooked = cook(ARENA_SCRIPTED).expect("the committed chunk cooks: a test holds it");
    let mut host = ScriptHost::new(Limits::default()).expect("a VM starts");
    host.bind(&cooked, "=arena-scripted.luau").expect("the committed chunk loads: a test holds it");
    host.beat_every(ARENA_SCRIPTED_BEAT);
    attach(sim, host);
    Ok(cast)
}
