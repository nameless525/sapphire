//! Replay scenarios with a script in them. They're here rather than in `sapphire_gameplay`'s list because
//! gameplay can't see this crate, so whoever plays them chains this list to that one.

use std::sync::LazyLock;

use sapphire_bytecode::compile::CookedScript;
use sapphire_dialogue::{Dialogue, DialogueDocument, FlagRegistry};
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

/// The chunk `dialogue-flag` binds, in source: a fight that answers a story flag (M3 Q2).
pub const DIALOGUE_FLAG: &str = include_str!("../test_content/dialogue-flag.luau");

/// The dialogue `dialogue-flag` loads, the game's own: the guide at the Lantern Ward, whose first effect
/// lights the ward. Its effects are in the replay's `tables_hash`.
pub const WARD_GUIDE: &str = include_str!("../../../content/dialogue/ward_guide.dialogue.ron");

/// The game's flag registry, which the guide's dialogue cooks against.
pub const FLAGS: &str = include_str!("../../../content/flags.ron");

/// How often `dialogue-flag`'s `on_beat` fires.
const DIALOGUE_FLAG_BEAT: u64 = 30;

/// Every scenario with a script in it. `arena-scripted` is `arena-skirmish`'s document with a script
/// bound after it: its config, its documents and its cast, so tuning those moves `arena-scripted` too.
pub static SCRIPTED: LazyLock<Vec<Scenario>> = LazyLock::new(|| {
    vec![
        Scenario {
            name: "arena-scripted",
            about: "arena-skirmish with a boss's sort of script on it: every hook, query and command on a real fight",
            setup: ScenarioSetup::Code(arena_scripted),
            ..*skirmish()
        },
        Scenario {
            name: "dialogue-flag",
            about: "arena-skirmish with the Lantern Ward guide's dialogue loaded, and a script that answers the \
                    flag its choice sets",
            setup: ScenarioSetup::Code(dialogue_flag),
            ..*skirmish()
        },
    ]
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
    let built_on = if SCRIPTED.iter().any(|scripted| scripted.name == name) { skirmish().name } else { name };
    find_scenario(&SCENARIOS, built_on)?.document()
}

/// The guide's dialogue, cooked as the pipeline cooks it, conditions and all.
///
/// # Panics
///
/// If the committed dialogue or registry doesn't cook, which a test holds.
#[must_use]
pub fn ward_guide() -> Dialogue {
    cook_ward_guide().0
}

/// The guide's chunk of conditions as the compiler made it, for [`ScriptHost::bind_conditions`]: the bytes
/// of [`ward_guide`]'s `presentation.chunk`.
///
/// # Panics
///
/// As [`ward_guide`].
#[must_use]
pub fn ward_guide_conditions() -> CookedScript {
    cook_ward_guide().1.expect("the guide's dialogue has conditions")
}

fn cook_ward_guide() -> (Dialogue, Option<CookedScript>) {
    let registry = FlagRegistry::from_ron(FLAGS).expect("the committed registry reads: a test holds it");
    let mut chunk = None;
    let dialogue = DialogueDocument::from_ron(WARD_GUIDE)
        .and_then(|document| {
            document.cook(&registry, |source| {
                let cooked = cook(source).map_err(|error| error.to_string())?;
                let bytes = cooked.as_ref().to_vec();
                chunk = Some(cooked);
                Ok(bytes)
            })
        })
        .expect("the committed dialogue cooks: a test holds it");
    (dialogue, chunk)
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

fn dialogue_flag(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = skirmish().set_up(sim)?;
    sim.add_dialogue(ward_guide().simulation)
        .expect("the committed dialogue's effects are valid: a test holds it");
    let cooked = cook(DIALOGUE_FLAG).expect("the committed chunk cooks: a test holds it");
    let mut host = ScriptHost::new(Limits::default()).expect("a VM starts");
    host.bind(&cooked, "=dialogue-flag").expect("the committed chunk loads: a test holds it");
    host.beat_every(DIALOGUE_FLAG_BEAT);
    attach(sim, host);
    Ok(cast)
}
