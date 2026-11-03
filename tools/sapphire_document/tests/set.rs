//! The documents open at once, against files that change under them: which is which, by ID, through a rename
//! and a move, and what's dirty, against a model of the rule and nothing else: the bytes differ from those of
//! the last save or take.

#![allow(clippy::print_stdout, reason = "the fuzz reports what it did")]

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{env, fs, process};

use sapphire_assets::AssetId;
use sapphire_document::identity::{self, sidecar_path};
use sapphire_document::{
    Document, DocumentSet, IdentityError, Kind, Noticed, Outcome, PropertyKind, SetError, kinds, registry_of,
    walk, write,
};
use sapphire_moves::MoveSetDocument;
use sapphire_vfx::EffectLibrary;
use support::{Random, another_value, read};

const PLAYER: &str = "content/combat/player/player.moves.ron";
const CUE: &str = "content/audio/hit_heavy.cue.ron";
const EFFECTS: &str = "crates/sapphire_vfx/default_effects.ron";

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("sapphire_document_{name}_{}_{unique}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().expect("a parent")).expect("its folder");
        fs::write(&path, text).expect("a test file");
        path
    }

    fn set(&self) -> DocumentSet {
        DocumentSet::new(vec![self.0.clone()])
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn open_moves(set: &mut DocumentSet, path: &Path) -> Result<AssetId, SetError> {
    set.open(path, kinds::moves(), Arc::new(registry_of::<MoveSetDocument>()))
}

fn edit(set: &mut DocumentSet, id: AssetId) {
    let path = "moves[0].startup".parse().expect("a path");
    let document = set.get_mut::<MoveSetDocument>(id).expect("moves");
    let next = document.value().expect("reads").moves[0].startup + 1;
    assert_eq!(document.set_text("edit", &path, &next.to_string()), Ok(Outcome::Changed));
}

#[test]
fn a_sidecar_appears_beside_a_document_with_none_and_stays() {
    let dir = TestDir::new("appears");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let id = open_moves(&mut set, &path).expect("opens");
    let sidecar = fs::read_to_string(sidecar_path(&path)).expect("beside it");
    assert_eq!(sidecar, identity::sidecar_text(id, Some("MoveSet(())")));
    assert_eq!(fs::read_to_string(&path).expect("reads"), read(PLAYER), "opening touches nothing else");

    let mut again = dir.set();
    assert_eq!(open_moves(&mut again, &path).expect("opens"), id, "and it says the same next time");
    assert_eq!(fs::read_to_string(sidecar_path(&path)).expect("reads"), sidecar);
    assert_eq!(open_moves(&mut set, &path).expect("opens"), id, "one document, opened twice");
    assert_eq!(set.stats().open, 1);
}

#[test]
fn every_sidecar_in_the_repository_reads() {
    let mut read_some = 0;
    let mut stack =
        vec![support::repository_root().join("content"), support::repository_root().join(EFFECTS)];
    stack[1] = sidecar_path(&stack[1]);
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            stack.extend(fs::read_dir(&path).expect("a folder").map(|entry| entry.expect("an entry").path()));
        } else if path.extension().is_some_and(|extension| extension == "meta") {
            identity::read_id(&path).unwrap_or_else(|error| panic!("{error}"));
            read_some += 1;
        }
    }
    assert!(read_some > 100, "{read_some}");
}

#[test]
fn a_document_stays_itself_through_a_rename_and_a_move() {
    let dir = TestDir::new("moves");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let id = open_moves(&mut set, &path).expect("opens");
    edit(&mut set, id);
    let unsaved = set.any(id).expect("open").text().to_owned();

    // By the set: the edits stay, and still aren't saved.
    let renamed = dir.0.join("b.moves.ron");
    set.move_to(id, &renamed).expect("moves");
    assert_eq!(set.path(id), Some(renamed.as_path()));
    assert!(!path.exists() && !sidecar_path(&path).exists());
    assert!(set.is_dirty(id));
    assert_eq!(set.any(id).expect("open").text(), unsaved);
    assert_eq!(identity::read_id(&sidecar_path(&renamed)).expect("reads"), id);

    // By a person, with its sidecar: found by its ID, in a new folder.
    fs::create_dir(dir.0.join("deeper")).expect("a folder");
    let moved = dir.0.join("deeper/c.moves.ron");
    fs::rename(&renamed, &moved).expect("moved");
    fs::rename(sidecar_path(&renamed), sidecar_path(&moved)).expect("moved");
    assert_eq!(set.refresh(), vec![Noticed::Moved { id, from: renamed, to: moved.clone() }]);
    assert_eq!(set.path(id), Some(moved.as_path()));
    assert!(set.is_dirty(id), "nothing was saved by moving it");

    // A save goes where it is now.
    set.save(id).expect("saves");
    assert_eq!(fs::read_to_string(&moved).expect("reads"), unsaved);
    assert_eq!(fs::read_dir(&dir.0).expect("a folder").count(), 1, "nothing written where it was");
}

#[test]
fn a_save_after_a_move_nobody_looked_at_follows_the_id() {
    let dir = TestDir::new("save_follows");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let id = open_moves(&mut set, &path).expect("opens");
    edit(&mut set, id);
    let moved = dir.0.join("b.moves.ron");
    fs::rename(&path, &moved).expect("moved");
    fs::rename(sidecar_path(&path), sidecar_path(&moved)).expect("moved");
    set.save(id).expect("saves");
    assert!(!path.exists());
    assert_eq!(fs::read_to_string(&moved).expect("reads"), set.any(id).expect("open").text());
}

#[test]
fn a_file_moved_without_its_sidecar_stops_and_says_so() {
    let dir = TestDir::new("without_sidecar");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    let id = open_moves(&mut dir.set(), &path).expect("opens");
    let moved = dir.0.join("b.moves.ron");
    fs::rename(&path, &moved).expect("moved");

    let refused = open_moves(&mut dir.set(), &moved).expect_err("a new ID would lose the old");
    let message = refused.to_string();
    assert!(matches!(refused, SetError::Identity(IdentityError::Moved { .. })), "{message}");
    assert!(message.contains("move its sidecar with it"), "{message}");
    assert!(!sidecar_path(&moved).exists(), "nothing was made");

    fs::rename(sidecar_path(&path), sidecar_path(&moved)).expect("moved");
    assert_eq!(open_moves(&mut dir.set(), &moved).expect("opens"), id);
}

#[test]
fn two_files_claiming_one_id_open_neither_until_the_copys_sidecar_goes() {
    let dir = TestDir::new("duplicate");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    let id = open_moves(&mut dir.set(), &path).expect("opens");
    let copy = dir.write("copy.moves.ron", &read(PLAYER));
    fs::copy(sidecar_path(&path), sidecar_path(&copy)).expect("copied");

    for either in [&path, &copy] {
        let refused = open_moves(&mut dir.set(), either).expect_err("two claims");
        assert!(matches!(refused, SetError::Identity(IdentityError::Duplicate { .. })), "{refused}");
        assert!(refused.to_string().contains("delete the sidecar of the copy"), "{refused}");
    }

    fs::remove_file(sidecar_path(&copy)).expect("deleted");
    let mut set = dir.set();
    assert_eq!(open_moves(&mut set, &path).expect("opens"), id);
    assert_ne!(open_moves(&mut set, &copy).expect("opens"), id, "the copy has a new one");
    assert_eq!(set.stats().open, 2);
}

#[test]
fn a_sidecar_that_cant_be_read_is_left_as_it_is() {
    let dir = TestDir::new("unreadable");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    for broken in [
        "not ron (",
        "(type: \"something else\", version: 1, data: (id: \"x\"))",
        "(type: \"sapphire::assets::AssetMeta\", version: 1, data: (id: \"short\"))",
    ] {
        fs::write(sidecar_path(&path), broken).expect("a broken sidecar");
        let refused = open_moves(&mut dir.set(), &path).expect_err("refused");
        assert!(matches!(refused, SetError::Identity(IdentityError::Unreadable { .. })), "{refused}");
        assert_eq!(fs::read_to_string(sidecar_path(&path)).expect("reads"), broken);
    }
}

#[test]
fn a_new_document_comes_whole_with_its_id() {
    let dir = TestDir::new("create");
    let mut set = dir.set();
    let registry = Arc::new(registry_of::<MoveSetDocument>());
    let path = dir.0.join("new.moves.ron");

    let refused = set.create(&path, "not a document", kinds::moves(), registry.clone()).expect_err("no");
    assert!(matches!(refused, SetError::DoesNotRead { .. }), "{refused}");
    assert!(!path.exists());

    let id = set.create(&path, &read(PLAYER), kinds::moves(), registry.clone()).expect("made");
    assert_eq!(fs::read_to_string(&path).expect("reads"), read(PLAYER));
    assert_eq!(identity::read_id(&sidecar_path(&path)).expect("reads"), id);
    assert!(!set.is_dirty(id));

    let refused = set.create(&path, &read(PLAYER), kinds::moves(), registry.clone()).expect_err("there");
    assert!(matches!(refused, SetError::AlreadyThere { .. }), "{refused}");

    let second = dir.write("second.moves.ron", "someone's");
    let refused = set.create(&second, &read(PLAYER), kinds::moves(), registry.clone()).expect_err("there");
    assert!(matches!(refused, SetError::AlreadyThere { .. }), "{refused}");
    assert_eq!(fs::read_to_string(&second).expect("reads"), "someone's");
    assert!(!sidecar_path(&second).exists());

    // A sidecar waiting for a file that moved isn't a new document's to take.
    let waiting = dir.0.join("waiting.moves.ron");
    fs::write(sidecar_path(&waiting), identity::sidecar_text(AssetId::from_bytes([3; 16]), None))
        .expect("one");
    let refused = set.create(&waiting, &read(PLAYER), kinds::moves(), registry).expect_err("waiting");
    assert!(matches!(refused, SetError::Identity(IdentityError::SidecarWaiting { .. })), "{refused}");
    assert!(!waiting.exists());
}

#[test]
fn a_move_onto_something_moves_nothing() {
    let dir = TestDir::new("move_refused");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let id = open_moves(&mut set, &path).expect("opens");

    let taken = dir.write("taken.moves.ron", "someone's");
    assert!(matches!(set.move_to(id, &taken), Err(SetError::AlreadyThere { .. })));
    assert_eq!(fs::read_to_string(&taken).expect("reads"), "someone's");

    let waiting = dir.0.join("waiting.moves.ron");
    fs::write(sidecar_path(&waiting), "someone's").expect("a sidecar");
    assert!(matches!(set.move_to(id, &waiting), Err(SetError::AlreadyThere { .. })));
    assert!(!waiting.exists());

    // The file goes first, and comes back if its sidecar can't follow.
    fs::remove_file(sidecar_path(&path)).expect("gone");
    let elsewhere = dir.0.join("elsewhere.moves.ron");
    assert!(set.move_to(id, &elsewhere).is_err());
    assert!(path.exists() && !elsewhere.exists());
    assert_eq!(set.path(id), Some(path.as_path()));
}

#[test]
fn a_save_writes_back_what_went_missing_and_touches_nothing_else() {
    let dir = TestDir::new("missing");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let id = open_moves(&mut set, &path).expect("opens");

    let before = fs::metadata(&path).expect("there").modified().expect("a time");
    set.save(id).expect("nothing to save");
    assert_eq!(fs::metadata(&path).expect("there").modified().expect("a time"), before);

    fs::remove_file(&path).expect("gone");
    assert_eq!(set.refresh(), vec![Noticed::Gone { id }]);
    assert_eq!(set.refresh(), Vec::new(), "said once");
    assert!(!set.is_dirty(id), "the bytes are what was last saved");
    set.save(id).expect("written again");
    assert_eq!(fs::read_to_string(&path).expect("reads"), read(PLAYER));

    fs::remove_file(sidecar_path(&path)).expect("gone");
    set.save(id).expect("saves");
    assert_eq!(identity::read_id(&sidecar_path(&path)).expect("reads"), id, "the same ID");
}

/// Opening walks nothing if the last look knows the sidecar, and walks again if there's anything else to it.
#[test]
fn opening_walks_the_roots_only_when_the_last_look_cant_say() {
    let dir = TestDir::new("scan_once");
    let first = dir.write("a/one.moves.ron", &read(PLAYER));
    let second = dir.write("b/two.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    assert_eq!(set.stats().scans, 0);

    // Two files with no sidecars: a new ID is worth a look, each time.
    let one = open_moves(&mut set, &first).expect("opens");
    let two = open_moves(&mut set, &second).expect("opens");
    assert_eq!(set.stats().scans, 2);

    // Known now, and open already: no walk.
    assert_eq!(open_moves(&mut set, &first).expect("opens"), one);
    assert!(set.close(two));
    assert_eq!(open_moves(&mut set, &second).expect("opens"), two);
    assert_eq!(set.stats().scans, 2, "the look they were noted in still says");

    // A new look finds the same; a refresh with nothing gone walks nothing.
    assert_eq!(set.rescan().expect("walks"), []);
    assert_eq!(set.stats().scans, 3);
    let _ = set.refresh();
    let _ = set.refresh();
    assert_eq!(set.stats().scans, 3);
    assert_eq!(open_moves(&mut set, &first).expect("opens"), one);
    assert_eq!(set.stats().scans, 3);
}

/// A copy made since the last look is still a copy: opening it looks again, and the timer's look says so of the
/// document that's open, once.
#[test]
fn a_copy_made_since_the_last_look_is_found() {
    let dir = TestDir::new("scan_copy");
    let original = dir.write("a/one.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let id = open_moves(&mut set, &original).expect("opens");
    edit(&mut set, id);

    let copy = dir.0.join("b/one.moves.ron");
    fs::create_dir_all(copy.parent().expect("a parent")).expect("its folder");
    fs::copy(&original, &copy).expect("the file");
    fs::copy(sidecar_path(&original), sidecar_path(&copy)).expect("its sidecar");

    match open_moves(&mut set, &copy) {
        Err(SetError::Identity(IdentityError::Duplicate { id: said, .. })) => assert_eq!(said, id),
        other => panic!("{:?}", other.map(|id| id.to_string())),
    }
    assert_eq!(set.rescan().ok().map(|noticed| noticed.len()), Some(1), "said of what's open");
    assert_eq!(set.rescan().expect("walks"), [], "said once");
    assert!(set.is_dirty(id), "what's open stays open, with its edits");
    set.save(id).expect("saving what's open is still fine");

    // Put right, it's over: and said again if it happens again.
    fs::remove_file(sidecar_path(&copy)).expect("the copy's sidecar");
    assert_eq!(set.rescan().expect("walks"), []);
    let other = open_moves(&mut set, &copy).expect("a new ID");
    assert_ne!(other, id);
}

/// A sidecar that lost its file since the last look stops a new ID as it would have, however old that look is.
#[test]
fn a_move_since_the_last_look_still_stops_a_new_id() {
    let dir = TestDir::new("scan_moved");
    let original = dir.write("a/one.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let id = open_moves(&mut set, &original).expect("opens");
    assert!(set.close(id));
    assert_eq!(set.rescan().expect("walks"), []);
    let moved = dir.0.join("a/renamed.moves.ron");
    fs::rename(&original, &moved).expect("moved, without its sidecar");
    assert!(matches!(open_moves(&mut set, &moved), Err(SetError::Identity(IdentityError::Moved { .. }))));
    assert!(!sidecar_path(&moved).exists(), "no ID was made");
}

/// A file outside the roots with an ID a file inside them holds is a copy too.
#[test]
fn a_copy_outside_the_roots_is_a_copy() {
    let inside = TestDir::new("outside_in");
    let outside = TestDir::new("outside_out");
    let original = inside.write("one.moves.ron", &read(PLAYER));
    let mut set = inside.set();
    let id = open_moves(&mut set, &original).expect("opens");
    assert!(set.close(id));
    let copy = outside.write("one.moves.ron", &read(PLAYER));
    fs::copy(sidecar_path(&original), sidecar_path(&copy)).expect("its sidecar");
    assert!(matches!(open_moves(&mut set, &copy), Err(SetError::Identity(IdentityError::Duplicate { .. }))));
}

/// Roots are told apart by where they lead. However they overlap, a file claims its ID once, the walk sees it
/// once, and the first spelling is what paths come out in.
#[test]
fn overlapping_roots_are_one_root() {
    let dir = TestDir::new("overlap");
    let file = dir.write("sub/inner/one.moves.ron", &read(PLAYER));
    let root = dir.0.clone();
    let sub = root.join("sub");
    let spellings: Vec<(&str, Vec<PathBuf>)> = vec![
        ("the same twice", vec![root.clone(), root.clone()]),
        ("through a parent", vec![sub.clone(), sub.join("inner/..")]),
        ("with a dot", vec![sub.clone(), sub.join(".")]),
        ("nested, outer first", vec![root.clone(), sub.clone()]),
        ("nested, inner first", vec![sub.clone(), root.clone()]),
        ("one that isn't there", vec![root.join("nothing"), sub.clone()]),
    ];
    for (what, roots) in spellings {
        let mut set = DocumentSet::new(roots.clone());
        let id = open_moves(&mut set, &file).unwrap_or_else(|error| panic!("{what}: {error}"));
        assert_eq!(set.rescan().expect("walks"), [], "{what}: no duplicate");
        let walked: Vec<_> = set.sidecars().documents().map(|(path, _)| path.to_owned()).collect();
        assert_eq!(walked.len(), 1, "{what}: seen once: {walked:?}");
        assert_eq!(
            set.sidecars().source_of(id).as_deref().map(fs::canonicalize).map(Result::ok),
            Some(fs::canonicalize(&file).ok()),
            "{what}"
        );
        assert_eq!(
            identity::distinct_roots(&roots).len(),
            if what.ends_with("there") { 2 } else { 1 },
            "{what}"
        );
    }
    // Each spelled as itself where they don't overlap.
    let (left, right) = (root.join("left"), root.join("right"));
    assert_eq!(identity::distinct_roots(&[left.clone(), right.clone()]), [left.as_path(), right.as_path()]);
    // A name a root's name starts with is no parent of it.
    let (near, nearer) = (root.join("near"), root.join("near_too"));
    fs::create_dir_all(&near).expect("a folder");
    fs::create_dir_all(&nearer).expect("a folder");
    assert_eq!(identity::distinct_roots(&[near.clone(), nearer.clone()]).len(), 2);
}

/// A link to a root is that root, and a link inside one isn't followed.
#[cfg(unix)]
#[test]
fn a_root_reached_by_a_link_is_the_same_root() {
    let dir = TestDir::new("overlap_link");
    let file = dir.write("real/one.moves.ron", &read(PLAYER));
    let real = dir.0.join("real");
    let link = dir.0.join("link");
    std::os::unix::fs::symlink(&real, &link).expect("a link");
    let mut set = DocumentSet::new(vec![link.clone(), real.clone()]);
    let id = open_moves(&mut set, &link.join("one.moves.ron")).expect("opens by the link");
    assert_eq!(set.rescan().expect("walks"), [], "no duplicate");
    // By its other name it's the same file, not a copy.
    assert_eq!(open_moves(&mut set, &file).expect("opens by its name"), id);
    assert_eq!(set.ids().count(), 1);
}

/// On a file system that doesn't care about case, a root in another case is the same root.
#[test]
fn a_root_in_another_case_is_the_same_root_where_that_is_true() {
    let dir = TestDir::new("overlap_case");
    let lower = dir.0.join("folder");
    let file = dir.write("folder/one.moves.ron", &read(PLAYER));
    let upper = dir.0.join("FOLDER");
    if !upper.exists() {
        return; // This file system cares, and they're two roots if both exist.
    }
    let mut set = DocumentSet::new(vec![lower, upper]);
    open_moves(&mut set, &file).expect("opens");
    assert_eq!(set.rescan().expect("walks"), [], "no duplicate");
}

/// What a browser reads: every document by its kind, sidecar or none, and nothing hidden.
#[test]
fn a_look_lists_the_documents_by_kind() {
    let dir = TestDir::new("listing");
    dir.write("a/one.moves.ron", &read(PLAYER));
    dir.write("a/hit.cue.ron", &read(CUE));
    dir.write("c/default_effects.ron", &read(EFFECTS));
    dir.write("c/other.ron", "()");
    dir.write(".hidden/three.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    set.rescan().expect("walks");
    let names: Vec<(String, Kind)> = set
        .sidecars()
        .documents()
        .map(|(path, kind)| (path.file_name().expect("a name").to_string_lossy().into_owned(), kind))
        .collect();
    assert_eq!(
        names,
        [
            ("hit.cue.ron".to_owned(), Kind::Cue),
            ("one.moves.ron".to_owned(), Kind::Moves),
            ("default_effects.ron".to_owned(), Kind::Effects),
        ]
    );
    let cue = dir.0.join("a/hit.cue.ron");
    let id = Kind::Cue.open(&mut set, &cue).expect("opens as its kind");
    assert!(set.get::<sapphire_assets::cue::Cue>(id).is_some());
}

/// What a look at the whole repository costs, which is why a set keeps its last one and a browser looks on a
/// timer. Its number goes in the report: load makes any limit here a lie.
#[test]
fn what_a_look_at_the_repository_costs() {
    let root = support::repository_root();
    let roots = vec![root.join("content"), root.join("crates")];
    let mut set = DocumentSet::new(roots);
    let mut looks = Vec::new();
    for _ in 0..5 {
        let stopwatch = sapphire_time::Stopwatch::start();
        set.rescan().expect("walks");
        looks.push(stopwatch.elapsed().as_secs_f64() * 1000.0);
    }
    let documents = set.sidecars().documents().count();
    println!("a look at the repository: {looks:.1?} ms, {documents} documents");
    assert!(documents >= 3, "{documents}");
}

/// The cap reaches a panel, and the set's count of what went.
#[test]
fn what_history_let_go_shows_through_the_set() {
    let dir = TestDir::new("cap_set");
    let file = dir.write("one.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let id = open_moves(&mut set, &file).expect("opens");
    set.any_mut(id).expect("open").set_history_limit(1);
    edit(&mut set, id);
    edit(&mut set, id);
    assert_eq!(set.stats().forgotten, 1);
    assert_eq!(set.stats().steps, 1);
    let document = set.any_mut(id).expect("open");
    assert!(document.undo());
    assert!(!document.undo());
    assert!(document.why_no_undo().expect("it says").contains("nothing older"));
    assert!(set.is_dirty(id), "the first edit's still in the text");
    set.save(id).expect("saves");
    assert!(!set.is_dirty(id));
}

#[test]
fn the_kinds_are_told_apart_and_a_panel_needs_none() {
    let dir = TestDir::new("kinds");
    let moves = dir.write("a.moves.ron", &read(PLAYER));
    let effects = dir.write("b.ron", &read(EFFECTS));
    let mut set = dir.set();
    let a = open_moves(&mut set, &moves).expect("opens");
    let b = set.open(&effects, kinds::effects(), Arc::new(registry_of::<EffectLibrary>())).expect("opens");
    assert_ne!(a, b);
    assert!(set.get::<EffectLibrary>(a).is_none());
    assert!(set.get::<MoveSetDocument>(a).is_some());
    assert!(fs::read_to_string(sidecar_path(&effects)).expect("reads").contains(identity::DOCUMENT_TYPE));
    for id in [a, b] {
        let any = set.any(id).expect("open");
        assert!(!any.properties().is_empty());
        assert!(any.value_reflect().is_some());
        assert_eq!(any.failure_message(), None);
    }
    let startup = "moves[0].startup".parse().expect("a path");
    assert_eq!(set.any_mut(a).expect("open").set_ron("edit", &startup, "77"), Ok(Outcome::Changed));
    assert_eq!(set.dirty(), vec![a]);
    assert_eq!(set.stats().dirty, 1);
    assert!(set.stats().history_bytes > 0);
    assert!(set.close(a));
    assert_eq!(set.ids().collect::<Vec<_>>(), vec![b]);
}

/// What the rule says of one file, kept apart from the code it's checking.
struct Model {
    id: AssetId,
    path: PathBuf,
    /// The bytes of the last save or take.
    baseline: Vec<u8>,
    conflict: Option<Vec<u8>>,
}

#[derive(Default, Debug)]
struct Fuzzed {
    steps: usize,
    edits: usize,
    gestures: usize,
    saves: usize,
    refused_saves: usize,
    outside: usize,
    taken: usize,
    conflicts: usize,
    settled: usize,
    moves: usize,
    clean_by_undo: usize,
    clean_saved_elsewhere: usize,
}

impl Model {
    /// A look at the file, by the rule.
    fn settle(&mut self, text: &str, on_disk: Vec<u8>, is_save: bool, tally: &mut Fuzzed) {
        let is_text = std::str::from_utf8(&on_disk).is_ok();
        if !is_save && !is_text {
            return;
        }
        if on_disk == self.baseline {
            self.conflict = None;
        } else if on_disk == text.as_bytes() {
            tally.clean_saved_elsewhere += 1;
            self.baseline = on_disk;
            self.conflict = None;
        } else if text.as_bytes() != self.baseline || !is_text {
            tally.conflicts += 1;
            self.conflict = Some(on_disk);
        } else {
            tally.taken += 1;
            self.baseline = on_disk;
            self.conflict = None;
        }
    }
}

fn one_edit(set: &mut DocumentSet, id: AssetId, random: &mut Random, tally: &mut Fuzzed) {
    let any = set.any(id).expect("open");
    let Some(root) = any.value_reflect() else { return };
    let properties: Vec<_> =
        any.properties().into_iter().filter(|property| property.kind == PropertyKind::Leaf).collect();
    if properties.is_empty() {
        return;
    }
    let property = &properties[random.below(properties.len())];
    let value = walk::get(root, &property.path).expect("a path the walker gave");
    let Some(new) = another_value(value, property.kind, any.registry(), random.below(4)) else { return };
    let ron = write::canonical(new.as_ref(), any.registry()).expect("writes");
    // A refusal is the fuzz's business in `fuzz.rs`. Here it's only that the bytes didn't move.
    if set.any_mut(id).expect("open").set_ron("fuzz", &property.path, &ron) == Ok(Outcome::Changed) {
        tally.edits += 1;
    }
}

fn hold(set: &DocumentSet, models: &[Model], what: &str) {
    let mut dirty = 0;
    for model in models {
        let text = set.any(model.id).expect("open").text();
        let differs = text.as_bytes() != model.baseline;
        assert_eq!(set.is_dirty(model.id), differs, "{what}: {}", model.path.display());
        dirty += usize::from(differs);
        assert_eq!(set.conflict(model.id), model.conflict.as_deref(), "{what}: {}", model.path.display());
        assert_eq!(set.path(model.id), Some(model.path.as_path()), "{what}");
    }
    assert_eq!(set.stats().dirty, dirty, "{what}");
    assert_eq!(set.dirty().len(), dirty, "{what}");
}

/// Everything at once over three kinds of document, with the files changing under them, and the rule held
/// after every step.
fn fuzz_dirty(seed: u64, steps: usize) -> Fuzzed {
    let dir = TestDir::new("fuzz");
    let mut set = dir.set();
    let mut models = Vec::new();
    let mut random = Random(seed);
    for (name, source) in [("a/player.moves.ron", PLAYER), ("b/hit.cue.ron", CUE), ("c/effects.ron", EFFECTS)]
    {
        let path = dir.write(name, &read(source));
        let id = match source {
            PLAYER => open_moves(&mut set, &path),
            CUE => set.open(&path, kinds::cue(), Arc::new(registry_of::<sapphire_assets::cue::Cue>())),
            _ => set.open(&path, kinds::effects(), Arc::new(registry_of::<EffectLibrary>())),
        }
        .expect("opens");
        // Half the seeds run at a cap they reach, so the rule is held with steps and save points going.
        if seed % 2 == 1 {
            set.any_mut(id).expect("open").set_history_limit(4096);
        }
        models.push(Model { id, path, baseline: read(source).into_bytes(), conflict: None });
    }
    hold(&set, &models, "opening");

    let mut tally = Fuzzed::default();
    let mut outside = 0;
    for step in 0..steps {
        let at = random.below(models.len());
        let id = models[at].id;
        let action = random.below(15);
        match action {
            0..=3 => one_edit(&mut set, id, &mut random, &mut tally),
            4 => {
                set.any_mut(id).expect("open").begin_gesture("drag");
                for _ in 0..random.below(5) {
                    one_edit(&mut set, id, &mut random, &mut tally);
                }
                if random.below(3) == 0 {
                    set.any_mut(id).expect("open").cancel_gesture();
                } else {
                    set.any_mut(id).expect("open").end_gesture();
                }
                tally.gestures += 1;
            }
            5 => {
                let dirty = set.is_dirty(id);
                set.any_mut(id).expect("open").undo();
                tally.clean_by_undo += usize::from(dirty && !set.is_dirty(id));
            }
            6 => {
                set.any_mut(id).expect("open").redo();
            }
            7 | 8 => {
                let on_disk = fs::read(&models[at].path).expect("reads");
                let text = set.any(id).expect("open").text().to_owned();
                models[at].settle(&text, on_disk, true, &mut tally);
                match set.save(id) {
                    Ok(()) => {
                        assert_eq!(models[at].conflict, None, "step {step}: saved over a conflict");
                        models[at].baseline = set.any(id).expect("open").text().as_bytes().to_vec();
                        assert_eq!(fs::read(&models[at].path).expect("reads"), models[at].baseline);
                        tally.saves += 1;
                    }
                    Err(SetError::ChangedOnDisk { .. }) => {
                        assert!(models[at].conflict.is_some(), "step {step}: refused with no conflict");
                        tally.refused_saves += 1;
                    }
                    Err(error) => panic!("step {step}: {error}"),
                }
            }
            9 | 10 => {
                // A write from outside: this text, another, or bytes that aren't text.
                outside += 1;
                tally.outside += 1;
                let text = set.any(id).expect("open").text().to_owned();
                let bytes = match random.below(4) {
                    0 => text.into_bytes(),
                    1 => vec![0xff, 0xfe, 0x00],
                    _ => format!("{}\n// from outside {outside}\n", read_baseline(&models[at])).into_bytes(),
                };
                fs::write(&models[at].path, bytes).expect("written");
            }
            11 => {
                let looked: Vec<_> = models
                    .iter()
                    .map(|model| {
                        (
                            set.any(model.id).expect("open").text().to_owned(),
                            fs::read(&model.path).expect("reads"),
                        )
                    })
                    .collect();
                set.refresh();
                for (model, (text, on_disk)) in models.iter_mut().zip(looked) {
                    model.settle(&text, on_disk, false, &mut tally);
                }
            }
            12 => {
                let held = models[at].conflict.clone();
                let took = set.take_disk(id);
                match held {
                    Some(bytes) if std::str::from_utf8(&bytes).is_ok() => {
                        assert!(took);
                        assert_eq!(set.any(id).expect("open").text().as_bytes(), bytes);
                        models[at].baseline = bytes;
                        models[at].conflict = None;
                        tally.settled += 1;
                    }
                    _ => assert!(!took, "step {step}"),
                }
            }
            13 => {
                let held = models[at].conflict.take();
                let settled = held.is_some();
                if random.below(2) == 0 {
                    let kept = set.keep_mine(id);
                    assert_eq!(kept, settled);
                    if let Some(bytes) = held {
                        models[at].baseline = bytes;
                        tally.settled += 1;
                    }
                } else {
                    let rebased = set.rebase(id);
                    match held {
                        Some(bytes) if std::str::from_utf8(&bytes).is_ok() => {
                            assert!(rebased.is_some());
                            models[at].baseline = bytes;
                            tally.settled += 1;
                        }
                        Some(bytes) => {
                            assert!(rebased.is_none());
                            models[at].conflict = Some(bytes);
                        }
                        None => assert!(rebased.is_none()),
                    }
                }
            }
            _ => {
                let from = models[at].path.clone();
                let name = from.file_name().expect("a name").to_string_lossy().into_owned();
                let to = from.with_file_name(format!("m{step}_{}", name.rsplit('_').next().expect("a name")));
                if random.below(2) == 0 {
                    set.move_to(id, &to).expect("moves");
                } else {
                    fs::rename(&from, &to).expect("moved");
                    fs::rename(sidecar_path(&from), sidecar_path(&to)).expect("moved");
                    let looked: Vec<_> = models
                        .iter()
                        .map(|model| {
                            let on_disk =
                                fs::read(if model.id == id { &to } else { &model.path }).expect("reads");
                            (set.any(model.id).expect("open").text().to_owned(), on_disk)
                        })
                        .collect();
                    assert!(set.refresh().contains(&Noticed::Moved { id, from, to: to.clone() }));
                    for (model, (text, on_disk)) in models.iter_mut().zip(looked) {
                        model.settle(&text, on_disk, false, &mut tally);
                    }
                }
                models[at].path = to;
                tally.moves += 1;
            }
        }
        tally.steps += 1;
        hold(&set, &models, &format!("step {step}, action {action}"));
        for model in &models {
            assert_eq!(
                identity::read_id(&sidecar_path(&model.path)).expect("reads"),
                model.id,
                "step {step}"
            );
        }
    }
    tally
}

/// Something other than this document's text, that reads.
fn read_baseline(model: &Model) -> String {
    match std::str::from_utf8(&model.baseline) {
        Ok(text) => text.to_owned(),
        Err(_) => String::new(),
    }
}

#[test]
fn dirty_is_exactly_the_bytes_differing_from_the_last_save_or_take() {
    let full = env::var_os("SAPPHIRE_DOCUMENT_FUZZ").is_some_and(|value| value == "full");
    let (seeds, steps) = if full { (8, 2_000) } else { (2, 300) };
    for seed in 0..seeds {
        let tally = fuzz_dirty(0x5e7 + seed, steps);
        println!("dirty fuzz, seed {seed}: {tally:?}");
        assert!(tally.edits > 0 && tally.saves > 0 && tally.taken > 0, "{tally:?}");
        assert!(tally.conflicts > 0 && tally.settled > 0 && tally.moves > 0, "{tally:?}");
    }
}

#[test]
fn a_real_gesture_in_a_set_is_one_step() {
    let dir = TestDir::new("gesture");
    let path = dir.write("a.moves.ron", &read(PLAYER));
    let mut set = dir.set();
    let id = open_moves(&mut set, &path).expect("opens");
    let document: &mut Document<MoveSetDocument> = set.get_mut(id).expect("moves");
    document.begin_gesture("drag");
    for tick in 0..50_u32 {
        let startup = "moves[0].startup".parse().expect("a path");
        assert!(document.set_text("drag", &startup, &(tick + 2).to_string()).is_ok());
    }
    // A look mid-gesture waits.
    fs::write(&path, "changed").expect("written");
    assert_eq!(set.refresh(), Vec::new());
    assert_eq!(set.conflict(id), None);
    let any = set.any_mut(id).expect("open");
    any.end_gesture();
    assert_eq!(any.history().1, 1);
    assert_eq!(set.refresh(), vec![Noticed::Conflict { id }]);
}
