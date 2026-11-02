//! Live tuning in `dev` builds (M2 T2, exit test 2): a saved change to a fight plays on the next attempt,
//! with no restart. Hot reload and live link both bring documents here (`reload.rs`'s [`Document`]):
//!
//! - **Moves and fighters** (`.moves.ron`): the document compiles on the device, its issues naming the
//!   move and the track, and replaces its set in whichever simulation plays it, by file name. A running
//!   move keeps the revision it started with, the next start reads the new one, and a fighter takes its
//!   numbers at once. There are no clips to bake from here, so a move that names one reads the tables its
//!   last cook gave it, and a move's windows change with no cook. Its length is its clip's, and an edit of
//!   its clip is a cook's to bring, through a pack ([`LiveTuning::follow_packs`]), by asset ID, with
//!   `cargo xtask content` and hot reload's remount or `cargo xtask link push`. The reply says so. A cook
//!   older than the newest save leaves the save playing, over the cook's tables.
//! - **The camera** (`default_camera.ron`): the director's tuning, whose goals ease to the new numbers.
//! - **The animation graph** (`.animgraph.ron`): bound against what's loaded, it takes effect at the next
//!   state's start ([`sapphire_anim::GraphPlayer::replace`]). Its pack's route was N3's, and still works.
//! - **The cue table** (`default_cues.ron`, M2 E3): what fires a fight's cues takes it if every name it holds
//!   is there. Camera tuning can move a shake's ID, so a new tuning resolves the table's shakes again
//!   ([`Scene::set_camera_tuning`]) and is refused if one has gone. **Colliders aren't tuned**, and a running
//!   fight never changes under its feet.
//!
//! A document that doesn't parse or check keeps what's playing, and says why in the log, in the overlay's
//! stats panel and, pushed, in `link push`'s non-zero exit. The next good one works.
//!
//! **Determinism.** All of this lands between frames, so between steps, and allocates there and nowhere
//! else. A recording ends its file at the first edit that changes something, and the fight goes on
//! (`sapphire_gameplay::replay::Recording`); a replay refuses edits; and a fight started later starts from
//! the newest edits, as a build with them in its documents would ([`LiveMoves`]), with a file that says
//! so through its hash. `--arena --fight` has no file to end, and starts over from its edits.
//!
//! **Knowing it worked.** The first frame a new revision's move plays logs `the next attempt plays the new
//! moves`, and live link's status lists what each fighter is playing, with its revision and windows.
//!
//! **For the timeline editor (M2 T3):** save the document, then `sapphire_livelink::Client::push_document`
//! with its file name and text: `Ok` says what it replaced, and `LinkError::Refused` carries the issues.
//! `Client::status()` then shows the attempts that read it. Nothing here needs to change for that.

use std::path::{Path, PathBuf};

use sapphire_assets::graph::{AnimGraph, clip_id};
use sapphire_assets::{AssetChange, AssetId, AssetKind, Handle};
use sapphire_camera::CameraTuning;
use sapphire_cues::CueTable;
use sapphire_gameplay::replay::{LiveMoves, SCENARIOS, compile_document, document_file_name};
use sapphire_gameplay::{ActorId, MoveSetId, Simulation, Tuned};
use sapphire_hot_reload::sources;
use sapphire_livelink::protocol::{LiveDocument, MoveAttempt, TickWindow};
use sapphire_moves::{MoveSet, Span};
use sapphire_render::Renderer;
use sapphire_rhi::GpuContext;
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::anim_graph::SHOW_MOVES_PATH;
use crate::combat::Combat;
use crate::content::Content;
use crate::options::Options;
use crate::reload::{Document, error_chain};
use crate::scene::Scene;

/// The animation graph's document (`lookdev::GRAPH`).
const GRAPH_PATH: &str = "content/lookdev/character/character.animgraph.ron";
/// The camera's tuning document.
const CAMERA_PATH: &str = "crates/sapphire_camera/default_camera.ron";
/// The cue table's document.
const CUES_PATH: &str = "crates/sapphire_cues/default_cues.ron";

/// The move sets a cook holds, by document and sidecar ID: what a pack's new `.smoves` is the new version
/// of. A document under `content/` with moves that name clips needs a row, or nothing brings it.
const COOKED_MOVES: [(&str, AssetId); 3] = [
    ("show.moves.ron", AssetId::from_text("e41a8d27-90c6-4f35-b1de-6a7c02f95d18")),
    ("player.moves.ron", AssetId::from_text("6fc61cbc-8189-4b6d-a4a0-b0e09475c6ce")),
    ("warrior.moves.ron", AssetId::from_text("c50887be-5ce3-498a-9f28-255e7e372b32")),
];

/// What a live edit reaches: the fight in the app, wherever it runs. The sandbox has two kinds, for now.
pub(crate) struct Fight<'a> {
    pub combat: Option<&'a mut Combat>,
    pub scene: &'a mut Scene,
}

/// A document the app takes as it runs.
struct Source {
    /// From the repository's root.
    path: String,
    document: Document,
    edits: u32,
}

/// A set's newest moves, which no attempt has played yet.
struct Waiting {
    document: String,
    set: MoveSetId,
    stopwatch: Stopwatch,
}

/// A cooked set held so a new version of it is told of.
struct CookedSet {
    document: &'static str,
    handle: Handle<MoveSet>,
}

/// Live tuning's state in the app. See the [module docs](self).
pub(crate) struct LiveTuning {
    moves: LiveMoves,
    sources: Vec<Source>,
    waiting: Vec<Waiting>,
    cooked: Vec<CookedSet>,
    /// The files hot reload reads this app's documents from, on a machine that has them.
    files: Vec<PathBuf>,
}

impl LiveTuning {
    /// The documents `options` runs can take: every scenario's, which the overlay's recorder can start,
    /// and the camera's, the cue table's, the graph's and the graph show's where those run. The cooked sets
    /// among them are loaded and kept (a few kilobytes each), so a pack's new version of one is told of.
    pub fn new(options: &Options, content: &mut Content) -> Self {
        let sources = Self::sources_of(options);
        let mut cooked = Vec::new();
        for (document, id) in COOKED_MOVES {
            if !sources.iter().any(|source| document_file_name(&source.path) == document) {
                continue;
            }
            match content.hold_move_set(id) {
                Ok(handle) => cooked.push(CookedSet { document, handle }),
                // A pack from before this document: its edits come as documents only.
                Err(error) => {
                    tracing::info!(target: LOG_TARGET, document, %error, "no cooked moves to follow")
                }
            }
        }
        let mut files = Vec::new();
        if !options.no_hot_reload {
            files.extend(sources.iter().map(|source| sources::repository_file(&source.path)));
            files.extend(options.watch_documents.iter().cloned());
        }
        Self { moves: LiveMoves::default(), sources, waiting: Vec::new(), cooked, files }
    }

    /// The documents `options` runs can take.
    fn sources_of(options: &Options) -> Vec<Source> {
        let mut sources: Vec<Source> = Vec::new();
        let mut add = |path: &str, document| {
            if !sources.iter().any(|source| source.path == path) {
                sources.push(Source { path: path.to_owned(), document, edits: 0 });
            }
        };
        for scenario in SCENARIOS {
            for path in scenario.documents {
                add(path, Document::Moves);
            }
        }
        // A fight's cues fire the camera's shakes, by their places in its tuning.
        if options.has_camera_director() || options.cues {
            add(CAMERA_PATH, Document::Camera);
        }
        if options.cues {
            add(CUES_PATH, Document::Cues);
        }
        if options.graph.is_some() {
            add(SHOW_MOVES_PATH, Document::Moves);
            add(GRAPH_PATH, Document::Graph);
        }
        sources
    }

    /// Following only this cooked set of this document, for tests of a pack's route.
    #[cfg(test)]
    pub fn following(document: &'static str, handle: Handle<MoveSet>) -> Self {
        let path = format!("content/{document}");
        let source = Source { path, document: Document::Moves, edits: 0 };
        let cooked = vec![CookedSet { document, handle }];
        Self {
            moves: LiveMoves::default(),
            sources: vec![source],
            waiting: Vec::new(),
            cooked,
            files: Vec::new(),
        }
    }

    /// Reading its document from `file`, for tests of a save meeting a cook.
    #[cfg(test)]
    pub fn reading(mut self, file: PathBuf) -> Self {
        self.files.push(file);
        self
    }

    /// What hot reload watches for it: each document, and each `--watch-document`, by its file's name.
    pub fn watched(&self, options: &Options) -> Vec<(Document, PathBuf)> {
        let own = self.sources.iter().map(|source| (source.document, sources::repository_file(&source.path)));
        let extra = options.watch_documents.iter().filter_map(|path| match kind_of_file(path) {
            Some(document) => Some((document, path.clone())),
            None => {
                tracing::warn!(
                    target: LOG_TARGET,
                    path = %path.display(),
                    "--watch-document takes `.moves.ron`, `.animgraph.ron`, `default_camera.ron` or \
                     `default_cues.ron`"
                );
                None
            }
        });
        own.chain(extra).collect()
    }

    /// What a new fight starts from: see [`LiveMoves`].
    pub fn moves(&self) -> &LiveMoves {
        &self.moves
    }

    /// What a cook made of this document, if it's one that a cook makes.
    pub fn cooked_set(&self, origin: &str, content: &Content) -> Option<MoveSet> {
        let document = document_file_name(origin);
        let cooked = self.cooked.iter().find(|cooked| cooked.document == document)?;
        content.move_set(&cooked.handle)?.to_owned_set().ok()
    }

    /// Takes an edited document of moves, with what its moves that name clips read their tables from: the
    /// newest edit, or else `cooked`. Says what became of it, or why what's playing stays.
    pub fn moves_document(
        &mut self,
        origin: &str,
        text: &str,
        cooked: Option<&MoveSet>,
        fight: Fight<'_>,
    ) -> Result<String, String> {
        let document = document_file_name(origin);
        self.check_takes(document)?;
        let running = self.moves.latest(document).or(cooked);
        let live = compile_document(text, running).map_err(|error| error_chain(&error))?;
        let taken = self.take_moves(document, live.set, fight)?;
        Ok(if live.motion_as_cooked {
            format!("{taken}; its clips' motion stays as last cooked, which only a cook changes")
        } else {
            taken
        })
    }

    /// Gives every simulation that plays `document` its new set. Nothing unknown gets as far as a set, so a
    /// misnamed file can't replace another's. A replay refuses before anything changes.
    fn take_moves(&mut self, document: &str, set: MoveSet, fight: Fight<'_>) -> Result<String, String> {
        self.check_takes(document)?;
        let Fight { combat, scene } = fight;
        let mut played = None;
        if let Some(combat) = combat.filter(|combat| combat.plays(document).is_some()) {
            played = combat.tune(document, set.clone())?;
        }
        if let Some(show) = scene.tune_show(document, set.clone()) {
            played = Some(show);
        }
        let unchanged = matches!(played, Some((_, Tuned::Unchanged { .. })));
        if !unchanged {
            self.moves.remember(document, set);
            self.count_edit(document);
        }
        Ok(match played {
            Some((set, Tuned::Replaced { revision })) => {
                self.waiting.retain(|waiting| waiting.document != document);
                let (document, stopwatch) = (document.to_owned(), Stopwatch::start());
                let reply = format!("moves of {document}, as revision {revision}");
                self.waiting.push(Waiting { document, set, stopwatch });
                reply
            }
            Some((_, Tuned::Unchanged { revision })) => {
                format!("moves of {document} with the same moves, still revision {revision}")
            }
            None => format!("moves of {document}, for the next fight: none plays it now"),
        })
    }

    /// A fight starts on the simulation's own cooked copy of its moves. A pack's set that differs from
    /// that is what a cook made of an edited document, as `cargo xtask run` makes one before it launches,
    /// so a fight starts from it, and a saved edit lasts past a relaunch. A fresh pack and a fresh copy
    /// are the same bytes, so this changes nothing (`tools/sapphire_pipeline/tests/player_moves.rs`).
    pub fn start_fight_from_cooks(&self, combat: &mut Combat, content: &Content) {
        for cooked in &self.cooked {
            let Some(set) = content.move_set(&cooked.handle).and_then(|set| set.to_owned_set().ok()) else {
                continue;
            };
            if combat.start_fight_from(cooked.document, set) {
                tracing::info!(
                    target: LOG_TARGET,
                    document = cooked.document,
                    "the fight starts from what a cook made of this document, not the build's copy",
                );
            }
        }
    }

    /// Says why a document of moves reaches nothing here, if it doesn't.
    fn check_takes(&self, document: &str) -> Result<(), String> {
        if self.takes(document) {
            return Ok(());
        }
        let known: Vec<&str> = self.documents_of(Document::Moves).collect();
        Err(format!("no fight here plays `{document}`: it takes {known:?}"))
    }

    /// Takes an edited cue table, for what fires a fight's cues, and has the renderer prewarm what its
    /// screen effects draw through. Says why not if a name it holds isn't there, keeping what fires.
    pub fn cue_document(
        &mut self,
        text: &str,
        scene: &mut Scene,
        renderer: &mut Renderer,
    ) -> Result<String, String> {
        let table = CueTable::from_ron(text).map_err(|error| error_chain(&error))?;
        scene.set_cue_table(table)?;
        renderer.prewarm_post(scene.effect_variants());
        self.count_edit(document_file_name(CUES_PATH));
        Ok(Document::Cues.name().to_owned())
    }

    /// Takes an edited camera tuning, for the director and for what fires a fight's cues.
    pub fn camera_document(&mut self, text: &str, scene: &mut Scene) -> Result<String, String> {
        let tuning = CameraTuning::from_ron(text).map_err(|error| error_chain(&error))?;
        scene.set_camera_tuning(tuning)?;
        self.count_edit(document_file_name(CAMERA_PATH));
        Ok("camera's tuning".to_owned())
    }

    /// Takes an edited animation graph: checked as a cook checks it, its clips loaded if they aren't, and
    /// bound against this character. Everything before the swap can fail, and leaves what's playing.
    pub fn graph_document(
        &mut self,
        text: &str,
        content: &mut Content,
        scene: &mut Scene,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<String, String> {
        let mut graph = AnimGraph::from_document(text).map_err(|error| error_chain(&error))?;
        graph.resolve(clip_id);
        scene.replace_graph_document(graph, content, renderer, gpu)?;
        self.count_edit(document_file_name(GRAPH_PATH));
        Ok("character's animation graph, from the next state's start".to_owned())
    }

    /// Takes the new versions of cooked sets among the asset server's `changes`, and returns what went
    /// wrong. Allocates nothing while none came.
    pub fn follow_packs(
        &mut self,
        changes: &[AssetChange],
        content: &Content,
        mut fight: Fight<'_>,
    ) -> Vec<String> {
        let mut errors = Vec::new();
        for change in changes {
            let AssetChange::Reloaded { id, kind: AssetKind::MoveSet, .. } = change else { continue };
            let Some(cooked) = self.cooked.iter().find(|cooked| cooked.handle.id() == *id) else { continue };
            let document = cooked.document;
            let Some(set) = content.move_set(&cooked.handle) else { continue };
            let fight = Fight { combat: fight.combat.as_deref_mut(), scene: &mut *fight.scene };
            let taken = set
                .to_owned_set()
                .map_err(|error| error_chain(&error))
                .and_then(|set| self.take_moves(document, self.newest_of(document, set), fight));
            match taken {
                Ok(what) => tracing::info!(target: LOG_TARGET, what, "a pack replaced the moves"),
                Err(error) => errors.push(format!("{document}, from its pack: {error}")),
            }
        }
        errors
    }

    /// What to take of a pack's `cooked` set. A cook reads a document when it starts, so one that started
    /// before the newest save brings what the file said then, and taking that would undo the save with
    /// nothing to say so, while the file went on saying otherwise. So where there's a file it's the word,
    /// over the tables the cook made. What a cook read is the same bytes, and so no edit past its own. With
    /// no file, on a phone, or one that doesn't read yet, the cook is all there is.
    fn newest_of(&self, document: &str, cooked: MoveSet) -> MoveSet {
        let modified = |path: &&PathBuf| std::fs::metadata(path).and_then(|file| file.modified()).ok();
        let named = self.files.iter().filter(|path| path.file_name().is_some_and(|name| name == document));
        let Some(text) = named.max_by_key(modified).and_then(|path| std::fs::read_to_string(path).ok()) else {
            return cooked;
        };
        compile_document(&text, Some(&cooked)).map_or(cooked, |live| live.set)
    }

    /// Once a frame, after its fixed steps and before it draws: says when an attempt first plays what an
    /// edit made. Allocates nothing while no edit waits.
    pub fn after_steps(&mut self, combat: Option<&Combat>, scene: &Scene) {
        if self.waiting.is_empty() {
            return;
        }
        let combat = combat.map(|combat| (combat.simulation(), combat.cast()));
        let show = scene.graph_show().map(|show| (show.simulation(), show.cast()));
        self.waiting.retain(|waiting| {
            let playing = [combat, show].into_iter().flatten().find_map(|(sim, cast)| {
                cast.iter().find_map(|actor| {
                    let actor = &sim.party().on_field(*actor);
                    let current = sim.actor(*actor)?.moves.current?;
                    // The newest, which a fight started over from an edit has as its revision 0.
                    let newest = sim.move_library().current_revision(waiting.set);
                    let new = current.at.set == waiting.set && Some(current.at.revision) == newest;
                    new.then_some((sim, *actor, current.at))
                })
            });
            let Some((sim, actor, at)) = playing else { return true };
            tracing::info!(
                target: LOG_TARGET,
                document = waiting.document,
                mv = sim.move_library().get(at).map_or("", |mv| mv.label()),
                revision = at.revision,
                actor = actor.raw(),
                tick = sim.tick(),
                since_ms = format_args!("{:.1}", waiting.stopwatch.elapsed().as_secs_f64() * 1e3),
                "the next attempt plays the new moves"
            );
            false
        });
    }

    /// What live link's status lists.
    pub fn documents(&self) -> Vec<LiveDocument> {
        let owner = |source: &Source| source.document.name().to_owned();
        self.sources
            .iter()
            .map(|source| LiveDocument {
                source: source.path.clone(),
                owner: owner(source),
                edits: source.edits,
            })
            .collect()
    }

    /// Whether `document`, a file name, is one of a fight's moves here.
    fn takes(&self, document: &str) -> bool {
        self.documents_of(Document::Moves).any(|known| known == document)
    }

    fn documents_of(&self, kind: Document) -> impl Iterator<Item = &str> {
        self.sources
            .iter()
            .filter(move |source| source.document == kind)
            .map(|source| document_file_name(&source.path))
    }

    fn count_edit(&mut self, document: &str) {
        if let Some(source) =
            self.sources.iter_mut().find(|source| document_file_name(&source.path) == document)
        {
            source.edits += 1;
        }
    }
}

/// What `cast` is playing, for live link's status.
pub(crate) fn attempts(sim: &Simulation, cast: &[ActorId]) -> Vec<MoveAttempt> {
    let library = sim.move_library();
    let window = |span: &Span| TickWindow { from: span.from, to: span.to };
    cast.iter()
        .filter_map(|actor| {
            let actor = sim.party().on_field(*actor);
            let view = sim.actor(actor)?;
            let current = view.moves.current?;
            let mv = library.get(current.at)?;
            Some(MoveAttempt {
                actor: actor.raw(),
                name: mv.label().to_owned(),
                revision: current.at.revision,
                newest_revision: library.current_revision(current.at.set)?,
                serial: current.serial,
                startup: mv.startup,
                active: mv.active,
                recovery: mv.recovery,
                hit_windows: mv.hitboxes.iter().map(|hitbox| window(&hitbox.span)).collect(),
                cancel_windows: mv.cancels.iter().map(|cancel| window(&cancel.span)).collect(),
            })
        })
        .collect()
}

/// A document's kind from its file's name, for a path hot reload is told to watch.
pub(crate) fn kind_of_file(path: &Path) -> Option<Document> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".moves.ron") {
        Some(Document::Moves)
    } else if name.ends_with(".animgraph.ron") {
        Some(Document::Graph)
    } else if name == document_file_name(CAMERA_PATH) {
        Some(Document::Camera)
    } else if name == document_file_name(CUES_PATH) {
        Some(Document::Cues)
    } else {
        None
    }
}

/// They need no GPU, but compile only in `dev` builds, which CI tests in its GPU job: run them with
/// `cargo test -p sapphire_sandbox --features dev --lib -- --ignored tuning::`.
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sapphire_moves::MoveSetDocument;
    use sapphire_render::MeshHandle;

    use super::*;
    use crate::combat::no_bindings;
    use crate::options::CombatOption;

    const SLICE: &str = include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron");
    const SLICE_FILE: &str = "slice.moves.ron";

    /// A file name of this process's own in the temp directory, deleted again when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let file = format!("sapphire-tuning-{}-{name}.sreplay", std::process::id());
            let path = std::env::temp_dir().join(file);
            let _ = std::fs::remove_file(&path);
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn tuning() -> LiveTuning {
        LiveTuning {
            moves: LiveMoves::default(),
            sources: vec![Source {
                path: "crates/sapphire_moves/test_content/slice.moves.ron".to_owned(),
                document: Document::Moves,
                edits: 0,
            }],
            waiting: Vec::new(),
            cooked: Vec::new(),
            files: Vec::new(),
        }
    }

    fn scene() -> Scene {
        Scene::with_character(MeshHandle::from_raw(0), [[0.0; 3]; 2], false)
    }

    fn recording(path: &Path) -> Combat {
        Combat::new(&CombatOption::Record {
            path: path.to_owned(),
            scenario: "duel".to_owned(),
            script: Some("attempts".to_owned()),
        })
        .expect("sets up")
    }

    fn slower_text() -> String {
        let mut document = MoveSetDocument::parse(SLICE).expect("parses");
        let light = document.moves.iter_mut().find(|mv| mv.name == "light_1").expect("there");
        light.startup += 2;
        light.recovery -= 2;
        for hitbox in &mut light.hitboxes {
            hitbox.from += 2;
            hitbox.to += 2;
        }
        document.to_text().expect("writes")
    }

    /// The rule the whole package is for, through the app's own path: pushed text, the next attempt.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_document_reaches_the_fight_by_its_file_name_and_refuses_what_it_cant_take() {
        let scratch = Scratch::new("reach");
        let mut combat = recording(&scratch.0);
        let mut scene = scene();
        let mut tuning = tuning();
        let mut take = |tuning: &mut LiveTuning, combat: &mut Combat, name: &str, text: &str| {
            tuning.moves_document(name, text, None, Fight { combat: Some(combat), scene: &mut scene })
        };

        let reply = take(&mut tuning, &mut combat, SLICE_FILE, &slower_text()).expect("a good document");
        assert!(reply.contains("revision 1"), "{reply}");
        assert_eq!(tuning.documents()[0].edits, 1);
        let again = take(&mut tuning, &mut combat, SLICE_FILE, &slower_text()).expect("the same document");
        assert!(again.contains("still revision 1"), "{again}");
        assert_eq!(tuning.documents()[0].edits, 1, "saving it again edits nothing");

        // A bad one names the move and the track, and keeps what's playing.
        let mut bad = MoveSetDocument::parse(SLICE).expect("parses");
        bad.moves[0].hitboxes[0].to = 400;
        let error = take(&mut tuning, &mut combat, SLICE_FILE, &bad.to_text().expect("writes"))
            .expect_err("a window past its move's end");
        assert!(error.contains("move `light_1`, track `hitboxes`"), "{error}");
        let error = take(&mut tuning, &mut combat, SLICE_FILE, "(not a document").expect_err("no document");
        assert!(!error.is_empty());

        // Another file's name, whatever its moves, reaches nothing.
        let error = take(&mut tuning, &mut combat, "other.moves.ron", SLICE).expect_err("nobody plays it");
        assert!(error.contains("other.moves.ron") && error.contains(SLICE_FILE), "{error}");

        // The next good one works.
        let mut good = MoveSetDocument::parse(&slower_text()).expect("parses");
        good.moves[0].recovery += 1;
        let reply =
            take(&mut tuning, &mut combat, SLICE_FILE, &good.to_text().expect("writes")).expect("good");
        assert!(reply.contains("revision 2"), "{reply}");

        // And an attempt plays it, which the first frame on it says.
        assert_eq!(tuning.waiting.len(), 1);
        let cast = combat.cast().to_vec();
        combat.step(&mut no_bindings);
        tuning.after_steps(Some(&combat), &scene);
        assert!(tuning.waiting.is_empty(), "an attempt reads it");
        let playing = attempts(combat.simulation(), &cast);
        let light = playing.iter().find(|attempt| attempt.name == "light_1").expect("the player's");
        assert_eq!((light.revision, light.newest_revision, light.startup), (2, 2, 7));
        assert!(!light.hit_windows.is_empty());
    }

    /// A replay plays what it recorded, so an edit is refused, and says why.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_replay_refuses_a_live_edit() {
        let scratch = Scratch::new("replay");
        let mut recorded = recording(&scratch.0);
        recorded.step(&mut no_bindings);
        recorded.finish().expect("writes");
        let option = CombatOption::Replay { path: scratch.0.clone(), to_end: true };
        let mut replay = Combat::new(&option).expect("reads");
        let mut scene = scene();
        let error = tuning()
            .moves_document(
                SLICE_FILE,
                &slower_text(),
                None,
                Fight { combat: Some(&mut replay), scene: &mut scene },
            )
            .expect_err("it plays what it recorded");
        assert!(error.contains("replay"), "{error}");
    }

    /// Moves that name clips, as a real character's do: a saved window plays with no cook, on the tables
    /// the cook baked, and the reply says what it left alone. With no cook behind it, it's refused.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn an_edit_of_moves_with_clips_reads_the_tables_a_cook_baked() {
        use sapphire_moves::{BakedTables, ClipMotionDocument, MoveDocument, RootSample, RootSource};

        let with_a_clip = |text: &str| {
            let mut document = MoveSetDocument::parse(text).expect("parses");
            let light = document.moves.iter_mut().find(|mv| mv.name == "light_1").expect("there");
            light.motion.root.clear();
            light.motion.clip = Some(ClipMotionDocument {
                source: "6d321a52-1aed-4279-bc32-a177b8fd81d4".to_owned(),
                name: None,
                rig: None,
                root: RootSource::Clip,
                paths: Vec::new(),
            });
            document
        };
        let cook = |document: &MoveSetDocument| {
            let tables = |mv: &MoveDocument| {
                let root = vec![RootSample::ZERO; mv.ticks() as usize + 1];
                let baked = BakedTables::new(root, Vec::new()).map_err(|error| error.to_string())?;
                Ok(mv.motion.clip.as_ref().map(|_| baked))
            };
            document.compile_with(tables).expect("a cook")
        };
        let cooked =
            cook(&with_a_clip(include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron")));
        let edited = with_a_clip(&slower_text());
        let text = edited.to_text().expect("writes");

        let scratch = Scratch::new("clips");
        let mut combat = recording(&scratch.0);
        let mut scene = scene();
        let mut tuning = tuning();
        let error = tuning
            .moves_document(SLICE_FILE, &text, None, Fight { combat: Some(&mut combat), scene: &mut scene })
            .expect_err("nothing to read its motion from");
        assert!(error.contains("only a cook"), "{error}");

        let reply = tuning
            .moves_document(
                SLICE_FILE,
                &text,
                Some(&cooked),
                Fight { combat: Some(&mut combat), scene: &mut scene },
            )
            .expect("no cook needed");
        assert!(reply.contains("as revision 1") && reply.contains("as last cooked"), "{reply}");
        let set = combat.plays(SLICE_FILE).expect("it plays this");
        assert!(combat.simulation().move_library().is_current(set, &cook(&edited)));

        // A later edit reads the last, with no cook handed over.
        let reply = tuning
            .moves_document(SLICE_FILE, &text, None, Fight { combat: Some(&mut combat), scene: &mut scene })
            .expect("it has the tables now");
        assert!(reply.contains("still revision 1"), "{reply}");
    }

    /// A recording that crossed an edit says so when it's saved, and the file holds what came before.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_recording_ends_its_file_at_a_live_edit_and_says_so() {
        let scratch = Scratch::new("cut");
        let mut combat = recording(&scratch.0);
        let mut scene = scene();
        let mut tuning = tuning();
        for _ in 0..20 {
            combat.step(&mut no_bindings);
        }
        tuning
            .moves_document(
                SLICE_FILE,
                &slower_text(),
                None,
                Fight { combat: Some(&mut combat), scene: &mut scene },
            )
            .expect("a good document");
        for _ in 0..20 {
            combat.step(&mut no_bindings);
        }
        assert_eq!(combat.simulation().tick(), 40, "the fight goes on");
        assert_eq!(combat.edited_at(), Some(20));
        combat.finish().expect("writes");
        let replay = sapphire_gameplay::replay::Replay::decode(&std::fs::read(&scratch.0).expect("written"))
            .expect("reads");
        assert_eq!(replay.ticks(), 20);
    }

    /// With no fight running, an edit waits for the next, whose file says what it ran on.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn an_edit_with_no_fight_running_is_what_the_next_one_starts_from() {
        let mut scene = scene();
        let mut tuning = tuning();
        let reply = tuning
            .moves_document(SLICE_FILE, &slower_text(), None, Fight { combat: None, scene: &mut scene })
            .expect("a good document");
        assert!(reply.contains("next fight"), "{reply}");
        let dir = std::env::temp_dir().join(format!("sapphire-tuning-{}-next", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut recorder = crate::combat::SessionRecorder::new(dir.clone());
        let mut combat = None;
        recorder.start_tuned(&mut combat, "duel", tuning.moves());
        let fight = combat.as_ref().expect("started");
        let set = sapphire_gameplay::replay::find_scenario(SCENARIOS, "duel")
            .and_then(|scenario| scenario.set_of(SLICE_FILE))
            .expect("its set");
        let library = fight.simulation().move_library();
        let edited = compile_document(&slower_text(), None).expect("compiles");
        assert!(library.is_current(set, &edited.set));
        assert_eq!(library.current_revision(set), Some(0));
        recorder.discard(&mut combat);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The table holds cooked sets to their sidecars, or a pack's new version goes unnoticed.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn cooked_moves_are_their_sidecars() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let sidecars = [
            ("show.moves.ron", "lookdev/character/show.moves.ron.meta"),
            ("player.moves.ron", "combat/player/player.moves.ron.meta"),
            ("warrior.moves.ron", "combat/enemy/warrior.moves.ron.meta"),
        ];
        for (document, id) in COOKED_MOVES {
            let (_, sidecar) = sidecars.iter().find(|(name, _)| *name == document).expect("a row");
            let text = std::fs::read_to_string(root.join(sidecar)).expect("a sidecar");
            let written = text.split("id: \"").nth(1).and_then(|rest| rest.split('"').next()).expect("an ID");
            assert_eq!(AssetId::from_text(written), id, "{document}");
        }
    }

    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_files_name_says_what_kind_of_document_it_is() {
        assert_eq!(kind_of_file(Path::new("x/fighter.moves.ron")), Some(Document::Moves));
        assert_eq!(kind_of_file(Path::new("x/character.animgraph.ron")), Some(Document::Graph));
        assert_eq!(kind_of_file(Path::new("x/default_camera.ron")), Some(Document::Camera));
        assert_eq!(kind_of_file(Path::new("x/default_cues.ron")), Some(Document::Cues));
        assert_eq!(kind_of_file(Path::new("x/default_day.ron")), None);
    }

    /// `player.moves.ron` with `light_1` two ticks later, over `running`, as a save with no cook makes it.
    fn later_light(running: &MoveSet) -> MoveSet {
        let text = include_str!("../../../content/combat/player/player.moves.ron");
        let mut document = MoveSetDocument::parse(text).expect("parses");
        let light = document.moves.iter_mut().find(|mv| mv.name == "light_1").expect("there");
        light.startup += 2;
        light.recovery -= 2;
        for hitbox in &mut light.hitboxes {
            hitbox.from += 2;
            hitbox.to += 2;
        }
        compile_document(&document.to_text().expect("writes"), Some(running)).expect("compiles").set
    }

    fn a_fight() -> Combat {
        let option = CombatOption::Fight { script: Some("attempts".to_owned()), party: false };
        Combat::new(&option).expect("it starts")
    }

    /// A fight with no file takes a live edit, ends no file over it, and starts over from it: revision 0,
    /// as if the build had it.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_fight_takes_an_edit_and_starts_over_from_it() {
        const PLAYER: &str = "player.moves.ron";
        let mut combat = a_fight();
        let id = combat.plays(PLAYER).expect("it plays this document");
        let edited = later_light(combat.simulation().move_library().current(id).expect("it runs"));
        let tuned = combat.tune(PLAYER, edited.clone()).expect("no refusal");
        assert_eq!(tuned, Some((id, Tuned::Replaced { revision: 1 })));
        assert_eq!(combat.edited_at(), None, "it has no file to end");
        let same = combat.tune(PLAYER, edited.clone()).expect("no refusal");
        assert_eq!(same, Some((id, Tuned::Unchanged { revision: 1 })));

        combat.restart();
        let library = combat.simulation().move_library();
        assert_eq!(library.current_revision(id), Some(0));
        assert!(library.is_current(id, &edited));
        assert_eq!(combat.simulation().tick(), 0);
    }

    /// All of a party plays one document, so an edit of it reaches a party's fight as it does anyone's, and
    /// the next fight starts from it as a party.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_partys_fight_takes_an_edit_and_starts_over_from_it() {
        const PLAYER: &str = "player.moves.ron";
        let option = CombatOption::Fight { script: Some("attempts".to_owned()), party: true };
        let mut combat = Combat::new(&option).expect("it starts");
        let id = combat.plays(PLAYER).expect("it plays this document");
        let edited = later_light(combat.simulation().move_library().current(id).expect("it runs"));
        let tuned = combat.tune(PLAYER, edited.clone()).expect("no refusal");
        assert_eq!(tuned, Some((id, Tuned::Replaced { revision: 1 })));

        combat.restart();
        let simulation = combat.simulation();
        assert!(simulation.move_library().is_current(id, &edited));
        assert_eq!(simulation.party().slots().count(), sapphire_gameplay::PARTY_SLOTS);
    }

    /// A fight that hasn't stepped starts from a cook that differs from the build's copy, and goes on
    /// starting from it. A cook that doesn't differ, a set it doesn't play and a fight under way change
    /// nothing.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_fight_starts_from_a_newer_cook_of_its_moves() {
        const PLAYER: &str = "player.moves.ron";
        let mut combat = a_fight();
        let id = combat.plays(PLAYER).expect("it plays this document");
        let baked = sapphire_gameplay::replay::player_moves().expect("reads");
        let edited = later_light(&baked);
        assert!(!combat.start_fight_from(PLAYER, baked.clone()), "the same bytes");
        assert!(!combat.start_fight_from(SLICE_FILE, edited.clone()), "not one of this fight's");
        let hash = combat.simulation().tables_hash();
        assert!(combat.start_fight_from(PLAYER, edited.clone()));
        assert_ne!(combat.simulation().tables_hash(), hash);
        assert_eq!(combat.simulation().move_library().current_revision(id), Some(0));
        assert!(combat.simulation().move_library().is_current(id, &edited));
        combat.restart();
        assert!(combat.simulation().move_library().is_current(id, &edited), "the next fight too");

        combat.step(&mut no_bindings);
        assert!(!combat.start_fight_from(PLAYER, baked), "under way");
    }

    /// The paths this module watches are the files the build compiled in.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn the_documents_it_watches_are_there() {
        for path in [GRAPH_PATH, CAMERA_PATH, CUES_PATH, SHOW_MOVES_PATH] {
            assert!(sources::repository_file(path).is_file(), "{path}");
        }
        let camera = std::fs::read_to_string(sources::default_camera()).expect("the camera's document");
        assert_eq!(CameraTuning::from_ron(&camera).expect("reads"), CameraTuning::default_tuning());
        assert!(sources::repository_file(CAMERA_PATH).ends_with("default_camera.ron"));
        let cues = std::fs::read_to_string(sources::default_cues()).expect("the cue table's document");
        assert_eq!(CueTable::from_ron(&cues).expect("reads"), CueTable::default_table());
        assert!(sources::repository_file(CUES_PATH).ends_with("default_cues.ron"));
    }

    /// The cue table has one route: a watched file and a pushed one both go through its `Document` row, and
    /// only a run that fires cues watches it, or lists it for `link watch`.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn the_cue_table_is_a_document_of_a_run_that_fires_cues() {
        let sources = |extra: &[&str]| {
            let run =
                ["sandbox", "--offscreen", "--record", "a", "--scenario", "duel", "--script", "attempts"];
            let args: Vec<String> = run.iter().chain(extra).map(|arg| (*arg).to_owned()).collect();
            LiveTuning::sources_of(&Options::parse(&args).expect("options").0)
        };
        let has = |sources: &[Source], document, path: &str| {
            sources.iter().any(|source| source.document == document && source.path == path)
        };
        let firing = sources(&["--cues"]);
        assert!(has(&firing, Document::Cues, CUES_PATH));
        assert!(has(&firing, Document::Camera, CAMERA_PATH), "its shakes read the camera's tuning");
        let quiet = sources(&[]);
        assert!(!has(&quiet, Document::Cues, CUES_PATH));
        assert!(!has(&quiet, Document::Camera, CAMERA_PATH));
    }
}
