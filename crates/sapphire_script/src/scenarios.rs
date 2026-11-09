//! Replay scenarios with a script in them. They're here rather than in `sapphire_gameplay`'s list because
//! gameplay can't see this crate, so whoever plays them chains this list to that one.

use std::sync::LazyLock;

use sapphire_gameplay::replay::document::ScenarioDocument;
use sapphire_gameplay::replay::{SCENARIOS, Scenario, ScenarioError, ScenarioSetup, find_scenario};
use sapphire_gameplay::script::ScriptEdits;
use sapphire_gameplay::{ActorId, Simulation};

use crate::{Limits, ScriptHost, attach, cook};

/// The chunk `arena-scripted` binds, in source. Its bytecode is in the replay's `tables_hash`, so changing
/// what this cooks to refuses the replay until it's recorded again.
pub const ARENA_SCRIPTED: &str = include_str!("../test_content/arena-scripted.luau");

/// Its file's name: what an edit of it goes by.
pub const ARENA_SCRIPTED_FILE: &str = "arena-scripted.luau";

/// Its path from the repository's root, for hot reload to watch.
pub const ARENA_SCRIPTED_PATH: &str = "crates/sapphire_script/test_content/arena-scripted.luau";

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

/// Every scenario a build with a VM plays: gameplay's, then those with a script, which an app and the replay
/// runner look names up in.
pub static ALL: LazyLock<Vec<Scenario>> =
    LazyLock::new(|| SCENARIOS.iter().chain(SCRIPTED.iter()).copied().collect());

/// Each script a scenario here binds, by its path from the repository's root: what an app's hot reload
/// watches and live link's status lists.
pub const SOURCES: &[&str] = &[ARENA_SCRIPTED_PATH];

/// The document a scenario's world comes from: its own, or for one with a script, the document of the one
/// it's built on. What an app asks of a world it stands a fight in.
#[must_use]
pub fn document_of(name: &str) -> Option<&'static ScenarioDocument> {
    let built_on = if name == SCRIPTED[0].name { skirmish().name } else { name };
    find_scenario(&SCENARIOS, built_on)?.document()
}

/// `arena-skirmish`, which this build holds as a document.
fn skirmish() -> &'static Scenario {
    find_scenario(&SCENARIOS, "arena-skirmish")
        .expect("arena-skirmish is a scenario document this build holds")
}

/// `arena-skirmish`, then its chunk, or the newest edit of it if the fight is a live one's
/// ([`ScriptEdits`]).
fn arena_scripted(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = skirmish().set_up(sim)?;
    let edit = sim.world().get_resource::<ScriptEdits>().and_then(|edits| edits.source(ARENA_SCRIPTED_FILE));
    let source = edit.unwrap_or(ARENA_SCRIPTED).to_owned();
    let refused = |problem: String| ScenarioError::Script { name: ARENA_SCRIPTED_FILE.to_owned(), problem };
    let cooked = cook(&source).map_err(|error| refused(error.to_string()))?;
    let mut host = ScriptHost::new(Limits::default()).map_err(|error| refused(error.to_string()))?;
    host.bind(&cooked, &format!("={ARENA_SCRIPTED_FILE}")).map_err(|error| refused(error.to_string()))?;
    host.beat_every(ARENA_SCRIPTED_BEAT);
    attach(sim, host);
    Ok(cast)
}
