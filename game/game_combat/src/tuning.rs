//! Live tuning in `dev` builds (M2 T2, exit test 2): a saved change to a fight plays on the next attempt,
//! with no restart. Hot reload and live link both bring documents here ([`Document`]):
//!
//! - **Moves and fighters** (`.moves.ron`): the document compiles on the device, its issues naming the
//!   move and the track, and replaces its set in whichever simulation plays it, by file name. A running
//!   move keeps the revision it started with, the next start reads the new one, and a fighter takes its
//!   numbers at once, the next step moving by its `movement`. There are no clips to bake from here, so a
//!   move that names one reads the tables its last cook gave it, and a move's windows change with no cook.
//!   Its length is its clip's, and an edit of
//!   its clip is a cook's to bring, through a pack ([`LiveTuning::follow_packs`]), by asset ID, with
//!   `cargo xtask content` and hot reload's remount or `cargo xtask link push`. The reply says so. A cook
//!   older than the newest save leaves the save playing, over the cook's tables.
//! - **The camera** (`default_camera.ron`): the director's tuning, whose goals ease to the new numbers.
//! - **The animation graphs** (`character.animgraph.ron`, on the graph show's character or a fight's player,
//!   and `warrior.animgraph.ron`, on a fight's enemy): bound against what's loaded, each takes effect at the
//!   next state's start ([`sapphire_anim::GraphPlayer::replace`]). A clip it names must be cooked for that
//!   character, and a new one comes through a cook. Their pack's route was N3's, and still works. A cook
//!   older than the newest save leaves the save playing, as with moves.
//! - **The cue table** (`default_cues.ron`, M2 E3): what fires a fight's cues takes it if every name it holds
//!   is there and every sound it plays was loaded as the app started. Camera tuning can move a shake's ID, so
//!   a new tuning resolves the table's shakes again (`FightShow::set_camera_tuning`) and is refused if one has
//!   gone. **Colliders aren't tuned**, and a running fight never changes under its feet.
//! - **The effects** (`default_effects.ron`): what a fight's cues and sword trails play takes them if the cue
//!   table still finds its names. What's playing ends, and a trail starts again at the next cut.
//!
//! - **A retained screen** (`.ui.ron`, M3 U4): a document that parses, checks and resolves against the
//!   model replaces the screen on show, with `--ui-screen`, and one that doesn't keeps it.
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
//! moves`, and live link's status lists what each fighter is playing, with its revision and windows. A
//! `movement` that changed logs `the next step moves by the new numbers` after that step, and the reply says
//! so too, since no press has to come.
//!
//! **For the timeline editor (M2 T3):** save the document, then `sapphire_livelink::Client::push_document`
//! with its file name and text: `Ok` says what it replaced, and `LinkError::Refused` carries the issues.
//! `Client::status()` then shows the attempts that read it. Nothing here needs to change for that.

use std::path::{Path, PathBuf};

use sapphire_anim::CharacterMotion;
use sapphire_assets::graph::{AnimGraph, clip_id};
use sapphire_assets::ui::UiDocument;
use sapphire_assets::{AssetChange, AssetKind, Handle};
use sapphire_camera::CameraTuning;
use sapphire_cues::CueTable;
use sapphire_gameplay::replay::{LiveMoves, SCENARIOS, compile_document, document_file_name};
use sapphire_gameplay::targeting::TargetingTuning;
use sapphire_gameplay::{ActorId, MoveSetId, Simulation, Tuned};
use sapphire_hot_reload::sources;
use sapphire_input::InputBindings;
use sapphire_livelink::protocol::{LiveDocument, MoveAttempt, TickWindow};
use sapphire_moves::{ControllerProfile, MoveSet, MoveSetDocument, Span};
use sapphire_reflect::stable_name;
use sapphire_render::Renderer;
use sapphire_rhi::GpuContext;
use sapphire_time::Stopwatch;
use sapphire_ui::HudDesign;
use sapphire_world::light_rig::LightRigLibrary;
use sapphire_world::screen_effects::EffectLibrary;
use sapphire_world::tod::TodTimeline;

use crate::LOG_TARGET;
use crate::combat::Combat;
pub use crate::documents::error_chain;
use crate::fight::{ENEMY_GRAPH, Fight, FightShow};
use sapphire_stage::anim_graph::SHOW_MOVES_PATH;
use sapphire_stage::content::{Content, GraphKey, newest_text};
use sapphire_stage::scene::Scene;

/// A document the sandbox swaps in while it runs, from hot reload or live link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Document {
    /// The day's timeline (`default_day.ron`, or the look-dev day with `--look-dev`): it replaces
    /// time of day's base timeline, if it comes from the file of the day the scene runs on
    /// ([`Scene::replace_day`]), parses and is complete.
    Day,
    /// The character light rig presets (`default_light_rigs.ron`, or the look-dev stage's with
    /// `--look-dev`): they replace the scene's library, if they come from the file of the presets
    /// the scene runs on ([`Scene::replace_light_rigs`]), parse and are valid; characters keep
    /// their preset IDs.
    LightRigs,
    /// The screen effect presets (`default_screen_effects.ron`, M1 P3): they replace the scene's
    /// library, if they parse and are valid.
    ScreenEffects,
    /// The cue table (`default_cues.ron`, M2 E3): it replaces the one a fight's cues fire through, if
    /// there's a fight, it parses and the names it holds are all there.
    Cues,
    /// The effects a fight's cues and sword trails play (`default_effects.ron`): they replace what's
    /// playing's, if there's a fight, they parse and the cue table's names are all there.
    Effects,
    /// A fight's moves and fighter (`.moves.ron`, M2 T2): they replace the set of that file's name, for
    /// the next attempt, if they compile.
    Moves,
    /// The camera's tuning (`default_camera.ron`, M2 T2): it replaces the director's, if there is one and
    /// it's in range.
    Camera,
    /// A fighter's animation graph (`.animgraph.ron`, M2 T2): it replaces the one playing from the next
    /// state's start, if it checks and binds.
    Graph,
    /// A retained screen (`.ui.ron`, M3 U4): it replaces the one on show, if there is one, and if it
    /// parses, checks and resolves against its model ([`DocumentHost::take_screen`]).
    Ui,
    /// Combat's bindings (`combat_bindings.ron`, M3 S3): they replace what the app reads, from its next update,
    /// if they compile and hold everything combat reads.
    Bindings,
    /// Who a soft lock picks (`default_targeting.ron`, M3 S3): the fight's simulation takes them, which ends
    /// a recording's file, and a new fight starts on them. A replay refuses.
    Targeting,
    /// Hair, cloth and IK (`default_character_motion.ron`, M3 S3): what stands takes them, wherever it's got
    /// to, and so does what's built later.
    CharacterMotion,
    /// Where the HUD's pieces go (`default_hud.ron`, M3 S3): it lays out again, if there's one.
    Hud,
}

/// What a document reaches that isn't the fight's: an app's controls and its HUD. Each answers at once, since
/// a live link's push waits to hear why it was refused.
pub trait DocumentHost {
    /// Takes an edited document of bindings, or says why what's there stays.
    fn take_bindings(&mut self, bindings: InputBindings) -> Result<(), String>;
    /// Lays the HUD out by `design`, or says there's none here.
    fn take_hud_design(&mut self, design: HudDesign) -> Result<(), String>;
    /// Replaces the retained screen on show from a document's text, keeping what's on screen if the text has
    /// anything wrong with it, or says there's none here.
    fn take_screen(&mut self, text: &str) -> Result<(), String>;
}

/// What applying a document can reach, once a document.
pub struct DocumentFrame<'a> {
    pub content: &'a mut Content,
    pub renderer: &'a mut Renderer,
    pub gpu: &'a GpuContext,
    pub scene: &'a mut Scene,
    pub fight: &'a mut Fight,
    pub tuning: &'a mut LiveTuning,
    pub host: &'a mut dyn DocumentHost,
}

impl Document {
    const ALL: [Self; 13] = [
        Self::Day,
        Self::LightRigs,
        Self::ScreenEffects,
        Self::Cues,
        Self::Effects,
        Self::Moves,
        Self::Camera,
        Self::Graph,
        Self::Ui,
        Self::Bindings,
        Self::Targeting,
        Self::CharacterMotion,
        Self::Hud,
    ];

    /// The document whose envelope names `type_name`, if the sandbox has one.
    pub fn of_type(type_name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|document| document.type_name() == type_name)
    }

    fn type_name(self) -> &'static str {
        match self {
            Self::Day => stable_name::<TodTimeline>(),
            Self::LightRigs => stable_name::<LightRigLibrary>(),
            Self::ScreenEffects => stable_name::<EffectLibrary>(),
            Self::Cues => stable_name::<CueTable>(),
            Self::Effects => stable_name::<sapphire_vfx::EffectLibrary>(),
            Self::Moves => stable_name::<MoveSetDocument>(),
            Self::Camera => stable_name::<CameraTuning>(),
            Self::Graph => stable_name::<AnimGraph>(),
            Self::Ui => stable_name::<UiDocument>(),
            Self::Bindings => stable_name::<InputBindings>(),
            Self::Targeting => stable_name::<TargetingTuning>(),
            Self::CharacterMotion => stable_name::<CharacterMotion>(),
            Self::Hud => stable_name::<HudDesign>(),
        }
    }

    /// What it is, in messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::LightRigs => "light rigs",
            Self::ScreenEffects => "screen effects",
            Self::Cues => "cue table",
            Self::Effects => "effects",
            Self::Moves => "moves",
            Self::Camera => "camera's tuning",
            Self::Graph => "animation graph",
            Self::Ui => "screen",
            Self::Bindings => "bindings",
            Self::Targeting => "targeting's weights",
            Self::CharacterMotion => "character's motion",
            Self::Hud => "HUD's design",
        }
    }

    /// Parses `text`, read from `origin` (a path or a file name), and swaps it into its owner,
    /// returning what it replaced, or says why it keeps what it has. A timeline replaces the day
    /// only from the file of the day the scene runs on, so the default day can't replace the
    /// look-dev stage's ([`Scene::replace_day`]). New screen effect presets have the renderer
    /// prewarm their post variants, which the overlay may fire. A fight's documents go through
    /// [`LiveTuning`].
    pub fn apply(self, origin: &str, text: &str, frame: DocumentFrame<'_>) -> Result<String, String> {
        let DocumentFrame { content, renderer, gpu, scene, fight, tuning, host } = frame;
        match self {
            Self::Day => {
                let timeline = TodTimeline::from_ron(text).map_err(|error| error_chain(&error))?;
                Ok(scene.replace_day(origin, timeline)?.name().to_owned())
            }
            Self::LightRigs => {
                let rigs = LightRigLibrary::from_ron(text).map_err(|error| error_chain(&error))?;
                scene.replace_light_rigs(origin, rigs)?;
                Ok(self.name().to_owned())
            }
            Self::ScreenEffects => {
                let effects = EffectLibrary::from_ron(text).map_err(|error| error_chain(&error))?;
                fight.show.set_effect_library(scene, effects)?;
                renderer.prewarm_post(scene.effect_variants());
                Ok(self.name().to_owned())
            }
            Self::Cues => tuning.cue_document(text, &mut fight.show, scene, renderer),
            Self::Effects => tuning.effects_document(text, &mut fight.show, scene),
            Self::Moves => {
                let cooked = if tuning.moves.latest(document_file_name(origin)).is_none() {
                    tuning.cooked_set(origin, content)
                } else {
                    None
                };
                let combat = fight.combat.as_mut();
                tuning.moves_document(origin, text, cooked.as_ref(), LiveFight { combat, scene })
            }
            Self::Camera => tuning.camera_document(text, &mut fight.show, scene),
            // Checked as a cook checks it, its clips loaded if they aren't, and bound against this character.
            // Everything before the swap can fail, and leaves what's playing.
            Self::Graph => {
                let (owner, path) = graph_of(origin)?;
                let mut graph = AnimGraph::from_document(text).map_err(|error| error_chain(&error))?;
                graph.resolve(clip_id);
                fight.show.replace_graph_document(scene, owner, graph, content, renderer, gpu)?;
                tuning.count_edit(document_file_name(path));
                Ok(format!("{}'s animation graph, from the next state's start", owner.0))
            }
            Self::Ui => {
                host.take_screen(text)?;
                tuning.count_edit(document_file_name(SCREEN_PATH));
                Ok(self.name().to_owned())
            }
            Self::Bindings => {
                let bindings = sapphire_serialize::from_ron(text).map_err(|error| error_chain(&error))?;
                host.take_bindings(bindings)?;
                tuning.count_edit(document_file_name(BINDINGS_PATH));
                Ok(self.name().to_owned())
            }
            Self::Targeting => {
                let weights = TargetingTuning::from_ron(text).map_err(|error| error_chain(&error))?;
                if let Some(combat) = &mut fight.combat {
                    combat.tune_targeting(weights)?;
                }
                tuning.moves.set_targeting(weights);
                tuning.count_edit(document_file_name(TARGETING_PATH));
                Ok(self.name().to_owned())
            }
            Self::CharacterMotion => {
                let motion = CharacterMotion::from_ron(text).map_err(|error| error_chain(&error))?;
                content.set_character_motion(motion);
                scene.set_character_motion(&motion);
                tuning.count_edit(document_file_name(MOTION_PATH));
                Ok(self.name().to_owned())
            }
            Self::Hud => {
                let design = HudDesign::from_ron(text).map_err(|error| error_chain(&error))?;
                host.take_hud_design(design)?;
                tuning.count_edit(document_file_name(HUD_PATH));
                Ok(self.name().to_owned())
            }
        }
    }
}

/// What a run has that live tuning follows: what decides the documents it can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TunedRun<'a> {
    /// The camera's director frames it.
    pub camera_director: bool,
    /// A fight's cues fire.
    pub cues: bool,
    /// The animation graph plays from a script.
    pub graph_show: bool,
    /// A fight runs in the arena, each fighter on a graph.
    pub arena_fight: bool,
    /// A retained screen shows (`--ui-screen`).
    pub screen: bool,
    /// The combat HUD is over it.
    pub hud: bool,
    /// Whether anything reads saved files: off, only what's pushed comes.
    pub reads_files: bool,
    /// Documents to read outside the repository, each by its file's name.
    pub extra_documents: &'a [PathBuf],
}

/// The sandbox's test screen's document.
const SCREEN_PATH: &str = "content/ui/test_screen.ui.ron";
/// The animation graph's document (`lookdev::GRAPH`).
const GRAPH_PATH: &str = "content/lookdev/character/character.animgraph.ron";
/// The enemy's animation graph's document (`enemy::GRAPH`).
const ENEMY_GRAPH_PATH: &str = "content/combat/enemy/warrior.animgraph.ron";
/// The camera's tuning document.
const CAMERA_PATH: &str = "crates/sapphire_camera/default_camera.ron";
/// The cue table's document.
const CUES_PATH: &str = "crates/sapphire_cues/default_cues.ron";
/// The effects' document.
const EFFECTS_PATH: &str = "crates/sapphire_vfx/default_effects.ron";
/// Combat's bindings' document.
const BINDINGS_PATH: &str = "crates/sapphire_controls/combat_bindings.ron";
/// Targeting's weights' document.
const TARGETING_PATH: &str = "crates/sapphire_gameplay/default_targeting.ron";
/// The character motion document.
const MOTION_PATH: &str = "crates/sapphire_anim/default_character_motion.ron";
/// The HUD's design's document.
const HUD_PATH: &str = "crates/sapphire_ui/default_hud.ron";

/// What a live edit of a move set reaches: the fight in the app, and the graph show on the scene.
pub struct LiveFight<'a> {
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

/// A set's newest `movement`, which no step has moved by yet.
struct Moving {
    document: String,
    set: MoveSetId,
    /// The simulation's tick as the edit landed: a step after this reads it.
    tick: u64,
    stopwatch: Stopwatch,
}

/// A cooked set held so a new version of it is told of.
struct CookedSet {
    document: String,
    handle: Handle<MoveSet>,
}

/// Live tuning's state in the app. See the [module docs](self).
pub struct LiveTuning {
    moves: LiveMoves,
    sources: Vec<Source>,
    waiting: Vec<Waiting>,
    moving: Vec<Moving>,
    cooked: Vec<CookedSet>,
    /// The files hot reload reads this app's documents from, on a machine that has them.
    files: Vec<PathBuf>,
}

impl LiveTuning {
    /// The documents `run` can take: every scenario's, which the overlay's recorder can start,
    /// and the camera's, the cue table's, the graph's and the graph show's where those run. The cooked sets
    /// among them are loaded and kept (a few kilobytes each), so a pack's new version of one is told of.
    pub fn new(run: &TunedRun<'_>, content: &mut Content) -> Self {
        let sources = Self::sources_of(run);
        let mut cooked = Vec::new();
        for source in sources.iter().filter(|source| source.document == Document::Moves) {
            let document = document_file_name(&source.path);
            let name = document.split('.').next().unwrap_or(document);
            let found =
                content.named_assets(AssetKind::MoveSet).find(|set| set.name == name).map(|set| set.id);
            // A document no cook takes, as a scenario's in a crate: its edits come as documents only.
            let Some(id) = found else {
                tracing::debug!(target: LOG_TARGET, document, "no cooked moves to follow");
                continue;
            };
            match content.hold_move_set(id) {
                Ok(handle) => cooked.push(CookedSet { document: document.to_owned(), handle }),
                Err(error) => {
                    tracing::info!(target: LOG_TARGET, document, %error, "no cooked moves to follow")
                }
            }
        }
        let mut files: Vec<PathBuf> = Vec::new();
        if run.reads_files {
            files.extend(sources.iter().map(|source| sources::repository_file(&source.path)));
            files.extend(run.extra_documents.iter().cloned());
        }
        let graph_owner = |file: &PathBuf| Some(graph_of(file.to_str()?).ok()?.0);
        content.read_graphs_from(
            files.iter().filter_map(|file| Some((graph_owner(file)?, file.clone()))).collect(),
        );
        Self { moves: LiveMoves::default(), sources, waiting: Vec::new(), moving: Vec::new(), cooked, files }
    }

    /// The documents `run` can take.
    fn sources_of(run: &TunedRun<'_>) -> Vec<Source> {
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
        if run.screen {
            add(SCREEN_PATH, Document::Ui);
        }
        // A fight's cues fire the camera's shakes, by their places in its tuning.
        if run.camera_director || run.cues {
            add(CAMERA_PATH, Document::Camera);
        }
        if run.cues {
            add(CUES_PATH, Document::Cues);
            add(EFFECTS_PATH, Document::Effects);
        }
        if run.graph_show {
            add(SHOW_MOVES_PATH, Document::Moves);
        }
        // A fight's player plays the character's graph too, and its enemy its own (`Scene::play_fighters`).
        if run.graph_show || run.arena_fight {
            add(GRAPH_PATH, Document::Graph);
        }
        if run.arena_fight {
            add(ENEMY_GRAPH_PATH, Document::Graph);
            add(TARGETING_PATH, Document::Targeting);
        }
        add(BINDINGS_PATH, Document::Bindings);
        add(MOTION_PATH, Document::CharacterMotion);
        if run.hud {
            add(HUD_PATH, Document::Hud);
        }
        sources
    }

    /// Following only this cooked set of this document, for tests of a pack's route.
    #[cfg(test)]
    pub(crate) fn following(document: &'static str, handle: Handle<MoveSet>) -> Self {
        let path = format!("content/{document}");
        let source = Source { path, document: Document::Moves, edits: 0 };
        let cooked = vec![CookedSet { document: document.to_owned(), handle }];
        Self {
            moves: LiveMoves::default(),
            sources: vec![source],
            waiting: Vec::new(),
            moving: Vec::new(),
            cooked,
            files: Vec::new(),
        }
    }

    /// Reading its document from `file`, for tests of a save meeting a cook.
    #[cfg(test)]
    pub(crate) fn reading(mut self, file: PathBuf) -> Self {
        self.files.push(file);
        self
    }

    /// What hot reload watches for it: each document, and each of `extra`, by its file's name.
    pub fn watched(&self, extra: &[PathBuf]) -> Vec<(Document, PathBuf)> {
        let own = self.sources.iter().map(|source| (source.document, sources::repository_file(&source.path)));
        let extra = extra.iter().filter_map(|path| match kind_of_file(path) {
            Some(document) => Some((document, path.clone())),
            None => {
                tracing::warn!(
                    target: LOG_TARGET,
                    path = %path.display(),
                    "--watch-document takes `.moves.ron`, `.animgraph.ron`, `default_camera.ron`, \
                     `default_cues.ron`, `default_effects.ron`, `combat_bindings.ron`, \
                     `default_targeting.ron`, `default_character_motion.ron` or `default_hud.ron`"
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
    pub(crate) fn cooked_set(&self, origin: &str, content: &Content) -> Option<MoveSet> {
        let document = document_file_name(origin);
        let cooked = self.cooked.iter().find(|cooked| cooked.document == document)?;
        content.move_set(&cooked.handle)?.to_owned_set().ok()
    }

    /// Takes an edited document of moves, with what its moves that name clips read their tables from: the
    /// newest edit, or else `cooked`. Says what became of it, or why what's playing stays.
    pub(crate) fn moves_document(
        &mut self,
        origin: &str,
        text: &str,
        cooked: Option<&MoveSet>,
        fight: LiveFight<'_>,
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
    fn take_moves(&mut self, document: &str, set: MoveSet, fight: LiveFight<'_>) -> Result<String, String> {
        self.check_takes(document)?;
        let LiveFight { combat, scene } = fight;
        let mut played = None;
        let mut moved_by = None;
        let mut tick = 0;
        if let Some(combat) = combat.filter(|combat| combat.plays(document).is_some()) {
            moved_by = current_movement(combat.simulation(), combat.plays(document));
            played = combat.tune(document, set.clone())?;
            tick = combat.simulation().tick();
        }
        let show_set = scene.graph_show().and_then(|show| show.plays(document));
        if let Some(show) = scene.graph_show().filter(|_| show_set.is_some()) {
            moved_by = current_movement(show.simulation(), show_set);
        }
        if let Some(show) = scene.tune_show(document, set.clone()) {
            played = Some(show);
            tick = scene.graph_show().map_or(tick, |show| show.simulation().tick());
        }
        let moves_differently = moved_by != set.movement().copied();
        let unchanged = matches!(played, Some((_, Tuned::Unchanged { .. })));
        if !unchanged {
            self.moves.remember(document, set);
            self.count_edit(document);
        }
        Ok(match played {
            Some((set, Tuned::Replaced { revision })) => {
                self.waiting.retain(|waiting| waiting.document != document);
                let (document, stopwatch) = (document.to_owned(), Stopwatch::start());
                let mut reply = format!("moves of {document}, as revision {revision}");
                if moves_differently {
                    reply.push_str("; the next step moves by its new numbers");
                    self.moving.retain(|moving| moving.document != document);
                    self.moving.push(Moving {
                        document: document.clone(),
                        set,
                        tick,
                        stopwatch: Stopwatch::start(),
                    });
                }
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
            if combat.start_fight_from(&cooked.document, set) {
                tracing::info!(
                    target: LOG_TARGET,
                    document = %cooked.document,
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
    pub(crate) fn cue_document(
        &mut self,
        text: &str,
        show: &mut FightShow,
        scene: &mut Scene,
        renderer: &mut Renderer,
    ) -> Result<String, String> {
        let table = CueTable::from_ron(text).map_err(|error| error_chain(&error))?;
        show.set_cue_table(scene, table)?;
        renderer.prewarm_post(scene.effect_variants());
        self.count_edit(document_file_name(CUES_PATH));
        Ok(Document::Cues.name().to_owned())
    }

    /// Takes edited effects, for what a fight's cues and sword trails play. Says why not if the cue table
    /// names one that's gone, keeping what plays.
    pub(crate) fn effects_document(
        &mut self,
        text: &str,
        show: &mut FightShow,
        scene: &mut Scene,
    ) -> Result<String, String> {
        let effects = sapphire_vfx::EffectLibrary::from_ron(text).map_err(|error| error_chain(&error))?;
        show.set_vfx_effects(scene, effects)?;
        self.count_edit(document_file_name(EFFECTS_PATH));
        Ok(Document::Effects.name().to_owned())
    }

    /// Takes an edited camera tuning, for the director and for what fires a fight's cues.
    pub(crate) fn camera_document(
        &mut self,
        text: &str,
        show: &mut FightShow,
        scene: &mut Scene,
    ) -> Result<String, String> {
        let tuning = CameraTuning::from_ron(text).map_err(|error| error_chain(&error))?;
        show.set_camera_tuning(scene, tuning)?;
        self.count_edit(document_file_name(CAMERA_PATH));
        Ok("camera's tuning".to_owned())
    }

    /// Takes the new versions of cooked sets among the asset server's `changes`, and returns what went
    /// wrong. Allocates nothing while none came.
    pub fn follow_packs(
        &mut self,
        changes: &[AssetChange],
        content: &Content,
        mut fight: LiveFight<'_>,
    ) -> Vec<String> {
        let mut errors = Vec::new();
        for change in changes {
            let AssetChange::Reloaded { id, kind: AssetKind::MoveSet, .. } = change else { continue };
            let Some(cooked) = self.cooked.iter().find(|cooked| cooked.handle.id() == *id) else { continue };
            let document = cooked.document.clone();
            let Some(set) = content.move_set(&cooked.handle) else { continue };
            let fight = LiveFight { combat: fight.combat.as_deref_mut(), scene: &mut *fight.scene };
            let taken = set
                .to_owned_set()
                .map_err(|error| error_chain(&error))
                .and_then(|set| self.take_moves(&document, self.newest_of(&document, set), fight));
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
        let named = self.files.iter().filter(|path| path.file_name().is_some_and(|name| name == document));
        let Some(text) = newest_text(named) else { return cooked };
        compile_document(&text, Some(&cooked)).map_or(cooked, |live| live.set)
    }

    /// Once a frame, after its fixed steps and before it draws: says when a step first moves by what an edit
    /// made, and when an attempt first plays it. Allocates nothing while no edit waits.
    pub fn after_steps(&mut self, combat: Option<&Combat>, scene: &Scene) {
        if self.waiting.is_empty() && self.moving.is_empty() {
            return;
        }
        let combat = combat.map(|combat| (combat.simulation(), combat.cast()));
        let show = scene.graph_show().map(|show| (show.simulation(), show.cast()));
        self.moving.retain(|moving| {
            let stepped = [combat, show].into_iter().flatten().find_map(|(sim, cast)| {
                let actor = cast
                    .iter()
                    .map(|actor| sim.party().on_field(*actor))
                    .find(|actor| sim.actor(*actor).is_some_and(|view| view.moves.set == Some(moving.set)))?;
                (sim.tick() > moving.tick).then_some((sim, actor))
            });
            let Some((sim, actor)) = stepped else { return true };
            let movement = sim.movement_of(actor).map(|movement| *movement.authored());
            let velocity =
                sim.actor(actor).map_or(sapphire_math::Vec3::ZERO, |view| view.controller.state.velocity);
            tracing::info!(
                target: LOG_TARGET,
                document = moving.document,
                actor = actor.raw(),
                tick = sim.tick(),
                run_speed = movement.map_or(0.0, |movement| movement.run_speed),
                speed = format_args!("{:.3}", (velocity.x * velocity.x + velocity.z * velocity.z).sqrt()),
                since_ms = format_args!("{:.1}", moving.stopwatch.elapsed().as_secs_f64() * 1e3),
                "the next step moves by the new numbers"
            );
            false
        });
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

/// What a set's document says its actors move by, where that's a set this simulation plays.
fn current_movement(sim: &Simulation, set: Option<MoveSetId>) -> Option<ControllerProfile> {
    sim.move_library().current(set?)?.movement().copied()
}

/// What `cast` is playing, for live link's status.
pub fn attempts(sim: &Simulation, cast: &[ActorId]) -> Vec<MoveAttempt> {
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

/// Whose graph a document with this name is, and its path, or why it reaches nothing here: an edit can
/// replace a fighter's graph, and any other would bind to the wrong character or fail for its clips.
fn graph_of(origin: &str) -> Result<(GraphKey, &'static str), String> {
    let document = document_file_name(origin);
    let graphs = [(GraphKey::CHARACTER, GRAPH_PATH), (ENEMY_GRAPH, ENEMY_GRAPH_PATH)];
    graphs.into_iter().find(|(_, path)| document == document_file_name(path)).ok_or_else(|| {
        let (character, enemy) = (document_file_name(GRAPH_PATH), document_file_name(ENEMY_GRAPH_PATH));
        format!("only `{character}` and `{enemy}` take edits, and a cook brings any other graph")
    })
}

pub(crate) fn kind_of_file(path: &Path) -> Option<Document> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".moves.ron") {
        Some(Document::Moves)
    } else if name.ends_with(".animgraph.ron") {
        Some(Document::Graph)
    } else if name.ends_with(".ui.ron") {
        Some(Document::Ui)
    } else if name == document_file_name(CAMERA_PATH) {
        Some(Document::Camera)
    } else if name == document_file_name(CUES_PATH) {
        Some(Document::Cues)
    } else if name == document_file_name(EFFECTS_PATH) {
        Some(Document::Effects)
    } else if name == document_file_name(BINDINGS_PATH) {
        Some(Document::Bindings)
    } else if name == document_file_name(TARGETING_PATH) {
        Some(Document::Targeting)
    } else if name == document_file_name(MOTION_PATH) {
        Some(Document::CharacterMotion)
    } else if name == document_file_name(HUD_PATH) {
        Some(Document::Hud)
    } else {
        None
    }
}

/// They need no GPU, but compile only in `dev` builds, which CI tests in its GPU job: run them with
/// `cargo test -p sapphire_stage --features dev --lib -- --ignored tuning::`.
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sapphire_render::MeshHandle;

    use super::*;
    use crate::combat::CombatOption;
    use crate::combat::no_bindings;

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
            moving: Vec::new(),
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
            tuning.moves_document(name, text, None, LiveFight { combat: Some(combat), scene: &mut scene })
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

    /// How a character moves, through the app's own path: the reply says the next step reads it, and the
    /// first frame past that step says it did. A number past its limit keeps what's playing, naming both the
    /// section and the number, and saving what's there again says nothing of new numbers.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_saved_movement_reaches_the_next_step_and_a_bad_one_is_refused_by_name() {
        let scratch = Scratch::new("movement");
        let mut combat = recording(&scratch.0);
        let mut scene = scene();
        let mut tuning = tuning();
        let mut take = |tuning: &mut LiveTuning, combat: &mut Combat, text: &str| {
            tuning.moves_document(
                SLICE_FILE,
                text,
                None,
                LiveFight { combat: Some(combat), scene: &mut scene },
            )
        };
        let with =
            |numbers: &str| SLICE.replacen("fighter: (", &format!("fighter: (\nmovement: ({numbers}),"), 1);

        let reply = take(&mut tuning, &mut combat, &with("run_speed: 8.0")).expect("a good document");
        assert!(reply.contains("the next step moves by its new numbers"), "{reply}");
        assert_eq!(tuning.moving.len(), 1);
        tuning.after_steps(Some(&combat), &self::scene());
        assert_eq!(tuning.moving.len(), 1, "no step has read it yet");
        combat.step(&mut no_bindings);
        tuning.after_steps(Some(&combat), &self::scene());
        assert!(tuning.moving.is_empty(), "a step has");
        let player = combat.cast()[0];
        let movement = combat.simulation().movement_of(player).expect("it has one");
        assert_eq!(movement.authored().run_speed, 8.0);

        let reply = take(&mut tuning, &mut combat, &with("run_speed: 8.0")).expect("the same document");
        assert!(reply.contains("still revision 1") && !reply.contains("new numbers"), "{reply}");

        let error = take(&mut tuning, &mut combat, &with("run_speed: 600.0")).expect_err("past its limit");
        assert!(error.contains("`movement`") && error.contains("`run_speed` is 600"), "{error}");
        let movement = combat.simulation().movement_of(player).expect("it has one");
        assert_eq!(movement.authored().run_speed, 8.0, "and what's playing stays");

        // An edit that doesn't touch how anyone moves doesn't claim to.
        let text = with("run_speed: 8.0");
        assert!(text.contains("health: 300,"), "the edit below is one");
        let reply = take(&mut tuning, &mut combat, &text.replacen("health: 300,", "health: 280,", 1));
        let reply = reply.expect("a good document");
        assert!(reply.contains("as revision 2") && !reply.contains("new numbers"), "{reply}");
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
                LiveFight { combat: Some(&mut replay), scene: &mut scene },
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
            .moves_document(
                SLICE_FILE,
                &text,
                None,
                LiveFight { combat: Some(&mut combat), scene: &mut scene },
            )
            .expect_err("nothing to read its motion from");
        assert!(error.contains("only a cook"), "{error}");

        let reply = tuning
            .moves_document(
                SLICE_FILE,
                &text,
                Some(&cooked),
                LiveFight { combat: Some(&mut combat), scene: &mut scene },
            )
            .expect("no cook needed");
        assert!(reply.contains("as revision 1") && reply.contains("as last cooked"), "{reply}");
        let set = combat.plays(SLICE_FILE).expect("it plays this");
        assert!(combat.simulation().move_library().is_current(set, &cook(&edited)));

        // A later edit reads the last, with no cook handed over.
        let reply = tuning
            .moves_document(
                SLICE_FILE,
                &text,
                None,
                LiveFight { combat: Some(&mut combat), scene: &mut scene },
            )
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
                LiveFight { combat: Some(&mut combat), scene: &mut scene },
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
            .moves_document(SLICE_FILE, &slower_text(), None, LiveFight { combat: None, scene: &mut scene })
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

    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_files_name_says_what_kind_of_document_it_is() {
        assert_eq!(kind_of_file(Path::new("x/fighter.moves.ron")), Some(Document::Moves));
        assert_eq!(kind_of_file(Path::new("x/character.animgraph.ron")), Some(Document::Graph));
        assert_eq!(kind_of_file(Path::new("x/menu.ui.ron")), Some(Document::Ui));
        assert_eq!(kind_of_file(Path::new("x/default_camera.ron")), Some(Document::Camera));
        assert_eq!(kind_of_file(Path::new("x/default_cues.ron")), Some(Document::Cues));
        assert_eq!(kind_of_file(Path::new("x/default_effects.ron")), Some(Document::Effects));
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
        for path in [
            GRAPH_PATH,
            CAMERA_PATH,
            CUES_PATH,
            SHOW_MOVES_PATH,
            BINDINGS_PATH,
            TARGETING_PATH,
            MOTION_PATH,
            HUD_PATH,
        ] {
            assert!(sources::repository_file(path).is_file(), "{path}");
        }
        let camera = std::fs::read_to_string(sources::default_camera()).expect("the camera's document");
        assert_eq!(CameraTuning::from_ron(&camera).expect("reads"), CameraTuning::default_tuning());
        assert!(sources::repository_file(CAMERA_PATH).ends_with("default_camera.ron"));
        let cues = std::fs::read_to_string(sources::default_cues()).expect("the cue table's document");
        assert_eq!(CueTable::from_ron(&cues).expect("reads"), CueTable::default_table());
        assert!(sources::repository_file(CUES_PATH).ends_with("default_cues.ron"));
        let effects = std::fs::read_to_string(sources::repository_file(EFFECTS_PATH)).expect("the effects");
        let shipped = sapphire_vfx::EffectLibrary::default_effects();
        assert_eq!(sapphire_vfx::EffectLibrary::from_ron(&effects).expect("reads"), shipped);
    }

    /// A run with nothing live in it but the scenarios' moves.
    const QUIET: TunedRun<'static> = TunedRun {
        camera_director: false,
        cues: false,
        graph_show: false,
        arena_fight: false,
        screen: false,
        hud: false,
        reads_files: true,
        extra_documents: &[],
    };

    /// A real fight's fighters each play a graph, so a run of one watches both and lists them for
    /// `link watch`. A graph show has no enemy, and a graph that's neither's never lands on either.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn a_fight_takes_both_its_fighters_graphs_and_no_other() {
        let graphs = |run: TunedRun<'_>| {
            let sources = LiveTuning::sources_of(&run);
            let found = sources.iter().filter(|source| source.document == Document::Graph);
            found.map(|source| source.path.clone()).collect::<Vec<_>>()
        };
        assert_eq!(graphs(TunedRun { arena_fight: true, ..QUIET }), [GRAPH_PATH, ENEMY_GRAPH_PATH]);
        assert_eq!(graphs(TunedRun { graph_show: true, ..QUIET }), [GRAPH_PATH]);
        assert_eq!(graphs(QUIET), Vec::<String>::new());
        assert_eq!(graph_of("x/character.animgraph.ron").map(|(owner, _)| owner), Ok(GraphKey::CHARACTER));
        assert_eq!(graph_of("warrior.animgraph.ron").map(|(owner, _)| owner), Ok(ENEMY_GRAPH));
        let refused = graph_of("slime.animgraph.ron").expect_err("nobody plays it");
        assert!(refused.contains("warrior.animgraph.ron"), "{refused}");
    }

    /// The cue table has one route: a watched file and a pushed one both go through its `Document` row, and
    /// only a run that fires cues watches it, or lists it for `link watch`.
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn the_cue_table_is_a_document_of_a_run_that_fires_cues() {
        let has = |sources: &[Source], document, path: &str| {
            sources.iter().any(|source| source.document == document && source.path == path)
        };
        let firing = LiveTuning::sources_of(&TunedRun { cues: true, ..QUIET });
        assert!(has(&firing, Document::Cues, CUES_PATH));
        assert!(has(&firing, Document::Effects, EFFECTS_PATH), "its sparks and trails read them");
        assert!(has(&firing, Document::Camera, CAMERA_PATH), "its shakes read the camera's tuning");
        let quiet = LiveTuning::sources_of(&QUIET);
        assert!(!has(&quiet, Document::Cues, CUES_PATH));
        assert!(!has(&quiet, Document::Effects, EFFECTS_PATH));
        assert!(!has(&quiet, Document::Camera, CAMERA_PATH));
    }
}
