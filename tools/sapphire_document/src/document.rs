//! A document open in an editor: its text, what the text says, and every edit as a [`ChangeRecord`].
//!
//! The text is the document. An edit is planned against it ([`RonPatch`]), and lands only if the new text,
//! read by the document's own parser, equals the value the edit was to make; or else the text is as it was.
//! Undo gives back the bytes a record took, so an undo is byte-for-byte whatever the file spelled.
//!
//! The file on disk is a person's too. [`Document::take_disk`] takes what they saved, as a step undo can
//! take back. [`Document::rebase_on_disk`] also lays unsaved records on top of it again by path: each lands
//! if what it was changing still says what it said, and is refused by name if not. It's told what the file
//! holds, and [`crate::set::DocumentSet`] does the looking.
//!
//! History is capped by the bytes it holds ([`HISTORY_LIMIT`]). Past the cap the oldest steps go, whole, and
//! the nearest undo (or, with no undo, the next redo) always stays, however large. What that leaves is a document that can't undo as far back as it
//! was, and says so ([`Document::why_no_undo`]): its text and its dirtiness are as they were, since dirty is
//! the bytes and never the history. A rebase that can no longer find the last save says [`Rebased::beneath`].

use bevy_reflect::{
    FromReflect, GetTypeRegistration, PartialReflect, Reflect, ReflectFromReflect, ReflectMut, ReflectRef,
    TypeInfo, TypeRegistry, Typed,
};
use std::sync::Arc;

use crate::blank;
use crate::kinds::Schema;
use crate::layout::{self, Splice};
use crate::patch::RonPatch;
use crate::path::{PropertyPath, Step};
use crate::record::{Change, ChangeRecord, Guard, HistoryStep, Op, absorb};
use crate::text::{self, Node};
use crate::walk::{self, NoSuchPath, Property};
use crate::write::{self, ValueTextError};

/// What a document's undo and redo may hold, in bytes ([`HistoryStep::bytes`]), before the oldest steps go.
/// A drag holds about a kilobyte however long it runs, so this is several thousand edits of the largest file
/// there is. A whole-file take counts both the removed and inserted bytes.
pub const HISTORY_LIMIT: usize = 16 * 1024 * 1024;

/// An edit that didn't land. The text is as it was.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    /// The text can't be edited now: it doesn't read, or this crate can't find its way around it.
    #[error("the file can't be edited now: {0}")]
    ReadOnly(String),
    #[error(transparent)]
    NoSuchPath(#[from] NoSuchPath),
    /// The value doesn't fit what's at the path.
    #[error("that value doesn't fit `{path}`: {why}")]
    WrongType { path: String, why: String },
    /// The document's parser won't read the result: its rule, as it words it.
    #[error("the document refuses this: {0}")]
    Document(String),
    /// The result reads, and says something other than what the edit was to make. A bug here: it found the
    /// wrong bytes, and the check caught it.
    #[error("this edit of `{path}` read back as something else: left as it was")]
    Disagrees { path: String },
    /// A record can't be written as text.
    #[error(transparent)]
    Value(#[from] ValueTextError),
}

/// Why a text can't be edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadFailure {
    /// It isn't this kind of document: its parser's words. A person's edit in progress, most likely.
    Parse(String),
    /// It's a document, and the scan and the value disagree about it: a bug here, so it reads only.
    Scan(String),
}

impl ReadFailure {
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Parse(message) | Self::Scan(message) => message,
        }
    }
}

/// What an edit came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A record, now on the history.
    Changed,
    /// The property already held that value, so no byte moved and nothing was recorded.
    Unchanged,
}

/// What a text says, and where.
#[derive(Clone, Debug)]
struct Parsed<T> {
    value: T,
    tree: Node,
}

/// A gesture in progress: edits that are to be one step of undo.
#[derive(Clone, Debug)]
struct Gesture {
    label: String,
    /// The text when it began: what its change is measured from, and what cancelling it goes back to.
    start: String,
    records: Vec<ChangeRecord>,
}

/// A record that couldn't go on top of a file that changed under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefusedRecord {
    pub label: String,
    pub record: ChangeRecord,
    pub why: String,
}

/// What came of taking a file that changed on disk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rebased {
    /// Unsaved records that landed on the new text.
    pub kept: usize,
    /// Those that didn't.
    pub refused: Vec<RefusedRecord>,
    /// There were edits here history couldn't call unsaved, so none were laid back. They're in undo.
    pub beneath: bool,
}

/// A document open in an editor.
pub struct Document<T> {
    schema: Schema<T>,
    registry: Arc<TypeRegistry>,
    text: String,
    parsed: Result<Parsed<T>, ReadFailure>,
    /// The value as it last read, once the text stops reading.
    last_good: Option<T>,
    /// What the file held when last read, written or settled. Its bytes, since a file can stop being text.
    disk: Vec<u8>,
    undo: Vec<HistoryStep>,
    redo: Vec<HistoryStep>,
    /// How many steps of undo there were when `disk` was the text. `None` once that can't be got back to.
    saved_at: Option<usize>,
    gesture: Option<Gesture>,
    /// What undo and redo may hold.
    history_limit: usize,
    /// How many steps have gone past that, oldest first, since it opened.
    forgotten: usize,
}

impl<T> Document<T>
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    /// Reads `text`. One that doesn't read opens all the same, and takes no edits until it does.
    #[must_use]
    pub fn open(text: String, schema: Schema<T>, registry: Arc<TypeRegistry>) -> Self {
        let parsed = read(&text, schema);
        Self {
            schema,
            registry,
            disk: text.clone().into_bytes(),
            text,
            parsed,
            undo: Vec::new(),
            redo: Vec::new(),
            saved_at: Some(0),
            gesture: None,
            last_good: None,
            history_limit: HISTORY_LIMIT,
            forgotten: 0,
        }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// What the text says now, if it reads.
    #[must_use]
    pub fn value(&self) -> Option<&T> {
        self.parsed.as_ref().ok().map(|parsed| &parsed.value)
    }

    /// The value as it last read, for showing while the text doesn't.
    #[must_use]
    pub fn last_good(&self) -> Option<&T> {
        self.value().or(self.last_good.as_ref())
    }

    /// Why the text can't be edited, if it can't.
    #[must_use]
    pub fn failure(&self) -> Option<&ReadFailure> {
        self.parsed.as_ref().err()
    }

    /// What the document's own rules say of it. Nothing here stops on these: they're for showing.
    #[must_use]
    pub fn issues(&self) -> Vec<String> {
        self.value().map_or_else(Vec::new, self.schema.check)
    }

    /// Whether the bytes differ from those of the last save or take. Nothing else counts: an edit undone, or
    /// made and made back, leaves a document clean.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.text.as_bytes() != self.disk
    }

    /// Says the file now holds the text.
    pub fn mark_saved(&mut self) {
        self.end_gesture();
        self.disk = self.text.clone().into_bytes();
        self.saved_at = Some(self.undo.len());
    }

    /// The bytes the file held when last read, written or settled.
    #[must_use]
    pub fn disk(&self) -> &[u8] {
        &self.disk
    }

    /// What its completed undo and redo hold, in bytes and steps, which [`crate::set::DocumentStats`]
    /// adds up. An active gesture holds its starting text and records separately until it ends.
    #[must_use]
    pub fn history(&self) -> (usize, usize) {
        let held = self.undo.iter().chain(&self.redo).map(HistoryStep::bytes).sum();
        (held, self.undo.len() + self.redo.len())
    }

    /// How many of the oldest steps have gone past the history's cap since it opened.
    #[must_use]
    pub const fn forgotten(&self) -> usize {
        self.forgotten
    }

    /// Why undo has nothing left, if it's because the cap took it: what an editor says of an undo that did
    /// nothing, since from outside it would look as if this document had never been edited further back.
    #[must_use]
    pub fn why_no_undo(&self) -> Option<String> {
        (self.undo_label().is_none() && self.forgotten > 0).then(|| {
            format!(
                "nothing older to undo: the oldest {} {} let go to keep this document's history under {} KiB",
                self.forgotten,
                if self.forgotten == 1 { "edit was" } else { "edits were" },
                self.history_limit / 1024,
            )
        })
    }

    /// Sets the cap on completed history and lets go of what's now past it. The nearest undo step,
    /// or the next redo when there is no undo, stays even if it alone exceeds the cap.
    pub fn set_history_limit(&mut self, bytes: usize) {
        self.history_limit = bytes;
        self.forget_past_limit();
    }

    /// Whether a gesture is in progress.
    #[must_use]
    pub const fn in_gesture(&self) -> bool {
        self.gesture.is_some()
    }

    /// Every property, written or left out.
    #[must_use]
    pub fn properties(&self) -> Vec<Property> {
        match &self.parsed {
            Ok(parsed) => walk::properties(parsed.value.as_partial_reflect(), body(&parsed.tree), &self.text),
            Err(_) => Vec::new(),
        }
    }

    /// The properties at `path` and under it, written or left out. A panel's look at one entry, which doesn't
    /// walk the rest of the file.
    pub fn properties_under(&self, path: &PropertyPath) -> Result<Vec<Property>, Refused> {
        let parsed = self.ready()?;
        let root = parsed.value.as_partial_reflect();
        Ok(walk::properties_under(root, body(&parsed.tree), &self.text, path)?)
    }

    /// The registry its values go through.
    #[must_use]
    pub fn registry(&self) -> &TypeRegistry {
        &self.registry
    }

    /// How a path calls the entry at `at` of the list at `list`, as of now.
    pub fn entry_path(&self, list: &PropertyPath, at: usize) -> Result<PropertyPath, Refused> {
        let parsed = self.ready()?;
        let value = walk::get(parsed.value.as_partial_reflect(), list)?;
        match value.reflect_ref() {
            ReflectRef::List(items) => Ok(list.join(walk::step_to_entry(items, at))),
            _ => Err(wrong_type(list, "it isn't a list")),
        }
    }

    fn ready(&self) -> Result<&Parsed<T>, Refused> {
        self.parsed.as_ref().map_err(|failure| Refused::ReadOnly(failure.message().to_owned()))
    }

    /// Sets the property at `path`.
    pub fn set(
        &mut self,
        label: &str,
        path: &PropertyPath,
        value: &dyn PartialReflect,
    ) -> Result<Outcome, Refused> {
        let parsed = self.ready()?;
        let place = walk::positional(parsed.value.as_partial_reflect(), path)?;
        let mut expected = parsed.value.clone();
        let target = walk::get_mut(expected.as_partial_reflect_mut(), &place)?;
        walk::assign(target, value, &self.registry).map_err(|why| wrong_type(path, &why))?;
        self.commit_set(label, path, &place, expected)
    }

    /// [`Self::set`], from RON: what a clipboard, a typed field or a replayed record holds.
    pub fn set_text(&mut self, label: &str, path: &PropertyPath, ron: &str) -> Result<Outcome, Refused> {
        let parsed = self.ready()?;
        let at = walk::get(parsed.value.as_partial_reflect(), path)?;
        let value = self.read_as(at, path, ron)?;
        self.set(label, path, value.as_ref())
    }

    /// [`Self::set`], from RON that goes in the file as it's spelled: what a person typed stays as they
    /// typed it, where [`Self::set_text`] writes the value in the file's style. A spelling that changes
    /// only its spelling is an edit too. Where the file leaves the property out there's no spelling to keep
    /// around it, and this is [`Self::set_text`].
    pub fn set_literal(
        &mut self,
        label: &str,
        path: &PropertyPath,
        literal: &str,
    ) -> Result<Outcome, Refused> {
        let parsed = self.ready()?;
        let old_root = parsed.value.as_partial_reflect();
        let place = walk::positional(old_root, path)?;
        let (node, taken) = walk::place(old_root, body(&parsed.tree), &self.text, &place)?;
        if taken < place.steps().len() {
            return self.set_text(label, path, literal);
        }
        if node.text(&self.text) == literal {
            return Ok(Outcome::Unchanged);
        }
        let old = walk::get(old_root, &place)?;
        let value = self.read_as(old, path, literal)?;
        let mut expected = parsed.value.clone();
        let target = walk::get_mut(expected.as_partial_reflect_mut(), &place)?;
        walk::assign(target, value.as_ref(), &self.registry).map_err(|why| wrong_type(path, &why))?;
        let (old_text, new_text) =
            (write::canonical(old, &self.registry)?, write::canonical(value.as_ref(), &self.registry)?);
        let guard = self.guard_of(path, Some(path.owner()))?;
        let splices = [Splice { range: node.span.clone(), text: literal.to_owned() }];
        let (parsed, next, comments_lost) = self.check_splices(&splices, &expected, path)?;
        let record = ChangeRecord {
            path: path.clone(),
            op: Op::Set,
            old: Some(old_text),
            new: Some(new_text),
            comments_lost,
            rewrote_whole: false,
            guard,
            guard_absorbed: false,
        };
        self.land(label, parsed, next, record);
        Ok(Outcome::Changed)
    }

    fn read_as(
        &self,
        like: &dyn PartialReflect,
        path: &PropertyPath,
        ron: &str,
    ) -> Result<Box<dyn PartialReflect>, Refused> {
        let info = like
            .get_represented_type_info()
            .ok_or_else(|| wrong_type(path, "it has no type to read that as"))?;
        self.read_type(info, path, ron)
    }

    fn read_type(
        &self,
        info: &TypeInfo,
        path: &PropertyPath,
        ron: &str,
    ) -> Result<Box<dyn PartialReflect>, Refused> {
        let registration = self
            .registry
            .get(info.type_id())
            .ok_or_else(|| wrong_type(path, &format!("`{}` isn't registered", info.type_path())))?;
        write::read(ron, registration, &self.registry).map_err(|error| wrong_type(path, &error.to_string()))
    }

    /// Lands a change to the value at `place`, which is `path` by entries' places, so that it finds the same
    /// thing in the new value when the edit renamed the entry `path` calls.
    fn commit_set(
        &mut self,
        label: &str,
        path: &PropertyPath,
        place: &PropertyPath,
        expected: T,
    ) -> Result<Outcome, Refused> {
        let parsed = self.ready()?;
        let old_root = parsed.value.as_partial_reflect();
        let new_root = expected.as_partial_reflect();
        let (old, new) = (walk::get(old_root, place)?, walk::get(new_root, place)?);
        let (old_text, new_text) =
            (write::canonical(old, &self.registry)?, write::canonical(new, &self.registry)?);
        if old_text == new_text {
            return Ok(Outcome::Unchanged);
        }
        let guard = self.guard_of(path, Some(path.owner()))?;
        // A field going to `None` leaves the file, which only the struct around it can do.
        let plan_at = match place.split_last() {
            Some((parent, Step::Field(_))) if walk::is_none(new) => parent,
            _ => place.clone(),
        };
        let (node, taken) = walk::place(old_root, body(&parsed.tree), &self.text, &plan_at)?;
        let written = plan_at.prefix(taken);
        let (from, to) = (walk::get(old_root, &written)?, walk::get(new_root, &written)?);
        let mut patch = RonPatch::new(&self.text, &self.registry);
        patch.diff(node, from, to)?;
        let fine = patch.into_splices();
        let rewrite = |document: &Self| -> Result<Vec<Splice>, Refused> {
            let mut patch = RonPatch::new(&document.text, &document.registry);
            patch.replace(node, to)?;
            Ok(patch.into_splices())
        };
        let checked = self.check_splices(&fine, &expected, path);
        let (parsed, next, comments_lost, rewrote_whole) = match checked {
            Ok((parsed, next, lost)) => (parsed, next, lost, false),
            Err(first) => {
                // What's refused for its value stays refused however it's written. Only a plan that came
                // to nothing gets a second try, by the whole value.
                if matches!(first, Refused::Document(_)) && !has_overlap(&fine) {
                    return Err(first);
                }
                let whole = rewrite(self)?;
                let (parsed, next, lost) = self.check_splices(&whole, &expected, path)?;
                (parsed, next, lost, true)
            }
        };
        let record = ChangeRecord {
            path: path.clone(),
            op: Op::Set,
            old: Some(old_text),
            new: Some(new_text),
            comments_lost,
            rewrote_whole,
            guard,
            guard_absorbed: false,
        };
        self.land(label, parsed, next, record);
        Ok(Outcome::Changed)
    }

    /// Adds `value` to the list at `list`, to be its entry at `at`.
    pub fn insert(
        &mut self,
        label: &str,
        list: &PropertyPath,
        at: usize,
        value: &dyn PartialReflect,
    ) -> Result<Outcome, Refused> {
        self.insert_spelled(label, list, at, value, None)
    }

    /// [`Self::insert`], from RON that goes in the file as it's spelled.
    pub fn insert_literal(
        &mut self,
        label: &str,
        list: &PropertyPath,
        at: usize,
        literal: &str,
    ) -> Result<Outcome, Refused> {
        let (at, value) = self.read_entry(list, at, literal)?;
        self.insert_spelled(label, list, at, value.as_ref(), Some(literal))
    }

    fn insert_spelled(
        &mut self,
        label: &str,
        list: &PropertyPath,
        at: usize,
        value: &dyn PartialReflect,
        literal: Option<&str>,
    ) -> Result<Outcome, Refused> {
        let parsed = self.ready()?;
        let mut expected = parsed.value.clone();
        let target = walk::get_mut(expected.as_partial_reflect_mut(), list)?;
        let ReflectMut::List(items) = target.reflect_mut() else {
            return Err(wrong_type(list, "it isn't a list"));
        };
        if at > items.len() {
            return Err(wrong_type(list, &format!("it has no place {at}")));
        }
        let info = items
            .get_represented_type_info()
            .and_then(|info| info.as_list().ok())
            .ok_or_else(|| wrong_type(list, "its entries have no type"))?;
        if !value.is_dynamic() && !list_holds(info, value) {
            return Err(wrong_type(list, "that isn't what its entries are"));
        }
        // Concrete List::insert panics if a dynamic value cannot be converted to its item type.
        // Perform that fallible conversion before calling it, just as assignment does.
        let entry = converted_entry(items, value, &self.registry)
            .ok_or_else(|| wrong_type(list, "that isn't what its entries are"))?;
        // A list the file leaves out may still hold its default's entries, which a new list of one would lose.
        let was_empty = items.is_empty();
        items.insert(at, entry);
        let new_item = walk::get(expected.as_partial_reflect(), &list.join(Step::Index(at)))?;
        let new_text = write::canonical(new_item, &self.registry)?;
        let guard = match walk::name_of(new_item) {
            Some(_) => Guard { owner: list.clone(), was: None },
            None => self.guard_of(list, Some(list.clone()))?,
        };
        let old_root = parsed.value.as_partial_reflect();
        let (node, taken) = walk::place(old_root, body(&parsed.tree), &self.text, list)?;
        let mut patch = RonPatch::new(&self.text, &self.registry);
        let written = list.prefix(taken);
        let new_root = expected.as_partial_reflect();
        if taken == list.steps().len() {
            match literal {
                Some(literal) => patch.insert_text(node, at, literal)?,
                None => patch.insert_item(node, at, new_item)?,
            }
        } else if let (true, true, Step::Field(name)) =
            (was_empty, taken + 1 == list.steps().len(), &list.steps()[list.steps().len() - 1])
        {
            patch.new_list(node, walk::get(new_root, &written)?, name, new_item, literal)?;
        } else {
            patch.diff(node, walk::get(old_root, &written)?, walk::get(new_root, &written)?)?;
        }
        let splices = patch.into_splices();
        let (parsed, next, comments_lost) = self.check_splices(&splices, &expected, list)?;
        let record = ChangeRecord {
            path: list.clone(),
            op: Op::Insert { at },
            old: None,
            new: Some(new_text),
            comments_lost,
            rewrote_whole: false,
            guard,
            guard_absorbed: false,
        };
        self.land(label, parsed, next, record);
        Ok(Outcome::Changed)
    }

    /// Adds a new entry to the end of the list at `list`, made from its type alone ([`crate::blank`]): what an
    /// inspector's "add" does. What it starts as is for a person to fill in, and the document's parser may
    /// refuse it, as it would any edit.
    pub fn insert_blank(&mut self, label: &str, list: &PropertyPath) -> Result<Outcome, Refused> {
        let parsed = self.ready()?;
        let value = walk::get(parsed.value.as_partial_reflect(), list)?;
        let ReflectRef::List(items) = value.reflect_ref() else {
            return Err(wrong_type(list, "it isn't a list"));
        };
        let at = items.len();
        let info = value
            .get_represented_type_info()
            .and_then(|info| info.as_list().ok())
            .and_then(|info| self.registry.get_type_info(info.item_ty().id()))
            .ok_or_else(|| wrong_type(list, "its entries have no registered type"))?;
        let entry =
            blank::blank(info, &self.registry).map_err(|error| wrong_type(list, &error.to_string()))?;
        self.insert(label, list, at, entry.as_ref())
    }

    /// Makes the enum at `path` its variant `name`, with that variant's fields new: what picking one in an
    /// inspector does, and turning an `Option` on (`Some`) or off (`None`). The variant it already is stays
    /// as it is.
    pub fn pick_variant(&mut self, label: &str, path: &PropertyPath, name: &str) -> Result<Outcome, Refused> {
        let parsed = self.ready()?;
        let value = walk::get(parsed.value.as_partial_reflect(), path)?;
        if walk::variant(value) == Some(name) {
            return Ok(Outcome::Unchanged);
        }
        let info = value
            .get_represented_type_info()
            .ok_or_else(|| wrong_type(path, "it has no type to pick from"))?;
        let new = blank::blank_variant(info, name, &self.registry)
            .map_err(|error| wrong_type(path, &error.to_string()))?;
        self.set(label, path, new.as_ref())
    }

    /// Takes out the entry at `path`, a list's entry by name or place.
    pub fn remove(&mut self, label: &str, path: &PropertyPath) -> Result<Outcome, Refused> {
        let parsed = self.ready()?;
        let (list, step) = path.split_last().ok_or_else(|| wrong_type(path, "that's the document"))?;
        let list_value = walk::get(parsed.value.as_partial_reflect(), &list)?;
        let ReflectRef::List(items) = list_value.reflect_ref() else {
            return Err(wrong_type(&list, "it isn't a list"));
        };
        let at = place_in_list(items, step).ok_or_else(|| wrong_type(path, "no such entry"))?;
        let old = write::canonical(walk::get(parsed.value.as_partial_reflect(), path)?, &self.registry)?;
        let guard = self
            .guard_of(path, Some(if matches!(step, Step::Key(_)) { path.clone() } else { list.clone() }))?;
        let mut expected = parsed.value.clone();
        let target = walk::get_mut(expected.as_partial_reflect_mut(), &list)?;
        let ReflectMut::List(items) = target.reflect_mut() else {
            return Err(wrong_type(&list, "it isn't a list"));
        };
        drop(items.remove(at));
        let old_root = parsed.value.as_partial_reflect();
        let (node, taken) = walk::place(old_root, body(&parsed.tree), &self.text, &list)?;
        let mut patch = RonPatch::new(&self.text, &self.registry);
        if taken == list.steps().len() {
            patch.remove_item(node, at);
        } else {
            // The file leaves the list out and its default holds entries: what's left of them is written.
            let written = list.prefix(taken);
            patch.diff(
                node,
                walk::get(old_root, &written)?,
                walk::get(expected.as_partial_reflect(), &written)?,
            )?;
        }
        let splices = patch.into_splices();
        let (parsed, next, comments_lost) = self.check_splices(&splices, &expected, path)?;
        let record = ChangeRecord {
            path: path.clone(),
            op: Op::Remove { at },
            old: Some(old),
            new: None,
            comments_lost,
            rewrote_whole: false,
            guard,
            guard_absorbed: false,
        };
        self.land(label, parsed, next, record);
        Ok(Outcome::Changed)
    }

    /// Moves the entry at `from` of the list at `list` to `to`.
    pub fn reorder(
        &mut self,
        label: &str,
        list: &PropertyPath,
        from: usize,
        to: usize,
    ) -> Result<Outcome, Refused> {
        if from == to {
            return Ok(Outcome::Unchanged);
        }
        let parsed = self.ready()?;
        let guard = self.guard_of(list, Some(list.clone()))?;
        let mut expected = parsed.value.clone();
        let target = walk::get_mut(expected.as_partial_reflect_mut(), list)?;
        let ReflectMut::List(items) = target.reflect_mut() else {
            return Err(wrong_type(list, "it isn't a list"));
        };
        if from >= items.len() || to >= items.len() {
            return Err(wrong_type(list, &format!("it has {} entries", items.len())));
        }
        let moved = items.remove(from);
        items.insert(to, moved);
        let (node, taken) =
            walk::place(parsed.value.as_partial_reflect(), body(&parsed.tree), &self.text, list)?;
        if taken != list.steps().len() {
            return Err(wrong_type(list, "the file doesn't write it"));
        }
        let mut patch = RonPatch::new(&self.text, &self.registry);
        patch.reorder_item(node, from, to);
        let splices = patch.into_splices();
        let (parsed, next, _) = self.check_splices(&splices, &expected, list)?;
        let record = ChangeRecord {
            path: list.clone(),
            op: Op::Reorder { from, to },
            old: None,
            new: None,
            comments_lost: 0,
            rewrote_whole: false,
            guard,
            guard_absorbed: false,
        };
        self.land(label, parsed, next, record);
        Ok(Outcome::Changed)
    }

    fn guard_of(&self, path: &PropertyPath, owner: Option<PropertyPath>) -> Result<Guard, Refused> {
        let parsed = self.ready()?;
        let owner = owner.unwrap_or_else(|| path.clone());
        let was = write::canonical(walk::get(parsed.value.as_partial_reflect(), &owner)?, &self.registry)?;
        Ok(Guard { owner, was: Some(was) })
    }

    /// The new text, held to the document's parser and the value it was to make.
    fn check_splices(
        &self,
        splices: &[Splice],
        expected: &T,
        path: &PropertyPath,
    ) -> Result<(Parsed<T>, String, usize), Refused> {
        let next = layout::apply(&self.text, splices)
            .ok_or_else(|| Refused::Disagrees { path: path.to_string() })?;
        let parsed = read(&next, self.schema).map_err(|failure| match failure {
            ReadFailure::Parse(message) => Refused::Document(message),
            ReadFailure::Scan(_) => Refused::Disagrees { path: path.to_string() },
        })?;
        if parsed.value != *expected {
            return Err(Refused::Disagrees { path: path.to_string() });
        }
        let lost = splices
            .iter()
            .map(|splice| {
                let mut kept = text::comments(&splice.text);
                text::comments(&self.text[splice.range.clone()])
                    .into_iter()
                    .filter(|comment| match kept.iter().position(|candidate| candidate == comment) {
                        Some(at) => {
                            kept.remove(at);
                            false
                        }
                        None => true,
                    })
                    .count()
            })
            .sum();
        Ok((parsed, next, lost))
    }

    fn land(&mut self, label: &str, parsed: Parsed<T>, next: String, record: ChangeRecord) {
        let before = std::mem::replace(&mut self.text, next);
        self.parsed = Ok(parsed);
        match &mut self.gesture {
            Some(gesture) => absorb(&mut gesture.records, record),
            None => {
                let change = Change::between(&before, &self.text);
                self.push_step(HistoryStep { label: label.to_owned(), change, records: vec![record] });
            }
        }
    }

    /// Runs `edit` as one edit: every set it makes lands, or none does and the text is as it was. It's a step
    /// of its own, under `label`, or part of the gesture in progress if there's one. The closure may
    /// make property edits and nested `together` calls; it must not manipulate history, gestures or
    /// the saved/disk state itself.
    pub fn together<R, E>(
        &mut self,
        label: &str,
        edit: impl FnOnce(&mut Self) -> Result<R, E>,
    ) -> Result<R, E> {
        let Some(gesture) = &self.gesture else {
            self.begin_gesture(label);
            let result = edit(self);
            match result {
                Ok(_) => self.end_gesture(),
                Err(_) => self.cancel_gesture(),
            }
            return result;
        };
        let (text, records) = (self.text.clone(), gesture.records.clone());
        let result = edit(self);
        if result.is_err() {
            if let Some(gesture) = &mut self.gesture {
                gesture.records = records;
            }
            if text != self.text {
                self.set_raw_text(text);
            }
        }
        result
    }

    /// Starts a gesture: edits until [`Self::end_gesture`] are one step of undo. One in progress ends, as
    /// a step of its own.
    pub fn begin_gesture(&mut self, label: &str) {
        self.end_gesture();
        self.gesture =
            Some(Gesture { label: label.to_owned(), start: self.text.clone(), records: Vec::new() });
    }

    /// Ends a gesture as one step. One that left the bytes as they were is no step, and redo stays.
    pub fn end_gesture(&mut self) {
        if let Some(gesture) = self.gesture.take()
            && gesture.start != self.text
        {
            let change = Change::between(&gesture.start, &self.text);
            self.push_step(HistoryStep { label: gesture.label, change, records: gesture.records });
        }
    }

    /// A new step of history. If the file's text was undone from, it's a branch history can't get back to the
    /// file along, so a rebase can't say what's unsaved.
    fn push_step(&mut self, step: HistoryStep) {
        if self.saved_at.is_some_and(|saved| saved > self.undo.len()) {
            self.saved_at = None;
        }
        self.undo.push(step);
        self.redo.clear();
        self.forget_past_limit();
    }

    /// Lets go of the oldest steps until what's held fits, or only the newest is left. A save point that goes
    /// with them can't be got back to, which is what `saved_at` being `None` says.
    fn forget_past_limit(&mut self) {
        let mut held: usize = self.undo.iter().chain(&self.redo).map(HistoryStep::bytes).sum();
        let mut gone = 0;
        // Keep the nearest undo step when possible. Redo runs from the vector's end, so dropping
        // its prefix later forgets the farthest future without breaking the remaining splice chain.
        while held > self.history_limit && gone + 1 < self.undo.len() {
            held -= self.undo[gone].bytes();
            gone += 1;
        }
        if gone > 0 {
            self.undo.drain(..gone);
            self.forgotten += gone;
            self.saved_at = self.saved_at.and_then(|saved| saved.checked_sub(gone));
        }
        let mut gone_redo = 0;
        while held > self.history_limit && self.undo.len() + self.redo.len() - gone_redo > 1 {
            held -= self.redo[gone_redo].bytes();
            gone_redo += 1;
        }
        if gone_redo > 0 {
            self.redo.drain(..gone_redo);
            // A saved point in the discarded future can no longer identify replayable unsaved edits.
            if self.saved_at.is_some_and(|saved| saved > self.undo.len() + self.redo.len()) {
                self.saved_at = None;
            }
        }
    }

    /// Ends a gesture by taking back what it did, leaving no step of undo.
    pub fn cancel_gesture(&mut self) {
        if let Some(gesture) = self.gesture.take()
            && gesture.start != self.text
        {
            self.set_raw_text(gesture.start);
        }
    }

    /// The last step of undo.
    #[must_use]
    pub fn last_step(&self) -> Option<&HistoryStep> {
        self.undo.last()
    }

    /// What undo would take back.
    #[must_use]
    pub fn undo_label(&self) -> Option<&str> {
        match &self.gesture {
            Some(gesture) if gesture.start != self.text => Some(&gesture.label),
            _ => self.undo.last().map(|step| step.label.as_str()),
        }
    }

    /// What redo would bring back.
    #[must_use]
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|step| step.label.as_str())
    }

    /// Takes back the last step. A gesture in progress ends first.
    pub fn undo(&mut self) -> bool {
        self.end_gesture();
        let Some(step) = self.undo.pop() else { return false };
        self.set_raw_text(step.change.undo(&self.text).expect("undo runs in the order it was done"));
        self.redo.push(step);
        true
    }

    /// Does the last undone step again.
    pub fn redo(&mut self) -> bool {
        self.end_gesture();
        let Some(step) = self.redo.pop() else { return false };
        self.set_raw_text(step.change.redo(&self.text).expect("redo runs in the order it was done"));
        self.undo.push(step);
        true
    }

    fn set_raw_text(&mut self, text: String) {
        let next = read(&text, self.schema);
        if let Ok(parsed) = std::mem::replace(&mut self.parsed, next) {
            self.last_good = Some(parsed.value);
        }
        self.text = text;
    }

    /// The file changed on disk to `disk`, and its text replaces what's here, as one step undo can take
    /// back. What was unsaved stays in the history beneath it. T3's two buttons are this and
    /// [`Self::keep_mine`], and [`Self::rebase_on_disk`] is the third.
    pub fn take_disk(&mut self, label: &str, disk: String) {
        self.end_gesture();
        if disk != self.text {
            self.push_step(external_step(label, &self.text, &disk));
            self.set_raw_text(disk.clone());
        }
        self.disk = disk.into_bytes();
        self.saved_at = Some(self.undo.len());
    }

    /// The file changed on disk to `disk`. Takes it, as a step undo can take back, and puts unsaved records
    /// back on top by path. A record lands if its [`Guard`] still holds: what it was changing says what it
    /// said, whatever a comment or a spelling around it did, and an entry it reached by a place is the same
    /// entry. It's refused if not, and a text that doesn't read refuses them all.
    ///
    /// What's unsaved is what the history holds past the last save. After an undo past that save and a new
    /// edit, or after [`Self::keep_mine`], history can't say, so nothing's laid back on top, and it's all still
    /// in undo beneath the file's text: [`Rebased::beneath`] says so.
    ///
    /// An editor can show this before committing to it by trying it on a clone of the document's records:
    /// saying no is [`Self::keep_mine`].
    pub fn rebase_on_disk(&mut self, disk: String) -> Rebased {
        self.end_gesture();
        if disk.as_bytes() == self.disk {
            return Rebased::default();
        }
        let was_dirty = self.is_dirty();
        let unsaved: Vec<HistoryStep> = match self.saved_at {
            Some(saved) if saved <= self.undo.len() => self.undo.split_off(saved),
            _ => Vec::new(),
        };
        for step in unsaved.iter().rev() {
            self.set_raw_text(step.change.undo(&self.text).expect("undo runs in the order it was done"));
        }
        self.push_step(external_step("the file changed on disk", &self.text, &disk));
        self.set_raw_text(disk.clone());
        self.disk = disk.into_bytes();
        self.saved_at = Some(self.undo.len());
        let mut rebased = Rebased { beneath: was_dirty && unsaved.is_empty(), ..Rebased::default() };
        for step in unsaved {
            let HistoryStep { label, records, .. } = step;
            // A step goes back as one step.
            self.begin_gesture(&label);
            let mut refused_owners = Vec::new();
            for record in records {
                // Absorption keeps one guard per owner in a gesture. A later record whose guard
                // was absorbed depends on that first replay having succeeded.
                let result = if record.guard_absorbed && refused_owners.contains(&record.guard.owner) {
                    Err(Refused::Document(format!("an earlier edit of `{}` was refused", record.guard.owner)))
                } else {
                    self.replay(&label, &record)
                };
                if result.is_err() && !refused_owners.contains(&record.guard.owner) {
                    refused_owners.push(record.guard.owner.clone());
                }
                match result {
                    Ok(_) => rebased.kept += 1,
                    Err(why) => rebased.refused.push(RefusedRecord {
                        label: label.clone(),
                        record,
                        why: why.to_string(),
                    }),
                }
            }
            self.end_gesture();
        }
        rebased
    }

    /// Settles a change on disk by keeping the text here: the next save overwrites the file with it. `disk`
    /// is the bytes there, which needn't be text.
    pub fn keep_mine(&mut self, disk: impl Into<Vec<u8>>) {
        self.end_gesture();
        self.disk = disk.into();
        self.saved_at = None;
    }

    /// Does `record` again on the text as it is now, by its path and its values.
    fn replay(&mut self, label: &str, record: &ChangeRecord) -> Result<Outcome, Refused> {
        let parsed = self.ready()?;
        let root = parsed.value.as_partial_reflect();
        let path = &record.path;
        let stale = |why: String| Refused::Document(why);
        match (&record.guard.was, walk::get(root, &record.guard.owner)) {
            (Some(was), Ok(now)) => {
                if write::canonical(now, &self.registry)? != *was {
                    return Err(stale(format!("`{}` changed in the file", record.guard.owner)));
                }
            }
            (Some(_), Err(_)) => return Err(stale(format!("the file has no `{}` now", record.guard.owner))),
            (None, _) => {}
        }
        match record.op {
            Op::Set => {
                let new = record.new.as_deref().ok_or_else(|| stale("a set with no value".to_owned()))?;
                self.set_text(label, path, new)
            }
            Op::Insert { at } => {
                let new =
                    record.new.as_deref().ok_or_else(|| stale("a new entry with no value".to_owned()))?;
                let (at, value) = self.read_entry(path, at, new)?;
                if let Some(name) = walk::name_of(value.as_ref())
                    && self.names_in(path)?.iter().any(|existing| existing == name)
                {
                    return Err(stale(format!("the file has its own `{name}` in `{path}` now")));
                }
                self.insert(label, path, at, value.as_ref())
            }
            Op::Remove { .. } => self.remove(label, path),
            Op::Reorder { from, to } => self.reorder(label, path, from, to),
        }
    }

    fn read_entry(
        &self,
        list: &PropertyPath,
        at: usize,
        ron: &str,
    ) -> Result<(usize, Box<dyn PartialReflect>), Refused> {
        let parsed = self.ready()?;
        let value = walk::get(parsed.value.as_partial_reflect(), list)?;
        let ReflectRef::List(items) = value.reflect_ref() else {
            return Err(wrong_type(list, "it isn't a list"));
        };
        let info = value
            .get_represented_type_info()
            .and_then(|info| info.as_list().ok())
            .and_then(|info| self.registry.get_type_info(info.item_ty().id()))
            .ok_or_else(|| wrong_type(list, "its entries have no registered type"))?;
        let entry = self.read_type(info, list, ron)?;
        Ok((at.min(items.len()), entry))
    }

    fn names_in(&self, list: &PropertyPath) -> Result<Vec<String>, Refused> {
        let parsed = self.ready()?;
        let ReflectRef::List(items) = walk::get(parsed.value.as_partial_reflect(), list)?.reflect_ref()
        else {
            return Err(wrong_type(list, "it isn't a list"));
        };
        Ok(items.iter().filter_map(walk::name_of).map(ToOwned::to_owned).collect())
    }
}

/// A step that came from outside: all it knows is the bytes.
fn external_step(label: &str, before: &str, after: &str) -> HistoryStep {
    let record = ChangeRecord {
        path: PropertyPath::root(),
        op: Op::Set,
        old: None,
        new: None,
        comments_lost: 0,
        rewrote_whole: true,
        guard: Guard { owner: PropertyPath::root(), was: None },
        guard_absorbed: false,
    };
    HistoryStep { label: label.to_owned(), change: Change::between(before, after), records: vec![record] }
}

fn place_in_list(items: &dyn bevy_reflect::list::List, step: &Step) -> Option<usize> {
    match step {
        Step::Index(at) => (*at < items.len()).then_some(*at),
        Step::Key(key) => items.iter().position(|entry| walk::name_of(entry) == Some(key)),
        _ => None,
    }
}

/// Resolves the list's fallible item conversion before invoking its infallible insert API.
fn converted_entry(
    list: &dyn bevy_reflect::list::List,
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
) -> Option<Box<dyn PartialReflect>> {
    let info = list.get_represented_type_info()?;
    if let Some(from) = registry.get_type_data::<ReflectFromReflect>(info.as_list().ok()?.item_ty().id()) {
        return from.from_reflect(value).map(|entry| entry.into_partial_reflect());
    }
    // Tuple and array registrations don't expose ReflectFromReflect. A singleton of the owning
    // list still performs that exact conversion through its statically known item type.
    let mut singleton = bevy_reflect::list::DynamicList::default();
    singleton.push_box(value.to_dynamic().ok()?);
    let mut converted =
        registry.get_type_data::<ReflectFromReflect>(info.type_id())?.from_reflect(&singleton)?;
    let ReflectMut::List(converted) = converted.reflect_mut() else { return None };
    (converted.len() == 1).then(|| converted.remove(0))
}

fn list_holds(list: &bevy_reflect::list::ListInfo, value: &dyn PartialReflect) -> bool {
    value.get_represented_type_info().is_some_and(|info| info.type_id() == list.item_ty().id())
}

fn wrong_type(path: &PropertyPath, why: &str) -> Refused {
    Refused::WrongType { path: path.to_string(), why: why.to_owned() }
}

fn has_overlap(splices: &[Splice]) -> bool {
    let mut ranges: Vec<_> = splices.iter().map(|splice| splice.range.clone()).collect();
    ranges.sort_by_key(|range| range.start);
    ranges.windows(2).any(|pair| pair[1].start < pair[0].end)
}

/// A document's body: under its envelope's `data`, or all of it if it hasn't one.
fn body(tree: &Node) -> &Node {
    tree.field("data").filter(|_| tree.field("type").is_some()).unwrap_or(tree)
}

/// Reads a text: the document it says, and the tree of where it says it, held to each other.
fn read<T: Reflect>(text: &str, schema: Schema<T>) -> Result<Parsed<T>, ReadFailure> {
    let value = (schema.parse)(text).map_err(ReadFailure::Parse)?;
    let tree = text::scan(text).map_err(|error| {
        ReadFailure::Scan(format!("the scan stopped at byte {}: {}", error.at, error.what))
    })?;
    walk::agreement(value.as_partial_reflect(), body(&tree), text).map_err(ReadFailure::Scan)?;
    Ok(Parsed { value, tree })
}
