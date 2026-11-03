//! The documents open at once, by the ID each file's sidecar gives it, and what's true of each on disk.
//!
//! **Dirty** is one thing: the bytes here differ from those of the last save or take. Nothing else counts, so
//! an edit undone is clean, and a file saved elsewhere with these same bytes is clean too. [`Document`] holds
//! the bytes, and this says when they change, because it does the looking.
//!
//! The file on disk is a person's too. [`DocumentSet::refresh`] looks in passing, and is tolerant, since a
//! file can be mid-save in someone's editor: a change is taken if there's nothing here to lose, and waits in
//! [`DocumentSet::conflict`] if there is. A file that's gone is looked for by its ID, so a rename with its
//! sidecar costs nothing. [`DocumentSet::save`] looks harder, and writes over nothing it hasn't checked.
//!
//! It keeps its last look at the sidecars, so opening a file walks nothing if that look knows the file's
//! sidecar and nothing else claims its ID. Anything else that look says is checked by another before it's
//! believed: a refusal, a new ID, a file gone. Someone calls [`DocumentSet::rescan`] on a timer, which also says
//! when a document open here turns out to have a copy.
//!
//! [`AnyDocument`] is what a panel that doesn't care about the kind sees: an inspector over a walker, a list
//! of what's unsaved.

use std::any::Any;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, io};

use bevy_reflect::{FromReflect, GetTypeRegistration, PartialReflect, Reflect, TypeRegistry, Typed};
use sapphire_assets::AssetId;

use crate::disk::{self, Temporary};
use crate::document::{Document, Outcome, Rebased, Refused};
use crate::identity::{self, IdentityError, Sidecars};
use crate::kinds::Schema;
use crate::path::PropertyPath;
use crate::walk::Property;

/// A document, whatever its kind.
pub trait AnyDocument: Any {
    fn text(&self) -> &str;
    fn is_dirty(&self) -> bool;
    /// Why it can't be edited now, if it can't.
    fn failure_message(&self) -> Option<String>;
    /// What's wrong with it that a person may mean to fix.
    fn issues(&self) -> Vec<String>;
    /// Every property the walker finds, with where the text writes it.
    fn properties(&self) -> Vec<Property>;
    /// [`Document::properties_under`].
    fn properties_under(&self, path: &PropertyPath) -> Result<Vec<Property>, Refused>;
    /// Its value, for a walker to read properties from.
    fn value_reflect(&self) -> Option<&dyn PartialReflect>;
    fn registry(&self) -> &TypeRegistry;
    /// [`Document::set_text`].
    fn set_ron(&mut self, label: &str, path: &PropertyPath, ron: &str) -> Result<Outcome, Refused>;
    /// [`Document::set_literal`].
    fn set_spelled(&mut self, label: &str, path: &PropertyPath, literal: &str) -> Result<Outcome, Refused>;
    /// [`Document::insert_blank`].
    fn insert_blank(&mut self, label: &str, list: &PropertyPath) -> Result<Outcome, Refused>;
    /// [`Document::pick_variant`].
    fn pick_variant(&mut self, label: &str, path: &PropertyPath, name: &str) -> Result<Outcome, Refused>;
    /// [`Document::remove`].
    fn remove_entry(&mut self, label: &str, path: &PropertyPath) -> Result<Outcome, Refused>;
    fn begin_gesture(&mut self, label: &str);
    fn end_gesture(&mut self);
    fn cancel_gesture(&mut self);
    fn in_gesture(&self) -> bool;
    fn undo_label(&self) -> Option<&str>;
    fn redo_label(&self) -> Option<&str>;
    fn undo(&mut self) -> bool;
    fn redo(&mut self) -> bool;
    /// [`Document::why_no_undo`].
    fn why_no_undo(&self) -> Option<String>;
    /// [`Document::forgotten`].
    fn forgotten(&self) -> usize;
    /// [`Document::set_history_limit`].
    fn set_history_limit(&mut self, bytes: usize);
    /// What its history holds: bytes, then steps.
    fn history(&self) -> (usize, usize);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    #[doc(hidden)]
    fn disk(&self) -> &[u8];
    #[doc(hidden)]
    fn mark_saved(&mut self);
    #[doc(hidden)]
    fn take_disk(&mut self, label: &str, disk: String);
    #[doc(hidden)]
    fn rebase_on_disk(&mut self, disk: String) -> Rebased;
    #[doc(hidden)]
    fn keep_mine(&mut self, disk: Vec<u8>);
}

impl<T> AnyDocument for Document<T>
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    fn text(&self) -> &str {
        Self::text(self)
    }

    fn is_dirty(&self) -> bool {
        Self::is_dirty(self)
    }

    fn failure_message(&self) -> Option<String> {
        self.failure().map(|failure| failure.message().to_owned())
    }

    fn issues(&self) -> Vec<String> {
        Self::issues(self)
    }

    fn properties(&self) -> Vec<Property> {
        Self::properties(self)
    }

    fn properties_under(&self, path: &PropertyPath) -> Result<Vec<Property>, Refused> {
        Self::properties_under(self, path)
    }

    fn value_reflect(&self) -> Option<&dyn PartialReflect> {
        self.value().map(PartialReflect::as_partial_reflect)
    }

    fn registry(&self) -> &TypeRegistry {
        Self::registry(self)
    }

    fn set_ron(&mut self, label: &str, path: &PropertyPath, ron: &str) -> Result<Outcome, Refused> {
        self.set_text(label, path, ron)
    }

    fn set_spelled(&mut self, label: &str, path: &PropertyPath, literal: &str) -> Result<Outcome, Refused> {
        self.set_literal(label, path, literal)
    }

    fn insert_blank(&mut self, label: &str, list: &PropertyPath) -> Result<Outcome, Refused> {
        Self::insert_blank(self, label, list)
    }

    fn pick_variant(&mut self, label: &str, path: &PropertyPath, name: &str) -> Result<Outcome, Refused> {
        Self::pick_variant(self, label, path, name)
    }

    fn remove_entry(&mut self, label: &str, path: &PropertyPath) -> Result<Outcome, Refused> {
        self.remove(label, path)
    }

    fn begin_gesture(&mut self, label: &str) {
        Self::begin_gesture(self, label);
    }

    fn end_gesture(&mut self) {
        Self::end_gesture(self);
    }

    fn cancel_gesture(&mut self) {
        Self::cancel_gesture(self);
    }

    fn in_gesture(&self) -> bool {
        Self::in_gesture(self)
    }

    fn undo_label(&self) -> Option<&str> {
        Self::undo_label(self)
    }

    fn redo_label(&self) -> Option<&str> {
        Self::redo_label(self)
    }

    fn undo(&mut self) -> bool {
        Self::undo(self)
    }

    fn redo(&mut self) -> bool {
        Self::redo(self)
    }

    fn why_no_undo(&self) -> Option<String> {
        Self::why_no_undo(self)
    }

    fn forgotten(&self) -> usize {
        Self::forgotten(self)
    }

    fn set_history_limit(&mut self, bytes: usize) {
        Self::set_history_limit(self, bytes);
    }

    fn history(&self) -> (usize, usize) {
        Self::history(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn disk(&self) -> &[u8] {
        Self::disk(self)
    }

    fn mark_saved(&mut self) {
        Self::mark_saved(self);
    }

    fn take_disk(&mut self, label: &str, disk: String) {
        Self::take_disk(self, label, disk);
    }

    fn rebase_on_disk(&mut self, disk: String) -> Rebased {
        Self::rebase_on_disk(self, disk)
    }

    fn keep_mine(&mut self, disk: Vec<u8>) {
        Self::keep_mine(self, disk);
    }
}

/// Why a document couldn't be opened, made, moved or saved.
#[derive(Debug, thiserror::Error)]
pub enum SetError {
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{}: isn't UTF-8 text", path.display())]
    NotText { path: PathBuf },
    #[error("{} is there already: nothing was changed", path.display())]
    AlreadyThere { path: PathBuf },
    #[error("{}: this isn't a document of that kind: {why}", path.display())]
    DoesNotRead { path: PathBuf, why: String },
    #[error("{} changed on disk since it was read: nothing was saved", path.display())]
    ChangedOnDisk { path: PathBuf },
    #[error("no document with the ID {0} is open")]
    NotOpen(AssetId),
}

/// What a look at the files found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Noticed {
    /// It changed on disk, there was nothing here to lose, and undo can take that back.
    Taken { id: AssetId },
    /// It changed on disk and there are edits here: [`DocumentSet::conflict`] holds what it says.
    Conflict { id: AssetId },
    /// It's somewhere else now, with its sidecar.
    Moved { id: AssetId, from: PathBuf, to: PathBuf },
    /// It's gone, and no sidecar under the roots says where. A save writes it again.
    Gone { id: AssetId },
    /// A second sidecar holds its ID: a copy, most likely, made since it opened. What's open stays open, and
    /// neither file opens again until a person deletes the copy's sidecar.
    Duplicated { id: AssetId, first: PathBuf, second: PathBuf },
}

/// What the documents open cost, for a panel and a log (R9). There's no budget for it in §3.2, since a tool's
/// memory isn't on a device: this says where a long session's went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DocumentStats {
    pub open: usize,
    pub dirty: usize,
    pub conflicts: usize,
    pub steps: usize,
    pub history_bytes: usize,
    /// Steps let go past the cap.
    pub forgotten: usize,
    pub text_bytes: usize,
    /// How many times it has walked the roots.
    pub scans: u64,
}

struct OpenFile {
    path: PathBuf,
    importer: Option<&'static str>,
    document: Box<dyn AnyDocument>,
    /// What the file held when it changed on disk while there were edits here, or changed to something that
    /// can't be taken.
    conflict: Option<Vec<u8>>,
    /// Whether [`Noticed::Gone`] has been said, so it's said once.
    gone: bool,
    /// The same of [`Noticed::Duplicated`].
    duplicated: bool,
}

/// The documents open, by ID.
pub struct DocumentSet {
    roots: Vec<PathBuf>,
    /// The last look at the sidecars under the roots.
    sidecars: Sidecars,
    scans: u64,
    files: BTreeMap<AssetId, OpenFile>,
}

impl DocumentSet {
    /// A set that finds a moved file, and a second claim on an ID, under `roots`.
    #[must_use]
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self { roots, sidecars: Sidecars::default(), scans: 0, files: BTreeMap::new() }
    }

    /// The roots it looks under.
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Looks under other roots from now on, keeping what's open.
    pub fn set_roots(&mut self, roots: Vec<PathBuf>) {
        if roots != self.roots {
            self.roots = roots;
            self.sidecars = Sidecars::default();
        }
    }

    /// Walks the roots again, and says what that shows of the documents open: a file that's gone or moved is
    /// [`Self::refresh`]'s to say. A walk that fails leaves the last look standing.
    pub fn rescan(&mut self) -> Result<Vec<Noticed>, IdentityError> {
        self.look()?;
        let mut noticed = Vec::new();
        for (id, file) in &mut self.files {
            match self.sidecars.check_one_claim(*id) {
                Err(IdentityError::Duplicate { id, first, second }) => {
                    if !std::mem::replace(&mut file.duplicated, true) {
                        noticed.push(Noticed::Duplicated { id, first, second });
                    }
                }
                _ => file.duplicated = false,
            }
        }
        Ok(noticed)
    }

    fn look(&mut self) -> Result<(), IdentityError> {
        self.scans += 1;
        self.sidecars = Sidecars::scan(&self.roots)?;
        Ok(())
    }

    /// The last look at what's under the roots: [`Self::rescan`] makes a new one.
    #[must_use]
    pub const fn sidecars(&self) -> &Sidecars {
        &self.sidecars
    }

    /// The ID of the file at `path`. The last look answers if it can say yes. Anything else it would say gets
    /// another look first: it may be older than whatever it's about.
    fn identify(&mut self, path: &Path, importer: Option<&str>) -> Result<AssetId, SetError> {
        let sidecar = identity::sidecar_path(path);
        let known = identity::read_id(&sidecar).is_ok_and(|id| self.sidecars.knows_alone(&sidecar, id));
        if !known {
            self.look()?;
        }
        let id = identity::identify(path, importer, &self.sidecars)?;
        self.sidecars.note(sidecar, id);
        Ok(id)
    }

    /// Opens the document at `path`, or gives the one already open there. It has a sidecar when this returns,
    /// or an error says why it can't: see [`crate::identity`].
    pub fn open<T>(
        &mut self,
        path: &Path,
        schema: Schema<T>,
        registry: Arc<TypeRegistry>,
    ) -> Result<AssetId, SetError>
    where
        T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
    {
        let bytes = fs::read(path).map_err(|source| SetError::Io { path: path.to_owned(), source })?;
        let text = String::from_utf8(bytes).map_err(|_| SetError::NotText { path: path.to_owned() })?;
        let id = self.identify(path, schema.importer)?;
        if let Some(open) = self.files.get_mut(&id) {
            // The same file, or a file that has moved without a look since.
            if same_file(&open.path, path) || !open.path.exists() {
                open.path = path.to_owned();
                return Ok(id);
            }
            return Err(IdentityError::Duplicate {
                id,
                first: identity::sidecar_path(&open.path),
                second: identity::sidecar_path(path),
            }
            .into());
        }
        self.insert(id, path, schema, registry, text);
        Ok(id)
    }

    /// Makes a new document at `path` holding `text`, which has to read as its kind, and its sidecar beside
    /// it. It refuses a path anything's at, and a place where a sidecar is waiting for a file that moved. The
    /// document's written first, since a crash between the two leaves a file that gets an ID when it opens.
    pub fn create<T>(
        &mut self,
        path: &Path,
        text: &str,
        schema: Schema<T>,
        registry: Arc<TypeRegistry>,
    ) -> Result<AssetId, SetError>
    where
        T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
    {
        (schema.parse)(text).map_err(|why| SetError::DoesNotRead { path: path.to_owned(), why })?;
        if fs::symlink_metadata(path).is_ok() {
            return Err(SetError::AlreadyThere { path: path.to_owned() });
        }
        let sidecar = identity::sidecar_path(path);
        if fs::symlink_metadata(&sidecar).is_ok() {
            return Err(IdentityError::SidecarWaiting { path: sidecar }.into());
        }
        let id = identity::new_id()?;
        let written = disk::create_whole(path, text.as_bytes())
            .map_err(|source| SetError::Io { path: path.to_owned(), source })?;
        if !written {
            return Err(SetError::AlreadyThere { path: path.to_owned() });
        }
        if !identity::write_sidecar(&sidecar, id, schema.importer)? {
            return Err(IdentityError::SidecarWaiting { path: sidecar }.into());
        }
        self.sidecars.note(sidecar, id);
        self.insert(id, path, schema, registry, text.to_owned());
        Ok(id)
    }

    fn insert<T>(
        &mut self,
        id: AssetId,
        path: &Path,
        schema: Schema<T>,
        registry: Arc<TypeRegistry>,
        text: String,
    ) where
        T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
    {
        let importer = schema.importer;
        let document = Box::new(Document::open(text, schema, registry));
        let open = OpenFile {
            path: path.to_owned(),
            importer,
            document,
            conflict: None,
            gone: false,
            duplicated: false,
        };
        self.files.insert(id, open);
    }

    /// The IDs open, in order.
    pub fn ids(&self) -> impl Iterator<Item = AssetId> + '_ {
        self.files.keys().copied()
    }

    /// A document, whatever its kind.
    #[must_use]
    pub fn any(&self, id: AssetId) -> Option<&dyn AnyDocument> {
        self.files.get(&id).map(|file| file.document.as_ref())
    }

    /// A document, whatever its kind, to edit.
    pub fn any_mut(&mut self, id: AssetId) -> Option<&mut (dyn AnyDocument + 'static)> {
        self.files.get_mut(&id).map(|file| file.document.as_mut())
    }

    /// A document of a kind. `None` if it's another kind.
    #[must_use]
    pub fn get<T: 'static>(&self, id: AssetId) -> Option<&Document<T>> {
        self.any(id)?.as_any().downcast_ref()
    }

    /// A document of a kind, to edit. `None` if it's another kind.
    pub fn get_mut<T: 'static>(&mut self, id: AssetId) -> Option<&mut Document<T>> {
        self.any_mut(id)?.as_any_mut().downcast_mut()
    }

    /// Where it is now.
    #[must_use]
    pub fn path(&self, id: AssetId) -> Option<&Path> {
        self.files.get(&id).map(|file| file.path.as_path())
    }

    /// Whether its bytes differ from those of the last save or take.
    #[must_use]
    pub fn is_dirty(&self, id: AssetId) -> bool {
        self.files.get(&id).is_some_and(|file| file.document.is_dirty())
    }

    /// The documents with unsaved bytes, in ID order.
    #[must_use]
    pub fn dirty(&self) -> Vec<AssetId> {
        self.files.iter().filter(|(_, file)| file.document.is_dirty()).map(|(id, _)| *id).collect()
    }

    /// What the file says, if it changed on disk and that's waiting to be settled.
    #[must_use]
    pub fn conflict(&self, id: AssetId) -> Option<&[u8]> {
        self.files.get(&id)?.conflict.as_deref()
    }

    /// Looks at every file. A file mid-gesture waits, and so does one that's unreadable or not text just
    /// now. A save looks harder.
    pub fn refresh(&mut self) -> Vec<Noticed> {
        let mut noticed = Vec::new();
        let mut looked = false;
        for (id, file) in &mut self.files {
            if file.document.in_gesture() {
                continue;
            }
            let on_disk = match fs::read(&file.path) {
                Ok(on_disk) => on_disk,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    // Once when it goes, since it's most likely a rename just now. After that, the
                    // timer's looks will find it, and a file that stays gone costs no walk a second.
                    if !looked && !file.gone {
                        looked = true;
                        self.scans += 1;
                        if let Ok(sidecars) = Sidecars::scan(&self.roots) {
                            self.sidecars = sidecars;
                        }
                    }
                    match self.sidecars.source_of(*id) {
                        Some(now) => {
                            let from = std::mem::replace(&mut file.path, now.clone());
                            noticed.push(Noticed::Moved { id: *id, from, to: now });
                            file.gone = false;
                            match fs::read(&file.path) {
                                Ok(on_disk) => on_disk,
                                Err(_) => continue,
                            }
                        }
                        None => {
                            if !file.gone {
                                noticed.push(Noticed::Gone { id: *id });
                            }
                            file.gone = true;
                            continue;
                        }
                    }
                }
                Err(_) => continue,
            };
            file.gone = false;
            if std::str::from_utf8(&on_disk).is_err() {
                continue;
            }
            settle(*id, file, on_disk, Some(&mut noticed));
        }
        noticed
    }

    /// Writes the text if it holds edits, in one step, so a watcher or a crash never sees half a file. A file
    /// with nothing to save isn't touched. One changed on disk since it was read is refused, whatever it
    /// changed to, and one that can't be read is too, since there's no telling what would be lost. One that's
    /// gone is looked for by its ID, and if it isn't anywhere it's written again, edits or none, since nothing
    /// would be lost and this is the only copy. A document that's a link stays one: the file it leads to is
    /// what's replaced. A sidecar that's gone comes back with the ID it held, since the next cook would
    /// otherwise make a new one.
    pub fn save(&mut self, id: AssetId) -> Result<(), SetError> {
        self.relocate_if_gone(id);
        let file = self.files.get_mut(&id).ok_or(SetError::NotOpen(id))?;
        file.document.end_gesture();
        let path = file.path.clone();
        let io_error = |source| SetError::Io { path: path.clone(), source };
        let target = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let gone = fs::metadata(&path).is_err_and(|error| error.kind() == io::ErrorKind::NotFound);
        let replacement = (gone || file.document.is_dirty())
            .then(|| Temporary::beside(&target, file.document.text()))
            .transpose()
            .map_err(io_error)?;
        // As late as it can be, with the new text already written beside it. What's left is the moment
        // before the rename, where another save lands and loses to this one whole.
        let there = match fs::read(&path) {
            Ok(on_disk) => {
                settle(id, file, on_disk, None);
                true
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => false,
            Err(source) => return Err(io_error(source)),
        };
        if file.conflict.is_some() {
            return Err(SetError::ChangedOnDisk { path });
        }
        if let Some(replacement) = replacement
            && (!there || file.document.is_dirty())
        {
            replacement.replace(&target).map_err(io_error)?;
            file.document.mark_saved();
            file.gone = false;
        }
        let sidecar = identity::sidecar_path(&path);
        if fs::symlink_metadata(&sidecar).is_err() {
            identity::write_sidecar(&sidecar, id, file.importer)?;
        }
        Ok(())
    }

    fn relocate_if_gone(&mut self, id: AssetId) {
        let Some(file) = self.files.get_mut(&id) else { return };
        if fs::symlink_metadata(&file.path).is_ok() {
            return;
        }
        self.scans += 1;
        if let Ok(sidecars) = Sidecars::scan(&self.roots) {
            self.sidecars = sidecars;
        }
        if let Some(now) = self.sidecars.source_of(id) {
            file.path = now;
        }
    }

    /// Saves each document with unsaved bytes, in ID order, and says what failed. One failing stops no other.
    pub fn save_all(&mut self) -> Vec<(AssetId, SetError)> {
        let mut failed = Vec::new();
        for id in self.dirty() {
            if let Err(error) = self.save(id) {
                failed.push((id, error));
            }
        }
        failed
    }

    /// Settles a conflict by taking the file's text, as an edit undo can take back. Bytes that aren't text
    /// can't be taken, and wait: `false`.
    pub fn take_disk(&mut self, id: AssetId) -> bool {
        let Some(file) = self.files.get_mut(&id) else { return false };
        let Some(on_disk) = file.conflict.take() else { return false };
        match String::from_utf8(on_disk) {
            Ok(on_disk) => {
                file.document.take_disk("took the file's text from disk", on_disk);
                true
            }
            Err(error) => {
                file.conflict = Some(error.into_bytes());
                false
            }
        }
    }

    /// Settles a conflict by keeping the text here: the next save overwrites the file.
    pub fn keep_mine(&mut self, id: AssetId) -> bool {
        let Some(file) = self.files.get_mut(&id) else { return false };
        let Some(on_disk) = file.conflict.take() else { return false };
        file.document.keep_mine(on_disk);
        true
    }

    /// Settles a conflict by taking the file's text and laying the unsaved edits on top of it again, by path.
    /// `None` if there's no conflict, or the bytes aren't text.
    pub fn rebase(&mut self, id: AssetId) -> Option<Rebased> {
        let file = self.files.get_mut(&id)?;
        let on_disk = file.conflict.take()?;
        match String::from_utf8(on_disk) {
            Ok(on_disk) => Some(file.document.rebase_on_disk(on_disk)),
            Err(error) => {
                file.conflict = Some(error.into_bytes());
                None
            }
        }
    }

    /// Moves a document and its sidecar together, so it keeps its ID, and leaves what's being edited alone. It
    /// refuses a place anything's at, either's name. If the second move fails, the first goes back.
    pub fn move_to(&mut self, id: AssetId, to: &Path) -> Result<(), SetError> {
        let file = self.files.get_mut(&id).ok_or(SetError::NotOpen(id))?;
        file.document.end_gesture();
        let from = file.path.clone();
        let (sidecar_from, sidecar_to) = (identity::sidecar_path(&from), identity::sidecar_path(to));
        if fs::symlink_metadata(&sidecar_to).is_ok() {
            return Err(SetError::AlreadyThere { path: sidecar_to });
        }
        let io_error = |path: &Path, source| SetError::Io { path: path.to_owned(), source };
        if !disk::move_whole(&from, to).map_err(|source| io_error(&from, source))? {
            return Err(SetError::AlreadyThere { path: to.to_owned() });
        }
        match disk::move_whole(&sidecar_from, &sidecar_to) {
            Ok(true) => {
                file.path = to.to_owned();
                Ok(())
            }
            moved => {
                let _ = disk::move_whole(to, &from);
                match moved {
                    Err(source) => Err(io_error(&sidecar_from, source)),
                    _ => Err(SetError::AlreadyThere { path: sidecar_to }),
                }
            }
        }
    }

    /// Stops holding it. What wasn't saved is dropped, so a caller asks first.
    pub fn close(&mut self, id: AssetId) -> bool {
        self.files.remove(&id).is_some()
    }

    /// What the open documents cost.
    #[must_use]
    pub fn stats(&self) -> DocumentStats {
        let mut stats =
            DocumentStats { open: self.files.len(), scans: self.scans, ..DocumentStats::default() };
        for file in self.files.values() {
            let (history_bytes, steps) = file.document.history();
            stats.dirty += usize::from(file.document.is_dirty());
            stats.conflicts += usize::from(file.conflict.is_some());
            stats.steps += steps;
            stats.history_bytes += history_bytes;
            stats.forgotten += file.document.forgotten();
            stats.text_bytes += file.document.text().len();
        }
        stats
    }
}

/// What the bytes on disk mean for a document: nothing if they're what it knew; clean, if they're what it
/// holds, since they were saved elsewhere; a conflict if there's something here to lose; and otherwise an edit
/// undo can take back. `noticed` is `None` for a save, which reports by failing.
fn settle(id: AssetId, file: &mut OpenFile, on_disk: Vec<u8>, noticed: Option<&mut Vec<Noticed>>) {
    if on_disk == file.document.disk() {
        file.conflict = None;
    } else if on_disk == file.document.text().as_bytes() {
        file.document.mark_saved();
        file.conflict = None;
    } else if file.document.is_dirty() {
        let new = file.conflict.as_deref() != Some(on_disk.as_slice());
        file.conflict = Some(on_disk);
        if let (true, Some(noticed)) = (new, noticed) {
            noticed.push(Noticed::Conflict { id });
        }
    } else {
        match String::from_utf8(on_disk) {
            Ok(on_disk) => {
                file.document.take_disk("the file changed on disk", on_disk);
                file.conflict = None;
                if let Some(noticed) = noticed {
                    noticed.push(Noticed::Taken { id });
                }
            }
            Err(error) => file.conflict = Some(error.into_bytes()),
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || match (fs::canonicalize(a), fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}
