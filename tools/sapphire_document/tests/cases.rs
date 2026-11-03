//! The hard cases, one at a time, on small documents written to show them, and on the real ones where that
//! says more: a field the file leaves out, an `Option` coming and going, a list growing and shrinking, a
//! variant changing shape, a float's spelling, what a document refuses, what an entry's identity survives,
//! and a file changing under an edit.

mod support;

use std::sync::Arc;

use bevy_reflect::{GetPath, PartialReflect, Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_document::{
    Document, Outcome, PropertyPath, ReadFailure, Refused, Schema, Step, kinds, registry_of,
};
use sapphire_moves::{MoveSetDocument, Shape};
use serde::{Deserialize, Serialize};
use support::{open, read};

fn path(text: &str) -> PropertyPath {
    text.parse().expect(text)
}

/// A document with one of everything hard about a text. Its fields are left out or written as each test
/// wants.
#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sample {
    #[serde(default)]
    count: u32,
    #[serde(default = "half")]
    share: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inner: Option<Inner>,
    #[serde(default)]
    nested: Inner,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    list: Vec<Entry>,
    #[serde(default)]
    pair: (f32, f32),
    #[serde(default)]
    shape: Form,
}

fn half() -> f32 {
    0.5
}

#[derive(Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct Inner {
    depth: u32,
    label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    #[serde(default)]
    at: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
enum Form {
    #[default]
    Point,
    Round(f32),
    Capsule {
        radius: f32,
        length: f32,
    },
}

fn sample_schema() -> Schema<Sample> {
    Schema {
        parse: |text| {
            ron::Options::default()
                .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
                .from_str(text)
                .map_err(|error| error.to_string())
        },
        check: |sample| if sample.count > 100 { vec!["too many".to_owned()] } else { Vec::new() },
        importer: None,
    }
}

fn sample(text: &str) -> Document<Sample> {
    Document::open(text.to_owned(), sample_schema(), Arc::new(registry_of::<Sample>()))
}

/// Sets `at` to the RON `new`, checks the text is `want`, then checks undo gives back the bytes.
fn edit(document: &mut Document<Sample>, at: &str, new: &str, want: &str) {
    let before = document.text().to_owned();
    assert_eq!(document.set_text("edit", &path(at), new), Ok(Outcome::Changed), "{at}");
    assert_eq!(document.text(), want, "{at}");
    assert!(document.undo());
    assert_eq!(document.text(), before, "{at}: undo");
    assert!(document.redo());
    assert_eq!(document.text(), want, "{at}: redo");
    assert!(document.undo());
}

const LAID_OUT: &str = "// The file's own comment.
(
    // How many.
    count: 3, // beside it
    share: 0.25,
    nested: (
        depth: 1, // deep
    ),
    list: [
        // The first.
        (name: \"a\", at: 1),
        (name: \"b\", at: 2), // the second
        // The third.
        (name: \"c\", at: 3),
    ],
    pair: (1.0, 2.0),
    shape: Capsule(radius: 0.3, /* thin */ length: 1.0),
)
";

#[test]
fn a_number_changes_and_nothing_else_does() {
    let mut document = sample(LAID_OUT);
    assert_eq!(document.failure(), None);
    edit(&mut document, "count", "4", &LAID_OUT.replace("count: 3,", "count: 4,"));
    edit(&mut document, "nested.depth", "9", &LAID_OUT.replace("depth: 1,", "depth: 9,"));
    edit(&mut document, "pair[1]", "7.5", &LAID_OUT.replace("(1.0, 2.0)", "(1.0, 7.5)"));
    edit(&mut document, r#"list["b"].at"#, "20", &LAID_OUT.replace("\"b\", at: 2", "\"b\", at: 20"));
    edit(&mut document, "shape.length", "2.0", &LAID_OUT.replace("length: 1.0", "length: 2.0"));
}

#[test]
fn a_default_left_out_appears_after_the_field_declared_before_it() {
    let text = "(\n    count: 3, // beside it\n    nested: (depth: 1),\n)\n";
    let mut document = sample(text);
    edit(
        &mut document,
        "share",
        "0.75",
        "(\n    count: 3, // beside it\n    share: 0.75,\n    nested: (depth: 1),\n)\n",
    );
    let inline = "(count: 3, nested: (depth: 1))";
    let mut document = sample(inline);
    edit(&mut document, "share", "0.75", "(count: 3, share: 0.75, nested: (depth: 1))");
    let first = "(\n    // about share\n    share: 0.25,\n)\n";
    let mut document = sample(first);
    edit(&mut document, "count", "1", "(\n    count: 1,\n    // about share\n    share: 0.25,\n)\n");
    let empty = "()";
    let mut document = sample(empty);
    edit(&mut document, "count", "1", "(count: 1)");
}

#[test]
fn a_nested_struct_left_out_comes_in_whole() {
    let text = "(\n    count: 3,\n)\n";
    let mut document = sample(text);
    edit(&mut document, "nested.depth", "5", "(\n    count: 3,\n    nested: (depth: 5),\n)\n");
}

#[test]
fn an_option_comes_and_goes_as_the_file_spells_it() {
    // This file leaves `Some` out, as our moves do.
    let text = "(\n    count: 3,\n)\n";
    let mut document = sample(text);
    edit(&mut document, "note", r#"Some("hello")"#, "(\n    count: 3,\n    note: \"hello\",\n)\n");
    assert_eq!(document.set_text("on", &path("note"), r#"Some("hello")"#), Ok(Outcome::Changed));
    let with_note = document.text().to_owned();
    // And off: its line goes, because `None` is what leaving it out says.
    assert_eq!(document.set_text("off", &path("note"), "None"), Ok(Outcome::Changed));
    assert_eq!(document.text(), text);
    assert!(document.undo());
    assert_eq!(document.text(), with_note);
    // A file that says `Some` gets it said.
    let says = "(\n    note: Some(\"a\"),\n    count: 3,\n)\n";
    let mut document = sample(says);
    edit(&mut document, "note", r#"Some("b")"#, "(\n    note: Some(\"b\"),\n    count: 3,\n)\n");
    edit(
        &mut document,
        "inner",
        "Some((depth: 2))",
        "(\n    note: Some(\"a\"),\n    inner: Some((depth: 2)),\n    count: 3,\n)\n",
    );
    // A step into what it holds keeps what's around it.
    let text = "(\n    inner: (\n        // a depth\n        depth: 2,\n    ),\n)\n";
    let mut document = sample(text);
    edit(&mut document, "inner?.depth", "3", &text.replace("depth: 2", "depth: 3"));
    // Written as `None`, which a person may do.
    let text = "(note: None, count: 1)";
    let mut document = sample(text);
    edit(&mut document, "note", r#"Some("x")"#, r#"(note: "x", count: 1)"#);
}

#[test]
fn taking_an_option_with_comments_says_what_went_and_undo_brings_it_back() {
    let text = "(\n    inner: (\n        // a depth\n        depth: 2,\n    ),\n    count: 1,\n)\n";
    let mut document = sample(text);
    assert_eq!(document.set_text("off", &path("inner"), "None"), Ok(Outcome::Changed));
    let record = &document.last_step().expect("a step").records[0];
    assert_eq!(record.comments_lost, 1);
    assert!(!document.text().contains("a depth"));
    assert!(document.undo());
    assert_eq!(document.text(), text);
}

#[test]
fn a_variant_changes_shape_by_replacing_all_of_it() {
    let mut document = sample(LAID_OUT);
    let whole = "Capsule(radius: 0.3, /* thin */ length: 1.0)";
    edit(&mut document, "shape", "Point", &LAID_OUT.replace(whole, "Point"));
    edit(&mut document, "shape", "Round(0.5)", &LAID_OUT.replace(whole, "Round(0.5)"));
    assert_eq!(document.set_text("edit", &path("shape"), "Point"), Ok(Outcome::Changed));
    assert_eq!(document.last_step().expect("a step").records[0].comments_lost, 1);
    assert!(document.undo());
    // A step into the other variant is no path, and the text stays.
    assert!(matches!(
        document.set_text("edit", &path("shape[0]"), "1.0"),
        Err(Refused::NoSuchPath(_) | Refused::WrongType { .. })
    ));
    assert_eq!(document.text(), LAID_OUT);
}

#[test]
fn a_real_hit_volume_changes_shape_and_the_file_around_it_stays() {
    let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
    let before = document.text().to_owned();
    let at = path(r#"moves["light_1"].hitboxes[0].shape"#);
    assert_eq!(document.set("shape", &at, &Shape::Sphere { radius: 0.5 }), Ok(Outcome::Changed));
    let step = document.last_step().expect("a step");
    assert_eq!(step.records[0].new.as_deref(), Some("Sphere(radius: 0.5)"));
    assert!(step.change.removed.starts_with("Capsule") || !step.change.removed.contains('\n'));
    assert!(document.undo());
    assert_eq!(document.text(), before);
}

#[test]
fn floats_keep_their_spelling_until_they_change_and_take_the_shortest_that_reads_back() {
    // The same value, spelled three ways: no edit, and no byte moves.
    for (text, value) in
        [("(share: 1)", 1.0_f32), ("(share: 1.0)", 1.0), ("(share: 1e0)", 1.0), ("(share: 0.30000001)", 0.3)]
    {
        let mut document = sample(text);
        assert_eq!(document.set("same", &path("share"), &value), Ok(Outcome::Unchanged), "{text}");
        assert_eq!(document.text(), text);
    }
    for (value, want) in [
        (1.0_f32, "1.0"),
        (0.1, "0.1"),
        (1.0 / 3.0, "0.33333334"),
        (1e-5, "0.00001"),
        (1e20, "100000000000000000000.0"),
        (-0.5, "-0.5"),
    ] {
        let mut document = sample("(share: 2)");
        assert_eq!(document.set("edit", &path("share"), &value), Ok(Outcome::Changed), "{value}");
        assert_eq!(document.text(), format!("(share: {want})"));
        assert!(document.undo());
        assert_eq!(document.text(), "(share: 2)", "the spelling comes back");
    }
    // Zero's sign is a real difference, though `==` says otherwise.
    let mut document = sample("(share: 0.0)");
    assert_eq!(document.set("edit", &path("share"), &-0.0_f32), Ok(Outcome::Changed));
    assert_eq!(document.text(), "(share: -0.0)");
    // A float that doesn't equal itself can't pass the check.
    let mut document = sample("(share: 0.0)");
    assert_eq!(
        document.set("edit", &path("share"), &f32::NAN),
        Err(Refused::Disagrees { path: "share".to_owned() })
    );
    assert_eq!(document.text(), "(share: 0.0)");
}

#[test]
fn a_list_grows_and_shrinks_in_its_own_layout() {
    let entry = Entry { name: "new".to_owned(), at: 9 };
    let list = path("list");
    let mut document = sample(LAID_OUT);
    assert_eq!(document.insert("add", &list, 3, &entry), Ok(Outcome::Changed));
    assert_eq!(
        document.text(),
        LAID_OUT.replace(
            "        (name: \"c\", at: 3),\n",
            "        (name: \"c\", at: 3),\n        (name: \"new\", at: 9),\n"
        )
    );
    assert!(document.undo());
    assert_eq!(document.insert("add", &list, 1, &entry), Ok(Outcome::Changed));
    assert_eq!(
        document.text(),
        LAID_OUT.replace(
            "        (name: \"a\", at: 1),\n",
            "        (name: \"a\", at: 1),\n        (name: \"new\", at: 9),\n"
        )
    );
    assert!(document.undo());
    assert_eq!(document.insert("add", &list, 0, &entry), Ok(Outcome::Changed));
    assert_eq!(
        document.text(),
        LAID_OUT.replace("    list: [\n", "    list: [\n        (name: \"new\", at: 9),\n")
    );
    assert!(document.undo());
    assert_eq!(document.text(), LAID_OUT);
    // Taken out by name, its comment stays where a person wrote it.
    assert_eq!(document.remove("off", &path(r#"list["a"]"#)), Ok(Outcome::Changed));
    assert_eq!(document.text(), LAID_OUT.replace("        (name: \"a\", at: 1),\n", ""));
    assert!(document.undo());
    assert_eq!(document.text(), LAID_OUT);
    // Left out of the file: the field appears, and goes again.
    let mut document = sample("(count: 1)");
    assert_eq!(document.insert("add", &list, 0, &entry), Ok(Outcome::Changed));
    assert_eq!(document.text(), r#"(count: 1, list: [(name: "new", at: 9)])"#);
    assert!(document.undo());
    assert_eq!(document.text(), "(count: 1)");
    // Into an empty one, and out to an empty one again.
    let mut document = sample("(list: [])");
    assert_eq!(document.insert("add", &list, 0, &entry), Ok(Outcome::Changed));
    assert_eq!(document.text(), r#"(list: [(name: "new", at: 9)])"#);
    assert_eq!(document.remove("off", &path(r#"list["new"]"#)), Ok(Outcome::Changed));
    assert_eq!(document.text(), "(list: [])");
    // A value that isn't an entry is refused, not a panic.
    let mut document = sample(LAID_OUT);
    assert!(matches!(document.insert("add", &list, 0, &3_u32), Err(Refused::WrongType { .. })));
    assert_eq!(document.text(), LAID_OUT);
}

#[test]
fn a_list_set_whole_to_another_length_is_rewritten_and_says_what_went() {
    let mut document = sample(LAID_OUT);
    let new = vec![Entry { name: "only".to_owned(), at: 1 }];
    assert_eq!(document.set("list", &path("list"), &new), Ok(Outcome::Changed));
    assert!(document.text().contains("list: [(name: \"only\", at: 1)],"));
    assert_eq!(document.last_step().expect("a step").records[0].comments_lost, 3);
    assert!(document.undo());
    assert_eq!(document.text(), LAID_OUT);
}

/// A move is known by its name, so an edit of one reaches it wherever it went, and undo unwinds through a
/// reorder because it gives back bytes, in order.
#[test]
fn identity_survives_a_reorder_and_undo_after_one() {
    let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
    let original = document.text().to_owned();
    let startup = path(r#"moves["light_2"].startup"#);
    let moves = path("moves");
    let value = |document: &Document<MoveSetDocument>| {
        let root = document.value().expect("it reads");
        root.moves.iter().find(|mv| mv.name == "light_2").expect("light_2").startup
    };
    let before = value(&document);
    assert_eq!(document.set("first", &startup, &(before + 1)), Ok(Outcome::Changed));
    let after_first = document.text().to_owned();
    assert_eq!(document.reorder("move", &moves, 1, 3), Ok(Outcome::Changed));
    let after_reorder = document.text().to_owned();
    assert_ne!(after_reorder, after_first);
    let names: Vec<_> = document.value().expect("reads").moves.iter().map(|mv| mv.name.clone()).collect();
    assert_eq!(names[3], "light_2");
    // The same path reaches the same move in its new place.
    assert_eq!(document.set("second", &startup, &(before + 2)), Ok(Outcome::Changed));
    assert_eq!(value(&document), before + 2);
    // And a rename: the record says the old name, and what's next says the new.
    let rename = path(r#"moves["light_2"].name"#);
    assert_eq!(document.set("rename", &rename, &"second_cut".to_owned()), Ok(Outcome::Changed));
    assert!(document.set("gone", &startup, &0_u32).is_err());
    assert!(document.undo());
    assert!(document.undo());
    assert_eq!(document.text(), after_reorder);
    assert!(document.undo());
    assert_eq!(document.text(), after_first);
    assert!(document.undo());
    assert_eq!(document.text(), original);
    while document.redo() {}
    assert_eq!(value_named(&document, "second_cut"), before + 2);
}

fn value_named(document: &Document<MoveSetDocument>, name: &str) -> u32 {
    let root = document.value().expect("it reads");
    root.moves.iter().find(|mv| mv.name == name).expect("the move").startup
}

/// Hit volumes can share a name, a hit group's, so a name alone can't be what they go by.
#[test]
fn entries_that_share_a_name_go_by_their_place() {
    let text = "(list: [(name: \"same\", at: 1), (name: \"same\", at: 2), (name: \"own\", at: 3)])";
    let document = sample(text);
    assert_eq!(document.entry_path(&path("list"), 1).expect("an entry").to_string(), "list[1]");
    assert_eq!(document.entry_path(&path("list"), 2).expect("an entry").to_string(), r#"list["own"]"#);
    let mut document = sample(text);
    assert!(document.set("ambiguous", &path(r#"list["same"].at"#), &9_u32).is_err());
    assert_eq!(document.text(), text);
}

/// What a document's own rule says: a parse refuses the edit, and `check` only reports.
#[test]
fn a_parse_refuses_and_a_check_reports() {
    // `EffectLibrary` checks everything as it reads.
    let mut library = open("crates/sapphire_vfx/default_effects.ron", kinds::effects());
    let text = library.text().to_owned();
    let name =
        library.entry_path(&path("effects"), 0).expect("an effect").join(Step::Field("name".to_owned()));
    let refused = library.set("rename", &name, &String::new());
    assert!(matches!(&refused, Err(Refused::Document(why)) if why.contains("name")), "{refused:?}");
    assert_eq!(library.text(), text);
    // A move with no ticks reads, and `check` says what's wrong with it. It lands: T3's rule.
    let mut moves = open("content/combat/player/player.moves.ron", kinds::moves());
    let text = moves.text().to_owned();
    assert_eq!(moves.issues(), Vec::<String>::new());
    let at = path(r#"moves["light_1"].active"#);
    assert_eq!(moves.set("zero", &at, &0_u32), Ok(Outcome::Changed));
    assert!(!moves.issues().is_empty(), "the check says something");
    assert!(moves.undo());
    assert_eq!(moves.text(), text);
    assert_eq!(moves.issues(), Vec::<String>::new());
    // A cue's `check` is the same kind.
    let mut cue = open("content/audio/hit_heavy.cue.ron", kinds::cue());
    assert_eq!(cue.set_text("bad", &path("sounds"), "[]"), Ok(Outcome::Changed));
    assert!(!cue.issues().is_empty());
}

#[test]
fn a_value_of_the_wrong_type_is_refused() {
    let mut document = sample(LAID_OUT);
    assert!(matches!(
        document.set("bad", &path("count"), &"a string".to_owned()),
        Err(Refused::WrongType { .. })
    ));
    assert!(matches!(
        document.set_text("bad", &path("count"), "\"a string\""),
        Err(Refused::WrongType { .. })
    ));
    assert!(matches!(document.set("bad", &path("nope"), &1_u32), Err(Refused::NoSuchPath(_))));
    assert_eq!(document.text(), LAID_OUT);
}

#[test]
fn a_gesture_is_one_step_and_a_cancelled_one_is_none() {
    let mut document = sample(LAID_OUT);
    document.begin_gesture("drag");
    for at in 4..30_u32 {
        assert_eq!(document.set("drag", &path("count"), &at), Ok(Outcome::Changed));
    }
    assert_eq!(document.undo_label(), Some("drag"));
    assert!(document.undo());
    assert_eq!(document.text(), LAID_OUT);
    assert!(document.redo());
    assert!(document.text().contains("count: 29,"));
    assert!(document.undo());
    document.begin_gesture("drag");
    assert_eq!(document.set("drag", &path("count"), &9_u32), Ok(Outcome::Changed));
    document.cancel_gesture();
    assert_eq!(document.text(), LAID_OUT);
    assert_eq!(document.undo_label(), None);
}

/// Two fields of one window, by turns, as a drag writes them.
fn drag(document: &mut Document<MoveSetDocument>, ticks: u32) {
    let (from, to) = (path("moves[0].hitboxes[0].from"), path("moves[0].hitboxes[0].to"));
    for tick in 0..ticks {
        let at = 2 + tick % 40;
        assert_eq!(document.set_text("drag", &to, &(at + 3).to_string()).map(|_| ()), Ok(()));
        assert_eq!(document.set_text("drag", &from, &at.to_string()).map(|_| ()), Ok(()));
    }
}

#[test]
fn a_long_drag_holds_what_a_short_one_does() {
    let mut held = Vec::new();
    for ticks in [100, 5_000] {
        let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
        let before = document.text().to_owned();
        document.begin_gesture("drag");
        drag(&mut document, ticks);
        document.end_gesture();
        let after = document.text().to_owned();
        let step = document.last_step().expect("a step");
        assert_eq!(step.records.len(), 2, "one record for each field it moved");
        held.push(step.bytes());
        assert_eq!(document.history().1, 1, "one step");
        assert!(document.undo());
        assert_eq!(document.text(), before, "undoing it moves no other byte");
        assert!(!document.is_dirty());
        assert!(document.redo());
        assert_eq!(document.text(), after);
    }
    assert!(
        held[1] <= held[0] + 8,
        "a step holds {} bytes after 100 ticks and {} after 5,000",
        held[0],
        held[1]
    );
    // The two changes, the two values, and one look at the entry a rebase has to check.
    assert!(held[1] < 2048, "{}", held[1]);
}

#[test]
fn a_drag_there_and_back_is_no_step_and_leaves_redo() {
    let mut document = sample(LAID_OUT);
    assert_eq!(document.set("edit", &path("count"), &4_u32), Ok(Outcome::Changed));
    assert!(document.undo());
    document.begin_gesture("drag");
    assert_eq!(document.set("drag", &path("count"), &9_u32), Ok(Outcome::Changed));
    assert_eq!(document.undo_label(), Some("drag"), "mid-gesture it's what undo would take back");
    assert_eq!(document.set("drag", &path("count"), &3_u32), Ok(Outcome::Changed));
    assert_eq!(document.undo_label(), None, "nothing differs now");
    document.end_gesture();
    assert_eq!(document.text(), LAID_OUT);
    assert_eq!(document.undo_label(), None);
    assert_eq!(document.redo_label(), Some("edit"));
}

#[test]
fn a_gesture_goes_back_on_a_changed_file_as_one_step() {
    let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
    let before = document.text().to_owned();
    document.begin_gesture("drag");
    drag(&mut document, 50);
    document.end_gesture();
    let mine = document.text().to_owned();
    let theirs = format!("// a note\n{before}");
    let rebased = document.rebase_on_disk(theirs.clone());
    assert_eq!(rebased.refused, Vec::new());
    assert_eq!(rebased.kept, 2);
    assert_eq!(document.text(), format!("// a note\n{mine}"));
    assert!(document.undo());
    assert_eq!(document.text(), theirs, "one step, on top of the file's");
    assert!(document.undo());
    assert_eq!(document.text(), before);
}

#[test]
fn a_look_at_an_entry_outlives_a_field_dragged_back() {
    let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
    let (from, to) = (path("moves[0].hitboxes[0].from"), path("moves[0].hitboxes[0].to"));
    let was = document.text().to_owned();
    document.begin_gesture("drag");
    let start = document.value().expect("reads").moves[0].hitboxes[0].from;
    assert_eq!(document.set_text("drag", &from, &(start + 1).to_string()).map(|_| ()), Ok(()));
    assert_eq!(document.set_text("drag", &to, "90").map(|_| ()), Ok(()));
    assert_eq!(document.set_text("drag", &from, &start.to_string()).map(|_| ()), Ok(()));
    document.end_gesture();
    assert_eq!(document.last_step().expect("a step").records.len(), 1);
    // The entry at that place says something else on disk, so the drag doesn't go on it.
    let there = document.value().expect("reads").moves[0].hitboxes[0].clone();
    let spelled = format!("from: {start},");
    assert!(was.contains(&spelled), "the first window's start, as the file spells it: {there:?}");
    let moved = was.replacen(&spelled, &format!("from: {},", start + 3), 1);
    let on_disk = MoveSetDocument::parse(&moved).expect("reads");
    assert_ne!(on_disk.moves[0].hitboxes[0].from, start, "this test's file changed: pick its entry");
    let rebased = document.rebase_on_disk(moved);
    assert_eq!(rebased.kept, 0);
    assert_eq!(rebased.refused.len(), 1);
}

#[test]
fn a_rebase_says_when_it_cant_tell_whats_unsaved() {
    let mut document = sample(LAID_OUT);
    assert_eq!(document.set("edit", &path("count"), &4_u32), Ok(Outcome::Changed));
    document.mark_saved();
    assert!(document.undo());
    assert_eq!(document.set("edit", &path("count"), &5_u32), Ok(Outcome::Changed));
    assert_eq!(document.set("edit", &path("count"), &6_u32), Ok(Outcome::Changed));
    let mine = document.text().to_owned();
    let theirs = LAID_OUT.replace("count: 3,", "count: 9,");
    let rebased = document.rebase_on_disk(theirs.clone());
    assert!(rebased.beneath);
    assert_eq!(rebased.kept, 0);
    assert_eq!(document.text(), theirs);
    assert!(!document.is_dirty());
    assert!(document.undo());
    assert_eq!(document.text(), mine);
}

#[test]
fn dirty_follows_the_bytes_not_the_history() {
    let mut document = sample(LAID_OUT);
    assert!(!document.is_dirty());
    assert_eq!(document.set("edit", &path("count"), &4_u32), Ok(Outcome::Changed));
    assert!(document.is_dirty());
    document.mark_saved();
    assert!(!document.is_dirty());
    assert!(document.undo());
    assert!(document.is_dirty(), "the file holds the edit now");
    assert!(document.redo());
    assert!(!document.is_dirty());
}

// A file that changes under an edit.

#[test]
fn a_clean_document_takes_the_file_as_a_step_undo_can_take_back() {
    let mut document = sample(LAID_OUT);
    let theirs = LAID_OUT.replace("// How many.", "// How many, today.");
    let rebased = document.rebase_on_disk(theirs.clone());
    assert_eq!((rebased.kept, rebased.refused.len()), (0, 0));
    assert_eq!(document.text(), theirs);
    assert!(!document.is_dirty());
    assert!(document.undo());
    assert_eq!(document.text(), LAID_OUT);
}

#[test]
fn an_unsaved_record_survives_an_edit_elsewhere_in_the_file() {
    let mut document = sample(LAID_OUT);
    assert_eq!(document.set("mine", &path("count"), &4_u32), Ok(Outcome::Changed));
    assert_eq!(document.set("mine too", &path(r#"list["c"].at"#), &30_u32), Ok(Outcome::Changed));
    // Someone added a comment at the top, and changed another field: every offset moved.
    let theirs = format!("// A new first line.\n{}", LAID_OUT.replace("share: 0.25", "share: 0.5"));
    let rebased = document.rebase_on_disk(theirs.clone());
    assert_eq!((rebased.kept, rebased.refused.len()), (2, 0));
    let want = theirs.replace("count: 3,", "count: 4,").replace("\"c\", at: 3", "\"c\", at: 30");
    assert_eq!(document.text(), want);
    assert!(document.is_dirty(), "the edits are still unsaved");
    assert!(document.undo());
    assert!(document.undo());
    assert_eq!(document.text(), theirs);
    assert!(!document.is_dirty());
    assert!(document.undo());
    assert_eq!(document.text(), LAID_OUT);
}

#[test]
fn an_unsaved_record_is_refused_by_name_when_the_file_changed_its_property() {
    let mut document = sample(LAID_OUT);
    assert_eq!(document.set("mine", &path("count"), &4_u32), Ok(Outcome::Changed));
    assert_eq!(document.set("kept", &path("share"), &0.75_f32), Ok(Outcome::Changed));
    let theirs = LAID_OUT.replace("count: 3,", "count: 5,");
    let rebased = document.rebase_on_disk(theirs.clone());
    assert_eq!(rebased.kept, 1);
    assert_eq!(rebased.refused.len(), 1);
    assert_eq!(rebased.refused[0].record.path, path("count"));
    assert!(rebased.refused[0].why.contains("count"), "{}", rebased.refused[0].why);
    assert_eq!(document.text(), theirs.replace("share: 0.25", "share: 0.75"));
}

#[test]
fn the_file_changing_a_spelling_isnt_a_change_of_the_value() {
    let mut document = sample(LAID_OUT);
    assert_eq!(document.set("mine", &path("share"), &0.75_f32), Ok(Outcome::Changed));
    let theirs = LAID_OUT.replace("share: 0.25", "share: 2.5e-1");
    let rebased = document.rebase_on_disk(theirs.clone());
    assert_eq!((rebased.kept, rebased.refused.len()), (1, 0));
    assert_eq!(document.text(), theirs.replace("2.5e-1", "0.75"));
}

#[test]
fn a_named_entry_reordered_in_the_file_is_still_found_and_a_placed_one_isnt() {
    let mut document = sample(LAID_OUT);
    assert_eq!(document.set("by name", &path(r#"list["a"].at"#), &10_u32), Ok(Outcome::Changed));
    let theirs = LAID_OUT
        .replace("(name: \"a\", at: 1)", "@")
        .replace("(name: \"b\", at: 2)", "(name: \"a\", at: 1)")
        .replace('@', "(name: \"b\", at: 2)");
    let rebased = document.rebase_on_disk(theirs.clone());
    assert_eq!((rebased.kept, rebased.refused.len()), (1, 0));
    assert_eq!(document.text(), theirs.replace("\"a\", at: 1", "\"a\", at: 10"));
    // Entries that can go only by place: the same swap is refused, even of an equal value.
    let text = "(list: [(name: \"s\", at: 1), (name: \"s\", at: 1, ), (name: \"t\", at: 2)])";
    let mut document = sample(text);
    assert_eq!(document.set("by place", &path("list[0].at"), &10_u32), Ok(Outcome::Changed));
    let theirs = "(list: [(name: \"t\", at: 1), (name: \"s\", at: 1), (name: \"s\", at: 2)])";
    let rebased = document.rebase_on_disk(theirs.to_owned());
    assert_eq!((rebased.kept, rebased.refused.len()), (0, 1));
    assert_eq!(document.text(), theirs);
}

#[test]
fn a_file_that_doesnt_read_refuses_all_and_takes_no_edits_until_it_does() {
    let mut document = sample(LAID_OUT);
    assert_eq!(document.set("mine", &path("count"), &4_u32), Ok(Outcome::Changed));
    let halfway = LAID_OUT.replace("count: 3,", "count: ");
    let rebased = document.rebase_on_disk(halfway.clone());
    assert_eq!((rebased.kept, rebased.refused.len()), (0, 1));
    assert_eq!(document.text(), halfway);
    assert!(matches!(document.failure(), Some(ReadFailure::Parse(_))));
    assert!(matches!(document.set("no", &path("count"), &4_u32), Err(Refused::ReadOnly(_))));
    assert!(document.undo(), "the change on disk is a step");
    assert_eq!(document.text(), LAID_OUT);
    assert_eq!(document.failure(), None);
}

#[test]
fn a_new_entry_survives_unless_its_name_is_taken() {
    let entry = Entry { name: "new".to_owned(), at: 9 };
    let mut document = sample(LAID_OUT);
    assert_eq!(document.insert("add", &path("list"), 3, &entry), Ok(Outcome::Changed));
    let theirs = LAID_OUT.replace("(name: \"b\", at: 2)", "(name: \"b\", at: 22)");
    assert_eq!(document.rebase_on_disk(theirs).kept, 1);
    assert!(document.text().contains("(name: \"new\", at: 9)"));
    let mut document = sample(LAID_OUT);
    assert_eq!(document.insert("add", &path("list"), 3, &entry), Ok(Outcome::Changed));
    let theirs = LAID_OUT.replace("(name: \"b\", at: 2)", "(name: \"new\", at: 2)");
    let rebased = document.rebase_on_disk(theirs.clone());
    assert_eq!((rebased.kept, rebased.refused.len()), (0, 1));
    assert_eq!(document.text(), theirs);
}

/// A save over a file that changed is the document's holder's call: keeping what's here leaves the record
/// history alone and makes the file's bytes the new baseline for dirty.
#[test]
fn keeping_mine_leaves_the_text_and_the_next_save_is_the_word() {
    let mut document = sample(LAID_OUT);
    assert_eq!(document.set("mine", &path("count"), &4_u32), Ok(Outcome::Changed));
    let text = document.text().to_owned();
    document.keep_mine(LAID_OUT.replace("count: 3,", "count: 5,"));
    assert_eq!(document.text(), text);
    assert!(document.is_dirty());
    document.mark_saved();
    assert!(!document.is_dirty());
}

/// `bevy_reflect`'s own paths work on a derived document, so nothing here had to replace them for what they
/// can say: a name for an entry and a field's name in the file are what they can't.
#[test]
fn reflections_own_paths_reach_the_same_values() {
    let document = open("content/combat/player/player.moves.ron", kinds::moves());
    let root = document.value().expect("it reads");
    let health = root.reflect_path(".fighter.0.health").expect("a field in an Option");
    assert_eq!(health.try_downcast_ref::<u32>(), Some(&300));
    let ours =
        sapphire_document::walk::get(root.as_partial_reflect(), &path("fighter?.health")).expect("a path");
    assert_eq!(ours.try_downcast_ref::<u32>(), Some(&300));
}

/// A document's envelope is the file's: a path starts inside `data`, and a comment above it is safe.
#[test]
fn a_real_file_keeps_its_header_and_every_comment_through_an_edit_in_its_middle() {
    let text = read("content/combat/player/player.moves.ron");
    let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
    assert_eq!(document.set("edit", &path("fighter?.health"), &301_u32), Ok(Outcome::Changed));
    assert_eq!(document.text(), text.replace("health: 300,", "health: 301,"));
    let absent = path(r#"intents["light"].button"#);
    assert_eq!(document.set_text("button", &absent, "Some(Skill)"), Ok(Outcome::Changed));
    assert!(document.text().contains(r#"(name: "light", button: Skill, buffer: 14, priority: 1),"#));
}

#[test]
fn a_literal_goes_in_as_it_was_typed() {
    let mut document = sample(LAID_OUT);
    assert_eq!(document.set_literal("typed", &path("share"), "0.50"), Ok(Outcome::Changed));
    assert_eq!(document.text(), LAID_OUT.replace("share: 0.25", "share: 0.50"));
    // Another spelling of the same value is an edit, and undo gives the spelling back.
    assert_eq!(document.set_literal("typed", &path("share"), "0.5"), Ok(Outcome::Changed));
    assert_eq!(document.set_literal("typed", &path("share"), "0.5"), Ok(Outcome::Unchanged));
    assert!(document.undo());
    assert!(document.text().contains("share: 0.50"));
    assert!(document.undo());
    assert_eq!(document.text(), LAID_OUT);
    // What doesn't read as that type is refused, and nothing moved.
    assert!(matches!(
        document.set_literal("typed", &path("count"), "\"half"),
        Err(Refused::WrongType { .. })
    ));
    assert_eq!(document.text(), LAID_OUT);
    // A new entry too.
    let list = path("list");
    assert_eq!(document.insert_literal("add", &list, 3, "(name:\"d\",at:4)"), Ok(Outcome::Changed));
    assert!(document.text().contains("        (name:\"d\",at:4),\n"), "{}", document.text());
}

#[test]
fn taking_the_file_keeps_what_was_unsaved_in_the_history_beneath_it() {
    let mut document = sample(LAID_OUT);
    assert_eq!(document.set_text("mine", &path("count"), "4"), Ok(Outcome::Changed));
    let mine = document.text().to_owned();
    let theirs = LAID_OUT.replace("depth: 1", "depth: 2");
    document.take_disk("took the file's text from disk", theirs.clone());
    assert_eq!(document.text(), theirs);
    assert!(!document.is_dirty());
    assert_eq!(document.undo_label(), Some("took the file's text from disk"));
    assert!(document.undo());
    assert_eq!(document.text(), mine);
    assert!(document.is_dirty());
}

#[test]
fn text_that_stops_reading_keeps_its_last_reading_to_show() {
    let mut document = sample(LAID_OUT);
    let value = document.value().expect("reads").clone();
    document.take_disk("the file changed on disk", "(count: ".to_owned());
    assert!(document.value().is_none());
    assert_eq!(document.last_good(), Some(&value));
}

/// What a person would see as noise in a diff: a new value spells only what it has to.
#[test]
fn a_new_value_spells_only_what_it_has_to() {
    let mut document = sample("(\n    count: 3,\n)\n");
    let fresh = Inner { depth: 2, label: None };
    assert_eq!(document.set("inner", &path("inner"), &Some(fresh)), Ok(Outcome::Changed));
    assert_eq!(document.text(), "(\n    count: 3,\n    inner: (depth: 2),\n)\n", "no `Some(` in this file");
    assert!(document.undo());
    // All defaults: nothing left of it.
    assert_eq!(document.set("nested", &path("nested.depth"), &4_u32), Ok(Outcome::Changed));
    assert_eq!(document.text(), "(\n    count: 3,\n    nested: (depth: 4),\n)\n");
}

/// Every variant of the real documents' enums can be picked, including those holding an array or a tuple.
#[test]
fn a_variant_holding_an_array_can_be_picked() {
    let mut document = open("content/lookdev/character/character.animgraph.ron", kinds::graph());
    let before = document.text().to_owned();
    let motion = document
        .properties()
        .into_iter()
        .find(|property| {
            property.path.to_string().ends_with(".plays")
                && document.text()[property.written.clone().expect("written")].starts_with("Clip")
        })
        .expect("a state playing one clip");
    let info =
        sapphire_document::walk::get(document.value().expect("reads").as_partial_reflect(), &motion.path)
            .expect("there")
            .get_represented_type_info()
            .expect("a type");
    let blend = sapphire_document::blank::blank_variant(info, "Blend", document.registry()).expect("a blank");
    assert_eq!(document.set("pick", &motion.path, blend.as_ref()), Ok(Outcome::Changed));
    let step = document.last_step().expect("a step");
    assert!(step.change.inserted.contains("Blend"), "{}", step.change.inserted);
    assert!(document.undo());
    assert_eq!(document.text(), before);
}
