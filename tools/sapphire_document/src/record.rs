//! The one edit: a change record, and the steps of undo that hold them.
//!
//! A record says what was meant, by path and value: what goes back on a file that changed, or to a device.
//! A step says what it did to the bytes, as one [`Change`], so undo gives those back whatever they spelled.
//! A gesture is one step with its records merged, so a long drag holds what a short one does.

use std::fmt;

use sapphire_assets::AssetId;

use crate::path::PropertyPath;

/// The smallest splice between two texts: what a record does to a file's bytes, so undoing it is giving
/// those bytes back, whatever they spelled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub at: usize,
    pub removed: String,
    pub inserted: String,
}

impl Change {
    /// What takes `old` to `new`.
    #[must_use]
    pub fn between(old: &str, new: &str) -> Self {
        let prefix = common_prefix(old, new);
        let suffix = common_suffix(&old[prefix..], &new[prefix..]);
        Self {
            at: prefix,
            removed: old[prefix..old.len() - suffix].to_owned(),
            inserted: new[prefix..new.len() - suffix].to_owned(),
        }
    }

    /// `text` with this taken back. `None` if `text` isn't what this left, which is a bug in whoever kept
    /// the order of a history.
    #[must_use]
    pub fn undo(&self, text: &str) -> Option<String> {
        swapped(text, self.at, &self.inserted, &self.removed)
    }

    /// `text` with this done again. `None` as for [`Self::undo`].
    #[must_use]
    pub fn redo(&self, text: &str) -> Option<String> {
        swapped(text, self.at, &self.removed, &self.inserted)
    }
}

fn swapped(text: &str, at: usize, out: &str, into: &str) -> Option<String> {
    let end = at.checked_add(out.len())?;
    (text.get(at..end)? == out).then(|| format!("{}{into}{}", &text[..at], &text[end..]))
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

/// What a record did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Set the property at the record's path.
    Set,
    /// Added an entry to the list at the record's path, at this place.
    Insert { at: usize },
    /// Took out the entry at the record's path, from this place in its list.
    Remove { at: usize },
    /// Moved an entry of the list at the record's path.
    Reorder { from: usize, to: usize },
}

/// What must still hold for a record to go on top of a file that changed under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guard {
    pub owner: PropertyPath,
    /// What was there, canonically, so that a change of spelling or a comment doesn't count. `None` if
    /// nothing was: a new entry, which only needs its name free.
    pub was: Option<String>,
}

/// Which object: a document, by the ID its sidecar gives it, and how far into it a path goes by names. It
/// doesn't change when the file is renamed or moved, or when the entry's list is reordered.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectId {
    pub document: AssetId,
    pub path: PropertyPath,
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.document, self.path)
    }
}

/// One edit: the spec's `(object, property, old, new)`. The object is the document and
/// [`PropertyPath::object`], and old and new are RON, as a clipboard or a device would take them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeRecord {
    pub path: PropertyPath,
    pub op: Op,
    /// `None` for a new entry.
    pub old: Option<String>,
    /// `None` for an entry taken out.
    pub new: Option<String>,
    /// Comments that went with it, which only undo brings back: inside a value that was replaced whole or
    /// a field that was taken out.
    pub comments_lost: usize,
    /// Whether it couldn't be walked and fell back to writing the whole value new.
    pub rewrote_whole: bool,
    pub(crate) guard: Guard,
}

impl ChangeRecord {
    /// The object it changed, in the document `document`.
    #[must_use]
    pub fn object(&self, document: AssetId) -> ObjectId {
        ObjectId { document, path: self.path.object() }
    }

    fn bytes(&self) -> usize {
        size_of::<Self>()
            + self.path.bytes()
            + self.guard.owner.bytes()
            + self.old.as_ref().map_or(0, String::len)
            + self.new.as_ref().map_or(0, String::len)
            + self.guard.was.as_ref().map_or(0, String::len)
    }
}

/// A step of undo: one edit, or everything in one gesture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryStep {
    pub label: String,
    /// What it did to the bytes, all of it.
    pub change: Change,
    pub records: Vec<ChangeRecord>,
}

impl HistoryStep {
    /// The bytes it holds, for what a history costs and what its cap counts: its strings, and the structs
    /// that hold them, so a thousand small edits aren't taken for nothing.
    #[must_use]
    pub fn bytes(&self) -> usize {
        size_of::<Self>()
            + self.label.len()
            + self.change.removed.len()
            + self.change.inserted.len()
            + self.records.iter().map(ChangeRecord::bytes).sum::<usize>()
    }
}

/// Adds `record` to those of a gesture so far, so that they stay one per property however long the gesture
/// runs. A set of what's already been set this gesture changes that record, unless the list it's in has
/// changed shape since, and a value dragged back to where it began leaves no record.
pub(crate) fn absorb(records: &mut Vec<ChangeRecord>, mut record: ChangeRecord) {
    if record.op == Op::Set {
        let earlier = records
            .iter()
            .rposition(|earlier| earlier.op != Op::Set || earlier.path == record.path)
            .filter(|at| records[*at].op == Op::Set);
        if let Some(at) = earlier {
            let merged = &mut records[at];
            merged.new = record.new;
            merged.comments_lost += record.comments_lost;
            merged.rewrote_whole |= record.rewrote_whole;
            if merged.old == merged.new {
                let gone = records.remove(at);
                // Its look at the owner goes to the next record that relied on it.
                if let Some(next) = records.iter_mut().find(|next| next.guard.owner == gone.guard.owner) {
                    next.guard.was = gone.guard.was;
                }
            }
            return;
        }
    }
    // What a gesture wrote itself isn't what the file might have changed: only its first look at an owner
    // has to hold when the records go back on a changed file.
    if records.iter().any(|earlier| earlier.guard.owner == record.guard.owner) {
        record.guard.was = None;
    }
    records.push(record);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_the_smallest_splice_on_character_edges() {
        let change = Change::between("héllo wörld", "héllo wurld");
        assert_eq!((change.at, change.removed.as_str(), change.inserted.as_str()), (8, "ö", "u"));
        assert_eq!(change.undo("héllo wurld").as_deref(), Some("héllo wörld"));
        assert_eq!(change.redo("héllo wörld").as_deref(), Some("héllo wurld"));
    }

    #[test]
    fn a_change_refuses_a_text_it_didnt_leave() {
        let change = Change::between("abc", "axc");
        assert_eq!(change.undo("abc"), None);
        assert_eq!(change.redo("axc"), None);
        assert_eq!(change.undo("a"), None);
    }
}
