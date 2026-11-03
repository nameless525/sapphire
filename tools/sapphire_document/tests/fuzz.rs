//! The fuzz the design leans on, over the real documents: every property of every file takes edits, each of
//! which either lands and undoes to the same bytes or is refused with the text as it was, and random runs of
//! edits undo and redo to the same bytes.

#![allow(clippy::print_stdout, reason = "with --nocapture, its numbers are what it is run for")]

mod support;

use bevy_reflect::{FromReflect, GetTypeRegistration, PartialReflect, Reflect, ReflectRef, Typed};
use sapphire_document::text::comments;
use sapphire_document::walk::{self, PropertyKind};
use sapphire_document::{Document, Outcome, Property, PropertyPath, Refused, Schema, Step, kinds};
use sapphire_time::Stopwatch;
use support::{Random, Tally, another_value, open, value_of};

/// How many values each property is tried with: enough to go through a float's hard spellings.
const CHOICES_PER_PROPERTY: usize = 8;

/// Whether to try everything: `SAPPHIRE_DOCUMENT_FUZZ=full`, some 22,000 edits and minutes of a debug build.
/// By default each test takes every seventh property, so `cargo test` stays quick and still covers each kind.
fn full() -> bool {
    std::env::var_os("SAPPHIRE_DOCUMENT_FUZZ").is_some_and(|value| value == "full")
}

fn stride() -> usize {
    if full() { 1 } else { 7 }
}

/// How many edits a random run makes before undoing them all.
fn run_length() -> usize {
    if full() { 400 } else { 80 }
}

/// Everything that must hold of one edit that landed.
fn hold_to_the_edit<T>(document: &Document<T>, before: &str, property: &Property)
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let path = &property.path;
    let after = document.text();
    let step = document.last_step().expect("a step");
    let record = last_record(document);
    // A leaf or a variant changes inside its own bytes, and nothing around them moves. What's left out of
    // the file goes in with nothing removed.
    match (&property.written, property.kind) {
        (Some(span), PropertyKind::Leaf | PropertyKind::Enum) => {
            let changed = step.change.at..step.change.at + step.change.removed.len();
            assert!(
                span.start <= changed.start && changed.end <= span.end,
                "{path}: {changed:?} isn't inside {span:?}"
            );
        }
        (None, _) => assert_eq!(step.change.removed, "", "{path}: adding it took bytes away"),
        _ => {}
    }
    // A comment goes only with what's replaced around it, and the record says so.
    let (old, new) = (comments(before), comments(after));
    assert_eq!(old.len(), new.len() + record.comments_lost, "{path}: lost a comment and didn't say");
    if record.comments_lost == 0 {
        assert_eq!(old, new, "{path}: comments changed");
    }
}

fn last_record<T>(document: &Document<T>) -> sapphire_document::ChangeRecord
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    document.last_step().expect("a step").records.last().expect("a record").clone()
}

/// Each property, with each of its values, then undone: what the design's whole claim comes to.
fn every_property<T>(mut document: Document<T>, may_refuse: bool) -> Tally
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    assert_eq!(document.failure(), None, "a real file opens for editing");
    let original = document.text().to_owned();
    let mut tally = Tally::default();
    let properties = document.properties();
    assert!(!properties.is_empty());
    for property in properties.iter().step_by(stride()) {
        for choice in 0..CHOICES_PER_PROPERTY {
            let value = value_of(&document, property);
            let Some(new) = another_value(value, property.kind, document.registry(), choice) else {
                tally.skipped += 1;
                break;
            };
            match document.set("fuzz", &property.path, new.as_ref()) {
                Ok(Outcome::Changed) => {
                    tally.edits += 1;
                    if property.written.is_none() {
                        tally.wrote_absent += 1;
                    }
                    hold_to_the_edit(&document, &original, property);
                    let record = last_record(&document);
                    tally.rewrote_whole += usize::from(record.rewrote_whole);
                    assert!(document.is_dirty(), "{}", property.path);
                    assert!(document.undo());
                    assert_eq!(document.text(), original, "{}: undo moved a byte", property.path);
                    assert!(document.redo());
                    assert!(document.undo());
                    assert_eq!(document.text(), original, "{}: redo and undo moved a byte", property.path);
                    assert!(!document.is_dirty());
                }
                Ok(Outcome::Unchanged) => tally.unchanged += 1,
                Err(Refused::Document(why)) if may_refuse => {
                    tally.refused_by_document += 1;
                    assert_eq!(document.text(), original, "{}: {why}", property.path);
                }
                Err(refused) => panic!("{}: {refused}", property.path),
            }
        }
    }
    tally
}

/// Lists: an entry copied onto each end and into the middle, each entry taken out, each moved.
fn every_list<T>(mut document: Document<T>, may_refuse: bool) -> Tally
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let original = document.text().to_owned();
    let mut tally = Tally::default();
    let lists: Vec<_> =
        document.properties().into_iter().filter(|property| property.kind == PropertyKind::List).collect();
    let mut outcome =
        |document: &mut Document<T>, path: &PropertyPath, result: Result<Outcome, Refused>| match result {
            Ok(Outcome::Changed) => {
                tally.edits += 1;
                assert!(document.undo());
                assert_eq!(document.text(), original, "{path}: undo moved a byte");
            }
            Ok(Outcome::Unchanged) => tally.unchanged += 1,
            Err(Refused::Document(why)) if may_refuse => {
                tally.refused_by_document += 1;
                assert_eq!(document.text(), original, "{path}: {why}");
            }
            Err(refused) => panic!("{path}: {refused}"),
        };
    for list in lists.iter().step_by(stride()) {
        let length = list_length(value_of(&document, list));
        for at in 0..length {
            let entry = document.entry_path(&list.path, at).expect("an entry");
            let result = document.remove("fuzz", &entry);
            outcome(&mut document, &entry, result);
            let result = document.reorder("fuzz", &list.path, at, (at + 1) % length);
            outcome(&mut document, &list.path, result);
        }
        if length > 0 {
            for at in [0, length / 2, length] {
                let copy = {
                    let value = value_of(&document, list);
                    let ReflectRef::List(items) = value.reflect_ref() else { panic!("a list") };
                    items.get(0).expect("an entry").to_dynamic().expect("clones")
                };
                let result = document.insert("fuzz", &list.path, at, copy.as_ref());
                outcome(&mut document, &list.path, result);
            }
        }
    }
    tally
}

fn list_length(value: &dyn PartialReflect) -> usize {
    match walk::through(value).reflect_ref() {
        ReflectRef::List(items) => items.len(),
        _ => panic!("a list"),
    }
}

/// A run of edits on top of each other, then all of it undone, redone and undone again.
fn a_random_run<T>(mut document: Document<T>, seed: u64, may_refuse: bool) -> Tally
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let original = document.text().to_owned();
    let mut random = Random(seed);
    let mut tally = Tally::default();
    let mut texts = Vec::with_capacity(run_length());
    for _ in 0..run_length() {
        let properties = document.properties();
        let property = &properties[random.below(properties.len())];
        let before = document.text().to_owned();
        let result = match property.kind {
            PropertyKind::List => {
                let length = list_length(value_of(&document, property));
                match (length, random.below(3)) {
                    (0, _) => continue,
                    (_, 0) => {
                        let entry =
                            document.entry_path(&property.path, random.below(length)).expect("an entry");
                        document.remove("fuzz", &entry)
                    }
                    (_, 1) => {
                        document.reorder("fuzz", &property.path, random.below(length), random.below(length))
                    }
                    _ => {
                        let copy = {
                            let ReflectRef::List(items) = value_of(&document, property).reflect_ref() else {
                                panic!("a list")
                            };
                            items.get(random.below(length)).expect("an entry").to_dynamic().expect("clones")
                        };
                        document.insert("fuzz", &property.path, random.below(length + 1), copy.as_ref())
                    }
                }
            }
            kind => {
                let value = value_of(&document, property);
                let Some(new) = another_value(value, kind, document.registry(), random.below(16)) else {
                    continue;
                };
                document.set("fuzz", &property.path, new.as_ref())
            }
        };
        match result {
            Ok(Outcome::Changed) => {
                tally.edits += 1;
                texts.push(before);
            }
            Ok(Outcome::Unchanged) => tally.unchanged += 1,
            Err(Refused::Document(why)) if may_refuse || is_a_duplicate(&why) => {
                tally.refused_by_document += 1;
                assert_eq!(document.text(), before, "{why}");
            }
            Err(refused) => panic!("{}: {refused}", property.path),
        }
    }
    let last = document.text().to_owned();
    for before in texts.iter().rev() {
        assert!(document.undo());
        assert_eq!(document.text(), before, "an undo in a run moved a byte");
    }
    assert_eq!(document.text(), original);
    while document.redo() {}
    assert_eq!(document.text(), last, "redo got somewhere else");
    while document.undo() {}
    assert_eq!(document.text(), original);
    tally
}

/// A document's parser may reject two fields of one name, though nothing here writes them.
fn is_a_duplicate(why: &str) -> bool {
    why.contains("duplicate")
}

fn run<T>(name: &str, relative: &str, schema: Schema<T>, may_refuse: bool)
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let clock = Stopwatch::start();
    let each = every_property(open(relative, schema), may_refuse);
    let lists = every_list(open(relative, schema), may_refuse);
    let mut random = Tally::default();
    for seed in 1..=3 {
        let run = a_random_run(open(relative, schema), seed, may_refuse);
        random.edits += run.edits;
        random.refused_by_document += run.refused_by_document;
    }
    println!(
        "{name}: {} property edits ({} of what the file leaves out, {} rewrote a whole value, {} refused by the \
         document, {} unchanged, {} properties with nothing to try), {} list edits ({} refused), {} in random \
         runs ({} refused), {:.1} s",
        each.edits,
        each.wrote_absent,
        each.rewrote_whole,
        each.refused_by_document,
        each.unchanged,
        each.skipped,
        lists.edits,
        lists.refused_by_document,
        random.edits,
        random.refused_by_document,
        clock.elapsed().as_secs_f64(),
    );
    assert!(each.edits + lists.edits + random.edits > 200, "{name}: the fuzz did too little");
}

#[test]
fn every_property_of_the_players_moves_undoes_to_the_same_bytes() {
    run("player.moves.ron", "content/combat/player/player.moves.ron", kinds::moves(), false);
}

#[test]
fn every_property_of_the_warriors_moves_undoes_to_the_same_bytes() {
    run("warrior.moves.ron", "content/combat/enemy/warrior.moves.ron", kinds::moves(), false);
}

#[test]
fn every_property_of_the_characters_graph_undoes_to_the_same_bytes() {
    run(
        "character.animgraph.ron",
        "content/lookdev/character/character.animgraph.ron",
        kinds::graph(),
        false,
    );
}

#[test]
fn every_property_of_a_cue_undoes_to_the_same_bytes() {
    run("hit_heavy.cue.ron", "content/audio/hit_heavy.cue.ron", kinds::cue(), false);
}

#[test]
fn every_property_of_a_timeline_undoes_to_the_same_bytes() {
    run(
        "cutscene.timeline.ron",
        "crates/sapphire_timeline/test_content/cutscene.timeline.ron",
        kinds::timeline(),
        false,
    );
}

/// The library checks everything as it reads, so many values are refused here, each with the text as it
/// was, and that's its rule at work.
#[test]
fn every_property_of_the_default_effects_undoes_or_is_refused() {
    run("default_effects.ron", "crates/sapphire_vfx/default_effects.ron", kinds::effects(), true);
}

/// A path goes through its text and back.
#[test]
fn every_path_prints_and_parses_back() {
    let document = open("content/combat/player/player.moves.ron", kinds::moves());
    for property in document.properties() {
        let printed = property.path.to_string();
        assert_eq!(printed.parse::<PropertyPath>().expect(&printed), property.path);
    }
    let steps: Vec<_> = document.properties().into_iter().flat_map(|p| p.path.steps().to_vec()).collect();
    assert!(steps.iter().any(|step| matches!(step, Step::Key(_))));
    assert!(steps.iter().any(|step| matches!(step, Step::Some)));
}

/// The walker agrees with every document of these kinds in the repository, not just the five the fuzz runs
/// on.
#[test]
fn every_cue_and_graph_and_moves_file_opens_for_editing() {
    let root = support::repository_root().join("content");
    let mut seen = 0;
    let mut stack = vec![root];
    while let Some(folder) = stack.pop() {
        for entry in std::fs::read_dir(folder).expect("a folder") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().expect("a name").to_string_lossy().into_owned();
            let relative = path.strip_prefix(support::repository_root()).expect("inside");
            let relative = relative.to_string_lossy().into_owned();
            let failure = if name.ends_with(".cue.ron") {
                open(&relative, kinds::cue()).failure().cloned()
            } else if name.ends_with(".moves.ron") {
                open(&relative, kinds::moves()).failure().cloned()
            } else if name.ends_with(".animgraph.ron") {
                open(&relative, kinds::graph()).failure().cloned()
            } else {
                continue;
            };
            assert_eq!(failure, None, "{relative}");
            seen += 1;
        }
    }
    assert!(seen >= 15, "{seen}");
}

/// What one edit costs, for a person dragging: it prints, and asserts nothing, since this Mac's load moves it.
#[test]
fn what_an_edit_costs() {
    let mut document = open("content/combat/player/player.moves.ron", kinds::moves());
    let path: PropertyPath = "moves[0].startup".parse().expect("a path");
    let clock = Stopwatch::start();
    const EDITS: u32 = 200;
    for tick in 0..EDITS {
        let value = (tick % 40 + 1).to_string();
        let _ = document.set_text("edit", &path, &value);
    }
    let edit = clock.elapsed().as_secs_f64() / f64::from(EDITS);
    let clock = Stopwatch::start();
    let mut undone = 0;
    while document.undo() {
        undone += 1;
    }
    let undo = clock.elapsed().as_secs_f64() / f64::from(undone);
    println!(
        "player.moves.ron, {} bytes: {:.2} ms an edit, {:.2} ms an undo",
        document.text().len(),
        edit * 1000.0,
        undo * 1000.0
    );
}

/// A copy of each list's first entry written new, as a value: it comes out spelling no field its type reads
/// the same without, in all five documents. A list of `None`s or a required `None` would stay, and none does.
fn entries_written_new<T>(mut document: Document<T>, may_refuse: bool)
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let lists: Vec<_> =
        document.properties().into_iter().filter(|property| property.kind == PropertyKind::List).collect();
    let mut wrote = 0;
    for list in &lists {
        let length = list_length(value_of(&document, list));
        if length == 0 {
            continue;
        }
        let copy = {
            let value = value_of(&document, list);
            let ReflectRef::List(items) = value.reflect_ref() else { panic!("a list") };
            items.get(0).expect("an entry").to_dynamic().expect("clones")
        };
        match document.insert("copy", &list.path, length, copy.as_ref()) {
            Ok(Outcome::Changed) => {
                wrote += 1;
                let inserted = &document.last_step().expect("a step").change.inserted;
                assert!(!inserted.contains("None"), "{}: {inserted}", list.path);
                assert!(!inserted.contains(": []"), "{}: {inserted}", list.path);
                assert!(document.undo());
            }
            Ok(Outcome::Unchanged) => {}
            Err(Refused::Document(_)) if may_refuse => {}
            Err(refused) => panic!("{}: {refused}", list.path),
        }
    }
    assert!(wrote > 0);
}

#[test]
fn entries_written_new_spell_only_what_they_have_to() {
    entries_written_new(open("content/combat/player/player.moves.ron", kinds::moves()), false);
    entries_written_new(open("content/combat/enemy/warrior.moves.ron", kinds::moves()), false);
    entries_written_new(open("content/lookdev/character/character.animgraph.ron", kinds::graph()), false);
    entries_written_new(open("crates/sapphire_vfx/default_effects.ron", kinds::effects()), true);
}
