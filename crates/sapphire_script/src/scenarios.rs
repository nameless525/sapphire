//! Replay scenarios with a script in them. They're here rather than in `sapphire_gameplay`'s list because
//! gameplay can't see this crate, so whoever plays them chains this list to that one.
//!
//! Two ways in: the scenarios of code listed here ([`SCRIPTED`]), and every scenario document whose kinds of
//! enemy name a script (M3 C2), which [`ALL`] binds after the document's setup with nothing here naming it.

use std::sync::LazyLock;

use sapphire_dialogue::{Dialogue, DialogueDocument, FlagRegistry};
use sapphire_gameplay::replay::document::{DocumentScript, ScenarioDocument};
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

/// [`DIALOGUE_FLAG`]'s file name: what an edit of it goes by.
pub const DIALOGUE_FLAG_FILE: &str = "dialogue-flag.luau";

/// [`DIALOGUE_FLAG`]'s path from the repository's root, for hot reload to watch.
pub const DIALOGUE_FLAG_PATH: &str = "crates/sapphire_script/test_content/dialogue-flag.luau";

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

/// Every scenario a build with a VM plays: gameplay's, each document whose kinds of enemy name a script
/// binding it after its setup, then those with a script of code, which an app and the replay runner look
/// names up in.
pub static ALL: LazyLock<Vec<Scenario>> =
    LazyLock::new(|| SCENARIOS.iter().map(with_its_script).chain(SCRIPTED.iter().copied()).collect());

/// `scenario`, binding the script its document's kinds of enemy name, if they name one.
fn with_its_script(scenario: &Scenario) -> Scenario {
    match scenario.setup {
        ScenarioSetup::Document(document) if document_scripts(document).next().is_some() => {
            Scenario { setup: ScenarioSetup::DocumentAnd(document, bind_its_script), ..*scenario }
        }
        _ => *scenario,
    }
}

/// The scripts `document`'s kinds of enemy name. One that can't say is a scenario that can't start, which
/// its setup says.
fn document_scripts(document: &ScenarioDocument) -> impl Iterator<Item = DocumentScript> {
    document.scripts().unwrap_or_default().into_iter()
}

/// Binds the script `document`'s kinds of enemy name, or the newest edit of it if the fight is a live one's
/// ([`ScriptEdits`]). A fight runs one script, so a document whose kinds name two can't start.
fn bind_its_script(document: &ScenarioDocument, sim: &mut Simulation) -> Result<(), ScenarioError> {
    let scripts = document.scripts()?;
    let [script] = scripts.as_slice() else {
        let names: Vec<&str> = scripts.iter().map(|script| script.file).collect();
        let problem = format!("a fight binds one script, and its kinds of enemy name {names:?}");
        return Err(ScenarioError::Script { name: names.join(", "), problem });
    };
    bind_source(sim, script.file, script.source, None)
}

/// Binds `file`, or the newest edit of it, in `sim`, with `beat` if it's given one.
fn bind_source(
    sim: &mut Simulation,
    file: &str,
    source: &str,
    beat: Option<u64>,
) -> Result<(), ScenarioError> {
    let edit = sim.world().get_resource::<ScriptEdits>().and_then(|edits| edits.source(file));
    let source = edit.unwrap_or(source).to_owned();
    let refused = |problem: String| ScenarioError::Script { name: file.to_owned(), problem };
    let cooked = cook(&source).map_err(|error| refused(error.to_string()))?;
    let mut host = ScriptHost::new(Limits::default()).map_err(|error| refused(error.to_string()))?;
    host.bind(&cooked, &format!("={file}")).map_err(|error| refused(error.to_string()))?;
    if let Some(beat) = beat {
        host.beat_every(beat);
    }
    attach(sim, host);
    Ok(())
}

/// The script a scenario here binds: a row of [`SCRIPTS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScriptOf {
    /// The scenario that binds it, one of [`SCRIPTED`].
    pub scenario: &'static str,
    /// The scenario whose document its world comes from.
    pub built_on: &'static str,
    /// Its file's name: what an edit of it goes by.
    pub file: &'static str,
    /// Its path from the repository's root, for hot reload to watch.
    pub path: &'static str,
}

/// Each scenario's script, one row for each of [`SCRIPTED`], which a test holds them to: a scenario with a
/// script added there and not here would have none to reload.
pub const SCRIPTS: &[ScriptOf] = &[
    ScriptOf {
        scenario: "arena-scripted",
        built_on: "arena-skirmish",
        file: ARENA_SCRIPTED_FILE,
        path: ARENA_SCRIPTED_PATH,
    },
    ScriptOf {
        scenario: "dialogue-flag",
        built_on: "arena-skirmish",
        file: DIALOGUE_FLAG_FILE,
        path: DIALOGUE_FLAG_PATH,
    },
];

/// Each script a scenario here binds, by its path from the repository's root, once each: what an app's hot
/// reload watches and live link's status lists. From [`SCRIPTS`], then from the scenario documents' kinds of
/// enemy.
pub static SOURCES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let documents = SCENARIOS.iter().filter_map(Scenario::document).flat_map(document_scripts);
    let mut sources: Vec<&'static str> = Vec::with_capacity(SCRIPTS.len());
    for path in SCRIPTS.iter().map(|script| script.path).chain(documents.map(|script| script.path)) {
        if !sources.contains(&path) {
            sources.push(path);
        }
    }
    sources
});

/// The document a scenario's world comes from: its own, or for one with a script, the document of the one
/// it's built on. What an app asks of a world it stands a fight in.
#[must_use]
pub fn document_of(name: &str) -> Option<&'static ScenarioDocument> {
    let built_on =
        SCRIPTS.iter().find(|script| script.scenario == name).map_or(name, |script| script.built_on);
    find_scenario(&SCENARIOS, built_on)?.document()
}

/// The guide's dialogue, cooked as the pipeline cooks it, conditions and all.
///
/// # Panics
///
/// If the committed dialogue or registry doesn't cook, which a test holds.
#[must_use]
pub fn ward_guide() -> Dialogue {
    let registry = FlagRegistry::from_ron(FLAGS).expect("the committed registry reads: a test holds it");
    DialogueDocument::from_ron(WARD_GUIDE)
        .and_then(|document| {
            document.cook(&registry, |source| cook(source).map(drop).map_err(|e| e.to_string()))
        })
        .expect("the committed dialogue cooks: a test holds it")
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
    bind_source(sim, ARENA_SCRIPTED_FILE, ARENA_SCRIPTED, Some(ARENA_SCRIPTED_BEAT))?;
    Ok(cast)
}

/// `arena-skirmish` with the ward guide's dialogue, then its chunk, or the newest edit of it if the fight is a
/// live one's ([`ScriptEdits`]).
fn dialogue_flag(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = skirmish().set_up(sim)?;
    sim.add_dialogue(ward_guide().simulation)
        .expect("the committed dialogue's effects are valid: a test holds it");
    let edit = sim.world().get_resource::<ScriptEdits>().and_then(|edits| edits.source(DIALOGUE_FLAG_FILE));
    let source = edit.unwrap_or(DIALOGUE_FLAG).to_owned();
    let refused = |problem: String| ScenarioError::Script { name: DIALOGUE_FLAG_FILE.to_owned(), problem };
    let cooked = cook(&source).map_err(|error| refused(error.to_string()))?;
    let mut host = ScriptHost::new(Limits::default()).map_err(|error| refused(error.to_string()))?;
    host.bind(&cooked, &format!("={DIALOGUE_FLAG_FILE}")).map_err(|error| refused(error.to_string()))?;
    host.beat_every(DIALOGUE_FLAG_BEAT);
    attach(sim, host);
    Ok(cast)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use sapphire_gameplay::replay::ScenarioSetup;

    use super::{ALL, SCRIPTED, SCRIPTS, SOURCES, document_of};

    /// Every scenario with a script has its script's row, so hot reload watches it, and every row is a
    /// scenario's, built on a document this build holds, with a file that's there.
    #[test]
    fn every_scripted_scenarios_source_is_watched() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for scenario in SCRIPTED.iter() {
            let rows = SCRIPTS.iter().filter(|script| script.scenario == scenario.name).count();
            assert_eq!(rows, 1, "{} has one script", scenario.name);
        }
        for script in SCRIPTS {
            assert!(SCRIPTED.iter().any(|scenario| scenario.name == script.scenario), "{script:?}");
            assert!(SOURCES.contains(&script.path), "{script:?}");
            assert!(script.path.ends_with(&format!("/{}", script.file)), "{script:?}");
            assert!(root.join(script.path).is_file(), "{script:?}");
            assert!(document_of(script.scenario).is_some(), "{script:?}");
        }
    }

    /// Every scenario document whose kinds of enemy name a script binds it after its setup, with nothing
    /// here naming it, and hot reload watches it: a file that's there, holding what this build has.
    #[test]
    fn every_script_a_document_names_is_bound_and_watched() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for scenario in ALL.iter() {
            let Some(document) = scenario.document() else { continue };
            let scripts = document.scripts().expect("it can say");
            let binds = matches!(scenario.setup, ScenarioSetup::DocumentAnd(..));
            assert_eq!(binds, !scripts.is_empty(), "{}", scenario.name);
            for script in scripts {
                assert!(SOURCES.contains(&script.path), "{}: {script:?}", scenario.name);
                let on_disk = std::fs::read_to_string(root.join(script.path)).expect("it's there");
                assert_eq!(on_disk, script.source, "{}", script.path);
            }
        }
    }
}
