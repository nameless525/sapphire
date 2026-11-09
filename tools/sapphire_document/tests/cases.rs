//! The hard cases, one at a time, on small documents written to show them, and on the real ones where that
//! says more: a field the file leaves out, an `Option` coming and going, a list growing and shrinking, a
//! variant changing shape, a float's spelling, what a document refuses, what an entry's identity survives,
//! and a file changing under an edit.

mod support;

use std::sync::Arc;

use bevy_reflect::{GetPath, PartialReflect, Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_document::{
    Document, Outcome, PropertyKind, PropertyPath, ReadFailure, Refused, Schema, Step, kinds, registry_of,
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

/// E5's panel: a key of a real timeline moves by its property path, the file around it stays, and what `check`
/// says of a key out of order lands to be fixed rather than being refused.
#[test]
fn a_timeline_is_a_document_like_any_other() {
    let mut timeline = open("crates/sapphire_timeline/test_content/cutscene.timeline.ron", kinds::timeline());
    assert_eq!(timeline.failure(), None);
    assert_eq!(timeline.issues(), Vec::<String>::new());
    let text = timeline.text().to_owned();
    let eye = path("camera?.keys[1].eye[1]");
    assert_eq!(timeline.set("raise the eye", &eye, &2.5_f32), Ok(Outcome::Changed));
    assert_eq!(timeline.text(), text.replace("(1.5, 1.6, -3.0)", "(1.5, 2.5, -3.0)"));
    assert!(timeline.undo());
    assert_eq!(timeline.text(), text);

    let at = path("camera?.keys[1].at");
    assert_eq!(timeline.set("too late", &at, &9.0_f32), Ok(Outcome::Changed));
    let issues = timeline.issues();
    assert!(issues.iter().any(|issue| issue.contains("camera.keys[1]")), "{issues:?}");
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

/// An edit big enough that a few of them reach a small cap: the whole of the first move's name, replaced.
fn long_edit(document: &mut Document<MoveSetDocument>, to: u32) {
    let at = path("moves[0].startup");
    assert_eq!(document.set_text("edit", &at, &to.to_string()), Ok(Outcome::Changed));
}

/// The cap, at its floor and past it: the oldest steps go whole, an undo there does nothing and says why, and
/// what the document holds is never touched by it.
#[test]
fn history_past_its_cap_lets_the_oldest_go_and_says_so() {
    let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
    let start = document.value().expect("reads").moves[0].startup;
    assert_eq!(document.why_no_undo(), None, "nothing has been let go");
    long_edit(&mut document, start + 1);
    let one_step = document.history().0;
    assert!(one_step > 0);
    document.set_history_limit(one_step * 4);
    let mut texts = vec![document.text().to_owned()];
    for offset in 2..40 {
        long_edit(&mut document, start + offset);
        texts.push(document.text().to_owned());
        assert!(
            document.history().0 <= one_step * 5,
            "{} past a cap of {}",
            document.history().0,
            one_step * 4
        );
    }
    let (_, steps) = document.history();
    assert!((3..=4).contains(&steps), "{steps} steps fit four of them");
    assert_eq!(document.forgotten(), 39 - steps);

    // Undo goes back exactly as far as it holds, each step to the bytes it left.
    let newest = texts.len() - 1;
    for back in 1..=steps {
        assert!(document.undo());
        if back < steps {
            assert_eq!(document.text(), texts[newest - back]);
        }
    }
    let floor = document.text().to_owned();
    assert_eq!(document.undo_label(), None);
    let said = document.why_no_undo().expect("it says why");
    assert!(
        said.contains("nothing older to undo") && said.contains(&document.forgotten().to_string()),
        "{said}"
    );

    // At the floor undo does nothing, and it's still an edit away from the file: dirty is the bytes.
    assert!(!document.undo());
    assert_eq!(document.text(), floor);
    assert!(document.is_dirty(), "the file holds what the oldest edits changed");

    // Redo brings it all back, and an edit from the floor is an ordinary edit.
    for _ in 0..steps {
        assert!(document.redo());
    }
    assert_eq!(document.text(), texts[newest]);
    for _ in 0..steps {
        assert!(document.undo());
    }
    long_edit(&mut document, start + 100);
    assert_eq!(document.why_no_undo(), None, "there's an edit to undo again");
    assert!(document.undo());
    assert_eq!(document.text(), floor);
    assert!(document.why_no_undo().is_some());
}

/// The newest step stays however large it is, or an edit couldn't be undone at all.
#[test]
fn the_newest_step_stays_whatever_the_cap() {
    let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
    let before = document.text().to_owned();
    document.set_history_limit(1);
    let start = document.value().expect("reads").moves[0].startup;
    long_edit(&mut document, start + 1);
    assert_eq!(document.forgotten(), 0);
    assert!(document.undo());
    assert_eq!(document.text(), before);
    assert!(!document.is_dirty());
    assert!(document.redo());
    long_edit(&mut document, start + 2);
    assert_eq!(document.forgotten(), 1);
    assert_eq!(document.history().1, 1);
    assert!(document.undo());
    assert!(!document.undo());
    assert_ne!(document.text(), before, "the first edit went past the cap");
    assert!(document.why_no_undo().expect("it says").contains("oldest 1 edit was"));
}

/// A save that history has let go of can't be got back to, so a rebase has no way to say what's unsaved: it
/// lays nothing back, and says the edits are beneath the file's text, in undo, or gone past the cap.
#[test]
fn a_rebase_past_a_save_the_cap_let_go_says_so() {
    let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
    let start = document.value().expect("reads").moves[0].startup;
    document.set_history_limit(1);
    long_edit(&mut document, start + 1);
    long_edit(&mut document, start + 2);
    assert!(document.is_dirty());
    let on_disk = document.disk().to_vec();
    let elsewhere = String::from_utf8(on_disk).expect("text").replacen("// ", "//  ", 1);
    let rebased = document.rebase_on_disk(elsewhere.clone());
    assert_eq!(rebased.kept, 0);
    assert!(rebased.refused.is_empty());
    assert!(rebased.beneath, "it says the edits weren't laid back");
    assert_eq!(document.text(), elsewhere);
    assert!(!document.is_dirty());
}

/// A take of a whole file counts against the cap like any other step, and past it undo says so.
#[test]
fn a_take_from_disk_counts_against_the_cap() {
    let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
    document.set_history_limit(1);
    let text = document.text().to_owned();
    document.take_disk("first", text.replacen("// ", "//  ", 1));
    document.take_disk("second", text.replacen("// ", "//   ", 1));
    assert_eq!(document.forgotten(), 1);
    assert_eq!(document.undo_label(), Some("second"));
    assert!(!document.is_dirty());
}

/// Edits made together are one step or none: a failure halfway takes back what went before it, by itself or
/// inside a gesture, and the gesture carries on.
#[test]
fn edits_made_together_land_together_or_not_at_all() {
    let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
    let before = document.text().to_owned();
    let startup = path("moves[0].startup");
    let active = path("moves[0].active");
    let start = document.value().expect("reads").moves[0].startup;

    let refused = document.together("both", |document| {
        document.set_text("x", &startup, &(start + 1).to_string())?;
        document.set_text("x", &active, "not a number")
    });
    assert!(refused.is_err());
    assert_eq!(document.text(), before);
    assert_eq!(document.history().1, 0, "no step");
    assert!(!document.in_gesture());

    let done = document.together("both", |document| {
        document.set_text("x", &startup, &(start + 1).to_string())?;
        document.set_text("x", &active, "9")
    });
    assert_eq!(done, Ok(Outcome::Changed));
    assert_eq!(document.history().1, 1, "one step");
    assert_eq!(document.undo_label(), Some("both"));
    let both = document.text().to_owned();

    document.begin_gesture("drag");
    document.set_text("x", &active, "10").expect("lands");
    let held = document.text().to_owned();
    let refused = document.together("ignored", |document| {
        document.set_text("x", &startup, &(start + 2).to_string())?;
        document.set_text("x", &active, "not a number")
    });
    assert!(refused.is_err());
    assert_eq!(document.text(), held, "what the gesture had stays");
    assert!(document.in_gesture());
    document.end_gesture();
    assert_eq!(document.last_step().expect("a step").records.len(), 1, "what failed left no record");
    assert_eq!(document.undo_label(), Some("drag"));
    assert!(document.undo());
    assert_eq!(document.text(), both);
}

/// What a panel reads of one entry is what the whole walk says of it.
#[test]
fn properties_under_a_path_are_the_whole_walks() {
    let document = open("content/combat/player/player.moves.ron", kinds::moves());
    let entry = document.entry_path(&path("moves"), 0).expect("a move");
    let hit = entry.join(Step::Field("hitboxes".to_owned()));
    let hit = document.entry_path(&hit, 0).expect("a hitbox");
    let under = document.properties_under(&hit).expect("there");
    let prefix = hit.to_string();
    let walked: Vec<_> = document
        .properties()
        .into_iter()
        .filter(|property| property.path.to_string().starts_with(&prefix))
        .collect();
    assert!(under.len() > 5, "{}", under.len());
    assert_eq!(under, walked);
    assert!(document.properties_under(&path("moves[0].nothing")).is_err());
}

#[test]
fn a_new_entry_and_a_picked_variant_come_from_the_type() {
    let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
    let before = document.text().to_owned();
    let list = path("moves[0].parry");
    let count = document.value().expect("reads").moves[0].parry.len();
    assert_eq!(document.insert_blank("add", &list), Ok(Outcome::Changed));
    assert_eq!(document.value().expect("reads").moves[0].parry.len(), count + 1);
    assert!(document.undo());
    assert_eq!(document.text(), before);

    let shape = document
        .properties()
        .into_iter()
        .find(|property| property.path.to_string().ends_with(".shape") && property.kind == PropertyKind::Enum)
        .expect("a hit volume");
    let value =
        sapphire_document::walk::get(document.value().expect("reads").as_partial_reflect(), &shape.path)
            .expect("there");
    let now = sapphire_document::walk::variant(value).expect("a variant").to_owned();
    assert_eq!(document.pick_variant("same", &shape.path, &now), Ok(Outcome::Unchanged));
    let other = if now == "Sphere" { "Capsule" } else { "Sphere" };
    assert_eq!(document.pick_variant("pick", &shape.path, other), Ok(Outcome::Changed));
    assert!(document.undo());
    assert!(document.pick_variant("no such", &shape.path, "Nothing").is_err());
    assert_eq!(document.text(), before);
}

#[test]
fn a_refused_owner_also_refuses_the_gestures_dependent_records() {
    let original = r#"(count: 1, list: [(name: "same", at: 1), (name: "same", at: 2)])"#;
    let mut document = sample(original);
    document.begin_gesture("both fields and an independent edit");
    document.set_text("at", &path("list[0].at"), "9").expect("at");
    document.set_text("name", &path("list[0].name"), r#""mine""#).expect("name");
    document.set_text("count", &path("count"), "5").expect("count");
    document.end_gesture();
    let disk = r#"(count: 1, list: [(name: "external", at: 7), (name: "same", at: 2)])"#;
    let rebased = document.rebase_on_disk(disk.to_owned());
    assert_eq!((rebased.kept, rebased.refused.len()), (1, 2));
    assert_eq!(document.text(), disk.replace("count: 1", "count: 5"));
    assert!(document.undo());
    assert_eq!(document.text(), disk);
    assert!(document.undo());
    assert_eq!(document.text(), original);
}

#[test]
fn an_omitted_list_can_be_added_to_an_empty_parent_without_losing_comments() {
    for original in ["()", "(/* keep */)", "(\n    // keep\n)\n"] {
        let mut document = sample(original);
        let entry = Entry { name: "new".to_owned(), at: 9 };
        assert_eq!(document.insert("add", &path("list"), 0, &entry), Ok(Outcome::Changed));
        assert_eq!(document.value().expect("reads").list, [entry]);
        assert_eq!(
            sapphire_document::text::comments(document.text()),
            sapphire_document::text::comments(original)
        );
        let added = document.text().to_owned();
        assert!(document.undo());
        assert_eq!(document.text(), original);
        assert!(document.redo());
        assert_eq!(document.text(), added);
    }
    let mut document = sample("()");
    assert_eq!(
        document.insert_literal("literal", &path("list"), 0, r#"(name: "new", at: 0x9)"#),
        Ok(Outcome::Changed)
    );
    assert_eq!(document.text(), r#"(list: [(name: "new", at: 0x9)])"#);
}

#[test]
fn malformed_dynamic_entries_are_refused_before_concrete_list_conversion() {
    use bevy_reflect::structs::DynamicStruct;

    let mut document = sample(LAID_OUT);
    let mut wrong = DynamicStruct::default();
    wrong.insert("name", 7_u32);
    wrong.insert("at", 3_u32);
    assert!(matches!(document.insert("bad", &path("list"), 0, &wrong), Err(Refused::WrongType { .. })));
    assert_eq!(document.text(), LAID_OUT);
    assert_eq!(document.history().1, 0);
    let mut right = DynamicStruct::default();
    right.insert("name", "dynamic".to_owned());
    right.insert("at", 3_u32);
    assert_eq!(document.insert("good", &path("list"), 0, &right), Ok(Outcome::Changed));
    assert_eq!(document.value().expect("reads").list[0], Entry { name: "dynamic".to_owned(), at: 3 });
    assert!(document.undo());
    assert_eq!(document.text(), LAID_OUT);
}

#[test]
fn lowering_the_history_cap_trims_the_farthest_redo_and_keeps_a_valid_chain() {
    for undo_count in [1, 3] {
        let mut document = sample("(count: 0)");
        for value in 1..=3_u32 {
            document.set("edit", &path("count"), &value).expect("edit");
        }
        document.mark_saved();
        for _ in 0..undo_count {
            assert!(document.undo());
        }
        let before = document.text().to_owned();
        let dirty = document.is_dirty();
        document.set_history_limit(1);
        assert_eq!(document.history().1, 1, "the single-step floor also applies to redo");
        assert_eq!(document.text(), before);
        assert_eq!(document.is_dirty(), dirty);
        if undo_count == 3 {
            assert!(document.redo());
            assert_eq!(document.text(), "(count: 1)");
            assert!(!document.redo());
            assert!(document.undo());
            assert_eq!(document.text(), "(count: 0)");
        } else {
            assert!(document.undo());
            assert_eq!(document.text(), "(count: 1)");
            assert!(document.redo());
            assert_eq!(document.text(), before);
            assert!(!document.redo());
        }
    }
}

#[test]
fn nested_transactions_roll_back_to_their_own_entry_point_and_keep_redo() {
    let mut document = sample("(count: 1, share: 0.25)");
    document.set_text("old", &path("count"), "2").expect("old");
    assert!(document.undo());
    let before = document.text().to_owned();
    let result: Result<(), Refused> = document.together("outer", |document| {
        document.set_text("outer", &path("count"), "3")?;
        let failed: Result<(), Refused> = document.together("inner", |document| {
            document.set_text("inner", &path("share"), "0.75")?;
            document.set_text("bad", &path("count"), "bad")?;
            Ok(())
        });
        assert!(failed.is_err());
        assert_eq!(document.text(), "(count: 3, share: 0.25)");
        Err(Refused::Document("abort the outer edit".to_owned()))
    });
    assert!(result.is_err());
    assert_eq!(document.text(), before);
    assert_eq!(document.undo_label(), None);
    assert_eq!(document.redo_label(), Some("old"));
    assert!(document.redo());
    assert_eq!(document.text(), "(count: 2, share: 0.25)");
}

#[test]
fn comments_after_a_quote_character_are_counted_when_their_value_is_replaced() {
    #[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
    #[reflect(Serialize, Deserialize)]
    struct Characters {
        letter: char,
        count: u32,
    }
    let schema = Schema {
        parse: |text| ron::from_str::<Characters>(text).map_err(|error| error.to_string()),
        check: |_| Vec::new(),
        importer: None,
    };
    let text = "(letter: '\"', // a real comment\n count: 1)";
    let mut document = Document::open(text.to_owned(), schema, Arc::new(registry_of::<Characters>()));
    assert_eq!(document.failure(), None);
    assert_eq!(
        document.set_literal("replace", &PropertyPath::root(), "(letter: 'x', count: 2)"),
        Ok(Outcome::Changed)
    );
    assert_eq!(document.last_step().expect("step").records[0].comments_lost, 1);
    assert!(document.undo());
    assert_eq!(document.text(), text);
}

#[test]
fn removing_an_inline_last_entry_preserves_comments_around_its_separator() {
    for (text, entry) in [
        (r#"(list: [(name: "a"), /* keep */ (name: "b")])"#, "b"),
        (r#"(list: [(name: "a") /* keep */, (name: "b")])"#, "a"),
        (r#"(list: [(name: "a") /* keep */, (name: "b")])"#, "b"),
    ] {
        let mut document = sample(text);
        document.remove("remove", &path(&format!("list[\"{entry}\"]"))).expect("remove");
        assert_eq!(sapphire_document::text::comments(document.text()), ["/* keep */"]);
        assert_eq!(document.last_step().expect("step").records[0].comments_lost, 0);
        assert_eq!(document.value().expect("reads").list.len(), 1);
        assert!(document.undo());
        assert_eq!(document.text(), text);
    }
}

#[test]
fn writing_new_values_strips_only_parser_defaults_and_keeps_signed_zero() {
    use sapphire_document::write::{Style, styled};
    #[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
    #[reflect(Serialize, Deserialize)]
    struct Defaults {
        required: u32,
        #[serde(default)]
        zero: f32,
        #[serde(default)]
        list: Vec<u32>,
        #[serde(default)]
        optional: Option<String>,
    }
    let registry = registry_of::<Defaults>();
    let style = Style { implicit_some: true, newline: "\n", indent: "" };
    let value = Defaults { required: 7, zero: -0.0, list: Vec::new(), optional: None };
    assert_eq!(styled(&value, &registry, style).expect("write"), "(required: 7, zero: -0.0)");
    let value = Defaults { zero: 0.0, ..value };
    assert_eq!(styled(&value, &registry, style).expect("write"), "(required: 7)");
}

#[test]
fn option_style_comes_from_values_instead_of_comments_or_strings() {
    for text in [r#"(/* Some( */ count: 1)"#, r#"(note: "Some(", count: 1)"#] {
        let mut document = sample(text);
        document.set_text("inner", &path("inner"), "Some((depth: 3))").expect("edit");
        assert!(document.text().contains("inner: (depth: 3)"), "{}", document.text());
        assert!(document.undo());
        assert_eq!(document.text(), text);
    }
    let mut document = sample(r#"(note: Some ("explicit"), count: 1)"#);
    document.set_text("inner", &path("inner"), "Some((depth: 3))").expect("edit");
    assert!(document.text().contains("inner: Some((depth: 3))"), "{}", document.text());
}

#[test]
fn arbitrary_invalid_public_splice_ranges_are_refused() {
    use sapphire_document::layout::{Splice, apply};
    use std::ops::Range;
    for range in [1..1, Range { start: 2, end: 1 }, 0..4] {
        assert_eq!(apply("éx", &[Splice { range, text: "new".to_owned() }]), None);
    }
}

#[test]
fn tuple_and_array_list_entries_keep_their_fallible_conversion() {
    #[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
    #[reflect(Serialize, Deserialize)]
    struct Lists {
        tuples: Vec<(u32, String)>,
        arrays: Vec<[u32; 2]>,
    }
    let schema = Schema {
        parse: |text| ron::from_str::<Lists>(text).map_err(|error| error.to_string()),
        check: |_| Vec::new(),
        importer: None,
    };
    let original = "(tuples: [], arrays: [])";
    let mut document = Document::open(original.to_owned(), schema, Arc::new(registry_of::<Lists>()));
    assert_eq!(
        document.insert("tuple", &path("tuples"), 0, &(7_u32, "text".to_owned())),
        Ok(Outcome::Changed)
    );
    assert_eq!(document.value().expect("reads").tuples, [(7, "text".to_owned())]);
    assert!(document.undo());
    assert_eq!(document.text(), original);
    assert_eq!(document.insert("array", &path("arrays"), 0, &[1_u32, 2_u32]), Ok(Outcome::Changed));
    assert_eq!(document.value().expect("reads").arrays, [[1, 2]]);
    assert!(document.undo());
    let mut malformed = bevy_reflect::tuple::DynamicTuple::default();
    malformed.insert("wrong".to_owned());
    malformed.insert(9_u32);
    assert!(matches!(document.insert("bad", &path("tuples"), 0, &malformed), Err(Refused::WrongType { .. })));
    assert_eq!(document.text(), original);
}

#[test]
fn nan_leaf_agreement_does_not_bypass_the_final_typed_equality_check() {
    let text = "(count: 1, share: NaN)";
    let mut document = sample(text);
    assert_eq!(document.failure(), None);
    assert!(document.value().expect("reads").share.is_nan());
    assert!(matches!(document.set_text("count", &path("count"), "2"), Err(Refused::Disagrees { .. })));
    assert_eq!(document.text(), text);
    assert_eq!(document.history().1, 0);
}

#[test]
fn grouped_history_counts_both_text_directions_and_records_in_redo_too() {
    let original = format!("(count: 1, note: {:?})", "old".repeat(400));
    let mut document = sample(&original);
    document.begin_gesture("grouped edit");
    document.set_text("note", &path("note"), &format!("Some({:?})", "new".repeat(700))).expect("note");
    document.set_text("count", &path("count"), "9").expect("count");
    document.end_gesture();
    let step = document.last_step().expect("step");
    let strings = step.label.len()
        + step.change.removed.len()
        + step.change.inserted.len()
        + step
            .records
            .iter()
            .map(|record| {
                record.old.as_ref().map_or(0, String::len) + record.new.as_ref().map_or(0, String::len)
            })
            .sum::<usize>();
    let held = step.bytes();
    assert!(held >= strings, "both byte directions and all record values count");
    assert_eq!(document.history(), (held, 1));
    assert!(document.undo());
    assert_eq!(document.text(), original);
    assert_eq!(document.history(), (held, 1), "undo changes ownership, not retained memory");
    document.set_history_limit(1);
    assert_eq!(document.history(), (held, 1), "a single oversized redo is the explicit floor");
    assert!(document.redo());
    assert_eq!(document.history(), (held, 1));
}

#[test]
fn a_refused_named_insert_does_not_refuse_an_independent_insert_in_the_same_list() {
    let original = "(list: [])";
    let mut document = sample(original);
    document.begin_gesture("two independent entries");
    for name in ["conflict", "free"] {
        document.insert("add", &path("list"), 0, &Entry { name: name.to_owned(), at: 9 }).expect("insert");
    }
    document.end_gesture();
    let disk = r#"(list: [(name: "conflict", at: 7)])"#;
    let rebased = document.rebase_on_disk(disk.to_owned());
    assert_eq!((rebased.kept, rebased.refused.len()), (1, 1));
    let entries = &document.value().expect("reads").list;
    assert_eq!(
        entries,
        &[Entry { name: "free".to_owned(), at: 9 }, Entry { name: "conflict".to_owned(), at: 7 }]
    );
    assert!(document.undo());
    assert_eq!(document.text(), disk);
}

/// A screen's own rules are its `issues`, each naming its field, and a person can go on editing past one.
#[test]
fn a_screen_edits_as_a_document_and_reports_its_rules() {
    let mut screen = open("content/ui/pause.ui.ron", kinds::ui());
    let text = screen.text().to_owned();
    assert_eq!(screen.issues(), Vec::<String>::new());
    let parent = screen.entry_path(&path("nodes"), 1).expect("a node").join(Step::Field("parent".to_owned()));
    assert_eq!(screen.set("orphan", &parent, &Some("nobody".to_owned())), Ok(Outcome::Changed));
    let issues = screen.issues();
    assert!(issues.iter().any(|issue| issue.contains("parent") && issue.contains("nobody")), "{issues:?}");
    assert!(screen.undo());
    assert_eq!(screen.text(), text);
}
