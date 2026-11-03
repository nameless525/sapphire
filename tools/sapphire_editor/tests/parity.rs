//! What T3's edits write and what `sapphire_document` writes for the same edit, byte for byte, over every
//! entry of the real documents. E2 replaces the first with the second, and this is what says it can.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use sapphire_document::{Document, Outcome, PropertyPath, kinds, registry_of};
use sapphire_editor::edits::{EntryRef, MovesText, Phases, Track};
use sapphire_editor::text::{self, Node};
use sapphire_moves::MoveSetDocument;

const FILES: [&str; 2] = ["content/combat/player/player.moves.ron", "content/combat/enemy/warrior.moves.ron"];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the repository")
}

fn path(text: &str) -> PropertyPath {
    text.parse().expect(text)
}

fn document(text: &str) -> Document<MoveSetDocument> {
    Document::open(text.to_owned(), kinds::moves(), Arc::new(registry_of::<MoveSetDocument>()))
}

/// Every entry of every track but root keys, which aren't a list of the move.
fn entries(text: &MovesText) -> Vec<EntryRef> {
    let moves = &text.document().expect("reads").moves;
    let mut entries = Vec::new();
    for (mv, one) in moves.iter().enumerate() {
        for track in Track::ALL.into_iter().filter(|track| *track != Track::RootKeys) {
            entries.extend((0..track.spans(one).len()).map(|index| EntryRef { mv, track, index }));
        }
    }
    entries
}

fn at(entry: EntryRef) -> String {
    format!("moves[{}].{}[{}]", entry.mv, entry.track.key(), entry.index)
}

/// Undo gives back the bytes, after each.
fn hold_to(document: &mut Document<MoveSetDocument>, original: &str, t3: &str, what: &str) {
    assert_eq!(document.text(), t3, "{what}");
    assert!(document.undo(), "{what}");
    assert_eq!(document.text(), original, "{what}: undo");
}

/// Every window and instant, moved a tick later, as a drag would.
#[test]
fn a_drag_writes_the_same_bytes() {
    let mut compared = 0;
    for file in FILES {
        let original = fs::read_to_string(root().join(file)).expect(file);
        let reading = MovesText::new(original.clone());
        for entry in entries(&reading) {
            let mut t3 = MovesText::new(original.clone());
            let mut new = document(&original);
            let span = entry.track.spans(&t3.document().expect("reads").moves[entry.mv])[entry.index];
            new.begin_gesture("drag");
            if entry.track.is_instant() {
                t3.set_instant(entry, span.from + 1).expect("T3");
                let tick = (span.from + 1).to_string();
                assert_eq!(
                    new.set_text("drag", &path(&format!("{}.at", at(entry))), &tick),
                    Ok(Outcome::Changed)
                );
            } else {
                t3.set_window(entry, span.from + 1, span.to + 1).expect("T3");
                for (name, value) in [("to", span.to + 1), ("from", span.from + 1)] {
                    let field = path(&format!("{}.{name}", at(entry)));
                    assert_eq!(new.set_text("drag", &field, &value.to_string()), Ok(Outcome::Changed));
                }
            }
            new.end_gesture();
            hold_to(&mut new, &original, t3.text(), &at(entry));
            compared += 1;
        }
    }
    assert!(compared > 50, "{compared}");
}

#[test]
fn a_moves_phases_write_the_same_bytes() {
    for file in FILES {
        let original = fs::read_to_string(root().join(file)).expect(file);
        let moves = MovesText::new(original.clone()).document().expect("reads").moves.len();
        for mv in 0..moves {
            let mut t3 = MovesText::new(original.clone());
            let mut new = document(&original);
            let phases = Phases::of(&t3.document().expect("reads").moves[mv]);
            let later = Phases {
                startup: phases.startup + 1,
                active: phases.active + 2,
                recovery: phases.recovery + 3,
            };
            t3.set_phases(mv, later).expect("T3");
            new.begin_gesture("phases");
            for (name, value) in
                [("startup", later.startup), ("active", later.active), ("recovery", later.recovery)]
            {
                let field = path(&format!("moves[{mv}].{name}"));
                assert_eq!(new.set_text("phases", &field, &value.to_string()), Ok(Outcome::Changed));
            }
            new.end_gesture();
            hold_to(&mut new, &original, t3.text(), &format!("move {mv}"));
        }
    }
}

/// Every number an entry writes, typed over with a literal.
#[test]
fn a_typed_number_writes_the_same_bytes() {
    let mut compared = 0;
    for file in FILES {
        let original = fs::read_to_string(root().join(file)).expect(file);
        let reading = MovesText::new(original.clone());
        let opened = document(&original);
        let properties = opened.properties();
        for entry in entries(&reading) {
            // The same numbers, by where the file writes them: T3 goes by the text's shape, and this by the
            // value's. Both say the numbers in the file's order.
            let prefix = format!("{}.", entry_path_by_name(&opened, entry));
            let mut mine: Vec<_> = properties
                .iter()
                .filter(|property| property.path.to_string().starts_with(&prefix))
                .filter_map(|property| {
                    property.written.clone().map(|written| (written, property.path.clone()))
                })
                .filter(|(written, _)| original[written.clone()].parse::<f64>().is_ok())
                .filter(|(_, path)| {
                    !matches!(path.to_string().rsplit('.').next(), Some("from" | "to" | "at"))
                })
                .collect();
            mine.sort_by_key(|(written, _)| written.start);
            let in_file_order = reading.entry_values(entry).expect("values");
            let in_file_order: Vec<_> = in_file_order.iter().filter(|value| value.is_number()).collect();
            assert_eq!(in_file_order.len(), mine.len(), "{}", at(entry));
            for (value, (_, property)) in in_file_order.iter().zip(&mine) {
                let literal = if value.literal.contains('.') { "0.75".to_owned() } else { "7".to_owned() };
                if literal == value.literal {
                    continue;
                }
                let mut t3 = MovesText::new(original.clone());
                let mut new = document(&original);
                t3.set_entry_value(entry, &value.path, &literal).expect("T3");
                assert_eq!(new.set_literal("typed", property, &literal), Ok(Outcome::Changed), "{property}");
                hold_to(&mut new, &original, t3.text(), &property.to_string());
                compared += 1;
            }
        }
    }
    assert!(compared > 100, "{compared}");
}

/// How a property's path calls an entry: by its name if it has one of its own.
fn entry_path_by_name(document: &Document<MoveSetDocument>, entry: EntryRef) -> String {
    let moves = path("moves");
    let mv = document.entry_path(&moves, entry.mv).expect("a move");
    let list: PropertyPath = format!("{mv}.{}", entry.track.key()).parse().expect("a path");
    document.entry_path(&list, entry.index).expect("an entry").to_string()
}

#[test]
fn deleting_each_entry_writes_the_same_bytes() {
    for file in FILES {
        let original = fs::read_to_string(root().join(file)).expect(file);
        for entry in entries(&MovesText::new(original.clone())) {
            let mut t3 = MovesText::new(original.clone());
            let mut new = document(&original);
            t3.delete_entry(entry).expect("T3");
            assert_eq!(new.remove("delete", &path(&at(entry))), Ok(Outcome::Changed), "{}", at(entry));
            hold_to(&mut new, &original, t3.text(), &at(entry));
        }
    }
}

fn list_of<'n>(tree: &'n Node, mv: usize, key: &str) -> Option<&'n Node> {
    tree.field("data")?.field("moves")?.items().get(mv)?.field(key)
}

/// A copy of the last entry added to each list the file writes, in its spelling.
#[test]
fn adding_an_entry_to_a_written_list_writes_the_same_bytes() {
    let mut compared = 0;
    for file in FILES {
        let original = fs::read_to_string(root().join(file)).expect(file);
        let tree = text::scan(&original).expect("a tree");
        let moves = MovesText::new(original.clone()).document().expect("reads").moves.len();
        for mv in 0..moves {
            for track in Track::ALL.into_iter().filter(|track| *track != Track::RootKeys) {
                let Some(list) = list_of(&tree, mv, track.key()) else { continue };
                let Some(last) = list.items().last() else { continue };
                let entry = last.text(&original);
                let mut t3 = MovesText::new(original.clone());
                let mut new = document(&original);
                t3.insert_entry(mv, track, entry).expect("T3");
                let target = path(&format!("moves[{mv}].{}", track.key()));
                let end = list.items().len();
                assert_eq!(new.insert_literal("add", &target, end, entry), Ok(Outcome::Changed), "{target}");
                hold_to(&mut new, &original, t3.text(), &target.to_string());
                compared += 1;
            }
        }
    }
    assert!(compared > 20, "{compared}");
}

/// A copy of an entry from elsewhere in the file added to each list a move leaves out: the new field lands
/// where T3 lands it, laid out the same.
#[test]
fn adding_an_entry_to_a_list_left_out_writes_the_same_bytes() {
    let mut compared = 0;
    for file in FILES {
        let original = fs::read_to_string(root().join(file)).expect(file);
        let tree = text::scan(&original).expect("a tree");
        let moves = MovesText::new(original.clone()).document().expect("reads").moves.len();
        for track in Track::ALL.into_iter().filter(|track| *track != Track::RootKeys) {
            let lists = (0..moves).filter_map(|mv| list_of(&tree, mv, track.key()));
            let Some(entry) = lists.filter_map(|list| list.items().first()).next() else { continue };
            let entry = entry.text(&original);
            for mv in (0..moves).filter(|mv| list_of(&tree, *mv, track.key()).is_none()) {
                let mut t3 = MovesText::new(original.clone());
                let mut new = document(&original);
                t3.insert_entry(mv, track, entry).expect("T3");
                let target = path(&format!("moves[{mv}].{}", track.key()));
                assert_eq!(new.insert_literal("add", &target, 0, entry), Ok(Outcome::Changed), "{target}");
                hold_to(&mut new, &original, t3.text(), &target.to_string());
                compared += 1;
            }
        }
    }
    assert!(compared > 50, "{compared}");
}
