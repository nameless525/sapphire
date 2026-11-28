//! The journal (M6 T3): what an editor hasn't saved, kept on disk after each step, so an editor that stops
//! without saving, by a crash or a kill, loses none of it.
//!
//! Each editor keeps a session of its own under the journal's folder (`target/editor_journal/` in a
//! repository), a folder holding a [`JournalEntry`] for each document with unsaved bytes, written whole through a
//! [`Temporary`] and a rename, so a kill mid-write leaves the entry before it. A session holds a lock on its
//! folder for as long as it runs, which the operating system lets go of however the editor stops, so an editor
//! starting knows which sessions ended without closing: their entries are its [`Offer`]s, and it holds their
//! locks too while it offers them, so two editors starting together don't offer the same text twice. A session
//! that closes, because its editor quit with nothing unsaved or a person chose to lose it, takes its folder with
//! it.
//!
//! An entry is the text, the hash of the bytes its edits were made over, and those edits as the steps history
//! holds, which is what the document model's rebase lays back on a file that moved on
//! ([`crate::DocumentSet::recover`]). Nothing here writes a source: an offer taken becomes a document with
//! unsaved bytes, or one in conflict with its file, and only a save writes it.

use std::collections::BTreeMap;
use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sapphire_assets::AssetId;
use serde::{Deserialize, Serialize};

use crate::disk::Temporary;
use crate::record::HistoryStep;
use crate::set::DocumentSet;

/// What a new entry's `version` says: 2 holds its text's hash ([`JournalEntry::text_hash`]).
pub const JOURNAL_VERSION: u32 = 2;
/// The versions an entry may say: 1, the first, had no hash of its text, and still reads.
const READS_VERSIONS: [u32; 2] = [1, JOURNAL_VERSION];
/// What an entry's file name ends with.
const ENTRY_SUFFIX: &str = ".journal.ron";
/// The file a session's lock is held on.
const LOCK: &str = "session.lock";
/// How long before a new session's start a session with no lock was last touched, for it to be one killed
/// while it was being made, never one being made now: its folder goes. Made takes well under a millisecond.
pub const STALE_MAKING: Duration = Duration::from_secs(60);
/// How many names a new session tries. Each one taken is a session that ran with this process's ID before.
const SESSION_NAMES: u32 = 1024;

/// One document's unsaved text, as a journal keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalEntry {
    /// [`JOURNAL_VERSION`].
    pub version: u32,
    /// The document's ID, from its sidecar.
    pub id: AssetId,
    /// Where its file was.
    pub path: PathBuf,
    /// The blake3 hash, in hex, of the bytes the file held when its edits began: what says whether the file
    /// has moved on since.
    pub disk: String,
    /// The text, whole.
    pub text: String,
    /// The blake3 hash, in hex, of the text: a version 2 entry whose text doesn't have it doesn't read, so a
    /// text changed on disk is never taken back. Version 1 entries have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_hash: Option<String>,
    pub unsaved: Unsaved,
}

/// What a journal knows of the edits between the file's bytes and an entry's text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unsaved {
    /// History's steps past the last save, oldest first, a gesture under way the last of them: undoing them
    /// from the text gives the bytes the hash names, and a rebase lays them on a file that moved on.
    Steps(Vec<HistoryStep>),
    /// History couldn't say which edits weren't saved (after an undo past a save and a new edit, say), so this
    /// is the text they were made over, whole, and a rebase lays nothing back.
    Over(String),
}

impl JournalEntry {
    /// The entry for a document's text and what its history says.
    #[must_use]
    pub fn new(
        id: AssetId,
        path: PathBuf,
        disk: &[u8],
        text: String,
        steps: Option<Vec<HistoryStep>>,
    ) -> Self {
        let unsaved = match steps {
            Some(steps) => Unsaved::Steps(steps),
            None => Unsaved::Over(String::from_utf8_lossy(disk).into_owned()),
        };
        let text_hash = Some(hash_of(text.as_bytes()));
        Self { version: JOURNAL_VERSION, id, path, disk: hash_of(disk), text, text_hash, unsaved }
    }

    /// Reads an entry's text. One of a version this editor doesn't read, with anything it doesn't know, or (from
    /// version 2) whose text doesn't have the hash it holds, is refused.
    pub fn parse(text: &str) -> Result<Self, String> {
        /// Only the version, passing over the rest. It's read first, so an entry of a later version is refused as
        /// that, not by the first field it has that this editor doesn't know.
        #[derive(Deserialize)]
        struct Version {
            version: u32,
        }
        let refuse = |version| {
            format!("an entry of version {version}, where this editor reads versions 1 to {JOURNAL_VERSION}")
        };
        if let Ok(Version { version }) = ron::from_str(text)
            && !READS_VERSIONS.contains(&version)
        {
            return Err(refuse(version));
        }
        let entry: Self = ron::from_str(text).map_err(|error| error.to_string())?;
        match (entry.version, &entry.text_hash) {
            (1, None) => {}
            (1, Some(_)) => {
                return Err("a version 1 entry with a hash of its text, which only 2 has".to_owned());
            }
            (JOURNAL_VERSION, Some(hash)) if *hash == hash_of(entry.text.as_bytes()) => {}
            (JOURNAL_VERSION, Some(_)) => return Err("its text doesn't have the hash it holds".to_owned()),
            (JOURNAL_VERSION, None) => return Err("a version 2 entry with no hash of its text".to_owned()),
            (version, _) => return Err(refuse(version)),
        }
        Ok(entry)
    }

    /// The entry's text, as its file holds it.
    #[must_use]
    pub fn to_text(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).expect("an entry is plain data")
    }

    /// The bytes its edits were made over, got back from its text and its steps, if they agree with the hash
    /// it holds, and the text with its own hash if it holds one.
    pub fn base(&self) -> Result<String, String> {
        if let Some(hash) = &self.text_hash
            && *hash != hash_of(self.text.as_bytes())
        {
            return Err("its text doesn't have the hash it holds".to_owned());
        }
        let base = match &self.unsaved {
            Unsaved::Steps(steps) => {
                let mut text = self.text.clone();
                for step in steps.iter().rev() {
                    text = step
                        .change
                        .undo(&text)
                        .ok_or_else(|| format!("its step `{}` doesn't undo from its text", step.label))?;
                }
                text
            }
            Unsaved::Over(base) => base.clone(),
        };
        if hash_of(base.as_bytes()) != self.disk {
            return Err("what its edits were made over doesn't have the hash it holds".to_owned());
        }
        Ok(base)
    }
}

/// The blake3 hash of `bytes`, in hex.
#[must_use]
pub fn hash_of(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Unsaved text from a session that ended without closing, waiting for a person to take it or let it go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub entry: JournalEntry,
    /// The entry's file.
    file: PathBuf,
}

/// What a journal has done, for a panel and a log (R9): the editor logs [`Self::counters`] when its session
/// closes. The plan's row is one write a step: a step that changes no document's text writes nothing, and an
/// editor writes once a frame at most, however many steps the frame took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JournalStats {
    /// Entries written whole since it opened.
    pub writes: u64,
    /// Their bytes.
    pub bytes: u64,
    /// Entries whose write failed since it opened: each one tried.
    pub failed: u64,
    /// Entries this session holds now.
    pub entries: usize,
    /// Offers not yet taken or let go.
    pub offers: usize,
}

/// [`JournalStats::writes`] by name.
pub const JOURNAL_WRITES: &str = "editor.journal_writes";
/// [`JournalStats::bytes`] by name.
pub const JOURNAL_BYTES: &str = "editor.journal_bytes";

impl JournalStats {
    /// The counters by name.
    #[must_use]
    pub const fn counters(&self) -> [(&'static str, u64); 2] {
        [(JOURNAL_WRITES, self.writes), (JOURNAL_BYTES, self.bytes)]
    }
}

/// One editor's session of the journal.
#[derive(Debug)]
pub struct Journal {
    /// This session's folder.
    session: PathBuf,
    /// Held while it runs: what tells another editor this session hasn't ended.
    _lock: File,
    /// The locks of the ended sessions whose entries it offers, by their folders.
    adopted: BTreeMap<PathBuf, File>,
    offers: Vec<Offer>,
    /// Entries it couldn't read, each with why: left where they are for a person to look at.
    unreadable: Vec<String>,
    /// What it last wrote of each document: its path and its text's revision ([`crate::Document::revision`]).
    written: BTreeMap<AssetId, (PathBuf, u64)>,
    writes: u64,
    failed: u64,
    bytes: u64,
}

impl Journal {
    /// Starts a session under `root`, and offers what the sessions there that ended without closing kept.
    pub fn open(root: &Path) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let session = new_session(root)?;
        // Locked under a hidden name and only then named, so an editor starting meanwhile never finds this
        // session's lock unheld: one with no lock is being made, and it leaves that alone.
        let making = session.join(format!(".{LOCK}.new"));
        let lock = File::create(&making)?;
        lock.try_lock().map_err(|error| match error {
            TryLockError::Error(error) => error,
            TryLockError::WouldBlock => io::Error::other("a new session's lock is held already"),
        })?;
        // When this one was made, by the file system's clock, which is what the others' times are by.
        let made = lock.metadata()?.modified().ok();
        fs::rename(&making, session.join(LOCK))?;
        let mut journal = Self {
            session,
            _lock: lock,
            adopted: BTreeMap::new(),
            offers: Vec::new(),
            unreadable: Vec::new(),
            written: BTreeMap::new(),
            writes: 0,
            failed: 0,
            bytes: 0,
        };
        let mut sessions: Vec<PathBuf> = fs::read_dir(root)?
            .filter_map(|dir| dir.ok().map(|dir| dir.path()))
            .filter(|dir| dir.is_dir() && *dir != journal.session)
            .collect();
        sessions.sort();
        for session in sessions {
            journal.adopt(&session, made);
        }
        Ok(journal)
    }

    /// Offers what an ended session kept, holding its lock while it does. A session whose lock is held is
    /// running; one with none was killed before it made it, and holds nothing.
    fn adopt(&mut self, session: &Path, made: Option<SystemTime>) {
        let lock = match fs::OpenOptions::new().write(true).open(session.join(LOCK)) {
            Ok(lock) => lock,
            // Killed between making its folder and its lock, or being made just now: either way it holds
            // nothing yet. One being made is its own editor's; one killed long since goes.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if let Some(made) = made {
                    sweep_if_killed_making(session, made);
                }
                return;
            }
            Err(error) => {
                self.unreadable.push(format!("{}: {error}", session.display()));
                return;
            }
        };
        if lock.try_lock().is_err() {
            return;
        }
        for file in entries_in(session) {
            match fs::read_to_string(&file)
                .map_err(|error| error.to_string())
                .and_then(|text| JournalEntry::parse(&text))
            {
                Ok(entry) => self.offers.push(Offer { entry, file }),
                Err(why) => self.unreadable.push(format!("{}: {why}", file.display())),
            }
        }
        self.adopted.insert(session.to_owned(), lock);
        self.let_go_of_ended(session);
    }

    /// This session's folder.
    #[must_use]
    pub fn session(&self) -> &Path {
        &self.session
    }

    /// What ended sessions kept, not yet taken or let go.
    #[must_use]
    pub fn offers(&self) -> &[Offer] {
        &self.offers
    }

    /// The offer of the document that was at `path`.
    #[must_use]
    pub fn offer(&self, path: &Path) -> Option<&Offer> {
        self.offers.iter().find(|offer| same_path(&offer.entry.path, path))
    }

    /// Entries it couldn't read, each with why.
    #[must_use]
    pub fn unreadable(&self) -> &[String] {
        &self.unreadable
    }

    /// Lets go of the offer of the document that was at `path`: its entry's file goes. `false` if there's no
    /// such offer.
    pub fn forget(&mut self, path: &Path) -> io::Result<bool> {
        let Some(at) = self.offers.iter().position(|offer| same_path(&offer.entry.path, path)) else {
            return Ok(false);
        };
        match fs::remove_file(&self.offers[at].file) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let offer = self.offers.remove(at);
        if let Some(session) = offer.file.parent() {
            self.let_go_of_ended(session);
        }
        Ok(true)
    }

    /// An ended session whose offers are all taken or let go goes, with its lock.
    fn let_go_of_ended(&mut self, session: &Path) {
        if self.offers.iter().any(|offer| offer.file.parent() == Some(session)) {
            return;
        }
        // Its lock goes first, since a file held open can't always be deleted. Another editor may adopt it in
        // between, find it as empty and take it away too, which comes to the same.
        if self.adopted.remove(session).is_some() && entries_in(session).is_empty() {
            let _ = fs::remove_dir_all(session);
        }
    }

    /// Writes an entry for each document in `documents` whose path or text changed since it last did, and
    /// takes this session's entry away for each that's saved, settled or closed. What failed is returned, and
    /// is tried again on the next call.
    pub fn write(&mut self, documents: &DocumentSet) -> Vec<(AssetId, io::Error)> {
        let mut failed = Vec::new();
        let mut kept = BTreeMap::new();
        let mut written = std::mem::take(&mut self.written);
        for id in documents.ids() {
            let (Some(path), Some(document)) = (documents.path(id), documents.any(id)) else { continue };
            if !document.is_dirty() {
                continue;
            }
            // By its revision, which every change of its text takes anew: no entry is made or hashed to pass over a
            // document that hasn't changed, however large. (`is_dirty`, above, still compares its text with the
            // file's bytes: one compare a frame.)
            let revision = document.revision();
            if let Some(stamp) = written.remove(&id) {
                if stamp.0 == path && stamp.1 == revision {
                    kept.insert(id, stamp);
                    continue;
                }
                written.insert(id, stamp);
            }
            let Some(entry) = documents.journal_entry(id) else { continue };
            let text = entry.to_text();
            let file = self.entry_file(id);
            // Not flushed to the disk: the rename keeps it whole however the editor ends, which is what it's for.
            match Temporary::beside_unflushed(&file, &text).and_then(|temporary| temporary.replace(&file)) {
                Ok(()) => {
                    self.writes += 1;
                    self.bytes += text.len() as u64;
                    written.remove(&id);
                    kept.insert(id, (path.to_owned(), revision));
                }
                Err(error) => {
                    // What it last wrote of this one stays, which is older but still the document's.
                    if let Some(old) = written.remove(&id) {
                        kept.insert(id, old);
                    }
                    self.failed += 1;
                    failed.push((id, error));
                }
            }
        }
        for id in written.keys() {
            let file = self.entry_file(*id);
            if let Err(error) = fs::remove_file(&file)
                && error.kind() != io::ErrorKind::NotFound
            {
                failed.push((*id, error));
            }
        }
        self.written = kept;
        failed
    }

    fn entry_file(&self, id: AssetId) -> PathBuf {
        self.session.join(format!("{id}{ENTRY_SUFFIX}"))
    }

    /// What it has done.
    #[must_use]
    pub fn stats(&self) -> JournalStats {
        JournalStats {
            writes: self.writes,
            failed: self.failed,
            bytes: self.bytes,
            entries: self.written.len(),
            offers: self.offers.len(),
        }
    }

    /// Ends the session as an editor that quits does, with nothing unsaved or having chosen to lose it: its
    /// folder goes. Offers not taken stay for the next editor. An editor that stops any other way, its
    /// journal dropped or its process killed, leaves its folder for the next to offer.
    pub fn close(self) -> io::Result<()> {
        let Self { session, _lock: lock, .. } = self;
        for entry in entries_in(&session) {
            fs::remove_file(entry)?;
        }
        // The lock goes before its file, for the reason [`Self::let_go_of_ended`] gives.
        drop(lock);
        match fs::remove_dir_all(&session) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// A folder of this process's own under `root`, made new.
fn new_session(root: &Path) -> io::Result<PathBuf> {
    for attempt in 0..SESSION_NAMES {
        let session = root.join(format!("{}-{attempt}", std::process::id()));
        match fs::create_dir(&session) {
            Ok(()) => return Ok(session),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no name left for a session"))
}

/// The entries in a session's folder, in name order. A temporary left by a kill mid-write is hidden, and
/// passed over.
/// Takes away a session with no lock if it holds nothing but the lock it was making, that lock isn't held, and
/// neither it nor the folder has been touched in the [`STALE_MAKING`] before `made`: an editor killed while it
/// made it. A lock that isn't held yet, made just now, is left for the editor about to hold it.
fn sweep_if_killed_making(session: &Path, made: SystemTime) {
    let making = session.join(format!(".{LOCK}.new"));
    let Ok(files) = fs::read_dir(session) else { return };
    if files.filter_map(Result::ok).any(|file| file.path() != making) {
        return;
    }
    let touched = |path: &Path| fs::metadata(path).and_then(|meta| meta.modified()).ok();
    let stale = |path: &Path| touched(path).is_some_and(|at| at + STALE_MAKING < made);
    if !stale(session) {
        return;
    }
    match File::options().write(true).open(&making) {
        Ok(lock) => {
            if !stale(&making) || lock.try_lock().is_err() {
                return;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return,
    }
    // It holds nothing: one that won't go is tried again by the next editor to start.
    let _ = fs::remove_dir_all(session);
}

fn entries_in(session: &Path) -> Vec<PathBuf> {
    let Ok(read) = fs::read_dir(session) else { return Vec::new() };
    let mut entries: Vec<PathBuf> = read
        .filter_map(|file| file.ok().map(|file| file.path()))
        .filter(|file| {
            file.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(ENTRY_SUFFIX) && !name.starts_with('.'))
        })
        .collect();
    entries.sort();
    entries
}

fn same_path(a: &Path, b: &Path) -> bool {
    let absolute = |path: &Path| std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    a == b || absolute(a) == absolute(b)
}
