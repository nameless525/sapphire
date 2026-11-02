//! Live tuning's rules for a fight (M2 T2): what an edited document of moves becomes, and what a new
//! fight starts from once one has been edited.
//!
//! Everything else is said where it happens: a running move keeps its revision and the next start reads
//! the newest ([`crate::Simulation::replace_move_set`]), and a recording stops at the edit
//! ([`super::Recording`]). A [`super::Player`] takes no edits at all, since a replay plays what it
//! recorded.
//!
//! This is plain logic an app's development tools call, so a shipping app, which calls none of it,
//! links none of it.

use sapphire_moves::{MoveSet, MoveSetDocument, MoveSetError};

use super::scenario::{Scenario, ScenarioError, document_file_name};
use crate::actors::ActorId;
use crate::simulation::{SimConfig, Simulation};

/// What an edited document compiles to on a running app. A failure's text names the move and the track,
/// and keeping what's playing is the caller's whole job then.
///
/// A move that names a clip fails here with an issue saying only a cook can bake it: a device has no
/// clips to bake from, so such a document reaches a fight as its cook's `.smoves` instead, through a pack.
pub fn compile_document(text: &str) -> Result<MoveSet, MoveSetError> {
    MoveSetDocument::parse(text)?.compile()
}

/// The newest edit of each document that has been edited since the app started: what a new fight starts
/// from, so starting one doesn't quietly go back to what the build compiled in.
#[derive(Clone, Debug, Default)]
pub struct LiveMoves {
    /// By file name. A handful at most, so a list.
    latest: Vec<(String, MoveSet)>,
}

impl LiveMoves {
    /// Keeps `set` as the newest edit of the document with this file name.
    pub fn remember(&mut self, document: &str, set: MoveSet) {
        match self.latest.iter_mut().find(|(name, _)| name == document) {
            Some((_, kept)) => *kept = set,
            None => self.latest.push((document.to_owned(), set)),
        }
    }

    /// The newest edit of the document with this file name.
    #[must_use]
    pub fn latest(&self, document: &str) -> Option<&MoveSet> {
        self.latest.iter().find(|(name, _)| name == document).map(|(_, set)| set)
    }

    /// A new simulation of `scenario` with `config`, as a build with these edits in its documents would
    /// make one: each edited set is revision 0, and everything the setup spawned has its numbers.
    pub fn start(
        &self,
        scenario: &Scenario,
        config: SimConfig,
    ) -> Result<(Simulation, Vec<ActorId>), ScenarioError> {
        let (mut sim, cast) = scenario.start(config)?;
        for path in scenario.documents {
            let name = document_file_name(path);
            if let (Some(id), Some(set)) = (scenario.set_of(name), self.latest(name)) {
                sim.start_move_set_from(id, set.clone());
            }
        }
        Ok((sim, cast))
    }
}

#[cfg(test)]
mod tests {
    use sapphire_core::Name;

    use super::super::scenario::{SCENARIOS, duel_on, find_scenario, test_moves};
    use super::super::script::{ATTEMPT_EVERY, attempts, fight};
    use super::super::{PlayError, Player, Recording, Replay};
    use super::*;
    use crate::moves::Tuned;

    const SLICE: &str = "slice.moves.ron";

    fn duel() -> &'static Scenario {
        find_scenario(SCENARIOS, "duel").expect("listed")
    }

    /// The test moves, with `light_1` two ticks slower to come out and its hit two ticks later, as a
    /// person tuning it might save.
    fn slower_light() -> MoveSet {
        slower_document().compile().expect("compiles")
    }

    fn slower_document() -> MoveSetDocument {
        let text = include_str!("../../../sapphire_moves/test_content/slice.moves.ron");
        let mut document = MoveSetDocument::parse(text).expect("parses");
        let light = document.moves.iter_mut().find(|mv| mv.name == "light_1").expect("there");
        light.startup += 2;
        light.recovery -= 2;
        for hitbox in &mut light.hitboxes {
            hitbox.from += 2;
            hitbox.to += 2;
        }
        document
    }

    fn the_players_move(recording: &Recording, cast: ActorId) -> Option<(Name, u32, u32)> {
        let actor = recording.simulation().actor(cast)?;
        let current = actor.moves.current?;
        let startup = recording.simulation().move_library().get(current.at)?.startup;
        Some((current.name, current.at.revision, startup))
    }

    /// What exit test 2 asks of the simulation: a saved window plays on the next attempt, and the
    /// attempt in progress keeps the one it opened with.
    #[test]
    fn the_next_attempt_reads_a_saved_window_and_the_one_in_progress_doesnt() {
        let mut recording = Recording::new(duel(), duel().config).expect("sets up");
        let player = recording.cast()[0];
        let mut source = attempts;
        for _ in 0..3 {
            recording.step(&mut source).expect("short");
        }
        let light = Name::new("light_1");
        assert_eq!(the_players_move(&recording, player), Some((light, 0, 5)));

        let id = duel().set_of(SLICE).expect("it plays this document");
        let tuned = recording.tune_move_set(id, slower_light()).expect("its set");
        assert_eq!(tuned, Tuned::Replaced { revision: 1 });
        recording.step(&mut source).expect("short");
        assert_eq!(the_players_move(&recording, player), Some((light, 0, 5)), "the attempt in progress");

        for _ in 4..=ATTEMPT_EVERY {
            recording.step(&mut source).expect("short");
        }
        assert_eq!(the_players_move(&recording, player), Some((light, 1, 7)), "the next one");
    }

    /// A recording that crossed an edit is cut at the edit, and what's kept plays back to what happened:
    /// no file claims a fight its tables can't reproduce.
    #[test]
    fn a_live_edit_ends_the_file_at_the_last_tick_it_can_answer_for() {
        const EDIT_AT: u32 = 100;
        let mut source = fight;
        let mut recording = Recording::new(duel(), duel().config).expect("sets up");
        for _ in 0..EDIT_AT {
            recording.step(&mut source).expect("short");
        }
        let before = recording.simulation().tables_hash();
        let id = duel().set_of(SLICE).expect("it plays this document");
        recording.tune_move_set(id, slower_light()).expect("its set");
        assert_ne!(recording.simulation().tables_hash(), before);
        assert_eq!(recording.edited_at(), Some(EDIT_AT));

        for _ in 0..40 {
            recording.step(&mut source).expect("the fight goes on");
        }
        assert_eq!(recording.simulation().tick(), u64::from(EDIT_AT) + 40);
        assert_eq!(recording.recorded_ticks(), EDIT_AT);
        // A second edit changes nothing about where it ended.
        let mut again = slower_document();
        again.moves[0].recovery += 1;
        recording.tune_move_set(id, again.compile().expect("compiles")).expect("its set");
        assert_eq!(recording.edited_at(), Some(EDIT_AT));

        let (replay, played) = recording.finish();
        assert_eq!(replay.ticks(), EDIT_AT);
        assert_eq!(played.trail.ticks(), EDIT_AT);
        assert_eq!(replay.tables_hash(), before, "what those ticks ran on");

        let mut player =
            Player::new(Replay::decode(&replay.encode()).expect("reads"), SCENARIOS).expect("starts");
        player.run_to_end();
        assert_eq!(player.finish().trail, played.trail);
    }

    /// Saving a file again changes nothing, so it doesn't end a recording or make a revision.
    #[test]
    fn saving_the_same_bytes_again_is_no_edit() {
        let mut recording = Recording::new(duel(), duel().config).expect("sets up");
        let id = duel().set_of(SLICE).expect("it plays this document");
        let same = test_moves().expect("compiles");
        let tuned = recording.tune_move_set(id, same).expect("its set");
        assert_eq!(tuned, Tuned::Unchanged { revision: 0 });
        assert_eq!(recording.edited_at(), None);
    }

    /// The same edit on the same tick reaches the same checksum: a reload is between steps and reads no
    /// clock, so it's as deterministic as anything else in a run.
    #[test]
    fn the_same_edit_on_the_same_tick_reaches_the_same_checksum() {
        let run = || {
            let (mut sim, cast) = duel().start(duel().config).expect("sets up");
            let id = duel().set_of(SLICE).expect("it plays this document");
            for tick in 0..300 {
                if tick == 120 {
                    sim.tune_move_set(id, slower_light()).expect("its set");
                }
                sim.step(&fight(tick, &cast));
            }
            sim.checksum()
        };
        assert_eq!(run(), run());
    }

    /// A fight started after an edit runs on the edit, tick for tick as if the build had compiled it in,
    /// and its file says so: a build without the edit refuses it, and one with it plays it.
    #[test]
    fn a_fight_started_after_an_edit_is_what_a_build_with_it_would_play() {
        fn edited_duel(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
            duel_on(sim, slower_light())
        }
        let mut live = LiveMoves::default();
        live.remember(SLICE, slower_light());
        assert!(live.latest(SLICE).is_some());
        assert!(live.latest("warrior.moves.ron").is_none());

        // What recording from the edit makes.
        let scenario = Scenario { setup: edited_duel, ..*duel() };
        let mut tuned = Recording::tuned(&scenario, duel().config, &live).expect("sets up");
        let mut source = attempts;
        for _ in 0..200 {
            tuned.step(&mut source).expect("short");
        }
        assert_eq!(tuned.edited_at(), None, "nothing changed while it ran");
        let (replay, played) = tuned.finish();

        // A build with the edit in its document plays it back.
        let mut with_the_edit = Player::new(replay.clone(), &[scenario]).expect("the same tables");
        with_the_edit.run_to_end();
        assert_eq!(with_the_edit.finish().trail, played.trail);

        // This build, without it, refuses.
        let refused = Player::new(replay, SCENARIOS);
        assert!(matches!(refused, Err(PlayError::MovesChanged { .. })), "{:?}", refused.err());
    }

    /// `LiveMoves::start` leaves revision 0 with the edit, so a fight's checksums are a build's, however
    /// many edits came before.
    #[test]
    fn a_new_fight_starts_at_revision_zero_of_the_newest_edit() {
        let mut live = LiveMoves::default();
        let mut older = slower_document();
        older.moves[0].recovery += 3;
        live.remember(SLICE, older.compile().expect("compiles"));
        live.remember(SLICE, slower_light());
        let (mut sim, cast) = live.start(duel(), duel().config).expect("sets up");
        let id = duel().set_of(SLICE).expect("it plays this document");
        assert_eq!(sim.move_library().current_revision(id), Some(0));
        assert!(sim.move_library().is_current(id, &slower_light()));
        sim.step(&attempts(0, &cast));
        assert_eq!(sim.actor(cast[0]).expect("alive").moves.current.expect("started").at.revision, 0);
    }

    #[test]
    fn a_move_that_names_a_clip_says_only_a_cook_can_bake_it() {
        let mut document = slower_document();
        document.moves[0].motion.clip = Some(sapphire_moves::ClipMotionDocument {
            source: "6d321a52-1aed-4279-bc32-a177b8fd81d4".to_owned(),
            name: None,
            rig: None,
            root: sapphire_moves::RootSource::Clip,
            paths: Vec::new(),
        });
        let error = compile_document(&document.to_text().expect("writes")).expect_err("no clips here");
        let error = error.to_string();
        assert!(error.contains("track `motion`") && error.contains("only a cook"), "{error}");
    }

    /// A document's issues name the move and the track: it's what a person hears from `link push`.
    #[test]
    fn a_bad_document_says_which_move_and_track() {
        let mut document = slower_document();
        document.moves[0].hitboxes[0].to = 400;
        let error = compile_document(&document.to_text().expect("writes")).expect_err("a bad window");
        let error = error.to_string();
        assert!(error.contains("move `light_1`, track `hitboxes`"), "{error}");
        assert!(compile_document("(not a document").is_err());
    }

    /// Holds every scenario's `documents` to what its setup loads: right files, right order, so a push is
    /// never routed to another set. A document's bytes are what a new reader of the row would open.
    #[test]
    fn a_scenarios_documents_are_the_sets_it_loads() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for scenario in SCENARIOS {
            let (sim, _) = scenario.start(scenario.config).expect("sets up");
            let library = sim.move_library();
            assert_eq!(library.set_count(), scenario.documents.len(), "{}", scenario.name);
            for path in scenario.documents {
                let text = std::fs::read_to_string(root.join(path)).expect(path);
                let set = compile_document(&text).expect(path);
                let id = scenario.set_of(document_file_name(path)).expect(path);
                assert!(library.is_current(id, &set), "{}: {path}", scenario.name);
            }
        }
    }
}
