//! The document's checks, the cook and the walk, on the guide's dialogue.

use std::collections::BTreeMap;

use sapphire_core::Name;
use sapphire_serialize::cooked::AlignedVec;

use crate::{
    Condition, ConditionId, Dialogue, DialogueDocument, DialogueWalk, DialogueWorld, EffectId, FlagRegistry,
};

const GUIDE: &str = include_str!("../test_content/guide.dialogue.ron");

fn registry() -> FlagRegistry {
    FlagRegistry { flags: vec!["met_guide".to_owned(), "ward_lit".to_owned(), "visits".to_owned()] }
}

fn guide() -> DialogueDocument {
    DialogueDocument::from_ron(GUIDE).expect("the guide reads")
}

/// What the cook would do: the pinned compiler and its scan.
fn compile(source: &str) -> Result<(), String> {
    sapphire_bytecode::cook(source).map(drop).map_err(|error| error.to_string())
}

fn cooked() -> Dialogue {
    guide().cook(&registry(), compile).expect("the guide cooks")
}

/// A world of flags in a map, asking conditions by their parsed form, and keeping what it did.
struct Flags {
    values: BTreeMap<u64, i32>,
    conditions: Vec<Condition>,
    dialogue: Dialogue,
    asked: Vec<u16>,
    applied: Vec<u16>,
}

impl Flags {
    fn new(set: &[(&str, i32)]) -> Self {
        let values = set.iter().map(|&(name, value)| (Name::new(name).as_u64(), value)).collect();
        let conditions = guide().conditions(&registry()).expect("parses");
        Self { values, conditions, dialogue: cooked(), asked: Vec::new(), applied: Vec::new() }
    }
}

impl DialogueWorld for Flags {
    fn ask(&mut self, condition: ConditionId) -> bool {
        self.asked.push(condition.0);
        let values = &self.values;
        self.conditions[usize::from(condition.0)]
            .holds(&|name| values.get(&Name::new(name).as_u64()).copied().unwrap_or(0))
    }

    fn apply(&mut self, effect: EffectId) {
        self.applied.push(effect.0);
        for write in &self.dialogue.simulation.effects[usize::from(effect.0)].writes {
            self.values.insert(write.flag, write.value);
        }
    }
}

fn walk(world: &mut Flags) -> DialogueWalk {
    let dialogue = world.dialogue.clone();
    DialogueWalk::begin(&dialogue, world)
}

fn line(walk: &DialogueWalk, world: &Flags) -> Option<String> {
    walk.view(&world.dialogue).map(|view| view.line.to_owned())
}

fn choices(walk: &DialogueWalk, world: &Flags) -> Vec<String> {
    let view = walk.view(&world.dialogue).expect("a line");
    view.choices.iter().flatten().map(|choice| choice.line.to_owned()).collect()
}

#[test]
fn a_cook_is_repeatable_and_reads_back() {
    let one = cooked().to_cooked().unwrap();
    let two = cooked().to_cooked().unwrap();
    assert_eq!(one.as_slice(), two.as_slice());
    let read = Dialogue::read(&one).expect("reads");
    assert_eq!(read, cooked());
    assert!(read.needs_scripts(), "it has conditions");
    assert_eq!(read.presentation.conditions, 2);
    assert_eq!(read.simulation.effects.len(), 2, "the choice's write and the line's");
    // What it ships is the source a load compiles, not bytecode, and the trigger that opens it.
    let source = guide().conditions_source(&registry()).unwrap().expect("it has conditions");
    assert_eq!(read.presentation.source, source);
    assert_eq!(read.presentation.opens_on, Some(Name::new("court_entered").as_u64()));
}

#[test]
fn the_conditions_chunk_lists_the_flags_it_reads_and_one_function_each() {
    let source = guide().conditions_source(&registry()).unwrap().expect("it has conditions");
    assert!(source.contains(r#"names = { "ward_lit", "met_guide", }"#), "{source}");
    assert!(source.contains("function() return (sapphire.flag(1) >= 1) end"), "{source}");
    assert!(source.contains("function() return (sapphire.flag(2) == 0) end"), "{source}");
    compile(&source).expect("the scan passes it");
}

#[test]
fn a_dialogue_with_no_conditions_has_no_chunk_and_never_compiles() {
    let text = GUIDE.replace(r#", when: "flag.ward_lit >= 1""#, "").replace(
        r#"branch: [(when: "flag.met_guide == 0", to: "greet")], otherwise: "farewell""#,
        r#"say: (speaker: "dialogue-guide-name", line: "dialogue-guide-again"), next: "farewell""#,
    );
    let document = DialogueDocument::from_ron(&text).expect("reads");
    let dialogue = document.cook(&registry(), |_| panic!("nothing to compile")).expect("cooks");
    assert!(!dialogue.needs_scripts());
    assert!(dialogue.presentation.source.is_empty());
}

/// A dialogue says only keys under the prefix a panel reads, so a line can't be missing from the panel's
/// fonts: the UI cook holds every key under it to them.
#[test]
fn every_key_a_dialogue_shows_is_under_the_panels_prefix() {
    for (from, to, field) in [
        (r#"line: "dialogue-guide-greet""#, r#"line: "guide-greet""#, "`nodes[0].say.line`"),
        (
            r#"speaker: "dialogue-guide-name", line: "dialogue-guide-ask""#,
            r#"speaker: "guide-name", line: "dialogue-guide-ask""#,
            "`nodes[1].say.speaker`",
        ),
        (r#"line: "dialogue-choice-leave""#, r#"line: "choice-leave""#, "`nodes[1].choices[2].line`"),
    ] {
        let text = GUIDE.replacen(from, to, 1);
        let error = DialogueDocument::from_ron(&text).unwrap().cook(&registry(), compile).unwrap_err();
        let error = error.to_string();
        assert!(error.contains(field) && error.contains(crate::KEY_PREFIX), "{to}: {error}");
    }
    let mut dialogue = cooked();
    dialogue.presentation.nodes[1].choices[0].line = "choice-light".to_owned();
    let error = Dialogue::read(&dialogue.to_cooked().unwrap()).unwrap_err().to_string();
    assert!(error.contains("`nodes[1].choices[0].line`"), "{error}");
}

/// The digest is of what the simulation reads and nothing else, and a pinned value, so a change to it is
/// one somebody meant.
#[test]
fn the_digest_is_the_effects_alone() {
    let dialogue = cooked();
    assert_eq!(dialogue.simulation.digest(), 0x1590_0ba2_77f2_2bfc);
    let mut reworded = guide();
    reworded.nodes[0].say.as_mut().unwrap().line = "dialogue-guide-hello".to_owned();
    let reworded = reworded.cook(&registry(), compile).unwrap();
    assert_eq!(reworded.simulation.digest(), dialogue.simulation.digest());
    let mut revalued = guide();
    revalued.nodes[2].set[0].1 = 2;
    let revalued = revalued.cook(&registry(), compile).unwrap();
    assert_ne!(revalued.simulation.digest(), dialogue.simulation.digest());
}

#[test]
fn a_walk_shows_lines_hides_choices_and_takes_branches() {
    let mut world = Flags::new(&[]);
    let dialogue = world.dialogue.clone();
    let mut at = walk(&mut world);
    let view = at.view(&dialogue).unwrap();
    assert_eq!(
        (view.speaker, view.line, view.vo, view.next),
        ("dialogue-guide-name", "dialogue-guide-greet", Some("guide_greet"), true)
    );
    assert!(!at.choose(&dialogue, 0, &mut world), "a line with no choices goes on by next");
    assert!(at.next(&dialogue, &mut world));
    assert_eq!(line(&at, &world).as_deref(), Some("dialogue-guide-ask"));
    assert_eq!(
        choices(&at, &world),
        ["dialogue-choice-light-ward", "dialogue-choice-leave"],
        "the ward isn't lit"
    );
    assert!(!at.next(&dialogue, &mut world), "a line with choices waits for one");
    assert!(!at.choose(&dialogue, 2, &mut world), "there's no third shown");

    // Lighting it sets the flag, then the line after sets another, then the branch reads it.
    assert!(at.choose(&dialogue, 0, &mut world));
    assert_eq!(world.applied, [0, 1]);
    assert_eq!(line(&at, &world).as_deref(), Some("dialogue-guide-thanks"));
    assert_eq!(at.at(&dialogue), Some(Name::new("lit")));
    assert!(at.next(&dialogue, &mut world));
    assert_eq!(line(&at, &world).as_deref(), Some("dialogue-guide-farewell"), "met_guide is 1 now");
    assert!(at.next(&dialogue, &mut world));
    assert!(at.is_over() && at.view(&dialogue).is_none());
    assert!(!at.next(&dialogue, &mut world), "nothing follows the end");
}

#[test]
fn a_choice_shows_once_its_condition_holds_and_slots_follow_what_shows() {
    let mut world = Flags::new(&[("ward_lit", 1)]);
    let dialogue = world.dialogue.clone();
    let mut at = walk(&mut world);
    at.next(&dialogue, &mut world);
    assert_eq!(
        choices(&at, &world),
        ["dialogue-choice-light-ward", "dialogue-choice-ask-again", "dialogue-choice-leave"]
    );
    // The second slot is the second shown: "ask again", to the branch, which goes back to the start.
    assert!(at.choose(&dialogue, 1, &mut world));
    assert_eq!(line(&at, &world).as_deref(), Some("dialogue-guide-greet"));
    assert!(world.applied.is_empty(), "nothing it took sets anything");

    // The last shown choice ends it.
    at.next(&dialogue, &mut world);
    assert!(at.choose(&dialogue, 2, &mut world));
    assert!(at.is_over());
}

/// Q3's seam: one line by its node's name, outside any conversation.
#[test]
fn a_line_is_found_by_its_nodes_name() {
    let dialogue = cooked();
    let line = dialogue.line(Name::new("farewell")).expect("a line");
    assert_eq!((line.speaker, line.line, line.vo), ("dialogue-guide-name", "dialogue-guide-farewell", None));
    assert_eq!(dialogue.line(Name::new("again")), None, "a branch says nothing");
    assert_eq!(dialogue.line(Name::new("nowhere")), None);
}

/// Every check says where, and these are what a document's author has to go on.
#[test]
fn checks_name_the_field() {
    let cases: &[(&str, &str, &str)] = &[
        (r#"start: "greet""#, r#"start: "nowhere""#, "`start`"),
        (r#"opens_on: "court_entered""#, r#"opens_on: "Court entered""#, "`opens_on`"),
        (r#"(name: "lit""#, r#"(name: "Lit""#, "`nodes[2].name`"),
        (r#"(name: "farewell""#, r#"(name: "greet""#, "`nodes[4].name`"),
        (r#"next: "ask""#, r#"next: "nowhere""#, "`nodes[0].next`"),
        (r#"line: "dialogue-guide-greet""#, r#"line: "Hello there!""#, "`nodes[0].say.line`"),
        (
            r#"speaker: "dialogue-guide-name", line: "dialogue-guide-ask""#,
            r#"speaker: "", line: "dialogue-guide-ask""#,
            "`nodes[1].say.speaker`",
        ),
        (r#"vo: "guide_greet""#, r#"vo: """#, "`nodes[0].say.vo`"),
        (r#"(role: "hero""#, r#"(role: """#, "`nodes[2].say.faces[0].role`"),
        (r#"expression: "happy""#, r#"expression: """#, "`nodes[2].say.faces[0].expression`"),
        (r#"weight: 0.7"#, r#"weight: 1.7"#, "`nodes[2].say.faces[0].weight`"),
        (r#"blend: 0.5"#, r#"blend: -0.5"#, "`nodes[2].say.faces[1].blend`"),
        (r#"to: "lit", set"#, r#"to: "gone", set"#, "`nodes[1].choices[0].to`"),
        (r#"line: "dialogue-choice-leave""#, r#"line: "leave now""#, "`nodes[1].choices[2].line`"),
        (r#""flag.ward_lit >= 1""#, r#""flag.ward_lit""#, "`nodes[1].choices[1].when`"),
        (r#""flag.ward_lit >= 1""#, r#""flag.unknown >= 1""#, "`nodes[1].choices[1].when`"),
        (r#"set: [("ward_lit", 1)]"#, r#"set: [("quest_done", 1)]"#, "`nodes[1].choices[0].set[0]`"),
        (
            r#"set: [("ward_lit", 1)]"#,
            r#"set: [("ward_lit", 1), ("ward_lit", 2)]"#,
            "`nodes[1].choices[0].set[1]`",
        ),
        (r#"set: [("met_guide", 1)]"#, r#"set: [("met", 1)]"#, "`nodes[2].set[0]`"),
        (
            r#"(line: "dialogue-choice-leave")"#,
            r#"(line: "dialogue-choice-leave"), (line: "a"), (line: "b")"#,
            "`nodes[1].choices`",
        ),
        (
            r#"set: [("met_guide", 1)], next: "again""#,
            r#"set: [("met_guide", 1)], next: "again", choices: [(line: "a")]"#,
            "`nodes[2].next`",
        ),
        (
            r#"(when: "flag.met_guide == 0", to: "greet")"#,
            r#"(when: "flag.met_guide == 0", to: "far")"#,
            "`nodes[3].branch[0].to`",
        ),
        (
            r#"(when: "flag.met_guide == 0", to: "greet")"#,
            r#"(when: "met_guide == 0", to: "greet")"#,
            "`nodes[3].branch[0].when`",
        ),
        (r#"otherwise: "farewell""#, r#"otherwise: "farewell", next: "greet""#, "`nodes[3].next`"),
        (r#"to: "greet")], otherwise"#, r#"to: "again")], otherwise"#, "`nodes[3]`"),
        (
            r#"blend: 0.5)])"#,
            r#"blend: 0.5)]), branch: [(when: "flag.visits > 1", to: "greet")]"#,
            "`nodes[2].branch`",
        ),
        (
            r#"(name: "farewell", say: (speaker: "dialogue-guide-name", line: "dialogue-guide-farewell"))"#,
            r#"(name: "farewell")"#,
            "`nodes[4].say`",
        ),
        (
            r#"line: "dialogue-guide-farewell")),"#,
            r#"line: "dialogue-guide-farewell")), (name: "extra", say: (speaker: "dialogue-a", line: "dialogue-b")),"#,
            "`nodes[5]`",
        ),
    ];
    for (from, to, field) in cases {
        assert!(GUIDE.contains(from), "{from} isn't in the guide");
        let text = GUIDE.replacen(from, to, 1);
        let error = match DialogueDocument::from_ron(&text) {
            Ok(document) => document.cook(&registry(), compile).expect_err(to).to_string(),
            Err(error) => error.to_string(),
        };
        assert!(error.contains(field), "{to}: {error}");
    }
}

#[test]
fn a_choice_always_shows_one_way_on() {
    let mut document = guide();
    for choice in &mut document.nodes[1].choices {
        choice.when.get_or_insert_with(|| "flag.visits > 1".to_owned());
    }
    let error = document.cook(&registry(), compile).unwrap_err().to_string();
    assert!(error.contains("`nodes[1].choices`"), "{error}");
}

#[test]
fn a_field_nobody_knows_is_refused() {
    let text = GUIDE.replacen(r#"(name: "lit","#, r#"(name: "lit", mood: 3,"#, 1);
    let error = DialogueDocument::from_ron(&text).unwrap_err().to_string();
    assert!(error.contains("mood"), "{error}");
}

#[test]
fn conditions_the_compiler_refuses_stop_the_cook() {
    let error = guide().cook(&registry(), |_| Err("no".to_owned())).unwrap_err().to_string();
    assert!(error.contains("don't compile"), "{error}");
}

/// M5 C5: a line's faces are presentation's, so the digest the guide's pinned to above holds with them, and
/// a version 1 document, which has none, still loads.
#[test]
fn faces_stay_out_of_the_digest_and_version_1_still_loads() {
    let dialogue = cooked();
    let faces = &dialogue.presentation.nodes[2].say.as_ref().unwrap().faces;
    assert_eq!(faces.len(), 2);
    assert_eq!(
        (faces[0].role, faces[0].expression),
        (Name::new("hero").as_u64(), Name::new("happy").as_u64())
    );
    assert_eq!((faces[0].weight, faces[0].blend, faces[1].weight, faces[1].blend), (0.7, 0.2, 1.0, 0.5));
    let mut plain = guide();
    plain.nodes[2].say.as_mut().unwrap().faces.clear();
    let plain = plain.cook(&registry(), compile).unwrap();
    assert_eq!(plain.simulation.digest(), dialogue.simulation.digest());

    let at = GUIDE.find(", faces: [").unwrap();
    let end = at + GUIDE[at..].find("])").unwrap() + 1;
    let old = format!("{}{}", &GUIDE[..at], &GUIDE[end..]).replacen("version: 2,", "version: 1,", 1);
    let old = DialogueDocument::from_ron(&old).expect("version 1 loads");
    assert_eq!(old.cook(&registry(), compile).unwrap(), plain);
    for version in ["version: 0,", "version: 3,"] {
        assert!(DialogueDocument::from_ron(&GUIDE.replacen("version: 2,", version, 1)).is_err(), "{version}");
    }
    let committed = include_str!("../../../content/dialogue/ward_guide.dialogue.ron");
    assert!(committed.contains("version: 1,") && DialogueDocument::from_ron(committed).is_ok());
}

/// M5 C6: a voiced line names whose mouth its voice moves; that's presentation's, so the digest holds, and
/// a line with no voice, in a document or in a pack's bytes, can't name one.
#[test]
fn a_voiced_lines_mouth_stays_out_of_the_digest() {
    let mut document = guide();
    document.nodes[0].say.as_mut().unwrap().mouth = Some("foe".to_owned());
    let dialogue = document.cook(&registry(), compile).unwrap();
    assert_eq!(dialogue.simulation.digest(), cooked().simulation.digest());
    let mut world = Flags::new(&[]);
    let mut walked = DialogueWalk::begin(&dialogue, &mut world);
    let view = walked.view(&dialogue).unwrap();
    assert_eq!((view.vo, view.mouth), (Some("guide_greet"), Some(Name::new("foe").as_u64())));
    walked.next(&dialogue, &mut world);
    assert_eq!(walked.view(&dialogue).unwrap().mouth, None);

    for (node, mouth) in [(1, "foe"), (0, "")] {
        let mut document = guide();
        document.nodes[node].say.as_mut().unwrap().mouth = Some(mouth.to_owned());
        let error = document.cook(&registry(), compile).unwrap_err().to_string();
        assert!(error.contains(&format!("`nodes[{node}].say.mouth`")), "{error}");
    }
    let mut spoiled = dialogue;
    spoiled.presentation.nodes[1].say.as_mut().unwrap().mouth = Some(1);
    let error = Dialogue::read(&spoiled.to_cooked().unwrap()).unwrap_err().to_string();
    assert!(error.contains("`nodes[1].say.mouth`"), "{error}");
}

#[test]
fn a_lines_view_has_its_faces() {
    let mut world = Flags::new(&[]);
    let dialogue = world.dialogue.clone();
    let mut at = walk(&mut world);
    assert!(at.view(&dialogue).unwrap().faces.is_empty());
    at.next(&dialogue, &mut world);
    at.choose(&dialogue, 0, &mut world);
    let view = at.view(&dialogue).unwrap();
    assert_eq!(view.line, "dialogue-guide-thanks");
    assert_eq!(view.faces.len(), 2);
}

/// Bytes from somewhere else get what a document gets: a graph a document couldn't hold, a pack can't.
#[test]
fn values_a_document_cant_hold_a_pack_cant_either() {
    type Spoil = fn(&mut Dialogue);
    let cases: &[(&str, Spoil)] = &[
        ("`start`", |d| d.presentation.start = 9),
        ("`nodes[0].next`", |d| d.presentation.nodes[0].next = Some(99)),
        ("`nodes[1].choices[1].when`", |d| d.presentation.nodes[1].choices[1].when = Some(7)),
        ("`nodes[1].choices[0].effect`", |d| d.presentation.nodes[1].choices[0].effect = Some(7)),
        ("`nodes[3].branch[0].to`", |d| d.presentation.nodes[3].branch[0].to = 50),
        ("`nodes[3]`", |d| d.presentation.nodes[3].branch[0].to = 3),
        ("`nodes[4]`", |d| d.presentation.nodes[3].otherwise = Some(0)),
        ("`nodes[0].say.line`", |d| d.presentation.nodes[0].say.as_mut().unwrap().line = "a b".to_owned()),
        ("`source`", |d| d.presentation.source.clear()),
        ("`source`", |d| d.presentation.source = "-".repeat(crate::MAX_SOURCE + 1)),
        ("`effects[0].writes`", |d| d.simulation.effects[0].writes.clear()),
        ("`nodes[2].say.faces[0].weight`", |d| {
            d.presentation.nodes[2].say.as_mut().unwrap().faces[0].weight = f32::NAN
        }),
        ("`nodes[2].say.faces`", |d| {
            let say = d.presentation.nodes[2].say.as_mut().unwrap();
            say.faces = vec![say.faces[0]; crate::MAX_FACES + 1];
        }),
        ("`nodes[1].choices`", |d| {
            for choice in &mut d.presentation.nodes[1].choices {
                choice.when = Some(0);
            }
        }),
    ];
    for (field, spoil) in cases {
        let mut dialogue = cooked();
        spoil(&mut dialogue);
        let bytes = dialogue.to_cooked().unwrap();
        let error = Dialogue::read(&bytes).expect_err(field).to_string();
        assert!(error.contains(field), "{field}: {error}");
    }
}

#[test]
fn hostile_bytes_are_refused_without_a_panic() {
    let bytes = cooked().to_cooked().unwrap();
    for length in (0..bytes.len()).step_by(7) {
        let mut cut = AlignedVec::<16>::new();
        cut.extend_from_slice(&bytes[..length]);
        assert!(Dialogue::read(&cut).is_err(), "cut at {length}");
    }
    for at in (0..bytes.len()).step_by(3) {
        let mut flipped = AlignedVec::<16>::new();
        flipped.extend_from_slice(&bytes);
        flipped[at] ^= 0x5a;
        let _ = Dialogue::read(&flipped);
    }
}
