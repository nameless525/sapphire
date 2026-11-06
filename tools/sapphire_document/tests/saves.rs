//! What a save and a change on disk promise of the one file behind a document, whatever's on either side: a
//! save is whole or refused, it never loses what it hasn't seen, and it leaves everything else where it was.
//! These came with the timeline editor (M2 T3), on a file type of its own. The set holds them now, and they
//! hold on the real player's moves.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{env, fs, process};

use sapphire_assets::AssetId;
use sapphire_document::disk::{Temporary, temporary_path};
use sapphire_document::{DocumentSet, Outcome, SetError, kinds, registry_of};
use sapphire_moves::MoveSetDocument;
use support::read;

const REAL: [&str; 2] = ["content/combat/player/player.moves.ron", "content/combat/enemy/warrior.moves.ron"];
const NOT_TEXT: [u8; 3] = [0xff, 0xfe, 0x42];

fn player() -> String {
    read(REAL[0])
}

fn by_hand() -> String {
    let player = player();
    let changed = player.replacen("startup: 5", "startup: 8", 1);
    assert_ne!(changed, player);
    changed
}

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("sapphire_document_saves_{name}_{}_{unique}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).expect("a test file");
        path
    }

    /// What's there, but for the document and its sidecar.
    fn left_beside(&self) -> usize {
        fs::read_dir(&self.0).expect("lists").count() - 2
    }

    fn opened(&self, name: &str, text: &str) -> (PathBuf, DocumentSet, AssetId) {
        let path = self.write(name, text);
        let mut set = DocumentSet::new(vec![self.0.clone()]);
        let id = open(&mut set, &path).expect("opens");
        (path, set, id)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn open(set: &mut DocumentSet, path: &Path) -> Result<AssetId, SetError> {
    set.open(path, kinds::moves(), Arc::new(registry_of::<MoveSetDocument>()))
}

/// The first cue of the first move, at `at`.
fn move_a_cue(set: &mut DocumentSet, id: AssetId, at: u32) {
    let path = "moves[0].cues[0].at".parse().expect("a path");
    let document = set.get_mut::<MoveSetDocument>(id).expect("moves");
    assert_eq!(document.set_text("move a cue", &path, &at.to_string()), Ok(Outcome::Changed));
}

fn text(set: &DocumentSet, id: AssetId) -> String {
    set.any(id).expect("open").text().to_owned()
}

/// Open and save with no edit, and the file is what it was, on the real documents, comments and layout
/// included. An edit undone is no edit, whatever it did on the way.
#[test]
fn a_round_trip_with_no_edit_leaves_the_real_files_byte_identical() {
    for name in REAL {
        let source = read(name);
        let dir = TestDir::new("round_trip");
        let (path, mut set, id) = dir.opened("moves.ron", &source);
        assert!(!set.is_dirty(id), "{name}");
        set.save(id).expect("saves");
        assert_eq!(fs::read(&path).expect("reads"), source.as_bytes(), "{name}");
        move_a_cue(&mut set, id, 3);
        assert!(set.is_dirty(id), "{name}");
        assert!(set.any_mut(id).expect("open").undo());
        assert!(!set.is_dirty(id), "{name}");
        set.save(id).expect("saves");
        assert_eq!(fs::read(&path).expect("reads"), source.as_bytes(), "{name}");
        assert_eq!(dir.left_beside(), 0, "{name} left nothing beside it");
    }
}

/// A conflict settled by undoing the edits here is settled: the file's text is taken, and the next save has
/// nothing to refuse.
#[test]
fn undoing_back_to_the_file_settles_a_conflict() {
    let dir = TestDir::new("conflict_undone");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    move_a_cue(&mut set, id, 3);
    let elsewhere = player().replacen("name:", "name :", 1);
    fs::write(&path, &elsewhere).expect("someone else's save");
    set.refresh();
    assert!(set.conflict(id).is_some());
    assert!(set.any_mut(id).expect("open").undo());
    assert!(!set.is_dirty(id));
    set.refresh();
    assert_eq!(text(&set, id), elsewhere);
    assert!(set.conflict(id).is_none(), "nothing's in conflict");
    set.save(id).expect("nothing to refuse");
}

#[test]
fn a_save_writes_exactly_the_text_here() {
    let dir = TestDir::new("save");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    move_a_cue(&mut set, id, 3);
    set.save(id).expect("saves");
    assert!(!set.is_dirty(id));
    assert_eq!(fs::read_to_string(&path).expect("reads"), text(&set, id));
    assert_eq!(dir.left_beside(), 0);
}

/// The name every save used to write through: what's there isn't a save's to touch.
#[test]
fn a_save_leaves_a_file_at_an_old_temporary_name_alone() {
    let dir = TestDir::new("old_temporary");
    let there = dir.write(".moves.ron.sapphire_editor", "someone's");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    move_a_cue(&mut set, id, 3);
    set.save(id).expect("saves");
    assert_eq!(fs::read_to_string(&there).expect("still there"), "someone's");
    assert_eq!(fs::read_to_string(&path).expect("reads"), text(&set, id));
}

/// Nor does a link there lead a save to what it points at, or end up over the document.
#[cfg(unix)]
#[test]
fn a_save_doesnt_follow_a_link_at_an_old_temporary_name() {
    let dir = TestDir::new("old_temporary_link");
    let unrelated = dir.write("unrelated", "someone's");
    std::os::unix::fs::symlink(&unrelated, dir.0.join(".moves.ron.sapphire_editor")).expect("a link");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    move_a_cue(&mut set, id, 3);
    set.save(id).expect("saves");
    assert_eq!(fs::read_to_string(&unrelated).expect("reads"), "someone's");
    assert!(!fs::symlink_metadata(&path).expect("the document").is_symlink());
    assert_eq!(fs::read_to_string(&path).expect("reads"), text(&set, id));
}

/// A name a save would use, taken, is passed over: it writes through a file nothing else was at.
#[test]
fn a_save_passes_over_names_that_are_taken() {
    let dir = TestDir::new("taken_names");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    let first = temporary_path(&path, 0);
    fs::write(&first, "someone's").expect("a file there");
    #[cfg(unix)]
    let unrelated = {
        let unrelated = dir.write("unrelated", "someone's");
        std::os::unix::fs::symlink(&unrelated, temporary_path(&path, 1)).expect("a link there");
        unrelated
    };
    move_a_cue(&mut set, id, 3);
    set.save(id).expect("saves");
    assert_eq!(fs::read_to_string(&path).expect("reads"), text(&set, id));
    assert_eq!(fs::read_to_string(&first).expect("still there"), "someone's");
    #[cfg(unix)]
    {
        assert!(fs::symlink_metadata(temporary_path(&path, 1)).expect("a link").is_symlink());
        assert_eq!(fs::read_to_string(&unrelated).expect("reads"), "someone's");
    }
}

/// A save that can't finish takes away only what it made.
#[test]
fn a_failed_save_removes_only_its_own_temporary() {
    let dir = TestDir::new("failed");
    let path = dir.write("moves.ron", "the document");
    let there = temporary_path(&path, 0);
    fs::write(&there, "someone's").expect("a file there");
    let temporary = Temporary::beside(&path, "mine").expect("written");
    assert_eq!(temporary.path, temporary_path(&path, 1));
    assert_eq!(fs::read_to_string(&temporary.path).expect("reads"), "mine");
    drop(temporary);
    assert!(!temporary_path(&path, 1).exists());
    assert_eq!(fs::read_to_string(&there).expect("still there"), "someone's");
    assert_eq!(fs::read_to_string(&path).expect("reads"), "the document");
}

/// The file's mode goes with it.
#[cfg(unix)]
#[test]
fn a_save_keeps_the_files_mode() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = TestDir::new("mode");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("a mode");
    move_a_cue(&mut set, id, 3);
    set.save(id).expect("saves");
    assert_eq!(fs::metadata(&path).expect("the file").permissions().mode() & 0o777, 0o640);
}

/// Editors saving one file at once: each is whole or refused, whoever reads it sees a whole file, and no
/// temporary is left, however the saves fall. Windows can fail a rename over a file someone's reading, which
/// is a save refused, not a broken file, and says nothing about this.
#[cfg(unix)]
#[test]
fn saves_racing_each_other_leave_a_whole_file() {
    const EDITORS: u32 = 4;
    const SAVES: u32 = 25;
    let dir = TestDir::new("racing");
    let source = player();
    let path = dir.write("moves.ron", &source);
    // The sidecar is the first opener's: the rest find it.
    open(&mut DocumentSet::new(vec![dir.0.clone()]), &path).expect("opens");
    let whole = |bytes: &[u8]| {
        let (head, tail) = (&source.as_bytes()[..200], &source.as_bytes()[source.len() - 200..]);
        bytes.starts_with(head) && bytes.ends_with(tail)
    };
    std::thread::scope(|scope| {
        let editors: Vec<_> = (0..EDITORS)
            .map(|editor| {
                let (path, root) = (&path, dir.0.clone());
                scope.spawn(move || {
                    let mut set = DocumentSet::new(vec![root]);
                    let id = open(&mut set, path).expect("opens");
                    for save in 0..SAVES {
                        move_a_cue(&mut set, id, (editor + 1) * 1000 + save);
                        match set.save(id) {
                            Ok(()) | Err(SetError::ChangedOnDisk { .. }) => {
                                set.keep_mine(id);
                            }
                            Err(error) => panic!("{error}"),
                        }
                    }
                })
            })
            .collect();
        while !editors.iter().all(std::thread::ScopedJoinHandle::is_finished) {
            let bytes = fs::read(&path).expect("always a file");
            assert!(whole(&bytes), "a save showed {} bytes of a file", bytes.len());
        }
        for editor in editors {
            editor.join().expect("an editor");
        }
    });
    assert!(whole(&fs::read(&path).expect("reads")));
    assert_eq!(dir.left_beside(), 0, "nothing beside it");
}

#[test]
fn a_change_on_disk_is_taken_if_nothing_would_be_lost() {
    let dir = TestDir::new("taken");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    set.refresh();
    assert_eq!(text(&set, id), player());

    fs::write(&path, by_hand()).expect("writes");
    set.refresh();
    assert_eq!(text(&set, id), by_hand());
    assert!(!set.is_dirty(id));
    assert_eq!(set.conflict(id), None);
    assert_eq!(set.any(id).expect("open").undo_label(), Some("the file changed on disk"));
}

#[test]
fn a_change_on_disk_over_edits_waits_and_a_save_wont_lose_either() {
    let dir = TestDir::new("conflict");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    move_a_cue(&mut set, id, 3);
    let mine = text(&set, id);

    fs::write(&path, by_hand()).expect("writes");
    set.refresh();
    assert_eq!(set.conflict(id), Some(by_hand().as_bytes()));
    assert_eq!(text(&set, id), mine);

    let refused = set.save(id).expect_err("would lose the hand edit");
    assert!(matches!(refused, SetError::ChangedOnDisk { .. }));
    assert_eq!(fs::read_to_string(&path).expect("reads"), by_hand());

    set.keep_mine(id);
    set.save(id).expect("saves");
    assert_eq!(fs::read_to_string(&path).expect("reads"), mine);

    fs::write(&path, by_hand()).expect("writes");
    move_a_cue(&mut set, id, 4);
    set.refresh();
    set.take_disk(id);
    assert_eq!(text(&set, id), by_hand());
    assert!(!set.is_dirty(id));
    assert_eq!(set.any(id).expect("open").undo_label(), Some("took the file's text from disk"));
}

/// Bytes that aren't text are still a change. A save won't write over what it couldn't read, unless it's told
/// to, and they can't be taken.
#[test]
fn a_save_wont_replace_a_file_that_stopped_being_text() {
    let dir = TestDir::new("not_text");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    move_a_cue(&mut set, id, 3);
    let mine = text(&set, id);
    fs::write(&path, NOT_TEXT).expect("someone else's save");

    set.refresh();
    let refused = set.save(id).expect_err("it would lose those bytes");
    assert!(matches!(refused, SetError::ChangedOnDisk { .. }), "{refused}");
    assert_eq!(fs::read(&path).expect("reads"), NOT_TEXT);
    assert_eq!(text(&set, id), mine);
    assert!(set.is_dirty(id));
    assert_eq!(dir.left_beside(), 0, "nothing left beside it");
    assert_eq!(set.conflict(id), Some(NOT_TEXT.as_slice()));

    assert!(!set.take_disk(id), "it can't be taken");
    assert_eq!(set.conflict(id), Some(NOT_TEXT.as_slice()));
    assert_eq!(text(&set, id), mine);
    assert!(set.save(id).is_err());
    set.keep_mine(id);
    set.refresh();
    assert_eq!(set.conflict(id), None, "looking in passing doesn't bring it back");
    set.save(id).expect("told to");
    assert_eq!(fs::read_to_string(&path).expect("reads"), mine);
    assert!(!set.is_dirty(id));
}

/// With nothing to write it's no save either, and it doesn't say it saved.
#[test]
fn a_save_with_no_edits_says_so_of_a_file_that_stopped_being_text() {
    let dir = TestDir::new("not_text_clean");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    fs::write(&path, NOT_TEXT).expect("someone else's save");
    let refused = set.save(id).expect_err("the text here isn't what's there");
    assert!(matches!(refused, SetError::ChangedOnDisk { .. }), "{refused}");
    assert_eq!(fs::read(&path).expect("reads"), NOT_TEXT);
    assert_eq!(text(&set, id), player());

    // Put right, the conflict goes by itself, and no edit stands in the way of a save.
    fs::write(&path, player()).expect("put right");
    set.refresh();
    assert_eq!(set.conflict(id), None);
    set.save(id).expect("nothing to refuse");
}

/// A directory in the file's place is no place to write, and a look in passing doesn't mind it.
#[test]
fn a_save_refuses_a_directory_in_the_files_place() {
    let dir = TestDir::new("directory");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    move_a_cue(&mut set, id, 3);
    fs::remove_file(&path).expect("removed");
    fs::create_dir(&path).expect("a directory");
    fs::write(path.join("inside"), "someone's").expect("a file in it");
    set.refresh();
    assert!(set.conflict(id).is_none());
    assert!(matches!(set.save(id), Err(SetError::Io { .. })));
    assert_eq!(fs::read_to_string(path.join("inside")).expect("reads"), "someone's");
    assert!(set.is_dirty(id));
}

/// A file that can't be read can't be checked, so it isn't replaced: renaming over it needs no leave from the
/// file itself.
#[cfg(unix)]
#[test]
fn a_save_refuses_a_file_it_cant_read() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = TestDir::new("unreadable");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    move_a_cue(&mut set, id, 3);
    fs::write(&path, by_hand()).expect("someone else's save");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("a mode");
    if fs::read(&path).is_ok() {
        return; // Root reads anything, so there's nothing to test.
    }
    set.refresh();
    assert!(set.conflict(id).is_none());
    let refused = set.save(id);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("a mode");
    assert!(matches!(refused, Err(SetError::Io { .. })), "{refused:?}");
    assert_eq!(fs::read_to_string(&path).expect("reads"), by_hand());
}

/// With no edits here too: a save that says it saved leaves a file there, whole, and the text here was the only
/// copy left.
#[test]
fn a_save_with_no_edits_puts_back_a_file_thats_gone() {
    let dir = TestDir::new("gone_clean");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    fs::remove_file(&path).expect("removed");
    assert!(!set.is_dirty(id), "looking at the text alone");
    set.save(id).expect("saves");
    assert_eq!(fs::read_to_string(&path).expect("put back"), player());
    assert!(!set.is_dirty(id));
    assert_eq!(dir.left_beside(), 0, "nothing beside it");
}

/// A file that's gone has nothing to lose, and edits here are the only copy left: a save puts it back. Looking
/// in passing loses nothing, since a rename may be about to bring it back.
#[test]
fn a_save_puts_back_a_file_thats_gone() {
    let dir = TestDir::new("gone");
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    move_a_cue(&mut set, id, 3);
    let mine = text(&set, id);
    fs::remove_file(&path).expect("removed");
    set.refresh();
    assert_eq!(text(&set, id), mine);
    assert!(set.conflict(id).is_none());
    set.save(id).expect("saves");
    assert_eq!(fs::read_to_string(&path).expect("reads"), mine);
}

/// A link in the file's place is read through like any other change: what it leads to is what's on disk.
#[cfg(unix)]
#[test]
fn a_link_to_other_text_in_the_files_place_is_a_change() {
    let dir = TestDir::new("link_replaced");
    let other = dir.write("other", &by_hand());
    let (path, mut set, id) = dir.opened("moves.ron", &player());
    move_a_cue(&mut set, id, 3);
    fs::remove_file(&path).expect("removed");
    std::os::unix::fs::symlink(&other, &path).expect("a link");
    assert!(matches!(set.save(id), Err(SetError::ChangedOnDisk { .. })));
    assert_eq!(fs::read_to_string(&other).expect("reads"), by_hand());
    assert!(fs::symlink_metadata(&path).expect("the link").is_symlink());
}

/// A document that is a link stays one: a save writes where it leads, and the link and whatever else reads that
/// file see it.
#[cfg(unix)]
#[test]
fn a_save_writes_through_a_document_that_is_a_link() {
    let dir = TestDir::new("link_document");
    let other = dir.write("elsewhere", &player());
    let path = dir.0.join("moves.ron");
    std::os::unix::fs::symlink(&other, &path).expect("a link");
    let mut set = DocumentSet::new(vec![dir.0.clone()]);
    let id = open(&mut set, &path).expect("opens");
    move_a_cue(&mut set, id, 3);
    set.save(id).expect("saves");
    assert!(fs::symlink_metadata(&path).expect("the link").is_symlink(), "it's still a link");
    assert_eq!(fs::read_to_string(&other).expect("reads"), text(&set, id));
    assert!(!set.is_dirty(id));
}

#[test]
fn a_file_that_isnt_text_is_refused_and_a_broken_one_opens_saying_why() {
    let dir = TestDir::new("refused");
    let path = dir.write("moves.ron", "");
    fs::write(&path, [0xff, 0xfe, 0x00]).expect("writes");
    let mut set = DocumentSet::new(vec![dir.0.clone()]);
    assert!(matches!(open(&mut set, &path), Err(SetError::NotText { .. })));
    fs::write(&path, "(not a document").expect("writes");
    let id = open(&mut set, &path).expect("opens");
    assert!(set.any(id).expect("open").failure_message().is_some());
}

#[test]
fn creating_a_file_never_writes_through_a_taken_staging_name() {
    use sapphire_document::disk::create_whole;
    let dir = TestDir::new("create_collision");
    let original = dir.write("original.ron", "keep these bytes");
    let destination = dir.0.join("new.ron");
    let collision = temporary_path(&destination, 0);
    fs::hard_link(&original, &collision).expect("a retained hard link");
    // Also cover the old PID/counter staging scheme (counter zero in an isolated regression run).
    let old_collision = destination.with_file_name(format!(".new.ron.{}-0.tmp", process::id()));
    fs::hard_link(&original, &old_collision).expect("a retained old staging link");
    assert!(create_whole(&destination, b"new complete bytes").expect("create"));
    assert_eq!(fs::read_to_string(&original).expect("original"), "keep these bytes");
    assert_eq!(fs::read_to_string(&collision).expect("collision"), "keep these bytes");
    assert_eq!(fs::read_to_string(&old_collision).expect("old collision"), "keep these bytes");
    assert_eq!(fs::read(&destination).expect("published"), b"new complete bytes");
    assert!(!create_whole(&destination, b"must not replace").expect("existing"));
    assert_eq!(fs::read(&destination).expect("unchanged"), b"new complete bytes");
    assert!(!temporary_path(&destination, 1).exists());
}

#[test]
fn moving_a_file_never_overwrites_an_existing_destination() {
    use sapphire_document::disk::move_whole;
    let dir = TestDir::new("move_no_replace");
    let source = dir.write("source.ron", "source");
    let destination = dir.write("destination.ron", "destination");
    assert!(!move_whole(&source, &destination).expect("existing"));
    assert_eq!(fs::read_to_string(&source).expect("source"), "source");
    assert_eq!(fs::read_to_string(&destination).expect("destination"), "destination");
    fs::remove_file(&destination).expect("free the name");
    assert!(move_whole(&source, &destination).expect("move"));
    assert!(!source.exists());
    assert_eq!(fs::read_to_string(&destination).expect("destination"), "source");
}

#[cfg(unix)]
#[test]
fn unsupported_symlink_moves_leave_the_link_and_its_target_alone() {
    use sapphire_document::disk::move_whole;
    let dir = TestDir::new("move_symlink");
    let target = dir.write("target.ron", "target");
    let source = dir.0.join("link.ron");
    let destination = dir.0.join("destination.ron");
    std::os::unix::fs::symlink(&target, &source).expect("link");
    assert_eq!(
        move_whole(&source, &destination).expect_err("unsupported").kind(),
        std::io::ErrorKind::Unsupported
    );
    assert!(fs::symlink_metadata(&source).expect("link").is_symlink());
    assert!(!destination.exists());
    assert_eq!(fs::read_to_string(target).expect("target"), "target");
}

#[test]
fn a_changed_sidecar_identity_refuses_to_publish_edited_source_bytes() {
    use sapphire_document::identity::{self, IdentityError};
    let dir = TestDir::new("changed_identity");
    let source = player();
    let (path, mut set, id) = dir.opened("moves.ron", &source);
    move_a_cue(&mut set, id, 3);
    let sidecar = identity::sidecar_path(&path);
    let replacement = AssetId::from_bytes([8; 16]);
    assert_ne!(replacement, id);
    let other = identity::sidecar_text(replacement, Some("MoveSet(())"));
    fs::write(&sidecar, &other).expect("external identity edit");
    assert!(
        matches!(set.save(id), Err(SetError::Identity(IdentityError::Changed { was, now, .. })) if was == id && now == replacement)
    );
    assert_eq!(fs::read_to_string(&path).expect("source"), source);
    assert_eq!(fs::read_to_string(&sidecar).expect("sidecar"), other);
    assert!(set.is_dirty(id));
    fs::write(&sidecar, identity::sidecar_text(id, Some("MoveSet(())"))).expect("restore");
    set.save(id).expect("save after recovery");
    assert_eq!(fs::read_to_string(&path).expect("source"), text(&set, id));
}
