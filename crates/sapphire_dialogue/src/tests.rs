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
fn compile(source: &str) -> Result<Vec<u8>, String> {
    sapphire_bytecode::cook(source).map(|chunk| chunk.into_bytes()).map_err(|error| error.to_string())
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
        r#"say: (speaker: "guide-name", line: "guide-again"), next: "farewell""#,
    );
    let document = DialogueDocument::from_ron(&text).expect("reads");
    let dialogue = document.cook(&registry(), |_| panic!("nothing to compile")).expect("cooks");
    assert!(!dialogue.needs_scripts());
    assert!(dialogue.presentation.chunk.is_empty());
}

/// The digest is of what the simulation reads and nothing else, and a pinned value, so a change to it is
/// one somebody meant.
#[test]
fn the_digest_is_the_effects_alone() {
    let dialogue = cooked();
    assert_eq!(dialogue.simulation.digest(), 0x1590_0ba2_77f2_2bfc);
    let mut reworded = guide();
    reworded.nodes[0].say.as_mut().unwrap().line = "guide-hello".to_owned();
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
        ("guide-name", "guide-greet", Some("guide_greet"), true)
    );
    assert!(!at.choose(&dialogue, 0, &mut world), "a line with no choices goes on by next");
    assert!(at.next(&dialogue, &mut world));
    assert_eq!(line(&at, &world).as_deref(), Some("guide-ask"));
    assert_eq!(choices(&at, &world), ["choice-light-ward", "choice-leave"], "the ward isn't lit");
    assert!(!at.next(&dialogue, &mut world), "a line with choices waits for one");
    assert!(!at.choose(&dialogue, 2, &mut world), "there's no third shown");

    // Lighting it sets the flag, then the line after sets another, then the branch reads it.
    assert!(at.choose(&dialogue, 0, &mut world));
    assert_eq!(world.applied, [0, 1]);
    assert_eq!(line(&at, &world).as_deref(), Some("guide-thanks"));
    assert_eq!(at.at(&dialogue), Some(Name::new("lit")));
    assert!(at.next(&dialogue, &mut world));
    assert_eq!(line(&at, &world).as_deref(), Some("guide-farewell"), "met_guide is 1 now");
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
    assert_eq!(choices(&at, &world), ["choice-light-ward", "choice-ask-again", "choice-leave"]);
    // The second slot is the second shown: "ask again", to the branch, which goes back to the start.
    assert!(at.choose(&dialogue, 1, &mut world));
    assert_eq!(line(&at, &world).as_deref(), Some("guide-greet"));
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
    assert_eq!((line.speaker, line.line, line.vo), ("guide-name", "guide-farewell", None));
    assert_eq!(dialogue.line(Name::new("again")), None, "a branch says nothing");
    assert_eq!(dialogue.line(Name::new("nowhere")), None);
}

/// Every check says where, and these are what a document's author has to go on.
#[test]
fn checks_name_the_field() {
    let cases: &[(&str, &str, &str)] = &[
        (r#"start: "greet""#, r#"start: "nowhere""#, "`start`"),
        (r#"(name: "lit""#, r#"(name: "Lit""#, "`nodes[2].name`"),
        (r#"(name: "farewell""#, r#"(name: "greet""#, "`nodes[4].name`"),
        (r#"next: "ask""#, r#"next: "nowhere""#, "`nodes[0].next`"),
        (r#"line: "guide-greet""#, r#"line: "Hello there!""#, "`nodes[0].say.line`"),
        (
            r#"speaker: "guide-name", line: "guide-ask""#,
            r#"speaker: "", line: "guide-ask""#,
            "`nodes[1].say.speaker`",
        ),
        (r#"vo: "guide_greet""#, r#"vo: """#, "`nodes[0].say.vo`"),
        (r#"to: "lit", set"#, r#"to: "gone", set"#, "`nodes[1].choices[0].to`"),
        (r#"line: "choice-leave""#, r#"line: "leave now""#, "`nodes[1].choices[2].line`"),
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
            r#"(line: "choice-leave")"#,
            r#"(line: "choice-leave"), (line: "a"), (line: "b")"#,
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
            r#"line: "guide-thanks")"#,
            r#"line: "guide-thanks"), branch: [(when: "flag.visits > 1", to: "greet")]"#,
            "`nodes[2].branch`",
        ),
        (
            r#"(name: "farewell", say: (speaker: "guide-name", line: "guide-farewell"))"#,
            r#"(name: "farewell")"#,
            "`nodes[4].say`",
        ),
        (
            r#"line: "guide-farewell")),"#,
            r#"line: "guide-farewell")), (name: "extra", say: (speaker: "a", line: "b")),"#,
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
fn a_chunk_the_compiler_refuses_stops_the_cook() {
    let error = guide().cook(&registry(), |_| Err("no".to_owned())).unwrap_err().to_string();
    assert!(error.contains("don't compile"), "{error}");
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
        ("`chunk`", |d| d.presentation.chunk.clear()),
        ("`effects[0].writes`", |d| d.simulation.effects[0].writes.clear()),
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
