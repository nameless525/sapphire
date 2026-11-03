//! An open `.moves.ron`: its text, what's on disk, and undo.
//!
//! The file on disk is a person's too. Opening and saving with no edit leaves it byte for byte as it
//! was, a save writes exactly the editor's text, and a change made elsewhere while it's open is noticed:
//! taken if there's nothing here to lose, and asked about if there is ([`MovesFile::conflict`]). Looking
//! in passing is tolerant, since a file can be mid-save in someone's editor; a save isn't, and writes over
//! nothing it hasn't checked ([`MovesFile::save`]).
//!
//! Undo keeps change records, one per gesture, whatever the gesture did to the text
//! ([`History`]): the smallest splice that takes one text to the other, so a drag across twenty ticks is
//! one step and a long session costs little.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::{fs, io};

use sapphire_moves::{Issue, MoveSet, MoveSetDocument, MoveSetError};

use crate::edits::{EditError, MovesText};

/// A file the editor couldn't open or save.
#[derive(Debug)]
pub enum FileError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// It isn't UTF-8, so no document of moves.
    NotText {
        path: PathBuf,
    },
    /// The file changed on disk since it was read, and writing would lose what changed.
    ChangedOnDisk {
        path: PathBuf,
    },
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::NotText { path } => write!(f, "{}: isn't UTF-8 text", path.display()),
            Self::ChangedOnDisk { path } => {
                write!(f, "{} changed on disk since it was read: nothing was saved", path.display())
            }
        }
    }
}

impl std::error::Error for FileError {}

/// One step of undo: the smallest splice between two texts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub label: String,
    at: usize,
    removed: String,
    inserted: String,
}

impl Change {
    /// What takes `old` to `new`. `None` if they're the same.
    #[must_use]
    pub fn between(old: &str, new: &str, label: &str) -> Option<Self> {
        if old == new {
            return None;
        }
        let prefix = common_prefix(old, new);
        let suffix = common_suffix(&old[prefix..], &new[prefix..]);
        Some(Self {
            label: label.to_owned(),
            at: prefix,
            removed: old[prefix..old.len() - suffix].to_owned(),
            inserted: new[prefix..new.len() - suffix].to_owned(),
        })
    }

    fn undo(&self, text: &str) -> String {
        format!("{}{}{}", &text[..self.at], self.removed, &text[self.at + self.inserted.len()..])
    }

    fn redo(&self, text: &str) -> String {
        format!("{}{}{}", &text[..self.at], self.inserted, &text[self.at + self.removed.len()..])
    }
}

/// How many bytes two strings start alike, ending on a character's edge.
fn common_prefix(a: &str, b: &str) -> usize {
    let mut same = a.bytes().zip(b.bytes()).take_while(|(a, b)| a == b).count();
    while !a.is_char_boundary(same) || !b.is_char_boundary(same) {
        same -= 1;
    }
    same
}

/// How many bytes two strings end alike, starting on a character's edge.
fn common_suffix(a: &str, b: &str) -> usize {
    let mut same = a.bytes().rev().zip(b.bytes().rev()).take_while(|(a, b)| a == b).count();
    while !a.is_char_boundary(a.len() - same) || !b.is_char_boundary(b.len() - same) {
        same -= 1;
    }
    same
}

/// What's been done, and undone.
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Change>,
    redo: Vec<Change>,
}

/// An edit in progress, which one drag makes many of: one step of undo when it ends.
#[derive(Clone, Debug)]
struct Gesture {
    label: String,
    before: String,
}

/// A `.moves.ron` open in the editor.
#[derive(Debug)]
pub struct MovesFile {
    path: PathBuf,
    text: MovesText,
    /// What the file held when last read or written, as bytes: what's there now needn't be text.
    disk: Vec<u8>,
    history: History,
    gesture: Option<Gesture>,
    /// What the file held when it changed on disk while there were edits here, or changed to something
    /// that can't be taken.
    conflict: Option<Vec<u8>>,
}

/// A change on disk waiting to be settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict<'a> {
    /// What the file says now, which [`MovesFile::take_disk`] can take.
    Text(&'a str),
    /// It isn't text any more, so all a person can do here is keep what they have.
    NotText,
}

impl MovesFile {
    /// Reads `path`. A file that doesn't parse opens all the same, saying why, since a person may be
    /// halfway through an edit of it.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, FileError> {
        let path = path.into();
        let disk = read_text(&path)?;
        Ok(Self {
            text: MovesText::new(disk.clone()),
            path,
            disk: disk.into_bytes(),
            history: History::default(),
            gesture: None,
            conflict: None,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What a push goes by.
    #[must_use]
    pub fn file_name(&self) -> String {
        self.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    }

    #[must_use]
    pub fn moves_text(&self) -> &MovesText {
        &self.text
    }

    #[must_use]
    pub fn text(&self) -> &str {
        self.text.text()
    }

    /// Whether the text holds edits the file doesn't.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.text.text().as_bytes() != self.disk
    }

    /// What the file changed to on disk, while there are edits here to lose or it can't be taken.
    #[must_use]
    pub fn conflict(&self) -> Option<Conflict<'_>> {
        let on_disk = self.conflict.as_deref()?;
        Some(std::str::from_utf8(on_disk).map_or(Conflict::NotText, Conflict::Text))
    }

    /// One edit, as one step of undo. Inside a gesture, the gesture's step takes it.
    pub fn edit<T>(
        &mut self,
        label: &str,
        edit: impl FnOnce(&mut MovesText) -> Result<T, EditError>,
    ) -> Result<T, EditError> {
        let before = (self.gesture.is_none()).then(|| self.text.text().to_owned());
        let result = edit(&mut self.text)?;
        if let Some(before) = before {
            self.record(&before, label);
        }
        Ok(result)
    }

    /// Starts a gesture: edits until [`Self::end_gesture`] are one step of undo. One in progress ends, as a
    /// step of its own.
    pub fn begin_gesture(&mut self, label: &str) {
        self.end_gesture();
        self.gesture = Some(Gesture { label: label.to_owned(), before: self.text.text().to_owned() });
    }

    pub fn end_gesture(&mut self) {
        if let Some(gesture) = self.gesture.take() {
            self.record(&gesture.before, &gesture.label);
        }
    }

    /// Ends a gesture by taking back what it did, leaving no step of undo.
    pub fn cancel_gesture(&mut self) {
        if let Some(gesture) = self.gesture.take() {
            self.text.set_text(gesture.before);
        }
    }

    /// Whether a gesture is in progress.
    #[must_use]
    pub fn in_gesture(&self) -> bool {
        self.gesture.is_some()
    }

    fn record(&mut self, before: &str, label: &str) {
        if let Some(change) = Change::between(before, self.text.text(), label) {
            self.history.undo.push(change);
            self.history.redo.clear();
        }
    }

    /// What undo would take back.
    #[must_use]
    pub fn undo_label(&self) -> Option<&str> {
        self.history.undo.last().map(|change| change.label.as_str())
    }

    /// What redo would bring back.
    #[must_use]
    pub fn redo_label(&self) -> Option<&str> {
        self.history.redo.last().map(|change| change.label.as_str())
    }

    pub fn undo(&mut self) -> bool {
        self.end_gesture();
        let Some(change) = self.history.undo.pop() else { return false };
        self.text.set_text(change.undo(self.text.text()));
        self.history.redo.push(change);
        true
    }

    pub fn redo(&mut self) -> bool {
        self.end_gesture();
        let Some(change) = self.history.redo.pop() else { return false };
        self.text.set_text(change.redo(self.text.text()));
        self.history.undo.push(change);
        true
    }

    /// Writes the text if it holds edits, in one step, so a watcher or a crash never sees half a file.
    /// A file with nothing to save isn't touched. One changed on disk since it was read is refused,
    /// whatever it changed to: [`Self::conflict`] says what, and [`Self::take_disk`] or [`Self::keep_mine`]
    /// settles it. One that can't be read is refused too, since there's no telling what would be lost,
    /// and one that's gone is written again, edits or none, since nothing would be and this is the only
    /// copy. A document that's a link stays one: the file it leads to is what's replaced.
    ///
    /// The file is read as late as it can be, with the new text already written beside it. What's left is
    /// the moment before the rename, where another save lands and loses to this one whole.
    pub fn save(&mut self) -> Result<(), FileError> {
        self.end_gesture();
        let path = self.path.clone();
        let io_error = |source| FileError::Io { path: path.clone(), source };
        let target = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let gone = fs::metadata(&path).is_err_and(|error| error.kind() == io::ErrorKind::NotFound);
        let replacement = (gone || self.is_dirty())
            .then(|| Temporary::beside(&target, self.text.text()))
            .transpose()
            .map_err(io_error)?;
        let there = self.check_disk()?;
        if let Some(replacement) = replacement
            && (!there || self.is_dirty())
        {
            replacement.replace(&target).map_err(io_error)?;
            self.disk = self.text.text().as_bytes().to_vec();
        }
        Ok(())
    }

    /// What a save asks of the file before writing, which a look in passing doesn't: that nothing it can't
    /// see goes unseen. Says whether there's a file there.
    fn check_disk(&mut self) -> Result<bool, FileError> {
        let there = match fs::read(&self.path) {
            Ok(on_disk) => {
                self.settle_disk(on_disk);
                true
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => false,
            Err(source) => return Err(FileError::Io { path: self.path.clone(), source }),
        };
        match self.conflict {
            Some(_) => Err(FileError::ChangedOnDisk { path: self.path.clone() }),
            None => Ok(there),
        }
    }

    /// Looks at the file again. A change made elsewhere is taken as an edit that undo can take back,
    /// unless there are edits here too: then it waits in [`Self::conflict`]. A file that's missing,
    /// unreadable or not text changes nothing here: it may be mid-save in someone's editor. A save looks
    /// harder.
    pub fn poll_disk(&mut self) {
        if self.gesture.is_some() {
            return;
        }
        let Ok(on_disk) = read_text(&self.path) else { return };
        self.settle_disk(on_disk.into_bytes());
    }

    fn settle_disk(&mut self, on_disk: Vec<u8>) {
        if on_disk == self.disk {
            self.conflict = None;
        } else if on_disk == self.text.text().as_bytes() {
            self.disk = on_disk;
            self.conflict = None;
        } else if self.is_dirty() {
            self.conflict = Some(on_disk);
        } else {
            match String::from_utf8(on_disk) {
                Ok(on_disk) => {
                    let before = self.text.text().to_owned();
                    self.text.set_text(on_disk.clone());
                    self.disk = on_disk.into_bytes();
                    self.conflict = None;
                    self.record(&before, "the file changed on disk");
                }
                Err(error) => self.conflict = Some(error.into_bytes()),
            }
        }
    }

    /// Settles a conflict by taking the file's text, as an edit undo can take back. Bytes that aren't
    /// text can't be taken, and wait.
    pub fn take_disk(&mut self) {
        let Some(on_disk) = self.conflict.take() else { return };
        match String::from_utf8(on_disk) {
            Ok(on_disk) => {
                let before = self.text.text().to_owned();
                self.text.set_text(on_disk.clone());
                self.disk = on_disk.into_bytes();
                self.record(&before, "took the file's text from disk");
            }
            Err(error) => self.conflict = Some(error.into_bytes()),
        }
    }

    /// Settles a conflict by keeping the text here: the next save overwrites the file.
    pub fn keep_mine(&mut self) {
        if let Some(on_disk) = self.conflict.take() {
            self.disk = on_disk;
        }
    }
}

fn read_text(path: &Path) -> Result<String, FileError> {
    let bytes = fs::read(path).map_err(|source| FileError::Io { path: path.to_owned(), source })?;
    String::from_utf8(bytes).map_err(|_| FileError::NotText { path: path.to_owned() })
}

/// How many names a save tries for its temporary. Each one taken is someone else's file.
const TEMPORARY_NAMES: u32 = 1024;

/// A file of one save's own, written whole beside the one it's to replace and renamed over it: all of it
/// or none of it, keeping the file's mode. Only a name nothing's at will do, so whatever is there already, another save's temporary, a person's file or a link to anything, is
/// left alone, and another editor's save can't write into this one or rename it away. Dropped before it
/// replaces anything, it's gone, and only it.
#[derive(Debug)]
struct Temporary {
    path: PathBuf,
    replaced: bool,
}

impl Temporary {
    fn beside(target: &Path, text: &str) -> io::Result<Self> {
        let (mut file, temporary) = Self::create(target)?;
        if let Ok(metadata) = fs::metadata(target) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        Ok(temporary)
    }

    fn create(target: &Path) -> io::Result<(fs::File, Self)> {
        for attempt in 0..TEMPORARY_NAMES {
            let path = temporary_path(target, attempt);
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((file, Self { path, replaced: false })),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "no name left for a temporary beside it"))
    }

    fn replace(mut self, target: &Path) -> io::Result<()> {
        fs::rename(&self.path, target)?;
        self.replaced = true;
        Ok(())
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.replaced {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// A hidden sibling, which the cook and a watcher's documents pass over. The process's ID keeps two editors
/// off each other's names, for speed: `create_new` is what makes a name this save's own.
fn temporary_path(target: &Path, attempt: u32) -> PathBuf {
    let name = target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    target.with_file_name(format!(".{name}.{}.{attempt}.sapphire_editor", std::process::id()))
}

/// What the move system says of a document, as a device would: its issues, and the set it became.
#[derive(Clone, Debug, Default)]
pub struct Verdict {
    /// Everything wrong with it, each naming its move and track.
    pub issues: Vec<Issue>,
    /// What it compiled to. `None` with issues, or with moves that name clips and no cook to read their
    /// tables from.
    pub set: Option<MoveSet>,
}

/// Compiles `document` as live tuning does (`sapphire_gameplay::replay::compile_document`), reading what
/// its moves' clips gave them in `running`, the set a cook made of it. With moves that name clips and no
/// cook, it can only check what the document can say by itself.
#[must_use]
pub fn judge(document: &MoveSetDocument, running: Option<&MoveSet>) -> Verdict {
    let compiled = match running {
        Some(running) => document.compile_over(running),
        None if document.names_clips() => return Verdict { issues: document.check(), set: None },
        None => document.compile(),
    };
    match compiled {
        Ok(set) => Verdict { issues: Vec::new(), set: Some(set) },
        Err(MoveSetError::Invalid(issues)) => Verdict { issues, set: None },
        Err(error) => Verdict {
            issues: vec![Issue { in_move: None, track: "moves", message: error.to_string() }],
            set: None,
        },
    }
}

#[cfg(test)]
pub(crate) mod test_dir {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::{env, fs, process};

    /// A directory of a test's own, gone when it ends.
    pub struct TestDir(PathBuf);

    impl TestDir {
        pub fn new(name: &str) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = env::temp_dir().join(format!("sapphire_editor_{name}_{}_{unique}", process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("a temp dir");
            Self(dir)
        }

        pub fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, text).expect("a test file");
            path
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_dir::TestDir;
    use super::*;
    use crate::edits::test_files::{ALL, PLAYER};
    use crate::edits::{EntryRef, Track};

    fn cue(index: usize) -> EntryRef {
        EntryRef { mv: 0, track: Track::Cues, index }
    }

    /// The brief's first promise: open and save with no edit, and the file is what it was, on the real
    /// documents, comments and layout included.
    #[test]
    fn a_round_trip_with_no_edit_leaves_the_real_files_byte_identical() {
        for (name, source) in ALL {
            let dir = TestDir::new("round_trip");
            let path = dir.write("moves.ron", source);
            let mut file = MovesFile::open(&path).expect("opens");
            assert!(!file.is_dirty(), "{name}");
            file.save().expect("saves");
            assert_eq!(fs::read(&path).expect("reads"), source.as_bytes(), "{name}");
            // And an edit undone is no edit, whatever it did on the way.
            file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
            assert!(file.is_dirty(), "{name}");
            assert!(file.undo());
            assert!(!file.is_dirty(), "{name}");
            file.save().expect("saves");
            assert_eq!(fs::read(&path).expect("reads"), source.as_bytes(), "{name}");
            assert_eq!(fs::read_dir(dir.path()).expect("lists").count(), 1, "{name} left nothing beside it");
        }
    }

    /// A conflict settled by undoing the edits here is settled: the file's text is taken, and the next
    /// save has nothing to refuse.
    #[test]
    fn undoing_back_to_the_file_settles_a_conflict() {
        let dir = TestDir::new("conflict_undone");
        let path = dir.write("moves.ron", PLAYER);
        let mut file = MovesFile::open(&path).expect("opens");
        file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
        let elsewhere = PLAYER.replacen("name:", "name :", 1);
        fs::write(&path, &elsewhere).expect("someone else's save");
        file.poll_disk();
        assert!(file.conflict().is_some());
        assert!(file.undo());
        assert!(!file.is_dirty());
        file.poll_disk();
        assert_eq!(file.text(), elsewhere);
        assert!(file.conflict().is_none(), "nothing's in conflict");
        file.save().expect("nothing to refuse");
    }

    #[test]
    fn a_save_writes_exactly_the_editors_text() {
        let dir = TestDir::new("save");
        let path = dir.write("moves.ron", PLAYER);
        let mut file = MovesFile::open(&path).expect("opens");
        file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
        file.save().expect("saves");
        assert!(!file.is_dirty());
        assert_eq!(fs::read_to_string(&path).expect("reads"), file.text());
        assert_eq!(fs::read_dir(dir.path()).expect("lists").count(), 1);
    }

    /// The name every save used to write through: what's there isn't a save's to touch.
    #[test]
    fn a_save_leaves_a_file_at_an_old_temporary_name_alone() {
        let dir = TestDir::new("old_temporary");
        let path = dir.write("moves.ron", PLAYER);
        let there = dir.write(".moves.ron.sapphire_editor", "someone's");
        let mut file = MovesFile::open(&path).expect("opens");
        file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
        file.save().expect("saves");
        assert_eq!(fs::read_to_string(&there).expect("still there"), "someone's");
        assert_eq!(fs::read_to_string(&path).expect("reads"), file.text());
    }

    /// Nor does a link there lead a save to what it points at, or end up over the document.
    #[cfg(unix)]
    #[test]
    fn a_save_doesnt_follow_a_link_at_an_old_temporary_name() {
        let dir = TestDir::new("old_temporary_link");
        let path = dir.write("moves.ron", PLAYER);
        let unrelated = dir.write("unrelated", "someone's");
        std::os::unix::fs::symlink(&unrelated, dir.path().join(".moves.ron.sapphire_editor"))
            .expect("a link");
        let mut file = MovesFile::open(&path).expect("opens");
        file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
        file.save().expect("saves");
        assert_eq!(fs::read_to_string(&unrelated).expect("reads"), "someone's");
        assert!(!fs::symlink_metadata(&path).expect("the document").is_symlink());
        assert_eq!(fs::read_to_string(&path).expect("reads"), file.text());
    }

    /// A name a save would use, taken, is passed over: it writes through a file nothing else was at.
    #[test]
    fn a_save_passes_over_names_that_are_taken() {
        let dir = TestDir::new("taken_names");
        let path = dir.write("moves.ron", PLAYER);
        let first = temporary_path(&path, 0);
        fs::write(&first, "someone's").expect("a file there");
        #[cfg(unix)]
        let unrelated = {
            let unrelated = dir.write("unrelated", "someone's");
            std::os::unix::fs::symlink(&unrelated, temporary_path(&path, 1)).expect("a link there");
            unrelated
        };
        let mut file = MovesFile::open(&path).expect("opens");
        file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
        file.save().expect("saves");
        assert_eq!(fs::read_to_string(&path).expect("reads"), file.text());
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
        let path = dir.write("moves.ron", PLAYER);
        let there = temporary_path(&path, 0);
        fs::write(&there, "someone's").expect("a file there");
        let temporary = Temporary::beside(&path, "mine").expect("written");
        assert_eq!(temporary.path, temporary_path(&path, 1));
        assert_eq!(fs::read_to_string(&temporary.path).expect("reads"), "mine");
        drop(temporary);
        assert!(!temporary_path(&path, 1).exists());
        assert_eq!(fs::read_to_string(&there).expect("still there"), "someone's");
        assert_eq!(fs::read_to_string(&path).expect("reads"), PLAYER);
    }

    /// The file's mode goes with it.
    #[cfg(unix)]
    #[test]
    fn a_save_keeps_the_files_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = TestDir::new("mode");
        let path = dir.write("moves.ron", PLAYER);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("a mode");
        let mut file = MovesFile::open(&path).expect("opens");
        file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
        file.save().expect("saves");
        assert_eq!(fs::metadata(&path).expect("the file").permissions().mode() & 0o777, 0o640);
    }

    /// Editors saving one file at once: each is whole or refused, whoever reads it sees a whole file, and
    /// no temporary is left, however the saves fall. Windows can fail a rename over a file someone's reading,
    /// which is a save refused, not a broken file, and says nothing about this.
    #[cfg(unix)]
    #[test]
    fn saves_racing_each_other_leave_a_whole_file() {
        const EDITORS: u32 = 4;
        const SAVES: u32 = 25;
        let dir = TestDir::new("racing");
        let path = dir.write("moves.ron", PLAYER);
        let whole = |bytes: &[u8]| {
            let (head, tail) = (&PLAYER.as_bytes()[..200], &PLAYER.as_bytes()[PLAYER.len() - 200..]);
            bytes.starts_with(head) && bytes.ends_with(tail)
        };
        std::thread::scope(|scope| {
            let editors: Vec<_> = (0..EDITORS)
                .map(|editor| {
                    let path = &path;
                    scope.spawn(move || {
                        let mut file = MovesFile::open(path).expect("opens");
                        for save in 0..SAVES {
                            let at = editor * 1000 + save;
                            file.edit("move a cue", |text| text.set_instant(cue(0), at)).expect("the edit");
                            match file.save() {
                                Ok(()) | Err(FileError::ChangedOnDisk { .. }) => file.keep_mine(),
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
        assert_eq!(fs::read_dir(dir.path()).expect("lists").count(), 1, "nothing beside it");
    }

    #[test]
    fn undo_and_redo_walk_edits_and_a_drag_is_one_step() {
        let dir = TestDir::new("undo");
        let path = dir.write("moves.ron", PLAYER);
        let mut file = MovesFile::open(&path).expect("opens");

        file.begin_gesture("drag a cue");
        for at in 1..=9 {
            file.edit("ignored", |text| text.set_instant(cue(0), at)).expect("the edit");
        }
        file.end_gesture();
        let dragged = file.text().to_owned();
        assert_eq!(file.undo_label(), Some("drag a cue"));

        file.edit("move another", |text| text.set_instant(cue(1), 2)).expect("the edit");
        let moved = file.text().to_owned();

        assert!(file.undo());
        assert_eq!(file.text(), dragged);
        assert!(file.undo());
        assert_eq!(file.text(), PLAYER);
        assert!(!file.undo());
        assert_eq!(file.redo_label(), Some("drag a cue"));
        assert!(file.redo());
        assert_eq!(file.text(), dragged);
        assert!(file.redo());
        assert_eq!(file.text(), moved);
        assert!(!file.redo());

        // A new edit forgets what was undone.
        assert!(file.undo());
        file.edit("something else", |text| text.set_instant(cue(1), 3)).expect("the edit");
        assert_eq!(file.redo_label(), None);
    }

    #[test]
    fn a_change_is_the_smallest_splice_and_never_cuts_a_character() {
        let change = Change::between("from: 7, é", "from: 12, é", "x").expect("a change");
        assert_eq!((change.at, change.removed.as_str(), change.inserted.as_str()), (6, "7", "12"));
        assert_eq!(change.undo("from: 12, é"), "from: 7, é");
        assert_eq!(change.redo("from: 7, é"), "from: 12, é");
        // Two characters sharing a first byte and a last one.
        let change = Change::between("é", "è", "x").expect("a change");
        assert_eq!(change.undo("è"), "é");
        assert!(Change::between("same", "same", "x").is_none());
    }

    #[test]
    fn a_change_on_disk_is_taken_if_nothing_would_be_lost() {
        let dir = TestDir::new("taken");
        let path = dir.write("moves.ron", PLAYER);
        let mut file = MovesFile::open(&path).expect("opens");
        file.poll_disk();
        assert_eq!(file.text(), PLAYER);

        let by_hand = PLAYER.replacen("startup: 7", "startup: 8", 1);
        fs::write(&path, &by_hand).expect("writes");
        file.poll_disk();
        assert_eq!(file.text(), by_hand);
        assert!(!file.is_dirty());
        assert_eq!(file.conflict(), None);
        assert_eq!(file.undo_label(), Some("the file changed on disk"));
    }

    #[test]
    fn a_change_on_disk_over_edits_waits_and_a_save_wont_lose_either() {
        let dir = TestDir::new("conflict");
        let path = dir.write("moves.ron", PLAYER);
        let mut file = MovesFile::open(&path).expect("opens");
        file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
        let mine = file.text().to_owned();

        let by_hand = PLAYER.replacen("startup: 7", "startup: 8", 1);
        fs::write(&path, &by_hand).expect("writes");
        file.poll_disk();
        assert_eq!(file.conflict(), Some(Conflict::Text(by_hand.as_str())));
        assert_eq!(file.text(), mine);

        let refused = file.save().expect_err("would lose the hand edit");
        assert!(matches!(refused, FileError::ChangedOnDisk { .. }));
        assert_eq!(fs::read_to_string(&path).expect("reads"), by_hand);

        file.keep_mine();
        file.save().expect("saves");
        assert_eq!(fs::read_to_string(&path).expect("reads"), mine);

        fs::write(&path, &by_hand).expect("writes");
        file.edit("move it again", |text| text.set_instant(cue(0), 4)).expect("the edit");
        file.poll_disk();
        file.take_disk();
        assert_eq!(file.text(), by_hand);
        assert!(!file.is_dirty());
        assert_eq!(file.undo_label(), Some("took the file's text from disk"));
    }

    const NOT_TEXT: [u8; 3] = [0xff, 0xfe, 0x42];

    /// Bytes that aren't text are still a change. Looking in passing leaves them be, since someone may be
    /// halfway through a save; a save won't write over what it couldn't read, unless it's told to.
    #[test]
    fn a_save_wont_replace_a_file_that_stopped_being_text() {
        let dir = TestDir::new("not_text");
        let path = dir.write("moves.ron", PLAYER);
        let mut file = MovesFile::open(&path).expect("opens");
        file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
        let mine = file.text().to_owned();
        fs::write(&path, NOT_TEXT).expect("someone else's save");

        file.poll_disk();
        assert!(file.conflict().is_none(), "looking in passing says nothing yet");
        let refused = file.save().expect_err("it would lose those bytes");
        assert!(matches!(refused, FileError::ChangedOnDisk { .. }), "{refused}");
        assert_eq!(fs::read(&path).expect("reads"), NOT_TEXT);
        assert_eq!(file.text(), mine);
        assert!(file.is_dirty());
        assert_eq!(fs::read_dir(dir.path()).expect("lists").count(), 1, "nothing left beside it");
        assert_eq!(file.conflict(), Some(Conflict::NotText));

        // It can't be taken, and it still can't be saved over, until someone says so.
        file.take_disk();
        assert_eq!(file.conflict(), Some(Conflict::NotText));
        assert_eq!(file.text(), mine);
        assert!(file.save().is_err());
        file.keep_mine();
        file.poll_disk();
        assert_eq!(file.conflict(), None, "looking in passing doesn't bring it back");
        file.save().expect("told to");
        assert_eq!(fs::read_to_string(&path).expect("reads"), mine);
        assert!(!file.is_dirty());
    }

    /// With nothing to write it's no save either, and it doesn't say it saved.
    #[test]
    fn a_save_with_no_edits_says_so_of_a_file_that_stopped_being_text() {
        let dir = TestDir::new("not_text_clean");
        let path = dir.write("moves.ron", PLAYER);
        let mut file = MovesFile::open(&path).expect("opens");
        fs::write(&path, NOT_TEXT).expect("someone else's save");
        let refused = file.save().expect_err("the text here isn't what's there");
        assert!(matches!(refused, FileError::ChangedOnDisk { .. }), "{refused}");
        assert_eq!(fs::read(&path).expect("reads"), NOT_TEXT);
        assert_eq!(file.text(), PLAYER);

        // Put right, the conflict goes by itself, and no edit stands in the way of a save.
        fs::write(&path, PLAYER).expect("put right");
        file.poll_disk();
        assert_eq!(file.conflict(), None);
        file.save().expect("nothing to refuse");
    }

    /// A directory in the file's place is no place to write, and a look in passing doesn't mind it.
    #[test]
    fn a_save_refuses_a_directory_in_the_files_place() {
        let dir = TestDir::new("directory");
        let path = dir.write("moves.ron", PLAYER);
        let mut file = MovesFile::open(&path).expect("opens");
        file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
        fs::remove_file(&path).expect("removed");
        fs::create_dir(&path).expect("a directory");
        fs::write(path.join("inside"), "someone's").expect("a file in it");
        file.poll_disk();
        assert!(file.conflict().is_none());
        assert!(matches!(file.save(), Err(FileError::Io { .. })));
        assert_eq!(fs::read_to_string(path.join("inside")).expect("reads"), "someone's");
        assert!(file.is_dirty());
    }

    /// A file that can't be read can't be checked, so it isn't replaced: renaming over it needs no leave
    /// from the file itself.
    #[cfg(unix)]
    #[test]
    fn a_save_refuses_a_file_it_cant_read() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = TestDir::new("unreadable");
        let path = dir.write("moves.ron", PLAYER);
        let mut file = MovesFile::open(&path).expect("opens");
        file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
        let by_hand = PLAYER.replacen("startup: 7", "startup: 8", 1);
        fs::write(&path, &by_hand).expect("someone else's save");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("a mode");
        if fs::read(&path).is_ok() {
            return; // Root reads anything, so there's nothing to test.
        }
        file.poll_disk();
        assert!(file.conflict().is_none());
        let refused = file.save();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("a mode");
        assert!(matches!(refused, Err(FileError::Io { .. })), "{refused:?}");
        assert_eq!(fs::read_to_string(&path).expect("reads"), by_hand);
    }

    /// With no edits here too: a save that says it saved leaves a file there, whole, and the text here was
    /// the only copy left. A file that's there and holds the text isn't touched.
    #[test]
    fn a_save_with_no_edits_puts_back_a_file_thats_gone() {
        let dir = TestDir::new("gone_clean");
        let path = dir.write("moves.ron", PLAYER);
        let mut file = MovesFile::open(&path).expect("opens");
        fs::remove_file(&path).expect("removed");
        assert!(!file.is_dirty(), "looking at the text alone");
        file.save().expect("saves");
        assert_eq!(fs::read_to_string(&path).expect("put back"), PLAYER);
        assert!(!file.is_dirty());
        assert_eq!(fs::read_dir(dir.path()).expect("lists").count(), 1, "nothing beside it");
    }

    /// A file that's gone has nothing to lose, and edits here are the only copy left: a save puts it back.
    /// Looking in passing keeps quiet, since a rename may be about to bring it back.
    #[test]
    fn a_save_puts_back_a_file_thats_gone() {
        let dir = TestDir::new("gone");
        let path = dir.write("moves.ron", PLAYER);
        let mut file = MovesFile::open(&path).expect("opens");
        file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
        let mine = file.text().to_owned();
        fs::remove_file(&path).expect("removed");
        file.poll_disk();
        assert_eq!(file.text(), mine);
        assert!(file.conflict().is_none());
        file.save().expect("saves");
        assert_eq!(fs::read_to_string(&path).expect("reads"), mine);
    }

    /// A link in the file's place is read through like any other change: what it leads to is what's on disk.
    #[cfg(unix)]
    #[test]
    fn a_link_to_other_text_in_the_files_place_is_a_change() {
        let dir = TestDir::new("link_replaced");
        let path = dir.write("moves.ron", PLAYER);
        let by_hand = PLAYER.replacen("startup: 7", "startup: 8", 1);
        let other = dir.write("other", &by_hand);
        let mut file = MovesFile::open(&path).expect("opens");
        file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
        fs::remove_file(&path).expect("removed");
        std::os::unix::fs::symlink(&other, &path).expect("a link");
        assert!(matches!(file.save(), Err(FileError::ChangedOnDisk { .. })));
        assert_eq!(fs::read_to_string(&other).expect("reads"), by_hand);
        assert!(fs::symlink_metadata(&path).expect("the link").is_symlink());
    }

    /// A document that is a link stays one: a save writes where it leads, and the link and whatever else
    /// reads that file see it.
    #[cfg(unix)]
    #[test]
    fn a_save_writes_through_a_document_that_is_a_link() {
        let dir = TestDir::new("link_document");
        let other = dir.write("elsewhere", PLAYER);
        let path = dir.path().join("moves.ron");
        std::os::unix::fs::symlink(&other, &path).expect("a link");
        let mut file = MovesFile::open(&path).expect("opens");
        file.edit("move a cue", |text| text.set_instant(cue(0), 3)).expect("the edit");
        file.save().expect("saves");
        assert!(fs::symlink_metadata(&path).expect("the link").is_symlink(), "it's still a link");
        assert_eq!(fs::read_to_string(&other).expect("reads"), file.text());
        assert!(!file.is_dirty());
    }

    #[test]
    fn a_file_that_isnt_text_is_refused_and_a_broken_one_opens_saying_why() {
        let dir = TestDir::new("refused");
        let path = dir.path().join("moves.ron");
        fs::write(&path, [0xff, 0xfe, 0x00]).expect("writes");
        assert!(matches!(MovesFile::open(&path), Err(FileError::NotText { .. })));
        fs::write(&path, "(not a document").expect("writes");
        let file = MovesFile::open(&path).expect("opens");
        assert!(file.moves_text().failure().is_some());
    }

    #[test]
    fn a_verdict_names_move_and_track_by_the_documents_own_words() {
        let document = MoveSetDocument::parse(&PLAYER.replacen(
            "from: 7,\n                        to: 11",
            "from: 12,\n                        to: 11",
            1,
        ))
        .expect("parses");
        let verdict = judge(&document, None);
        let issue = verdict.issues.iter().find(|issue| issue.track == "hitboxes").expect("the issue");
        assert_eq!(issue.in_move.as_deref(), Some("light_1"));
        assert_eq!(issue.message, "window 12..11 is empty");
        assert!(verdict.set.is_none());
    }
}
