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

use sapphire_assets::AssetId;
use serde::{Deserialize, Serialize};

use crate::disk::Temporary;
use crate::record::HistoryStep;
use crate::set::DocumentSet;

/// What an entry's `version` must say.
pub const JOURNAL_VERSION: u32 = 1;
/// What an entry's file name ends with.
const ENTRY_SUFFIX: &str = ".journal.ron";
/// The file a session's lock is held on.
const LOCK: &str = "session.lock";
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
        Self { version: JOURNAL_VERSION, id, path, disk: hash_of(disk), text, unsaved }
    }

    /// Reads an entry's text. One of another version, or with anything it doesn't know, is refused.
    pub fn parse(text: &str) -> Result<Self, String> {
        let entry: Self = ron::from_str(text).map_err(|error| error.to_string())?;
        if entry.version != JOURNAL_VERSION {
            return Err(format!(
                "an entry of version {}, where this editor reads version {JOURNAL_VERSION}",
                entry.version
            ));
        }
        Ok(entry)
    }

    /// The entry's text, as its file holds it.
    #[must_use]
    pub fn to_text(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).expect("an entry is plain data")
    }

    /// The bytes its edits were made over, got back from its text and its steps, if they agree with the hash
    /// it holds.
    pub fn base(&self) -> Result<String, String> {
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

/// What a journal has done, for a panel and a log (R9). The plan's row is one write a step: a step that
/// changes no document's text writes nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JournalStats {
    /// Entries written whole since it opened.
    pub writes: u64,
    /// Their bytes.
    pub bytes: u64,
    /// Entries this session holds now.
    pub entries: usize,
    /// Offers not yet taken or let go.
    pub offers: usize,
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
    /// What it last wrote of each document: the hash of its path and text.
    written: BTreeMap<AssetId, String>,
    writes: u64,
    bytes: u64,
}

impl Journal {
    /// Starts a session under `root`, and offers what the sessions there that ended without closing kept.
    pub fn open(root: &Path) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let session = new_session(root)?;
        let lock = File::create(session.join(LOCK))?;
        lock.try_lock().map_err(|error| match error {
            TryLockError::Error(error) => error,
            TryLockError::WouldBlock => io::Error::other("a new session's lock is held already"),
        })?;
        let mut journal = Self {
            session,
            _lock: lock,
            adopted: BTreeMap::new(),
            offers: Vec::new(),
            unreadable: Vec::new(),
            written: BTreeMap::new(),
            writes: 0,
            bytes: 0,
        };
        let mut sessions: Vec<PathBuf> = fs::read_dir(root)?
            .filter_map(|dir| dir.ok().map(|dir| dir.path()))
            .filter(|dir| dir.is_dir() && *dir != journal.session)
            .collect();
        sessions.sort();
        for session in sessions {
            journal.adopt(&session);
        }
        Ok(journal)
    }

    /// Offers what an ended session kept, holding its lock while it does. A session whose lock is held is
    /// running; one with none was killed before it made it, and holds nothing.
    fn adopt(&mut self, session: &Path) {
        let lock = match fs::OpenOptions::new().write(true).open(session.join(LOCK)) {
            Ok(lock) => lock,
            // Killed between making its folder and its lock, or being made just now: either way it holds
            // nothing yet, and only its own editor may take it away.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
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
        for id in documents.ids() {
            let (Some(path), Some(document)) = (documents.path(id), documents.any(id)) else { continue };
            if !document.is_dirty() {
                continue;
            }
            let stamp = hash_of(format!("{}\n{}", path.display(), document.text()).as_bytes());
            if self.written.get(&id) == Some(&stamp) {
                kept.insert(id, stamp);
                continue;
            }
            let Some(entry) = documents.journal_entry(id) else { continue };
            let text = entry.to_text();
            let file = self.entry_file(id);
            match Temporary::beside(&file, &text).and_then(|temporary| temporary.replace(&file)) {
                Ok(()) => {
                    self.writes += 1;
                    self.bytes += text.len() as u64;
                    kept.insert(id, stamp);
                }
                Err(error) => {
                    // What it last wrote of this one stays, which is older but still the document's.
                    if let Some(old) = self.written.get(&id) {
                        kept.insert(id, old.clone());
                    }
                    failed.push((id, error));
                }
            }
        }
        for id in self.written.keys().filter(|id| !kept.contains_key(id)) {
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
