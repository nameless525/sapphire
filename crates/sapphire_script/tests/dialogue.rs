//! Dialogue on a real fight (M3 Q2): conditions asked of the VM between steps move nothing, a choice that
//! sets a flag a script reads moves the fight from its tick and no sooner, and a save's flags start a later
//! run where the first left off.

use sapphire_core::Name;
use sapphire_dialogue::{ConditionId, Dialogue, DialogueWalk, DialogueWorld, EffectId, FlagRegistry};
use sapphire_gameplay::replay::{
    CHOICE_AT, COMMANDS_FORMAT_VERSION, FLAGS_FORMAT_VERSION, LIGHT_THE_WARD, LiveMoves, Player, Recording,
    Replay, ReplayCommand, RunStart, Scenario, ScenarioError, ScenarioSetup, Script, Trail, find_scenario,
    find_script,
};
use sapphire_gameplay::{ActorId, DialogueEffect, DialogueId, FlagValues, Simulation};
use sapphire_save::ProgressStore;
use sapphire_script::scenarios::{DIALOGUE_FLAG, FLAGS, SCRIPTED, ward_guide};
use sapphire_script::{ConditionSet, Limits, ScriptHost, VmError, ask, attach, cook, host_of};

/// The guide's choice that shows once the ward is lit, and the branch that sends a stranger back to the
/// greeting: the conditions in the order the cook meets them.
const WARD_IS_LIT: ConditionId = ConditionId(0);
const NEVER_MET: ConditionId = ConditionId(1);

fn scenario() -> &'static Scenario {
    find_scenario(&SCRIPTED, "dialogue-flag").expect("listed")
}

fn choice_script() -> &'static Script {
    find_script("dialogue-choice").expect("listed")
}

/// `dialogue-choice`'s presses with no choice made.
fn no_choice() -> Script {
    Script { commands: &[], ..*choice_script() }
}

fn conditions(on: &Recording) -> ConditionSet {
    host_of(on.simulation()).bind_conditions(&ward_guide(), "=ward_guide").expect("binds")
}

fn recorded(script: &Script) -> (Replay, Trail) {
    let mut recording = Recording::new(scenario(), scenario().config).expect("starts");
    recording.run_script(script, script.ticks).expect("runs");
    let (replay, played) = recording.finish();
    played.allocation_gate().expect("a step allocates nothing, flags and all");
    (replay, played.trail)
}

fn checksums(trail: &Trail, from: u32) -> Vec<u64> {
    (from..=trail.ticks()).map(|tick| trail.at(tick).expect("in the trail")).collect()
}

#[test]
fn the_guides_dialogue_cooks_against_the_registry_and_binds() {
    let dialogue = ward_guide();
    assert!(dialogue.needs_scripts());
    assert_eq!(dialogue.presentation.conditions, 2);
    let recording = Recording::new(scenario(), scenario().config).expect("starts");
    assert_eq!(conditions(&recording).len(), 2);

    // Source a cooked file says that makes a different number than the graph asks is refused, naming the
    // dialogue, and so is source that doesn't compile or that the scan refuses: a pack's text is compiled
    // here, and its bytes are never run.
    let host = host_of(recording.simulation());
    let mut short = dialogue.clone();
    short.presentation.source =
        "return { names = {}, conditions = { function() return true end } }".to_owned();
    match host.bind_conditions(&short, "=short") {
        Err(VmError::ConditionCount { name, expected: 2, found: 1 }) => assert_eq!(name, "=short"),
        other => panic!("bound {:?}", other.map(|set| set.len())),
    }
    for source in ["return { names = {}, conditions = {", "x = 1 return { names = {}, conditions = {} }"] {
        let mut broken = dialogue.clone();
        broken.presentation.source = source.to_owned();
        match host.bind_conditions(&broken, "=broken") {
            Err(VmError::Cook(_)) => {}
            other => panic!("{source}: bound {:?}", other.map(|set| set.len())),
        }
    }
    let registry = FlagRegistry::from_ron(FLAGS).expect("reads");
    assert!(["ward_lit", "met_guide", "ward_answered"].iter().all(|flag| registry.has(flag)));
}

#[test]
fn a_condition_reads_the_flags_as_they_stand() {
    let mut recording = Recording::new(scenario(), scenario().config).expect("starts");
    let mut set = conditions(&recording);
    assert!(!ask(&mut recording, &mut set, WARD_IS_LIT));
    assert!(ask(&mut recording, &mut set, NEVER_MET));
    recording.apply_dialogue_effect(LIGHT_THE_WARD).expect("loaded");
    assert!(ask(&mut recording, &mut set, WARD_IS_LIT), "at once, between steps");
    assert!(!ask(&mut recording, &mut set, ConditionId(2)), "a number the set doesn't have");
    let stats = host_of(recording.simulation()).stats();
    assert_eq!((stats.conditions_asked, stats.conditions_faulted), (4, 1));
}

/// Ten thousand questions over a recorded fight with a choice in it, every tick, and the fight plays to the
/// same checksum at every tick: reading a dialogue never moves one.
#[test]
fn ten_thousand_questions_move_nothing() {
    let (replay, trail) = recorded(choice_script());
    let tables = replay.tables_hash();
    let mut player = Player::new(replay, &SCRIPTED).expect("starts");
    let mut set = host_of(player.simulation()).bind_conditions(&ward_guide(), "=ward_guide").expect("binds");
    let mut asked = 0_u32;
    loop {
        while asked < 10_000 && asked < (player.simulation().tick() as u32 + 1) * 17 {
            let id = if asked.is_multiple_of(2) { WARD_IS_LIT } else { NEVER_MET };
            ask(&mut player, &mut set, id);
            asked += 1;
        }
        if !player.step() {
            break;
        }
    }
    assert_eq!(asked, 10_000);
    let stats = host_of(player.simulation()).stats();
    assert_eq!((stats.conditions_asked, stats.conditions_faulted), (10_000, 0));
    assert_eq!(player.simulation().tables_hash(), tables);
    let played = player.finish();
    assert_eq!(checksums(&played.trail, 0), checksums(&trail, 0));
}

/// A condition can only read. Whatever would move a checksum is a fault and does nothing, and a condition
/// that faults, fails, runs out of budget or returns anything but `true` doesn't hold.
#[test]
fn a_condition_that_writes_draws_or_fails_is_refused_and_reads_false() {
    let bodies = [
        "sapphire.set_board(0, 7) return true",
        "return sapphire.random() < 2",
        "sapphire.mark(1) return true",
        "sapphire.set_flag(1, 5) return true",
        "sapphire.force_move(2, 1) return true",
        "error('no') return true",
        "while true do end return true",
        "return 1",
        "return sapphire.flag(9) == 0",
    ];
    let mut source = String::from("return { names = { \"ward_lit\" }, conditions = {");
    for body in bodies.iter().chain(&["return sapphire.flag(1) == 0 and sapphire.health(2) > 0"]) {
        source.push_str(&format!(" function() {body} end,"));
    }
    source.push_str(" } }");
    let mut dialogue = ward_guide();
    dialogue.presentation.source = source;
    dialogue.presentation.conditions = bodies.len() as u16 + 1;

    let mut recording = Recording::new(scenario(), scenario().config).expect("starts");
    recording.step(&mut no_choice().actions).expect("steps");
    let (checksum, tables) = (recording.simulation().checksum(), recording.simulation().tables_hash());
    let host = host_of(recording.simulation());
    let mut set = host.bind_conditions(&dialogue, "=writes").expect("binds");
    for (place, body) in bodies.iter().enumerate() {
        assert!(!ask(&mut recording, &mut set, ConditionId(place as u16)), "{body}");
    }
    assert!(ask(&mut recording, &mut set, ConditionId(bodies.len() as u16)), "reading holds");
    let stats = host_of(recording.simulation()).stats();
    // `return 1` ends well, and doesn't hold.
    assert_eq!(stats.conditions_faulted, bodies.len() as u64 - 1);
    assert_eq!((recording.simulation().checksum(), recording.simulation().tables_hash()), (checksum, tables));
    assert!(recording.simulation().flags().is_empty());
    let (replay, _) = recording.finish();
    assert!(replay.commands().is_empty(), "nothing asked was recorded");
}

/// Conditions belong to the VM that bound them. A game that starts a new fight, or attaches a new host, while
/// a dialogue's set is still in hand asks the new VM a function it never loaded: that's a condition that
/// doesn't hold, never a panic.
#[test]
fn a_condition_bound_in_another_vm_reads_false() {
    let mut old = Recording::new(scenario(), scenario().config).expect("starts");
    let mut set = conditions(&old);
    assert!(ask(&mut old, &mut set, NEVER_MET), "asked of its own VM, it holds");
    let mut recording = Recording::new(scenario(), scenario().config).expect("starts");
    let checksum = recording.simulation().checksum();
    assert!(!ask(&mut recording, &mut set, NEVER_MET));
    let stats = host_of(recording.simulation()).stats();
    assert_eq!((stats.conditions_asked, stats.conditions_faulted), (1, 1));
    assert_eq!(recording.simulation().checksum(), checksum);
    assert!(host_of(recording.simulation()).last_error().is_some_and(|error| error.contains("another VM")));
}

/// `dialogue-flag` with the script and no dialogue loaded.
fn no_dialogue(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let skirmish = find_scenario(&sapphire_gameplay::replay::SCENARIOS, "arena-skirmish").expect("listed");
    let cast = skirmish.set_up(sim)?;
    let mut host = ScriptHost::new(Limits::default()).expect("a VM starts");
    host.bind(&cook(DIALOGUE_FLAG).expect("cooks"), "=dialogue-flag").expect("binds");
    host.beat_every(30);
    attach(sim, host);
    Ok(cast)
}

/// With no choice, loading a dialogue moves no checksum (only `tables_hash`, which names it); with the
/// choice, the run is the same through the tick before it and moves from there, and the script answered.
#[test]
fn a_choice_moves_the_fight_from_its_tick_and_no_choice_moves_nothing() {
    let (chose, chose_trail) = recorded(choice_script());
    let (_, plain_trail) = recorded(&no_choice());

    let bare = Scenario { name: "no-dialogue", setup: ScenarioSetup::Code(no_dialogue), ..*scenario() };
    let mut without = Recording::new(&bare, bare.config).expect("starts");
    without.run_script(&no_choice(), no_choice().ticks).expect("runs");
    assert_ne!(without.simulation().tables_hash(), chose.tables_hash(), "the dialogue's effects are tables");
    let (_, without) = without.finish();
    assert_eq!(
        checksums(&without.trail, 0),
        checksums(&plain_trail, 0),
        "a dialogue nobody chose in is nothing"
    );

    let first = (0..=chose_trail.ticks()).find(|&tick| chose_trail.at(tick) != plain_trail.at(tick));
    assert_eq!(first, Some(CHOICE_AT + 1), "the choice comes before tick {CHOICE_AT}'s step");
    assert_eq!(chose.commands(), [(CHOICE_AT, ReplayCommand::Dialogue(LIGHT_THE_WARD))]);

    let mut player = Player::new(chose, &SCRIPTED).expect("starts");
    player.run_to_end();
    let flags = player.simulation().flags();
    assert_eq!(flags.get(Name::new("ward_lit")), 1);
    assert_eq!(flags.get(Name::new("ward_answered")), 1, "the script read the flag and set its own");
    assert_eq!(checksums(&player.finish().trail, 0), checksums(&chose_trail, 0));
}

/// Walks a dialogue on a recording, which applies each effect as a recorded command as the walk meets it.
struct Talk<'a> {
    recording: &'a mut Recording,
    conditions: &'a mut ConditionSet,
    dialogue: DialogueId,
}

impl DialogueWorld for Talk<'_> {
    fn ask(&mut self, condition: ConditionId) -> bool {
        ask(self.recording, self.conditions, condition)
    }

    fn apply(&mut self, effect: EffectId) {
        self.recording
            .apply_dialogue_effect(DialogueEffect { dialogue: self.dialogue, effect })
            .expect("loaded");
    }
}

fn walk_to_the_choice(dialogue: &Dialogue, talk: &mut Talk<'_>) -> DialogueWalk {
    let mut walk = DialogueWalk::begin(dialogue, talk);
    let view = walk.view(dialogue).expect("a line");
    assert_eq!(
        (view.speaker, view.line, view.vo, view.next),
        ("dialogue-guide-name", "dialogue-guide-greet", Some("guide_greet"), true)
    );
    assert!(walk.next(dialogue, talk));
    let view = walk.view(dialogue).expect("a line");
    assert_eq!(view.line, "dialogue-guide-ask");
    let shown: Vec<_> = view.choices.iter().flatten().map(|choice| choice.line).collect();
    assert_eq!(
        shown,
        ["dialogue-guide-choice-light", "dialogue-guide-choice-leave"],
        "the ward isn't lit yet"
    );
    walk
}

/// The proof the plan asks for: a save made mid-quest, loaded, replays to the same checksums as never
/// having stopped. One session walks the guide's dialogue and lights the ward, then saves its flags
/// through the real progress store; the next loads them, and its fight starts from them, held in its
/// replay's header. From the first step on, the two fights are one.
#[test]
fn a_save_made_mid_quest_replays_as_never_having_stopped() {
    let dialogue = ward_guide();
    let mut first = Recording::new(scenario(), scenario().config).expect("starts");
    let mut set = conditions(&first);
    let mut talk = Talk { recording: &mut first, conditions: &mut set, dialogue: DialogueId::from_raw(0) };
    let mut walk = walk_to_the_choice(&dialogue, &mut talk);
    assert!(walk.choose(&dialogue, 0, &mut talk));
    // Lit, thanked (which sets `met_guide`), and the branch reads that and goes on to the farewell.
    assert_eq!(walk.view(&dialogue).expect("a line").line, "dialogue-guide-thanks");
    assert!(walk.next(&dialogue, &mut talk));
    assert_eq!(walk.view(&dialogue).expect("a line").line, "dialogue-guide-farewell");
    assert!(walk.next(&dialogue, &mut talk) && walk.is_over());

    // Saved, through the store a game writes, and loaded back.
    let dir = std::env::temp_dir().join(format!("sapphire-q2-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch folder");
    let store = ProgressStore::<FlagValues>::new(&dir, "progress", |path, bytes| std::fs::write(path, bytes));
    let saved = first.simulation().flags().values(None);
    assert_eq!(saved.flags.len(), 2);
    store.write(&store.snapshot(&saved).expect("serializes")).expect("writes");
    let loaded = store.load().expect("reads").expect("there").value;
    std::fs::remove_dir_all(&dir).expect("cleans up");
    assert_eq!(loaded, saved);
    let registry = FlagRegistry::from_ron(FLAGS).expect("reads");
    let registered: Vec<Name> = registry.flags.iter().map(|flag| Name::intern(flag)).collect();
    let starting = loaded.starting(&registered);
    assert!(starting.ignored.is_empty());

    let mut second = Recording::starting_from(
        scenario(),
        scenario().config,
        &LiveMoves::default(),
        &RunStart { zone: None, flags: starting.flags.clone() },
        |_, _, _| Ok(()),
    )
    .expect("starts");
    let script = no_choice();
    first.run_script(&script, script.ticks).expect("runs");
    second.run_script(&script, script.ticks).expect("runs");
    let (first_replay, first_played) = first.finish();
    let (second_replay, second_played) = second.finish();
    // The first's start is before its effects; from the first step on they're one fight.
    assert_ne!(first_played.trail.at(0), second_played.trail.at(0));
    assert_eq!(checksums(&second_played.trail, 1), checksums(&first_played.trail, 1));

    // The first's choice is in its commands, the second's flags in its header, and each plays back.
    assert_eq!(first_replay.commands().len(), 2, "the choice's effect, and the thanks'");
    assert!(first_replay.starting_flags().is_empty());
    let first_bytes = first_replay.encode();
    assert_eq!(u32::from_le_bytes(first_bytes[8..12].try_into().unwrap()), COMMANDS_FORMAT_VERSION);
    let second_bytes = second_replay.encode();
    assert_eq!(u32::from_le_bytes(second_bytes[8..12].try_into().unwrap()), FLAGS_FORMAT_VERSION);
    let decoded = Replay::decode(&second_bytes).expect("reads");
    assert_eq!(decoded.starting_flags(), starting.flags.as_slice());
    assert!(decoded.commands().is_empty());
    for (bytes, trail) in [(first_bytes, &first_played.trail), (second_bytes, &second_played.trail)] {
        let mut player = Player::new(Replay::decode(&bytes).expect("reads"), &SCRIPTED).expect("starts");
        player.run_to_end();
        assert_eq!(checksums(&player.finish().trail, 0), checksums(trail, 0));
    }
    assert_eq!(second_played.stats.flags_refused, 0);
}
