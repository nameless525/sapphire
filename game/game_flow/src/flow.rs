//! The flow machine: where the game is, from the title through a run's beats to a defeat or a win, and what
//! the caller has to do about it.
//!
//! [`Flow`] reads the world only through [`FlowView`], plain questions a caller answers from its simulation,
//! and acts on it only through what it returns, a [`FlowAction`]. It runs between steps, never in one: a
//! timeline it starts, a checkpoint it brings the player to and a dialogue's effects are commands the caller
//! records, so a replay needs no flow and editing a flow moves no checksum.
//!
//! **Calls:** [`Flow::observe`] after every step that ran, [`Flow::command`] for what the player chose,
//! [`Flow::update`] once a frame with the real time that passed, and [`Flow::wipe`] with where the screen's
//! wipe stands. [`Flow::observe`] allocates nothing: what it remembers is fixed in size.

use sapphire_serialize::AssetId;

use crate::document::{FlowDocument, Then, When};

/// The most beats a flow may have: which have fired is one word.
pub const MAX_BEATS: usize = 32;

/// What the flow reads of a run, after a step. A caller answers from its simulation, allocating nothing.
pub trait FlowView {
    /// Whether the encounter, by its document's ID, has begun this run, cleared or not.
    fn encounter_started(&self, encounter: AssetId) -> bool;
    /// Whether the encounter has been cleared.
    fn encounter_cleared(&self, encounter: AssetId) -> bool;
    /// Whether the timeline has played to its end, or been skipped there, this run.
    fn timeline_ended(&self, timeline: AssetId) -> bool;
    /// Whether the checkpoint, by its name, has been reached.
    fn checkpoint_reached(&self, name: &str) -> bool;
    /// The checkpoint the player would come back to, by name.
    fn latest_checkpoint(&self) -> Option<&str>;
    /// A flag's value: 0 when nothing set it.
    fn flag(&self, name: &str) -> i32;
    /// Whether the player has fallen.
    fn player_fallen(&self) -> bool;
}

/// A run the stage can stand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    /// The scenario from its start: a new game.
    New,
    /// The run the save holds.
    Saved,
}

/// Where a restart lands once the wipe clears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    /// The title, with the run that was stood behind it.
    Title,
    /// Play.
    Playing,
}

/// What a pause holds, to go back to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    Playing,
    Cutscene { beat: u8 },
    Dialogue { beat: u8 },
}

/// Where the game is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlowState {
    /// The title, over the run it would open, `stood`, on the gameplay camera. No steps.
    Title { stood: Run },
    /// The wipe covers, the old run ends and `run` stands up under it; then the wipe clears onto `then`. No
    /// steps.
    Restarting { run: Run, then: After, covered: bool },
    /// Steps run, and the beats are checked after each.
    Playing,
    /// A beat's timeline plays. Steps run, held by the timeline.
    Cutscene { beat: u8 },
    /// A beat's dialogue is open. No steps.
    Dialogue { beat: u8 },
    /// No steps.
    Paused { under: Held },
    /// The player fell `since` real seconds ago. Steps run, so the fall plays.
    Defeat { since: f32 },
    /// The run was won `since` real seconds ago. Steps run.
    Victory { since: f32 },
}

/// What the player chose, from a menu, a button or a pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowCommand {
    /// From the title: a new game, which writes a fresh save at once.
    NewGame,
    /// From the title: the run the save holds.
    Continue,
    Pause,
    Resume,
    /// From the pause: back to the title, standing the saved run behind it.
    ToTitle,
    /// Skips the cutscene that's playing.
    Skip,
    /// The dialogue that's open has ended.
    DialogueEnded,
    /// From the title or the pause: quits.
    Leave,
}

/// What the wipe should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WipeAsk {
    Cover,
    Clear,
}

/// A banner the HUD shows for [`FlowHolds::banner`](crate::document::FlowHolds::banner) seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Banner {
    Checkpoint,
    Victory,
    Defeat,
}

/// What the caller has to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowAction<'a> {
    /// Write a fresh save, a new game's.
    NewProgress,
    /// The wipe covers the screen: end the run on the stage and stand `run` there, then let the wipe clear.
    StandRun(Run),
    /// Play a timeline, as a recorded command, bringing the player to `align` first, at the same boundary.
    PlayTimeline { timeline: AssetId, align: Option<&'a str> },
    /// Skip the timeline that's playing, as a recorded command.
    SkipTimeline,
    /// Open a dialogue.
    OpenDialogue(AssetId),
    /// Quit.
    Exit,
}

/// What the flow has done, for the overlay and the logs (R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowStats {
    /// Runs stood under the wipe.
    pub restarts: u32,
    /// Beats fired, over every run.
    pub beats_fired: u32,
    /// Commands that meant nothing where the flow was: a Continue with no save, a Skip outside a cutscene.
    pub commands_ignored: u32,
}

/// The game's flow over its document. See the module docs.
#[derive(Debug)]
pub struct Flow {
    document: FlowDocument,
    state: FlowState,
    /// Whether there's a save to continue.
    saved: bool,
    /// The beats that have fired this run, one bit each.
    fired: u32,
    /// The latest checkpoint's name, hashed, so a new one shows a banner; `None` until the run's first
    /// observation, which only learns it.
    checkpoint: Option<Option<u64>>,
    banner: Option<(Banner, f32)>,
    stats: FlowStats,
}

impl Flow {
    /// The title, over the saved run if there's one, else a new game's. `document` must have passed
    /// [`FlowDocument::check`].
    #[must_use]
    pub fn new(document: FlowDocument, saved: bool) -> Self {
        debug_assert!(document.check().is_ok(), "an unchecked flow");
        let stood = if saved { Run::Saved } else { Run::New };
        Self {
            document,
            state: FlowState::Title { stood },
            saved,
            fired: 0,
            checkpoint: None,
            banner: None,
            stats: FlowStats::default(),
        }
    }

    #[must_use]
    pub fn document(&self) -> &FlowDocument {
        &self.document
    }

    #[must_use]
    pub fn state(&self) -> FlowState {
        self.state
    }

    #[must_use]
    pub fn stats(&self) -> FlowStats {
        self.stats
    }

    /// Whether there's a save to continue: the title's Continue.
    #[must_use]
    pub fn can_continue(&self) -> bool {
        self.saved
    }

    /// Whether the simulation steps in this state.
    #[must_use]
    pub fn runs_steps(&self) -> bool {
        matches!(
            self.state,
            FlowState::Playing
                | FlowState::Cutscene { .. }
                | FlowState::Defeat { .. }
                | FlowState::Victory { .. }
        )
    }

    /// Whether the HUD offers to skip.
    #[must_use]
    pub fn can_skip(&self) -> bool {
        matches!(self.state, FlowState::Cutscene { .. })
    }

    /// The banner showing, if one is.
    #[must_use]
    pub fn banner(&self) -> Option<Banner> {
        self.banner.map(|(banner, _)| banner)
    }

    /// The score's phase the flow asks for, where it has a say: the title, a defeat, a win.
    #[must_use]
    pub fn music_phase(&self) -> Option<&str> {
        let music = &self.document.music;
        match self.state {
            FlowState::Title { .. } => Some(&music.title),
            FlowState::Defeat { .. } => Some(&music.defeat),
            FlowState::Victory { .. } => Some(&music.victory),
            _ => None,
        }
    }

    /// What the wipe should be doing: covering for a restart, then clearing once the run stands.
    #[must_use]
    pub fn wipe_ask(&self) -> Option<WipeAsk> {
        match self.state {
            FlowState::Restarting { covered: false, .. } => Some(WipeAsk::Cover),
            FlowState::Restarting { covered: true, .. } => Some(WipeAsk::Clear),
            _ => None,
        }
    }

    /// After a step that ran: the beats, a checkpoint's banner, a fall, a cutscene's end. Allocates nothing.
    pub fn observe(&mut self, view: &impl FlowView) -> Option<FlowAction<'_>> {
        if let FlowState::Cutscene { beat } = self.state {
            match self.document.beats[usize::from(beat)].then {
                Then::Timeline(timeline) if view.timeline_ended(timeline) => self.state = FlowState::Playing,
                _ => return None,
            }
        }
        if self.state != FlowState::Playing {
            return None;
        }
        let checkpoint = view.latest_checkpoint().map(name_hash);
        if self.checkpoint.is_some_and(|seen| seen != checkpoint) && checkpoint.is_some() {
            self.show(Banner::Checkpoint);
        }
        self.checkpoint = Some(checkpoint);
        if view.player_fallen() {
            self.state = FlowState::Defeat { since: 0.0 };
            self.show(Banner::Defeat);
            return None;
        }
        let index = self.document.beats.iter().enumerate().position(|(index, beat)| {
            self.fired & (1 << index) == 0
                && beat.unless.as_deref().is_none_or(|flag| view.flag(flag) == 0)
                && match &beat.when {
                    When::Started(encounter) => view.encounter_started(*encounter),
                    When::Cleared(encounter) => view.encounter_cleared(*encounter),
                    When::Ended(timeline) => view.timeline_ended(*timeline),
                    When::Reached(checkpoint) => view.checkpoint_reached(checkpoint),
                }
        })?;
        self.fired |= 1 << index;
        self.stats.beats_fired += 1;
        let beat_index = u8::try_from(index).expect("a checked flow has at most MAX_BEATS beats");
        let beat = &self.document.beats[index];
        match beat.then {
            Then::Timeline(timeline) => {
                self.state = FlowState::Cutscene { beat: beat_index };
                Some(FlowAction::PlayTimeline { timeline, align: beat.align.as_deref() })
            }
            Then::Dialogue(dialogue) => {
                self.state = FlowState::Dialogue { beat: beat_index };
                Some(FlowAction::OpenDialogue(dialogue))
            }
            Then::Victory => {
                self.state = FlowState::Victory { since: 0.0 };
                self.banner = Some((Banner::Victory, self.document.holds.banner));
                None
            }
        }
    }

    /// What the player chose. A command that means nothing where the flow is does nothing, and is counted.
    pub fn command(&mut self, command: FlowCommand) -> Option<FlowAction<'static>> {
        let held = match self.state {
            FlowState::Playing => Some(Held::Playing),
            FlowState::Cutscene { beat } => Some(Held::Cutscene { beat }),
            FlowState::Dialogue { beat } => Some(Held::Dialogue { beat }),
            _ => None,
        };
        if command == FlowCommand::Pause
            && let Some(under) = held
        {
            self.state = FlowState::Paused { under };
            return None;
        }
        match (self.state, command) {
            (FlowState::Title { stood }, FlowCommand::NewGame) => {
                self.saved = true;
                self.state = if stood == Run::New {
                    self.begin_run();
                    FlowState::Playing
                } else {
                    FlowState::Restarting { run: Run::New, then: After::Playing, covered: false }
                };
                Some(FlowAction::NewProgress)
            }
            (FlowState::Title { stood }, FlowCommand::Continue) if self.saved => {
                if stood == Run::Saved {
                    self.begin_run();
                    self.state = FlowState::Playing;
                } else {
                    self.state =
                        FlowState::Restarting { run: Run::Saved, then: After::Playing, covered: false };
                }
                None
            }
            (FlowState::Paused { under }, FlowCommand::Resume) => {
                self.state = match under {
                    Held::Playing => FlowState::Playing,
                    Held::Cutscene { beat } => FlowState::Cutscene { beat },
                    Held::Dialogue { beat } => FlowState::Dialogue { beat },
                };
                None
            }
            (FlowState::Paused { .. }, FlowCommand::ToTitle) => {
                self.state = FlowState::Restarting { run: Run::Saved, then: After::Title, covered: false };
                None
            }
            (FlowState::Cutscene { .. }, FlowCommand::Skip) => Some(FlowAction::SkipTimeline),
            (FlowState::Dialogue { .. }, FlowCommand::DialogueEnded) => {
                self.state = FlowState::Playing;
                None
            }
            (FlowState::Title { .. } | FlowState::Paused { .. }, FlowCommand::Leave) => {
                Some(FlowAction::Exit)
            }
            _ => {
                self.stats.commands_ignored += 1;
                None
            }
        }
    }

    /// Once a frame, with the real seconds that passed: holds run out, a defeat restarts at the save and a
    /// win goes back to the title.
    pub fn update(&mut self, seconds: f32) {
        if let Some((_, left)) = &mut self.banner {
            *left -= seconds;
            if *left <= 0.0 {
                self.banner = None;
            }
        }
        let holds = self.document.holds;
        match &mut self.state {
            FlowState::Defeat { since } => {
                *since += seconds;
                if *since >= holds.defeat {
                    self.state =
                        FlowState::Restarting { run: Run::Saved, then: After::Playing, covered: false };
                }
            }
            FlowState::Victory { since } => {
                *since += seconds;
                if *since >= holds.victory {
                    self.state =
                        FlowState::Restarting { run: Run::Saved, then: After::Title, covered: false };
                }
            }
            _ => {}
        }
    }

    /// Where the wipe stands: `covered` when it hides the whole screen, `clear` when it's gone. Once it
    /// covers, the run to stand is returned, and the caller stands it before letting the wipe clear.
    pub fn wipe(&mut self, covered: bool, clear: bool) -> Option<FlowAction<'static>> {
        let FlowState::Restarting { run, then, covered: was_covered } = self.state else { return None };
        if !was_covered && covered {
            self.state = FlowState::Restarting { run, then, covered: true };
            self.begin_run();
            self.stats.restarts += 1;
            return Some(FlowAction::StandRun(run));
        }
        if was_covered && clear {
            self.state = match then {
                After::Title => FlowState::Title { stood: run },
                After::Playing => FlowState::Playing,
            };
        }
        None
    }

    /// A run starts over: no beat has fired, and its first observation only learns the checkpoint.
    fn begin_run(&mut self) {
        self.fired = 0;
        self.checkpoint = None;
        self.banner = None;
    }

    fn show(&mut self, banner: Banner) {
        self.banner = Some((banner, self.document.holds.banner));
    }
}

/// FNV-1a: a checkpoint's name in a word, to tell a new one from the last with nothing kept on the heap.
fn name_hash(name: &str) -> u64 {
    name.bytes()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FLOW: &str = include_str!("../test_content/test.flow.ron");

    const GATE_FIGHT: &str = "00000000-0000-0000-0000-0000000000e1";
    const LAST_FIGHT: &str = "00000000-0000-0000-0000-0000000000e2";
    const GATE_SCENE: &str = "00000000-0000-0000-0000-0000000000a1";
    const GATE_TALK: &str = "00000000-0000-0000-0000-0000000000d1";
    const TERRACE_TALK: &str = "00000000-0000-0000-0000-0000000000d2";

    fn id(text: &str) -> AssetId {
        text.parse().expect("an ID")
    }

    /// A run as plain facts.
    #[derive(Default)]
    struct Seen {
        started: Vec<AssetId>,
        cleared: Vec<AssetId>,
        ended: Vec<AssetId>,
        reached: Vec<&'static str>,
        flags: Vec<(&'static str, i32)>,
        fallen: bool,
    }

    impl FlowView for Seen {
        fn encounter_started(&self, encounter: AssetId) -> bool {
            self.started.contains(&encounter)
        }
        fn encounter_cleared(&self, encounter: AssetId) -> bool {
            self.cleared.contains(&encounter)
        }
        fn timeline_ended(&self, timeline: AssetId) -> bool {
            self.ended.contains(&timeline)
        }
        fn checkpoint_reached(&self, name: &str) -> bool {
            self.reached.contains(&name)
        }
        fn latest_checkpoint(&self) -> Option<&str> {
            self.reached.last().copied()
        }
        fn flag(&self, name: &str) -> i32 {
            self.flags.iter().find(|(flag, _)| *flag == name).map_or(0, |(_, value)| *value)
        }
        fn player_fallen(&self) -> bool {
            self.fallen
        }
    }

    fn flow(saved: bool) -> Flow {
        Flow::new(FlowDocument::from_ron(TEST_FLOW).expect("reads"), saved)
    }

    /// Plays from a title with no save.
    fn playing() -> Flow {
        let mut flow = flow(false);
        assert_eq!(flow.command(FlowCommand::NewGame), Some(FlowAction::NewProgress));
        assert_eq!(flow.state(), FlowState::Playing);
        flow
    }

    /// Covers, stands the run and clears, as a wipe would over a few frames.
    fn wipe_through(flow: &mut Flow) -> Run {
        assert_eq!(flow.wipe_ask(), Some(WipeAsk::Cover));
        assert_eq!(flow.wipe(false, false), None, "still covering");
        let Some(FlowAction::StandRun(run)) = flow.wipe(true, false) else { panic!("covered: stand a run") };
        assert_eq!(flow.wipe_ask(), Some(WipeAsk::Clear));
        assert_eq!(flow.wipe(true, true), None);
        run
    }

    #[test]
    fn with_no_save_a_new_game_plays_the_run_behind_the_title_at_once() {
        let mut flow = flow(false);
        assert_eq!(flow.state(), FlowState::Title { stood: Run::New });
        assert!(!flow.can_continue() && !flow.runs_steps());
        assert_eq!(flow.music_phase(), Some("waiting"));
        assert_eq!(flow.command(FlowCommand::Continue), None, "nothing to continue");
        assert_eq!(flow.stats().commands_ignored, 1);
        assert_eq!(flow.command(FlowCommand::NewGame), Some(FlowAction::NewProgress));
        assert_eq!(flow.state(), FlowState::Playing);
        assert!(flow.can_continue() && flow.runs_steps());
        assert_eq!((flow.wipe_ask(), flow.music_phase()), (None, None));
    }

    /// Over a save, the title stands the saved run: Continue plays it, and a new game restarts under the wipe.
    #[test]
    fn over_a_save_continue_plays_on_and_a_new_game_restarts_under_the_wipe() {
        let mut continued = flow(true);
        assert_eq!(continued.state(), FlowState::Title { stood: Run::Saved });
        assert_eq!(continued.command(FlowCommand::Continue), None);
        assert_eq!(continued.state(), FlowState::Playing);

        let mut fresh = flow(true);
        assert_eq!(fresh.command(FlowCommand::NewGame), Some(FlowAction::NewProgress));
        assert!(!fresh.runs_steps(), "nothing steps under the wipe");
        assert_eq!(wipe_through(&mut fresh), Run::New);
        assert_eq!(fresh.state(), FlowState::Playing);
        assert_eq!(fresh.stats().restarts, 1);
    }

    /// The gate's beats in order: its fight starts the cutscene, aligned; a skip is a command; its end opens
    /// the conversation in the same observation; and none fires twice in a run.
    #[test]
    fn beats_fire_in_order_and_once_a_run() {
        let mut flow = playing();
        let mut seen = Seen::default();
        assert_eq!(flow.observe(&seen), None);
        seen.started.push(id(GATE_FIGHT));
        let played = flow.observe(&seen);
        assert_eq!(played, Some(FlowAction::PlayTimeline { timeline: id(GATE_SCENE), align: Some("gate") }));
        assert!(flow.can_skip() && flow.runs_steps());
        assert_eq!(flow.observe(&seen), None, "held by the timeline");
        assert_eq!(flow.command(FlowCommand::Skip), Some(FlowAction::SkipTimeline));
        seen.ended.push(id(GATE_SCENE));
        assert_eq!(flow.observe(&seen), Some(FlowAction::OpenDialogue(id(GATE_TALK))));
        assert_eq!(flow.state(), FlowState::Dialogue { beat: 1 });
        assert!(!flow.runs_steps());
        assert_eq!(flow.command(FlowCommand::DialogueEnded), None);
        assert_eq!(flow.observe(&seen), None, "each fired once");
        assert_eq!(flow.stats().beats_fired, 2);
        assert_eq!(flow.command(FlowCommand::Skip), None, "no cutscene to skip");
    }

    /// A flag the gate's conversation set keeps its beats from firing in a later run.
    #[test]
    fn a_set_flag_keeps_its_beats_from_firing() {
        let mut flow = playing();
        let seen =
            Seen { started: vec![id(GATE_FIGHT)], flags: vec![("gate_answered", 2)], ..Seen::default() };
        assert_eq!(flow.observe(&seen), None);
        assert_eq!(flow.state(), FlowState::Playing);
    }

    /// The first observation of a run learns its checkpoint and shows nothing: a continued run starts at one.
    /// A new one shows the banner for its hold, and a beat may wait on it.
    #[test]
    fn a_new_checkpoint_shows_its_banner_for_its_hold() {
        let mut flow = playing();
        let mut seen = Seen { reached: vec!["gate"], ..Seen::default() };
        assert_eq!(flow.observe(&seen), None);
        assert_eq!(flow.banner(), None, "where the run started");
        seen.reached.push("terrace");
        assert_eq!(flow.observe(&seen), Some(FlowAction::OpenDialogue(id(TERRACE_TALK))));
        assert_eq!(flow.banner(), Some(Banner::Checkpoint));
        flow.update(1.5);
        assert_eq!(flow.banner(), Some(Banner::Checkpoint));
        flow.update(0.5);
        assert_eq!(flow.banner(), None);
    }

    /// A fall plays, holds, and restarts at the save, where every beat can fire again.
    #[test]
    fn a_defeat_holds_then_restarts_at_the_save() {
        let mut flow = playing();
        let mut seen = Seen { started: vec![id(GATE_FIGHT)], ..Seen::default() };
        assert!(matches!(flow.observe(&seen), Some(FlowAction::PlayTimeline { .. })));
        seen.ended.push(id(GATE_SCENE));
        assert!(matches!(flow.observe(&seen), Some(FlowAction::OpenDialogue(_))));
        flow.command(FlowCommand::DialogueEnded);
        seen.fallen = true;
        assert_eq!(flow.observe(&seen), None);
        assert_eq!(flow.state(), FlowState::Defeat { since: 0.0 });
        assert!(flow.runs_steps(), "the fall plays");
        assert_eq!((flow.banner(), flow.music_phase()), (Some(Banner::Defeat), Some("waiting")));
        flow.update(2.9);
        assert!(matches!(flow.state(), FlowState::Defeat { .. }));
        flow.update(0.2);
        assert_eq!(
            flow.state(),
            FlowState::Restarting { run: Run::Saved, then: After::Playing, covered: false }
        );
        assert_eq!(wipe_through(&mut flow), Run::Saved);
        assert_eq!(flow.state(), FlowState::Playing);
        let again = Seen { started: vec![id(GATE_FIGHT)], ..Seen::default() };
        assert!(matches!(flow.observe(&again), Some(FlowAction::PlayTimeline { .. })), "a new run");
    }

    /// The win holds, then goes to the title over the saved run, which Continue plays with no wipe.
    #[test]
    fn a_win_holds_then_goes_to_the_title() {
        let mut flow = playing();
        let seen = Seen { cleared: vec![id(LAST_FIGHT)], ..Seen::default() };
        assert_eq!(flow.observe(&seen), None);
        assert_eq!(flow.state(), FlowState::Victory { since: 0.0 });
        assert_eq!((flow.banner(), flow.music_phase()), (Some(Banner::Victory), Some("cleared")));
        flow.update(5.0);
        assert_eq!(wipe_through(&mut flow), Run::Saved);
        assert_eq!(flow.state(), FlowState::Title { stood: Run::Saved });
        flow.command(FlowCommand::Continue);
        assert_eq!(flow.state(), FlowState::Playing);
    }

    /// A pause holds whatever it paused, a cutscene or a conversation too, and goes back to it; from it the
    /// title restarts at the save, and Leave quits.
    #[test]
    fn a_pause_holds_what_it_paused() {
        let mut flow = playing();
        let mut seen = Seen { started: vec![id(GATE_FIGHT)], ..Seen::default() };
        flow.observe(&seen);
        assert_eq!(flow.command(FlowCommand::Pause), None);
        assert_eq!(flow.state(), FlowState::Paused { under: Held::Cutscene { beat: 0 } });
        assert!(!flow.runs_steps());
        assert_eq!(flow.command(FlowCommand::Skip), None, "a paused cutscene isn't skipped");
        flow.command(FlowCommand::Resume);
        assert_eq!(flow.state(), FlowState::Cutscene { beat: 0 });
        seen.ended.push(id(GATE_SCENE));
        flow.observe(&seen);
        flow.command(FlowCommand::Pause);
        assert_eq!(flow.state(), FlowState::Paused { under: Held::Dialogue { beat: 1 } });
        assert_eq!(flow.command(FlowCommand::Leave), Some(FlowAction::Exit));
        flow.command(FlowCommand::ToTitle);
        assert_eq!(
            flow.state(),
            FlowState::Restarting { run: Run::Saved, then: After::Title, covered: false }
        );
        assert_eq!(flow.command(FlowCommand::Pause), None);
        assert!(matches!(flow.state(), FlowState::Restarting { .. }), "nothing pauses a restart");
        assert_eq!(flow.observe(&seen), None, "nor does a restart observe");
    }

    /// The title offers what there is to continue, and the HUD shows a checkpoint's banner while it holds and
    /// a skip while a cutscene plays.
    #[test]
    fn the_title_and_the_hud_show_what_the_flow_has() {
        use game_ui::menus::{HudModel, TitleModel};

        use crate::menus::{show_hud, show_title};

        let mut title = TitleModel::default();
        show_title(&flow(true), &mut title);
        assert!(title.can_continue);
        show_title(&flow(false), &mut title);
        assert!(!title.can_continue);

        let mut flow = playing();
        let mut hud = HudModel { banner: "hud-banner-defeat".to_owned(), ..HudModel::default() };
        let mut seen = Seen { reached: vec!["gate"], ..Seen::default() };
        flow.observe(&seen);
        show_hud(&flow, &mut hud);
        assert!(!hud.has_banner && !hud.can_skip);
        seen.reached.push("terrace");
        flow.observe(&seen);
        show_hud(&flow, &mut hud);
        assert!(hud.has_banner && !hud.can_skip);
        assert_eq!(hud.banner, "hud-banner-checkpoint");
        flow.update(2.0);
        show_hud(&flow, &mut hud);
        assert!(!hud.has_banner, "its hold is over");

        let mut flow = playing();
        flow.observe(&Seen { started: vec![id(GATE_FIGHT)], ..Seen::default() });
        show_hud(&flow, &mut hud);
        assert!(hud.can_skip, "the gate's cutscene plays");
    }
}
