//! A fight's cinematics (M3 Q3b, Z5a): every timeline its move sets name, loaded before the sets, each under
//! the name its moves give it.
//!
//! **Where one is:** a move's `cinematic` names a timeline beside its set's document, by the timeline's file:
//! `cinematic: "ultimate"` in `content/combat/player/player.moves.ron` is
//! `content/combat/player/ultimate.timeline.ron`. `build.rs` compiles in every `.timeline.ron` beside a
//! document a scenario names, so a new cinematic is a document and a name, and no Rust. A set's move keeps
//! only its cinematic's name's hash, so a cooked set, the player's among them, and a live edit find theirs
//! the same way: by the hash of each timeline's name beside their document.
//!
//! **In a fixed order:** each name once, in the order the sets first give it, set by set in the order a fight
//! adds them and move by move in each set's own, so a run's `tables_hash`, which holds each cinematic and
//! its name, comes out the same every time. A name is one timeline in a fight: the first set to give it says
//! which.
//!
//! **A name with no timeline** is the scenario's error, naming the document, the move and the name, before
//! anything is loaded.

use sapphire_core::Name;
use sapphire_moves::{MoveSet, MoveSetDocument};
use sapphire_timeline::{TimelineDocument, TimelineSimulation};

use super::document::NAMED_DOCUMENTS;
use super::scenario::ScenarioError;
use crate::simulation::Simulation;

/// What a cinematic's document ends with.
const TIMELINE: &str = ".timeline.ron";

/// A move's cinematic, as a fight loads it: the name its moves give it, and its simulation half.
#[derive(Clone, Debug)]
pub struct Cinematic {
    name: &'static str,
    timeline: TimelineSimulation,
}

impl Cinematic {
    /// The name its moves give it: its document's, before `.timeline.ron`.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

/// Every cinematic `sets` name, each a set and the document it comes from, from the repository's root: see
/// the [module docs](self). Reads each from this build's documents.
pub fn cinematics_of<'a>(
    sets: impl IntoIterator<Item = (&'a str, &'a MoveSet)>,
) -> Result<Vec<Cinematic>, ScenarioError> {
    cinematics_among(sets, NAMED_DOCUMENTS)
}

/// [`cinematics_of`], loaded into `sim` in that order, between steps and before the sets: what every fight
/// does before it adds its sets, so [`Simulation::check_cinematics`] finds them.
pub fn add_cinematics<'a>(
    sim: &mut Simulation,
    sets: impl IntoIterator<Item = (&'a str, &'a MoveSet)>,
) -> Result<(), ScenarioError> {
    load(sim, &cinematics_of(sets)?)
}

/// Loads `cinematics` into `sim`, in their order.
pub(super) fn load(sim: &mut Simulation, cinematics: &[Cinematic]) -> Result<(), ScenarioError> {
    for cinematic in cinematics {
        sim.add_cinematic(cinematic.name, cinematic.timeline.clone())?;
    }
    Ok(())
}

/// [`cinematics_of`], among `documents`, each `(path, text)`.
pub(super) fn cinematics_among<'a>(
    sets: impl IntoIterator<Item = (&'a str, &'a MoveSet)>,
    documents: &[(&'static str, &'static str)],
) -> Result<Vec<Cinematic>, ScenarioError> {
    let mut found: Vec<(Name, Cinematic)> = Vec::new();
    for (path, set) in sets {
        let folder = folder_of(path);
        for mv in set.moves() {
            let Some(name) = mv.cinematic() else { continue };
            if found.iter().any(|(loaded, _)| *loaded == name) {
                continue;
            }
            let beside = documents.iter().find_map(|&(document, text)| {
                let stem = document.strip_prefix(folder)?.strip_suffix(TIMELINE)?;
                (!stem.contains('/') && Name::new(stem) == name).then_some((stem, text))
            });
            let Some((stem, text)) = beside else {
                return Err(missing(path, mv.label(), name, documents));
            };
            let timeline = TimelineDocument::from_ron(text)?.cook()?.simulation;
            found.push((name, Cinematic { name: stem, timeline }));
        }
    }
    Ok(found.into_iter().map(|(_, cinematic)| cinematic).collect())
}

/// The folder `path` is in, with its last `/`: what a timeline beside it starts with.
fn folder_of(path: &str) -> &str {
    path.rfind('/').map_or("", |slash| &path[..=slash])
}

/// The error for move `label` of the set from `path`, whose cinematic `name` has no timeline beside it. The
/// name's text is its document's, which a cooked set doesn't keep.
fn missing(path: &str, label: &str, name: Name, documents: &[(&str, &str)]) -> ScenarioError {
    let document = documents.iter().find(|(document, _)| *document == path);
    let written = document
        .and_then(|(_, text)| MoveSetDocument::parse(text).ok())
        .and_then(|document| document.moves.into_iter().find(|mv| mv.name == label)?.cinematic);
    let name = written.unwrap_or_else(|| name.to_string());
    ScenarioError::Document {
        field: format!("{path}: move `{label}`"),
        problem: format!("its cinematic `{name}` has no `{}{name}{TIMELINE}` beside it", folder_of(path)),
    }
}

#[cfg(test)]
mod tests {
    use sapphire_moves::MoveSetDocument;

    use super::*;
    use crate::simulation::SimConfig;

    const MOVES: &str = include_str!("../../../sapphire_moves/test_content/slice.moves.ron");
    const ULTIMATE: &str = include_str!("../../../../content/combat/player/ultimate.timeline.ron");
    const FINISHER: &str = include_str!("../../../sapphire_timeline/test_content/cinematic.timeline.ron");

    /// A test-only folder: its moves, beside the two cinematics they name.
    const FIGHT: &str = "test/fight/fight.moves.ron";
    const DOCUMENTS: [(&str, &str); 3] = [
        ("test/fight/ultimate.timeline.ron", ULTIMATE),
        ("test/fight/finisher.timeline.ron", FINISHER),
        // Another folder's timeline of the same name is never this set's.
        ("test/other/finisher.timeline.ron", ULTIMATE),
    ];

    /// The test content, with its first move playing `first` and its last `last`.
    fn moves(first: &str, last: &str) -> MoveSet {
        let mut document = MoveSetDocument::parse(MOVES).expect("parses");
        document.moves.first_mut().expect("it has moves").cinematic = Some(first.to_owned());
        document.moves.last_mut().expect("it has moves").cinematic = Some(last.to_owned());
        document.compile().expect("compiles")
    }

    /// Two cinematics, neither of them `ultimate`'s constant, each found by its name beside the set's document
    /// and loaded in the order the set names them, under those names, which a fight's check then finds.
    #[test]
    fn every_cinematic_a_set_names_loads_from_beside_it() {
        let set = moves("finisher", "ultimate");
        let found = cinematics_among([(FIGHT, &set)], &DOCUMENTS).expect("both are there");
        let names: Vec<&str> = found.iter().map(Cinematic::name).collect();
        assert_eq!(names, ["finisher", "ultimate"], "in the order the set names them");
        let finisher = TimelineDocument::from_ron(FINISHER).expect("reads").cook().expect("cooks");
        assert_eq!(
            found[0].timeline.digest(),
            finisher.simulation.digest(),
            "this folder's, not the other's"
        );

        let mut sim = Simulation::new(SimConfig::default());
        load(&mut sim, &found).expect("loads");
        sim.add_move_set(set);
        sim.check_cinematics().expect("both are loaded");
    }

    /// The order is the sets', then their moves', each name once, so two runs on the same sets load the same
    /// timelines under the same IDs, and so reach the same `tables_hash`.
    #[test]
    fn the_order_is_the_sets_and_a_name_loads_once() {
        let (a, b) = (moves("ultimate", "finisher"), moves("finisher", "ultimate"));
        let found = cinematics_among([(FIGHT, &a), (FIGHT, &b)], &DOCUMENTS).expect("both are there");
        let names: Vec<&str> = found.iter().map(Cinematic::name).collect();
        assert_eq!(names, ["ultimate", "finisher"]);

        let hash = |sets: [(&'static str, &MoveSet); 2]| {
            let mut sim = Simulation::new(SimConfig::default());
            load(&mut sim, &cinematics_among(sets, &DOCUMENTS).expect("found")).expect("loads");
            sim.tables_hash()
        };
        assert_eq!(hash([(FIGHT, &a), (FIGHT, &b)]), hash([(FIGHT, &a), (FIGHT, &b)]));
        assert_ne!(hash([(FIGHT, &a), (FIGHT, &b)]), hash([(FIGHT, &b), (FIGHT, &a)]), "the order is in it");
    }

    /// A name with no timeline beside its set is the scenario's error, saying the document, the move and the
    /// name as it's written.
    #[test]
    fn a_cinematic_with_no_timeline_names_its_move() {
        let set = moves("finisher", "encore");
        let error = cinematics_among([(FIGHT, &set)], &DOCUMENTS).expect_err("no encore");
        let last = set.moves().last().expect("it has moves").label().to_owned();
        let ScenarioError::Document { field, problem } = error else { panic!("{error}") };
        assert_eq!(field, format!("{FIGHT}: move `{last}`"));
        assert_eq!(problem, "its cinematic `encore` has no `test/fight/encore.timeline.ron` beside it");
    }

    /// The shipped game: the player's moves name their ultimate's, which this build holds beside them.
    #[test]
    fn the_players_moves_find_their_ultimates() {
        let player = super::super::player_moves().expect("reads");
        let found = cinematics_of([(super::super::PLAYER_MOVES_PATH, &player)]).expect("it's there");
        let names: Vec<&str> = found.iter().map(Cinematic::name).collect();
        assert_eq!(names, ["ultimate"]);
    }
}
