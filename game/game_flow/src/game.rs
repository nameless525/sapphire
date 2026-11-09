//! The flow over a game's runs (M3 G1): which run stands, what a step's beats ask of it, and when it's saved.
//!
//! [`Game`] holds the [`Flow`], the save as the game goes on ([`Progress`]) and the writer that keeps it
//! ([`Kept`]). It makes each run a [`CombatOption::Run`], from the scenario's start for a new game or from the
//! save's place for a continue, and it acts on a run only between steps and only through what a recording
//! records: a timeline played, a return to a checkpoint. So a run's file replays with no flow.
//!
//! **Calls:** [`Game::next_run`] for the run to stand, [`Game::after_step`] after each step that ran,
//! [`Game::command`] for what the player chose, [`Game::update`] once a frame, and [`Game::wipe`] with where
//! the wipe stands, which says when to stand the next run. [`Game::after_step`] allocates nothing on a step
//! that saves nothing.
//!
//! **One zone:** a save holds the flow's zone and a place in it (`Progress` version 1). A run that's gone
//! through an exit into another zone writes nothing until it's back, and a restart stands the run where the
//! save says, in the flow's zone. What stands it on the stage is the caller's.

use std::path::PathBuf;

use game_combat::combat::{Combat, CombatOption};
use sapphire_assets::documents::{GameDocuments, MANIFEST_FILE, SHIPPED_DIR, shipped_path};
use sapphire_core::Name;
use sapphire_dialogue::FlagRegistry;
use sapphire_gameplay::replay::RunStart;
use sapphire_gameplay::{ActorId, EncounterPhase, Simulation, TimelineId};
use sapphire_save::ProgressStore;
use sapphire_serialize::AssetId;

use crate::LOG_TARGET;
use crate::document::FlowDocument;
use crate::flow::{Flow, FlowAction, FlowCommand, FlowState, FlowView, Run};
use crate::kept::{Kept, KeptStats};
use crate::progress::{self, Progress};

/// What a game's content says of its flow, as its manifest names it: the flow, and the flags its registry
/// names.
#[derive(Clone, Debug)]
pub struct FlowContent {
    pub document: FlowDocument,
    /// Every flag the registry names: a save's others are kept, and start nothing.
    pub registered: Vec<Name>,
}

impl FlowContent {
    /// The flow and the flags the manifest a cook writes names, with `read` giving a file's bytes from the
    /// content's root. `None` for a game that names no flow.
    pub fn read(read: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<Option<Self>, String> {
        let manifest_path = format!("{SHIPPED_DIR}/{MANIFEST_FILE}");
        let text = |path: &str| {
            read(path).and_then(|bytes| String::from_utf8(bytes).map_err(|error| error.to_string()))
        };
        let manifest = GameDocuments::from_ron(&text(&manifest_path)?)
            .map_err(|error| format!("{manifest_path}: {error}"))?;
        let Some(flow) = &manifest.flow else { return Ok(None) };
        let path = shipped_path(flow);
        let document = FlowDocument::from_ron(&text(&path)?)
            .map_err(|error| format!("the game's flow, {path}: {error}"))?;
        let registered = match &manifest.flags {
            Some(flags) => {
                let path = shipped_path(flags);
                let registry = FlagRegistry::from_ron(&text(&path)?)
                    .map_err(|error| format!("the game's flags, {path}: {error}"))?;
                registry.flags.iter().map(|flag| Name::new(flag)).collect()
            }
            None => Vec::new(),
        };
        Ok(Some(Self { document, registered }))
    }
}

/// How a game runs.
#[derive(Clone, Default)]
pub struct GameSetup {
    /// Where its save is kept, and what it holds: no store keeps nothing, as a capture's doesn't.
    pub store: Option<ProgressStore<Progress>>,
    /// The script each run plays, for a run nobody's playing.
    pub script: Option<String>,
    /// The directory each run's replay is kept in, `<scenario>-<n>.sreplay`; with none, they're kept nowhere.
    pub record: Option<PathBuf>,
}

impl std::fmt::Debug for GameSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GameSetup")
            .field("saves", &self.store.is_some())
            .field("script", &self.script)
            .field("record", &self.record)
            .finish()
    }
}

/// What [`Game`] has done, for the overlay and the logs (R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GameStats {
    /// Runs stood, the first among them.
    pub runs: u32,
    /// Saves asked for, and what became of them.
    pub saves: KeptStats,
    /// Beats' commands that didn't take: a timeline or a checkpoint the run doesn't have.
    pub refused: u32,
}

/// What a run has done that the flow asks about and the simulation doesn't keep: the checkpoints that have
/// been the latest, and the timelines a beat played that have ended. Fixed in size.
#[derive(Clone, Copy, Debug, Default)]
struct RunSeen {
    /// By checkpoint, one bit each: past 64, a checkpoint counts as reached only while it's the latest.
    reached: u64,
    /// By place in [`Game::timelines`], one bit each.
    ended: u64,
    /// The timeline a beat played, by place in [`Game::timelines`], until it ends.
    playing: Option<usize>,
}

/// A run's simulation, as the flow reads it.
struct RunView<'a> {
    sim: &'a Simulation,
    player: Option<ActorId>,
    seen: &'a RunSeen,
    timelines: &'a [(AssetId, TimelineId)],
}

impl FlowView for RunView<'_> {
    fn encounter_started(&self, encounter: AssetId) -> bool {
        self.phase(encounter).is_some_and(|phase| phase != EncounterPhase::Waiting)
    }

    fn encounter_cleared(&self, encounter: AssetId) -> bool {
        self.phase(encounter) == Some(EncounterPhase::Cleared)
    }

    fn timeline_ended(&self, timeline: AssetId) -> bool {
        let place = self.timelines.iter().position(|&(asset, _)| asset == timeline);
        place.is_some_and(|place| place < 64 && self.seen.ended & (1 << place) != 0)
    }

    fn checkpoint_reached(&self, name: &str) -> bool {
        let Some(zone) = self.sim.zone() else { return false };
        let Some(id) = zone.checkpoint(name) else { return false };
        zone.latest_checkpoint() == Some(id) || (id.raw() < 64 && self.seen.reached & (1 << id.raw()) != 0)
    }

    fn latest_checkpoint(&self) -> Option<&str> {
        let zone = self.sim.zone()?;
        zone.latest_checkpoint().and_then(|id| zone.checkpoint_name(id))
    }

    fn flag(&self, name: &str) -> i32 {
        self.sim.flags().get(Name::new(name))
    }

    fn player_fallen(&self) -> bool {
        let Some(player) = self.player else { return false };
        let on_field = self.sim.party().on_field(player);
        self.sim.actor(on_field).is_some_and(|actor| actor.fighter.is_defeated())
    }
}

impl RunView<'_> {
    fn phase(&self, encounter: AssetId) -> Option<EncounterPhase> {
        let zone = self.sim.zone()?;
        zone.encounters().iter().find(|held| held.id() == encounter).map(|held| held.phase())
    }
}

/// The game's flow over its runs. See the [module docs](self).
pub struct Game {
    flow: Flow,
    /// The registry's flags.
    registered: Vec<Name>,
    /// The save as the game goes on: what a restart stands. `None` until a new game, with no save.
    progress: Option<Progress>,
    kept: Option<Kept<ProgressStore<Progress>>>,
    setup: GameSetup,
    /// The timelines a beat can play, by their documents' IDs, and where the run's scenario loads each.
    /// Nothing yet: a scenario lists its timelines with the Lantern Ward's walk (G1's eighth commit).
    timelines: Vec<(AssetId, TimelineId)>,
    seen: RunSeen,
    stats: GameStats,
}

impl std::fmt::Debug for Game {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Game").field("state", &self.flow.state()).field("stats", &self.stats).finish()
    }
}

impl Game {
    /// The game at its title, over the save its store holds if it's one of the flow's zone, else over a new
    /// game's run.
    #[must_use]
    pub fn new(content: FlowContent, setup: GameSetup) -> Self {
        let zone = content.document.zone;
        let saved = setup.store.as_ref().and_then(progress::load).filter(|saved| {
            let ours = saved.zone == zone;
            if !ours {
                tracing::warn!(
                    target: LOG_TARGET,
                    saved = %saved.zone,
                    flow = %zone,
                    "the save is of another zone than the flow's, so there's nothing to continue"
                );
            }
            ours
        });
        let kept = setup.store.clone().map(Kept::new);
        Self {
            flow: Flow::new(content.document, saved.is_some()),
            registered: content.registered,
            progress: saved,
            kept,
            setup,
            timelines: Vec::new(),
            seen: RunSeen::default(),
            stats: GameStats::default(),
        }
    }

    #[must_use]
    pub fn flow(&self) -> &Flow {
        &self.flow
    }

    #[must_use]
    pub fn progress(&self) -> Option<&Progress> {
        self.progress.as_ref()
    }

    #[must_use]
    pub fn stats(&self) -> GameStats {
        GameStats { saves: self.kept.as_ref().map(Kept::stats).unwrap_or_default(), ..self.stats }
    }

    /// The run the title stands behind it, to stand first.
    #[must_use]
    pub fn first_run(&mut self) -> CombatOption {
        let run = match self.flow.state() {
            FlowState::Title { stood } => stood,
            _ => Run::New,
        };
        self.next_run(run)
    }

    /// `run`, as a fight to stand: the flow's scenario from its start, or from the save's place in its zone,
    /// kept in the record directory if there's one. A run every restart takes anew, so a kept one has the
    /// next number.
    pub fn next_run(&mut self, run: Run) -> CombatOption {
        let document = self.flow.document();
        let start = match (run, &self.progress) {
            (Run::Saved, Some(saved)) => saved.run_start(document.zone, &self.registered).unwrap_or_default(),
            _ => RunStart::default(),
        };
        self.stats.runs += 1;
        self.seen = RunSeen::default();
        let scenario = document.scenario.clone();
        let keep =
            self.setup.record.as_ref().map(|dir| dir.join(format!("{scenario}-{}.sreplay", self.stats.runs)));
        CombatOption::Run { scenario, start, script: self.setup.script.clone(), keep }
    }

    /// What the player chose. A new game writes a fresh save at once. What's left for the caller: a skip,
    /// and a quit.
    pub fn command(&mut self, command: FlowCommand) -> Option<FlowAction<'static>> {
        match self.flow.command(command)? {
            FlowAction::NewProgress => {
                let fresh = Progress::new_game(self.flow.document().zone);
                self.keep(&fresh);
                self.progress = Some(fresh);
                None
            }
            action => Some(action),
        }
    }

    /// After a step of `combat`'s run that ran: the beats, whose timelines and checkpoints it asks of the run
    /// as recorded commands, and the save, written if the run has moved on in the flow's zone. Returns a
    /// dialogue a beat opened, for the caller to show.
    pub fn after_step(&mut self, combat: &mut Combat) -> Option<AssetId> {
        let sim = combat.simulation();
        if let Some(zone) = sim.zone()
            && let Some(latest) = zone.latest_checkpoint()
            && latest.raw() < 64
        {
            self.seen.reached |= 1 << latest.raw();
        }
        if let Some(place) = self.seen.playing {
            let id = self.timelines[place].1;
            if sim.timeline().is_none_or(|playing| playing.id != id) {
                self.seen.playing = None;
                if place < 64 {
                    self.seen.ended |= 1 << place;
                }
            }
        }
        let view = RunView {
            sim,
            player: combat.cast().first().copied(),
            seen: &self.seen,
            timelines: &self.timelines,
        };
        let action = self.flow.observe(&view);
        let opened = match action {
            Some(FlowAction::PlayTimeline { timeline, align }) => {
                let align = align.map(str::to_owned);
                self.play(combat, timeline, align.as_deref());
                None
            }
            Some(FlowAction::OpenDialogue(dialogue)) => Some(dialogue),
            _ => None,
        };
        self.save(combat.simulation());
        opened
    }

    /// A beat's timeline: the player brought to `align` first, then it plays, each a recorded command. One the
    /// run can't play has ended, so the flow goes on.
    fn play(&mut self, combat: &mut Combat, timeline: AssetId, align: Option<&str>) {
        if let Some(checkpoint) = align
            && let Err(error) = combat.return_to_checkpoint(checkpoint)
        {
            self.stats.refused += 1;
            tracing::warn!(target: LOG_TARGET, checkpoint, %error, "a beat's checkpoint didn't take");
        }
        let place = self.timelines.iter().position(|&(asset, _)| asset == timeline);
        let played = match place {
            Some(place) => combat.play_timeline(self.timelines[place].1).map(|()| place),
            None => Err("the run's scenario loads no such timeline".to_owned()),
        };
        match played {
            Ok(place) => self.seen.playing = Some(place),
            Err(error) => {
                self.stats.refused += 1;
                tracing::warn!(target: LOG_TARGET, %timeline, %error, "a beat's timeline didn't play");
                // It ends now, so a beat waiting on its end can fire.
                self.timelines.push((timeline, TimelineId::from_raw(u32::MAX)));
                let place = self.timelines.len() - 1;
                if place < 64 {
                    self.seen.ended |= 1 << place;
                }
            }
        }
    }

    /// Writes the save if the run has moved on in the flow's zone: not while it's somewhere else, nor after a
    /// fall, which doesn't change the save.
    fn save(&mut self, sim: &Simulation) {
        if matches!(self.flow.state(), FlowState::Defeat { .. }) {
            return;
        }
        let Some(progress) = &self.progress else { return };
        let away = sim
            .current_zone()
            .and_then(|id| sim.zone_ground(id))
            .is_some_and(|row| row.asset() != progress.zone);
        let Some(zone) = sim.zone().filter(|_| !away) else { return };
        if !progress.moved_on(zone, sim.flags()) {
            return;
        }
        let Some(next) = progress.after(&zone.zone_start(), sim.flags()) else { return };
        tracing::info!(
            target: LOG_TARGET,
            checkpoint = next.start.checkpoint.as_deref(),
            cleared = next.start.cleared.len(),
            flags = next.flags.flags.len(),
            "saved"
        );
        self.keep(&next);
        self.progress = Some(next);
    }

    fn keep(&mut self, progress: &Progress) {
        if let Some(kept) = &mut self.kept {
            kept.keep(progress);
        }
    }

    /// Once a frame, with the real seconds that passed: the flow's holds, and how the save's writes went.
    pub fn update(&mut self, seconds: f32) {
        self.flow.update(seconds);
        if let Some(kept) = &mut self.kept {
            kept.poll();
        }
    }

    /// Where the wipe stands, as of its last frame: once it covers a restart, the run to stand there.
    pub fn wipe(&mut self, covered: bool, clear: bool) -> Option<CombatOption> {
        match self.flow.wipe(covered, clear)? {
            FlowAction::StandRun(run) => Some(self.next_run(run)),
            _ => None,
        }
    }

    /// Waits for the save's writes: the game's on its way out.
    pub fn finish(&mut self) {
        if let Some(kept) = &mut self.kept {
            kept.finish();
        }
    }
}
