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

use game_combat::combat::{Combat, CombatOption, SCENARIOS};
use sapphire_assets::documents::{GameDocuments, MANIFEST_FILE, SHIPPED_DIR, shipped_path};
use sapphire_core::Name;
use sapphire_dialogue::FlagRegistry;
use sapphire_gameplay::replay::{RunStart, find_scenario};
use sapphire_gameplay::{ActorId, DialogueId, EncounterPhase, Simulation, TimelineId};
use sapphire_save::ProgressStore;
use sapphire_serialize::AssetId;

use crate::LOG_TARGET;
use crate::document::{FlowDocument, Then, When};
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
/// been the latest, and the timelines a beat played that have ended. Fixed in size. Its checkpoints and
/// encounters are the flow's zone's, by their places there, so they're marked only while the run is in it.
#[derive(Clone, Copy, Debug, Default)]
struct RunSeen {
    /// By checkpoint, one bit each: past 64, a checkpoint counts as reached only while it's the latest.
    reached: u64,
    /// By place in [`Game::timelines`], one bit each.
    ended: u64,
    /// The timeline a beat played, by place in [`Game::timelines`], until it ends.
    playing: Option<usize>,
    /// By encounter, one bit each, those cleared before the run's first step, as a save's are: they didn't
    /// begin or end in this run, so a beat waiting on one beginning or being cleared doesn't fire for them. A
    /// save written as the last fight's cleared would otherwise win again on every continue.
    cleared_before: u64,
    /// Whether a step has been looked at, which [`Self::cleared_before`] is taken at.
    looked: bool,
}

/// A run's simulation, as the flow reads it.
struct RunView<'a> {
    sim: &'a Simulation,
    /// Whether the run is in the flow's zone. Elsewhere no checkpoint is reached and no encounter starts or
    /// clears, as far as the beats go: they name the flow's zone's, and another zone's places aren't theirs.
    home: bool,
    player: Option<ActorId>,
    seen: &'a RunSeen,
    timelines: &'a [(AssetId, TimelineId)],
}

impl FlowView for RunView<'_> {
    fn encounter_started(&self, encounter: AssetId) -> bool {
        self.phase_this_run(encounter).is_some_and(|phase| phase != EncounterPhase::Waiting)
    }

    fn encounter_cleared(&self, encounter: AssetId) -> bool {
        self.phase_this_run(encounter) == Some(EncounterPhase::Cleared)
    }

    fn timeline_ended(&self, timeline: AssetId) -> bool {
        let place = self.timelines.iter().position(|&(asset, _)| asset == timeline);
        place.is_some_and(|place| place < 64 && self.seen.ended & (1 << place) != 0)
    }

    fn checkpoint_reached(&self, name: &str) -> bool {
        let Some(zone) = self.sim.zone().filter(|_| self.home) else { return false };
        let Some(id) = zone.checkpoint(name) else { return false };
        zone.latest_checkpoint() == Some(id) || (id.raw() < 64 && self.seen.reached & (1 << id.raw()) != 0)
    }

    /// Wherever the run is: a banner shows for another zone's checkpoint too.
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
    /// The encounter's phase, if the run's zone has it and it wasn't cleared before the run began.
    fn phase_this_run(&self, encounter: AssetId) -> Option<EncounterPhase> {
        let zone = self.sim.zone().filter(|_| self.home)?;
        let place = zone.encounters().iter().position(|held| held.id() == encounter)?;
        let before = place < 64 && self.seen.cleared_before & (1 << place) != 0;
        (!before).then(|| zone.encounters()[place].phase())
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
    /// The timelines a beat can play, by their documents' IDs, and where the run's scenario loads each: its
    /// document's `timelines`.
    timelines: Vec<(AssetId, TimelineId)>,
    /// The dialogues a beat can open, by their documents' IDs, and where the run loads each: its document's
    /// `dialogues`, which the show opens on the fight's runner.
    dialogues: Vec<(AssetId, DialogueId)>,
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
    ///
    /// # Errors
    ///
    /// What the flow names that the game hasn't got, which only the game can check: a scenario it doesn't
    /// hold, a timeline or dialogue the scenario doesn't load, a flag the registry doesn't name.
    pub fn new(content: FlowContent, setup: GameSetup) -> Result<Self, String> {
        let (timelines, dialogues) = check(&content)?;
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
        Ok(Self {
            flow: Flow::new(content.document, saved.is_some()),
            registered: content.registered,
            progress: saved,
            kept,
            setup,
            timelines,
            dialogues,
            seen: RunSeen::default(),
            stats: GameStats::default(),
        })
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

    /// Where the run loads the dialogue a beat names by `asset`, if it does.
    #[must_use]
    pub fn dialogue_of(&self, asset: AssetId) -> Option<DialogueId> {
        self.dialogues.iter().find(|&&(held, _)| held == asset).map(|&(_, id)| id)
    }

    /// What the player chose. A new game writes a fresh save at once. What's left for the caller: a skip,
    /// and a quit.
    pub fn command(&mut self, command: FlowCommand) -> Option<FlowAction<'static>> {
        let action = self.flow.command(command)?;
        self.act(action)
    }

    /// What the flow asked of the game, done: a new game's fresh save. The rest is the caller's.
    fn act(&mut self, action: FlowAction<'static>) -> Option<FlowAction<'static>> {
        match action {
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
        let home = in_zone(sim, self.flow.document().zone);
        if !self.seen.looked && home {
            self.seen.looked = true;
            let encounters = sim.zone().map_or(&[][..], |zone| zone.encounters());
            for (place, encounter) in encounters.iter().enumerate().take(64) {
                // Cleared on its first step is cleared before it: a fight takes longer than a step.
                if encounter.phase() == EncounterPhase::Cleared {
                    self.seen.cleared_before |= 1 << place;
                }
            }
        }
        if let Some(zone) = sim.zone().filter(|_| home)
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
            home,
            player: combat.cast().first().copied(),
            seen: &self.seen,
            timelines: &self.timelines,
        };
        let action = self.flow.observe(&view);
        let opened = match action {
            Some(FlowAction::PlayTimeline { timeline, align }) => {
                // `align` is the flow's document's, so the run's marks go by field, not through `self`.
                play_beat(combat, &self.timelines, &mut self.seen, &mut self.stats, timeline, align);
                None
            }
            Some(FlowAction::OpenDialogue(dialogue)) => Some(dialogue),
            _ => None,
        };
        self.save(combat.simulation());
        opened
    }

    /// Writes the save if the run has moved on in the flow's zone: not while it's somewhere else, nor after a
    /// fall, which doesn't change the save.
    fn save(&mut self, sim: &Simulation) {
        if matches!(self.flow.state(), FlowState::Defeat { .. }) {
            return;
        }
        let Some(progress) = &self.progress else { return };
        let Some(zone) = sim.zone().filter(|_| in_zone(sim, progress.zone)) else { return };
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
            // A new game chosen on the title under the wipe, now it's cleared.
            action => {
                self.act(action);
                None
            }
        }
    }

    /// Waits for the save's writes: the game's on its way out.
    pub fn finish(&mut self) {
        if let Some(kept) = &mut self.kept {
            kept.finish();
        }
    }
}

/// A beat's timeline: the player brought to `align` first, then it plays, each a recorded command. One the
/// run can't play, as when a move's cinematic is playing, has ended, so the flow goes on. What takes
/// allocates nothing.
fn play_beat(
    combat: &mut Combat,
    timelines: &[(AssetId, TimelineId)],
    seen: &mut RunSeen,
    stats: &mut GameStats,
    timeline: AssetId,
    align: Option<&str>,
) {
    if let Some(checkpoint) = align
        && let Err(error) = combat.return_to_checkpoint(checkpoint)
    {
        stats.refused += 1;
        tracing::warn!(target: LOG_TARGET, checkpoint, %error, "a beat's checkpoint didn't take");
    }
    // `check` held every beat's timeline to what the scenario loads.
    let Some(place) = timelines.iter().position(|&(asset, _)| asset == timeline) else {
        stats.refused += 1;
        tracing::error!(target: LOG_TARGET, %timeline, "the run's scenario loads no such timeline");
        return;
    };
    match combat.play_timeline(timelines[place].1) {
        Ok(()) => seen.playing = Some(place),
        Err(error) => {
            stats.refused += 1;
            tracing::warn!(target: LOG_TARGET, %timeline, %error, "a beat's timeline didn't play");
            // It ends now, where the flow and a beat waiting on its end look for it.
            if place < 64 {
                seen.ended |= 1 << place;
            }
        }
    }
}

/// Whether `sim`'s run is in `zone`: a run that's taken no trip is in the zone it started in, which no shelf row
/// holds, and one back from a trip is in its zone's row.
fn in_zone(sim: &Simulation, zone: AssetId) -> bool {
    sim.current_zone().and_then(|id| sim.zone_ground(id)).is_none_or(|row| row.asset() == zone)
}

/// What a scenario loads that a beat can name: its timelines and its dialogues, each by its document's ID
/// and where the run holds it.
type Loads = (Vec<(AssetId, TimelineId)>, Vec<(AssetId, DialogueId)>);

/// What a flow names that only the game can say it has: its scenario, and in it each beat's timeline and
/// dialogue; and in the registry, each beat's `unless`. Returns what the scenario loads.
fn check(content: &FlowContent) -> Result<Loads, String> {
    let document = &content.document;
    let scenario = find_scenario(&SCENARIOS, &document.scenario)
        .ok_or_else(|| format!("the flow's scenario `{}` isn't one this build holds", document.scenario))?;
    // A game's run walks the whole zone, so its pool holds everyone it'll meet: one that runs out refuses the
    // last to come, and their encounter clears with nobody in it (M3 G1c).
    let room = scenario.room_for_everyone().map_err(|error| format!("`{}`: {error}", document.scenario))?;
    if let Some(room) = room.filter(|&room| room > scenario.config.max_actors as usize) {
        return Err(format!(
            "`{}` holds {} actors, and its cast and every fighter its encounters bring need {room}: the last \
             would never come",
            document.scenario, scenario.config.max_actors
        ));
    }
    let timelines = scenario.timelines().map_err(|error| format!("`{}`: {error}", document.scenario))?;
    let dialogues = scenario.dialogues().map_err(|error| format!("`{}`: {error}", document.scenario))?;
    for (index, beat) in document.beats.iter().enumerate() {
        let missing = match beat.then {
            Then::Timeline(id) => {
                (!timelines.iter().any(|&(held, _)| held == id)).then_some(("timeline", id))
            }
            Then::Dialogue(id) => {
                (!dialogues.iter().any(|&(held, _)| held == id)).then_some(("dialogue", id))
            }
            Then::Victory => None,
        };
        if let Some((kind, id)) = missing {
            return Err(format!("beats[{index}]: `{}` loads no {kind} {id}", document.scenario));
        }
        if let When::Ended(id) = beat.when
            && !timelines.iter().any(|&(held, _)| held == id)
        {
            return Err(format!("beats[{index}]: `{}` loads no timeline {id}", document.scenario));
        }
        if let Some(flag) = &beat.unless
            && !content.registered.contains(&Name::new(flag))
        {
            return Err(format!("beats[{index}]: the registry names no flag `{flag}`"));
        }
    }
    Ok((timelines, dialogues))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// content/zones/lantern_ward/court_gate.timeline.ron
    const COURT_GATE: AssetId = AssetId::from_text("7c42b73f-d94d-4eb2-b064-2637819640a8");

    /// content/zones/proving_ground/proving_ground.zone.ron
    const GROUND: AssetId = AssetId::from_text("e8c21831-7d98-4a79-b592-912b806cf3e2");

    /// The ward's flow over `ward-ground-trip`, with a beat on a checkpoint the proving ground has by name.
    const TRIP_FLOW: &str = r#"(type: "sapphire::game::FlowDocument", version: 1, data: (
        scenario: "ward-ground-trip",
        zone: "f15b7d04-631d-59b5-8c42-338147fc4e86",
        beats: [(when: Reached("platform"), then: Victory)],
        music: (title: "waiting", victory: "cleared", defeat: "waiting"),
        holds: (banner: 2.0, defeat: 3.0, victory: 5.0),
    ))"#;

    fn ward() -> Game {
        let document =
            FlowDocument::from_ron(include_str!("../../../content/game.flow.ron")).expect("the flow reads");
        let registry =
            FlagRegistry::from_ron(include_str!("../../../content/flags.ron")).expect("the flags read");
        let registered = registry.flags.iter().map(|flag| Name::new(flag)).collect();
        Game::new(FlowContent { document, registered }, GameSetup::default()).expect("the ward's flow stands")
    }

    /// A beat's timeline the run refuses (a fight with no file plays none, and a recording refuses one while
    /// a move's cinematic plays) has ended where the flow looks for it, so its cutscene doesn't wait for
    /// ever, and nothing grows.
    #[test]
    fn a_timeline_the_run_refuses_has_ended() {
        let mut game = ward();
        let mut fight = Combat::new(&CombatOption::Fight { script: None, party: false }).expect("stands");
        let loaded = game.timelines.len();
        play_beat(&mut fight, &game.timelines, &mut game.seen, &mut game.stats, COURT_GATE, None);
        assert_eq!(game.stats().refused, 1);
        let view = RunView {
            sim: fight.simulation(),
            home: true,
            player: None,
            seen: &game.seen,
            timelines: &game.timelines,
        };
        assert!(view.timeline_ended(COURT_GATE), "the flow goes on from it");
        assert_eq!(game.timelines.len(), loaded, "nothing grows");
    }

    /// A step of `combat` with nobody pressing anything, and the game after it.
    fn step(game: &mut Game, combat: &mut Combat) {
        let mut neutral = |_: u64, _: &[ActorId]| sapphire_gameplay::TickActions::NEUTRAL;
        combat.step(&mut neutral);
        game.after_step(combat);
    }

    /// The flow's checkpoints and encounters are its zone's, by their places there. On a trip into the
    /// proving ground, reaching its `platform` marks nothing of the ward's, not the checkpoint at the same
    /// place in the ward's list, and a beat naming `platform` doesn't fire: it isn't the flow's zone's.
    #[test]
    fn a_trip_out_of_the_flows_zone_reaches_nothing_of_the_flows() {
        let document = FlowDocument::from_ron(TRIP_FLOW).expect("reads");
        let content = FlowContent { document, registered: Vec::new() };
        let mut game = Game::new(content, GameSetup::default()).expect("stands");
        let mut combat = Combat::new(&game.first_run()).expect("the run stands");
        game.command(FlowCommand::NewGame);
        step(&mut game, &mut combat);
        let ward = combat.simulation().zone().expect("the ward");
        assert_eq!(ward.checkpoint("platform"), None, "the ward has no platform");
        let ward_names: Vec<String> = (0..ward.checkpoint_count() as u32)
            .map(|raw| {
                ward.checkpoint_name(sapphire_gameplay::CheckpointId::from_raw(raw))
                    .expect("listed")
                    .to_owned()
            })
            .collect();
        combat.enter_zone(GROUND, "", Name::new("player_start").as_u64(), |_| None).expect("on the shelf");
        step(&mut game, &mut combat);
        combat.return_to_checkpoint("platform").expect("the ground has one");
        for _ in 0..5 {
            step(&mut game, &mut combat);
        }
        let ground = combat.simulation().zone().expect("the ground");
        let latest = ground.latest_checkpoint().expect("one reached");
        assert_eq!(ground.checkpoint_name(latest), Some("platform"));
        let shared = &ward_names[latest.raw() as usize];
        assert_eq!(
            game.seen.reached & (1 << latest.raw()),
            0,
            "the ward's {shared}, at the same place, isn't marked"
        );
        assert_eq!(game.flow().state(), FlowState::Playing, "nothing of the ward's was reached");
    }

    /// A new game chosen on the title while the wipe that brought it clears writes its fresh save once it
    /// has, as one chosen after would.
    #[test]
    fn a_new_game_chosen_under_the_titles_wipe_writes_a_fresh_save() {
        let document = FlowDocument::from_ron(TRIP_FLOW).expect("reads");
        let zone = document.zone;
        let content = FlowContent { document, registered: Vec::new() };
        let mut game = Game::new(content, GameSetup::default()).expect("stands");
        let mut combat = Combat::new(&game.first_run()).expect("the run stands");
        game.command(FlowCommand::NewGame);
        step(&mut game, &mut combat);
        let fresh = Progress::new_game(zone);
        assert_ne!(game.progress(), Some(&fresh), "the ward's fights, fought, are saved");
        game.command(FlowCommand::Pause);
        game.command(FlowCommand::ToTitle);
        assert!(game.wipe(true, false).is_some(), "covered: the saved run stands");
        assert_eq!(game.command(FlowCommand::NewGame), None);
        assert_ne!(game.progress(), Some(&fresh), "not yet");
        assert_eq!(game.wipe(true, true), None);
        assert_eq!(game.progress(), Some(&fresh), "a new game's save");
        let restarting =
            FlowState::Restarting { run: Run::New, then: crate::flow::After::Playing, covered: false };
        assert_eq!(game.flow().state(), restarting);
    }
}
