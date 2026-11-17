//! The journal (M6 T3): a session's unsaved text kept after each step, offered to the next editor once the
//! session ends without closing, and taken back as the document it was, or as a conflict with a file that
//! moved on, through the rebase. Nothing here writes a source.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{env, fs, process};

use sapphire_assets::AssetId;
use sapphire_document::journal::{JOURNAL_VERSION, Unsaved};
use sapphire_document::{
    DocumentSet, Journal, JournalEntry, Kind, Noticed, Outcome, SetError, kinds, registry_of,
};
use sapphire_moves::MoveSetDocument;
use support::read;

const PLAYER: &str = "content/combat/player/player.moves.ron";

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("sapphire_journal_{name}_{}_{unique}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).expect("a test file");
        path
    }

    fn set(&self) -> DocumentSet {
        DocumentSet::new(vec![self.0.clone()])
    }

    fn journal(&self) -> Journal {
        Journal::open(&self.0.join("journal")).expect("a session")
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn open_moves(set: &mut DocumentSet, path: &Path) -> AssetId {
    set.open(path, kinds::moves(), Arc::new(registry_of::<MoveSetDocument>())).expect("opens")
}

/// Adds one to a number of the first move, as a step of its own or inside the gesture under way.
fn bump(set: &mut DocumentSet, id: AssetId, field: &str) {
    bump_move(set, id, 0, field);
}

/// Adds one to a number of a move.
fn bump_move(set: &mut DocumentSet, id: AssetId, mv: usize, field: &str) {
    let path = format!("moves[{mv}].{field}").parse().expect("a path");
    let document = set.get_mut::<MoveSetDocument>(id).expect("moves");
    let now = document.value().expect("reads").moves[mv].clone();
    let next = match field {
        "startup" => now.startup,
        "active" => now.active,
        _ => now.recovery,
    } + 1;
    assert_eq!(document.set_text(&format!("bump {field}"), &path, &next.to_string()), Ok(Outcome::Changed));
}

fn text(set: &DocumentSet, id: AssetId) -> String {
    set.any(id).expect("open").text().to_owned()
}

fn entries(journal: &Journal) -> usize {
    fs::read_dir(journal.session())
        .expect("its folder")
        .filter(|file| {
            file.as_ref().is_ok_and(|file| file.file_name().to_string_lossy().ends_with(".journal.ron"))
        })
        .count()
}

/// The text an ended session left, a gesture's middle in it, comes back byte for byte and as dirty, with its
/// steps for undo to walk back to the file. A session still running offers nothing.
#[test]
fn an_ended_session_offers_its_text_and_a_running_one_offers_nothing() {
    let dir = TestDir::new("offers");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let id = open_moves(&mut set, &path);
    bump(&mut set, id, "startup");
    set.any_mut(id).expect("open").begin_gesture("drag");
    bump(&mut set, id, "active");
    bump(&mut set, id, "active");
    let unsaved = text(&set, id);

    let mut first = dir.journal();
    assert!(first.write(&set).is_empty());
    assert!(first.write(&set).is_empty(), "nothing new, nothing written");
    assert_eq!(first.stats().writes, 1);
    assert_eq!(entries(&first), 1);

    let running = dir.journal();
    assert!(running.offers().is_empty(), "a session that's running isn't offered");
    drop(running);
    // Stopped as a kill would stop it: its folder stays, and its lock goes.
    drop(first);

    let mut next = dir.journal();
    assert_eq!(next.offers().len(), 1, "{:?}", next.unreadable());
    let entry = next.offer(&path).expect("offered by its path").entry.clone();
    assert_eq!(entry.text, unsaved);
    let mut again = dir.set();
    let noticed = Kind::Moves.recover(&mut again, &entry).expect("taken");
    assert!(noticed.is_empty(), "{noticed:?}");
    assert_eq!(text(&again, id), unsaved, "byte for byte");
    assert!(again.is_dirty(id));
    assert_eq!(again.conflict(id), None);
    assert_eq!(fs::read_to_string(&path).expect("the file"), read(PLAYER), "no source written");
    let document = again.any_mut(id).expect("open");
    assert_eq!(document.undo_label(), Some("drag"), "the gesture is a step");
    while document.undo() {}
    assert_eq!(document.text(), read(PLAYER), "undo walks back to the file");
    assert!(!document.is_dirty());

    assert!(next.forget(&path).expect("let go"));
    assert!(next.offers().is_empty());
    let sessions = fs::read_dir(dir.0.join("journal")).expect("the journal").count();
    assert_eq!(sessions, 1, "the ended sessions went with their last offers, and only this one's left");
    next.close().expect("closed");
    assert_eq!(fs::read_dir(dir.0.join("journal")).expect("the journal").count(), 0);
}

/// A file that moved on since its edits began comes back as a conflict, and the rebase lays the edits on top
/// of what's there. The file isn't written by any of it.
#[test]
fn a_file_that_moved_on_comes_back_as_a_conflict_and_the_edits_go_on_top() {
    let dir = TestDir::new("conflict");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let id = open_moves(&mut set, &path);
    bump(&mut set, id, "startup");
    let mut journal = dir.journal();
    assert!(journal.write(&set).is_empty());
    drop(journal);
    drop(set);

    // Someone else's save, of another move: an entry reached by its place guards all of itself.
    let mut theirs = dir.set();
    open_moves(&mut theirs, &path);
    bump_move(&mut theirs, id, 1, "recovery");
    theirs.save(id).expect("saved");
    let on_disk = fs::read_to_string(&path).expect("the file");

    let journal = dir.journal();
    let entry = journal.offer(&path).expect("offered").entry.clone();
    let mut set = dir.set();
    let noticed = Kind::Moves.recover(&mut set, &entry).expect("taken");
    assert_eq!(noticed, [Noticed::Conflict { id }]);
    assert_eq!(set.conflict(id), Some(on_disk.as_bytes()));
    assert_eq!(text(&set, id), entry.text, "the entry's text, until someone settles it");
    assert!(matches!(set.save(id), Err(SetError::ChangedOnDisk { .. })), "a save refuses");
    assert_eq!(fs::read_to_string(&path).expect("the file"), on_disk, "nothing written over it");

    let rebased = set.rebase(id).expect("a conflict to settle");
    assert_eq!((rebased.kept, rebased.refused.len(), rebased.beneath), (1, 0, false));
    let both = set.get::<MoveSetDocument>(id).expect("moves").value().expect("reads").moves.clone();
    let original = MoveSetDocument::parse(&read(PLAYER)).expect("reads").moves;
    assert_eq!(
        (both[0].startup, both[1].recovery),
        (original[0].startup + 1, original[1].recovery + 1),
        "the entry's and theirs"
    );
    assert_eq!(fs::read_to_string(&path).expect("the file"), on_disk, "still only a save writes");
}

/// After an undo past a save and a new edit, history can't say which edits weren't saved, so the entry keeps
/// the text they were made over, and it comes back as it was.
#[test]
fn history_that_cannot_say_keeps_what_its_edits_were_made_over() {
    let dir = TestDir::new("over");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let id = open_moves(&mut set, &path);
    bump(&mut set, id, "startup");
    set.save(id).expect("saved");
    let saved = fs::read_to_string(&path).expect("the file");
    assert!(set.any_mut(id).expect("open").undo());
    bump(&mut set, id, "active");
    let entry = set.journal_entry(id).expect("dirty");
    assert_eq!(entry.unsaved, Unsaved::Over(saved));
    let mut journal = dir.journal();
    assert!(journal.write(&set).is_empty());
    drop(journal);

    let journal = dir.journal();
    let mut again = dir.set();
    let taken = journal.offer(&path).expect("offered").entry.clone();
    assert_eq!(taken, entry);
    assert!(Kind::Moves.recover(&mut again, &taken).expect("taken").is_empty());
    assert_eq!(text(&again, id), entry.text);
    assert!(again.is_dirty(id));
}

/// A document saved, or closed, takes its entry with it; a session that closes takes its folder.
#[test]
fn a_saved_document_leaves_no_entry_and_a_closed_session_no_folder() {
    let dir = TestDir::new("saved");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    let other = dir.write("b.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let (a, b) = (open_moves(&mut set, &path), open_moves(&mut set, &other));
    bump(&mut set, a, "startup");
    bump(&mut set, b, "startup");
    let mut journal = dir.journal();
    assert!(journal.write(&set).is_empty());
    assert_eq!(entries(&journal), 2);
    set.save(a).expect("saved");
    set.close(b);
    assert!(journal.write(&set).is_empty());
    assert_eq!(entries(&journal), 0);
    assert_eq!(journal.stats().entries, 0);
    let session = journal.session().to_owned();
    journal.close().expect("closed");
    assert!(!session.exists());
    assert!(dir.journal().offers().is_empty(), "nothing's offered from a session that closed");
}

/// An entry that doesn't agree with itself is refused by name and changes nothing; one of another version, or
/// with a field it doesn't know, doesn't read.
#[test]
fn an_entry_that_does_not_agree_with_itself_is_refused() {
    let dir = TestDir::new("refused");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let id = open_moves(&mut set, &path);
    bump(&mut set, id, "startup");
    let entry = set.journal_entry(id).expect("dirty");
    assert_eq!(JournalEntry::parse(&entry.to_text()), Ok(entry.clone()));

    let mut wrong = entry.clone();
    wrong.disk = sapphire_document::journal::hash_of(b"something else");
    let mut again = dir.set();
    let refused = Kind::Moves.recover(&mut again, &wrong).expect_err("refused");
    assert!(matches!(refused, SetError::Journal { .. }), "{refused}");
    assert_eq!(again.stats().open, 0);

    let later = entry.to_text().replacen(&format!("version: {JOURNAL_VERSION}"), "version: 2", 1);
    assert!(JournalEntry::parse(&later).expect_err("another version").contains("version 2"));
    let unknown = entry.to_text().replacen("text:", "by: \"me\",\n    text:", 1);
    assert!(JournalEntry::parse(&unknown).is_err());

    // A document open with edits of its own isn't given up for an entry.
    let mut busy = dir.set();
    open_moves(&mut busy, &path);
    bump(&mut busy, id, "recovery");
    assert!(matches!(Kind::Moves.recover(&mut busy, &entry), Err(SetError::OpenWithEdits { .. })));
    assert_eq!(fs::read_to_string(&path).expect("the file"), read(PLAYER));
}
