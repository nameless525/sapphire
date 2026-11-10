//! The dialogue panel on a real conversation (M3 Q2b): the Lantern Ward's guide, opened by the court's gate
//! trigger as the ward's walk goes through it, on the real documents and tables, walked by a pad, a keyboard
//! and a finger through `MenuControls`. The game opens the guide from its flow after the gate's cutscene
//! (M3 G1), so its document names no trigger: here it's given the gate's, as Q2b's had it. A choice's effect is recorded as the app records commands, and the
//! recording replays to the same checksums; no step allocates. No GPU and no cooked content.

mod support;

use std::sync::Arc;

use game_combat::dialogue::{DialogueEnded, DialogueRunner};
use game_ui::dialogue::DialoguePanel;
use game_ui::menus::{DialogueModel, MenuScreen};
use sapphire_core::Name;
use sapphire_dialogue::Dialogue;
use sapphire_gameplay::replay::{
    LiveMoves, Player, Recording, ReplayCommand, RunStart, SCENARIOS, Scenario, ScenarioError, ScenarioSetup,
    Script, Trail, find_scenario, find_script,
};
use sapphire_gameplay::{ActorId, DialogueId, Simulation};
use sapphire_script::scenarios::ward_guide;
use sapphire_script::{Limits, ScriptHost, attach};
use support::assets;
use support::walk::{Device, Walk};

/// The guide's dialogue, the only one the scenario loads.
const GUIDE: DialogueId = DialogueId::from_raw(0);
/// What the panel's checked at.
const SIZE: (u32, u32) = (1280, 720);
/// Ticks walked on after a conversation, so its effects reach a few checksums.
const AFTER: u32 = 30;

/// `lantern-walk`, with the guide's dialogue loaded and a VM to ask its conditions: what a game in the ward
/// would hold.
fn ward_with_the_guide(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let cast = lantern().set_up(sim)?;
    sim.add_dialogue(ward_guide().simulation).expect("the guide's effects are valid");
    attach(sim, ScriptHost::new(Limits::default()).expect("a VM starts"));
    Ok(cast)
}

/// The guide, opened by the court's gate trigger rather than by a game's flow.
fn guide_on_the_gate() -> Arc<Dialogue> {
    let mut guide = ward_guide();
    guide.presentation.opens_on = Some(Name::new("court_entered").as_u64());
    Arc::new(guide)
}

fn lantern() -> &'static Scenario {
    find_scenario(&SCENARIOS, "lantern-walk").expect("listed")
}

fn scenario() -> Scenario {
    Scenario { name: "ward-guide", setup: ScenarioSetup::Code(ward_with_the_guide), ..*lantern() }
}

/// A conversation in the ward: the walk, the runner, the panel and the menus, a frame at a time.
struct Visit {
    recording: Recording,
    runner: DialogueRunner,
    panel: DialoguePanel,
    walk: Walk,
    actions: &'static Script,
}

impl Visit {
    fn new(locale: &str, device: Device, flags: &[(Name, i32)]) -> Self {
        let scenario = scenario();
        let recording = Recording::starting_from(
            &scenario,
            scenario.config,
            &LiveMoves::default(),
            &RunStart { zone: None, flags: flags.to_vec() },
            |_, _, _| Ok(()),
        )
        .expect("starts");
        let runner = DialogueRunner::new(vec![guide_on_the_gate()]);
        let mut walk = Walk::new(assets(), locale, false, SIZE, device);
        walk.menus.open_under(MenuScreen::HUD);
        walk.settle();
        let actions = find_script("walk-the-ward").expect("listed");
        Self { recording, runner, panel: DialoguePanel::default(), walk, actions }
    }

    /// Steps the ward's walk, as a game's frame does, until the court's trigger opens the guide: the tick it
    /// opened before.
    fn walk_in(&mut self) -> u32 {
        for _ in 0..4000 {
            self.recording.step(&mut self.actions).expect("steps");
            if let Some(opened) = self.runner.follow(&mut self.recording).expect("opens") {
                assert_eq!(opened, GUIDE);
                self.show();
                return self.recording.recorded_ticks();
            }
        }
        panic!("the walk never reached the court's gate");
    }

    /// Walks on, after the conversation, as a game's frames would.
    fn walk_on(&mut self, ticks: u32) {
        for _ in 0..ticks {
            assert!(!self.runner.is_open(), "the fight holds while a conversation's open");
            self.recording.step(&mut self.actions).expect("steps");
            self.runner.follow(&mut self.recording).expect("nothing else opens");
        }
    }

    /// What a frame does after the menus' input: their commands to the runner, then the panel follows it.
    fn show(&mut self) {
        for command in std::mem::take(&mut self.walk.commands) {
            self.panel.hear(command, &mut self.runner, &mut self.recording);
        }
        self.panel.follow(&mut self.walk.menus, &self.runner);
        self.walk.settle();
        if self.panel.is_up() {
            self.walk.check(MenuScreen::DIALOGUE, false);
            assert!(self.walk.wrong.is_empty(), "{:?}", self.walk.wrong);
        }
    }

    fn choose(&mut self, node: &str) {
        self.walk.choose(MenuScreen::DIALOGUE, node);
        self.show();
    }

    fn model(&mut self) -> &DialogueModel {
        self.walk.menus.model_mut::<DialogueModel>(MenuScreen::DIALOGUE).expect("the panel's up")
    }

    /// The line the panel shows, and the answers, in their slots.
    fn shown(&mut self) -> (String, Vec<String>) {
        let model = self.model();
        let answers = [&model.first, &model.second, &model.third, &model.fourth]
            .into_iter()
            .filter(|choice| choice.shown)
            .map(|choice| choice.line.clone())
            .collect();
        (model.line.clone(), answers)
    }

    fn finish(self) -> (sapphire_gameplay::replay::Replay, Trail) {
        let (replay, played) = self.recording.finish();
        played.allocation_gate().expect("a step allocates nothing, the conversation's included");
        (replay, played.trail)
    }
}

fn checksums(trail: &Trail) -> Vec<u64> {
    (0..=trail.ticks()).map(|tick| trail.at(tick).expect("in the trail")).collect()
}

/// By the keyboard, in English: greeted, asked with the hidden answer hidden, the ward lit, thanked, and the
/// branch the thanks' flag sends on to the farewell. Both effects are recorded where they were made, and
/// replay to the same checksums.
#[test]
fn the_keyboard_lights_the_ward_and_its_choice_replays() {
    let mut visit = Visit::new("en", Device::Keyboard, &[]);
    let opened = visit.walk_in();
    assert_eq!(visit.shown(), ("dialogue-guide-greet".to_owned(), Vec::new()));
    assert!(visit.model().next);
    assert_eq!(visit.model().speaker, "dialogue-guide-name");

    visit.choose("next");
    let (line, answers) = visit.shown();
    assert_eq!(line, "dialogue-guide-ask");
    assert_eq!(answers, ["dialogue-guide-choice-light", "dialogue-guide-choice-leave"], "the ward isn't lit");
    assert!(!visit.model().next, "an answer's wanted");

    visit.choose("first");
    assert_eq!(visit.shown().0, "dialogue-guide-thanks");
    visit.choose("next");
    assert_eq!(visit.shown().0, "dialogue-guide-farewell", "the thanks set met_guide, so the branch goes on");
    visit.choose("next");
    assert!(!visit.walk.menus.is_open(MenuScreen::DIALOGUE), "the panel goes with the conversation");
    assert!(!visit.walk.menus.is_modal());
    assert_eq!(visit.runner.take_ended(), Some(DialogueEnded { dialogue: GUIDE, tick: u64::from(opened) }));
    assert_eq!(visit.runner.take_ended(), None, "once");
    assert_eq!(visit.runner.lines_shown(), 4);

    visit.walk_on(AFTER);
    let flags = visit.recording.simulation().flags();
    assert_eq!((flags.get(Name::new("ward_lit")), flags.get(Name::new("met_guide"))), (1, 1));
    let (replay, trail) = visit.finish();
    let effect = |raw| ReplayCommand::Dialogue(sapphire_gameplay::DialogueEffect::from_raw(raw));
    assert_eq!(
        replay.commands(),
        [(opened, effect(0)), (opened, effect(1))],
        "the choice's, then the thanks'"
    );

    let mut player = Player::new(replay, &[scenario()]).expect("starts");
    player.run_to_end();
    assert_eq!(checksums(&player.finish().trail), checksums(&trail), "what the panel chose replays");

    // The same walk with nothing chosen is the same fight up to the choice, and only then moves.
    let mut away = Visit::new("en", Device::Keyboard, &[]);
    assert_eq!(away.walk_in(), opened);
    away.choose("next");
    away.choose("second");
    assert!(!away.runner.is_open());
    away.walk_on(AFTER);
    let (unchosen, away_trail) = away.finish();
    assert!(unchosen.commands().is_empty(), "leaving sets nothing");
    let first = (0..=trail.ticks()).find(|&tick| trail.at(tick) != away_trail.at(tick));
    assert_eq!(first, Some(opened + 1), "the effect lands in the first checksum after it");
}

/// By a pad, in Japanese, with the ward already lit from a save: the hidden answer shows, and asking again
/// takes the branch back to the greeting, since the guide hasn't thanked anyone yet. Nothing's set.
#[test]
fn a_pad_finds_the_hidden_answer_and_the_branch_back() {
    let lit = [(Name::new("ward_lit"), 1)];
    let mut visit = Visit::new("ja", Device::Pad, &lit);
    visit.walk_in();
    visit.choose("next");
    let (line, answers) = visit.shown();
    assert_eq!(line, "dialogue-guide-ask");
    assert_eq!(
        answers,
        ["dialogue-guide-choice-light", "dialogue-guide-choice-again", "dialogue-guide-choice-leave"],
        "the ward's lit, so asking again shows"
    );
    visit.choose("second");
    assert_eq!(
        visit.shown(),
        ("dialogue-guide-greet".to_owned(), Vec::new()),
        "met_guide is 0: back to the start"
    );
    visit.choose("next");
    visit.choose("third");
    assert!(!visit.runner.is_open() && !visit.walk.menus.is_open(MenuScreen::DIALOGUE));
    visit.walk_on(AFTER);
    let (replay, _) = visit.finish();
    assert!(replay.commands().is_empty());
    assert_eq!(replay.starting_flags(), lit);
}

/// By a finger, in English: every answer's a tap, and a tap on "next" goes on.
#[test]
fn a_finger_taps_through() {
    let mut visit = Visit::new("en", Device::Finger, &[]);
    visit.walk_in();
    visit.choose("next");
    visit.choose("first");
    assert_eq!(visit.shown().0, "dialogue-guide-thanks");
    visit.choose("next");
    visit.choose("next");
    assert!(!visit.runner.is_open());
    assert_eq!(visit.recording.simulation().flags().get(Name::new("ward_lit")), 1);
}

/// A replay opens nothing: it applies what it recorded, between the ticks it recorded them.
#[test]
fn a_replay_opens_no_conversation() {
    let mut visit = Visit::new("en", Device::Keyboard, &[]);
    let opened = visit.walk_in();
    visit.choose("next");
    visit.choose("first");
    visit.choose("next");
    visit.choose("next");
    visit.walk_on(AFTER);
    let (replay, _) = visit.finish();
    let mut player = Player::new(replay, &[scenario()]).expect("starts");
    let mut runner = DialogueRunner::new(vec![guide_on_the_gate()]);
    struct Played<'a>(&'a mut Player);
    impl game_combat::dialogue::DialogueHost for Played<'_> {
        fn simulation(&self) -> &Simulation {
            self.0.simulation()
        }
        fn ask(
            &mut self,
            conditions: &mut sapphire_script::ConditionSet,
            id: sapphire_dialogue::ConditionId,
        ) -> bool {
            sapphire_script::ask(self.0, conditions, id)
        }
        fn apply(&mut self, _: sapphire_gameplay::DialogueEffect) -> Result<(), String> {
            Err("a replay applies what it recorded".to_owned())
        }
        fn takes_dialogue(&self) -> bool {
            false
        }
    }
    while player.step() {
        assert_eq!(runner.follow(&mut Played(&mut player)).expect("nothing to open"), None);
    }
    assert!(player.simulation().tick() > u64::from(opened));
    assert_eq!(player.simulation().flags().get(Name::new("ward_lit")), 1, "the file's effect, applied");
}
