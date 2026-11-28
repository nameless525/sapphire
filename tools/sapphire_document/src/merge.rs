//! A three-way merge by property path (M6 T7): what `sapphire_document merge` does as git's merge driver for the
//! documents this crate knows.
//!
//! Each side's change from the base is read as edits by path, the way a [`crate::ChangeRecord`] names one: a
//! property set, an entry added to or taken out of a list. A list's entries go by their `id` or `name`
//! ([`walk::name_of`]), never by place, so two sides that each add an entry, or edit two entries, or two fields
//! of one, don't meet. A list whose entries have no names is one value, since a place can't say which entry an
//! edit meant once the other side moved one; so is a list whose named entries the other side reordered. Their
//! edits are laid on our text by the document model, through [`crate::RonPatch`], each spelled as they wrote it,
//! so our comments and layout stay and theirs go with what they changed. What they changed of comments and
//! layout alone is carried by a line merge on top, if it merges cleanly; if not, the values' merge stands and a
//! note says so.
//!
//! Two edits clash where one's path holds the other's (the same property, or an entry one side took out and the
//! other changed), unless they're the same edit. A clash is marked in the text at the value, in git's diff3
//! style: the entry or field as each side writes it, between `<<<<<<<`, `|||||||`, `=======` and `>>>>>>>` lines.
//! Those lines aren't RON, so a marked file doesn't read until a person picks, and nothing commits it as a
//! document by accident; its parse problem names the marker as an unresolved merge conflict ([`unresolved`]).
//! An edit the document's own parser refuses on our side (a zone's ID that a placement of
//! ours took, which a merge never renumbers) is a clash too.
//!
//! What has no model merges by lines, as git's own merge would, and says so ([`How::ByLines`]): a file of no
//! kind, a side that doesn't read, sides at different versions of their document, and anything the edits by
//! path can't remake their side from.

mod lines;
mod markers;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use bevy_reflect::{
    FromReflect, GetTypeRegistration, PartialReflect, Reflect, ReflectRef, TypeRegistry, Typed,
};

use crate::document::{self, Document, Refused};
use crate::kinds::{Kind, Schema, registry_of};
use crate::layout;
use crate::path::{PropertyPath, Step};
use crate::text::{self, Node};
use crate::walk::{self, Shape};
use crate::write::{self, ValueTextError};

pub use lines::{Lines, merge as merge_lines, merge_bytes as merge_lines_bytes};

/// A side of a merge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Ours,
    Theirs,
}

/// How a merge went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum How {
    /// Only one side changed anything, or both changed it alike: that side's bytes.
    Took(Side),
    /// By property path, as a document of `kind`, with `laid` of their edits laid on ours.
    ByPath { kind: Kind, laid: usize },
    /// By lines, as git's own merge would, and why not by path.
    ByLines { why: String },
}

/// Where the sides clash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The property, as a path prints (`moves["light_1"].startup`; empty for the whole document), or for a merge
    /// by lines the line of the merged text its marker starts on.
    pub at: String,
    /// Why, where it's more than both sides changing it.
    pub why: Option<String>,
}

/// What came of a merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merged {
    /// The merged text, with a marker at each conflict.
    pub text: String,
    pub how: How,
    pub conflicts: Vec<Conflict>,
    /// What a person should hear of a merge: what wasn't carried, what the document's own rules now say.
    pub notes: Vec<String>,
}

impl Merged {
    /// Whether it merged with no conflict: git's exit code 0.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }

    /// One line saying how it merged.
    #[must_use]
    pub fn summary(&self) -> String {
        let how = match &self.how {
            How::Took(Side::Ours) => "only ours changed it, or both alike: kept ours".to_owned(),
            How::Took(Side::Theirs) => "only theirs changed it: took theirs".to_owned(),
            How::ByPath { kind, laid } => {
                format!("merged by property path as a {kind:?} document, {laid} of their edits laid on ours")
            }
            How::ByLines { why } => format!("merged by lines, since {why}"),
        };
        match self.conflicts.len() {
            0 => how,
            1 => format!("{how}; 1 conflict"),
            count => format!("{how}; {count} conflicts"),
        }
    }
}

/// Merges `ours` and `theirs`, each a change of `base`, as the document at `name` (the file's path in the tree,
/// which says its kind).
#[must_use]
pub fn merge(name: &Path, base: &str, ours: &str, theirs: &str) -> Merged {
    let took = |side, text: &str| Merged {
        text: text.to_owned(),
        how: How::Took(side),
        conflicts: Vec::new(),
        notes: Vec::new(),
    };
    if theirs == base || ours == theirs {
        return took(Side::Ours, ours);
    }
    if ours == base {
        return took(Side::Theirs, theirs);
    }
    // Git gives an empty base when both sides add the file: there's no base to merge by path from.
    if base.is_empty() {
        return by_lines(base, ours, theirs, "both sides added it, so it has no base".to_owned(), None);
    }
    let kinds = [base, ours, theirs].map(|text| Kind::of_text(name, || Some(text.to_owned())));
    match kinds {
        [Some(kind), ..] if kinds.iter().all(|other| *other == Some(kind)) => kind.merge(base, ours, theirs),
        [None, None, None] => {
            by_lines(base, ours, theirs, "no document kind has a model for this file".to_owned(), None)
        }
        _ => by_lines(base, ours, theirs, "the sides aren't the same kind of document".to_owned(), None),
    }
}

/// [`merge`] as a document of one kind, whose schema is `schema`: what [`Kind::merge`] calls.
pub(crate) fn by_path<T>(kind: Kind, schema: Schema<T>, base: &str, ours: &str, theirs: &str) -> Merged
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let registry = Arc::new(registry_of::<T>());
    let open = |text: &str| Document::open(text.to_owned(), schema, Arc::clone(&registry));
    let lines = |why: String| by_lines(base, ours, theirs, why, Some(kind));
    // Versions first: an older version may not read as the newer's text says, and that's the reason to give.
    let versions = [base, ours, theirs].map(written_version);
    if versions[0] != versions[1] || versions[0] != versions[2] {
        let [base, ours, theirs] =
            versions.map(|version| version.map_or("none".to_owned(), |v| v.to_string()));
        return lines(format!(
            "the sides are at different versions of the document (base {base}, ours {ours}, theirs {theirs})"
        ));
    }
    let sides = [open(base), open(ours), open(theirs)];
    for (name, side) in ["the base", "ours", "theirs"].into_iter().zip(&sides) {
        if let Some(failure) = side.failure() {
            return lines(format!("{name} doesn't read as a {kind:?} document ({})", failure.message()));
        }
    }
    let [base_document, ours_document, theirs_document] = &sides;
    let edits_to = |side: &Document<T>| {
        let spelled =
            Spelled::of(root_of(side), side.text()).ok_or_else(|| ValueTextError("no scan".to_owned()))?;
        edits(root_of(base_document), &spelled, &registry)
    };
    let (mine, theirs_edits) = match (edits_to(ours_document), edits_to(theirs_document)) {
        (Ok(mine), Ok(theirs)) => (mine, theirs),
        (Err(why), _) | (_, Err(why)) => return lines(format!("a value can't be written as text ({why})")),
    };
    // Their edits laid on the base must remake their side, or laying them on ours can't be trusted.
    let mut remade = open(base);
    for edit in &theirs_edits {
        if let Err(why) = lay(&mut remade, edit) {
            return lines(format!("their edit of `{}` doesn't go back on the base ({why})", edit.touches()));
        }
    }
    if remade.value() != theirs_document.value() {
        return lines("their edits by path don't remake their side".to_owned());
    }

    let mut merged = open(ours);
    let (mut clashes, mut laid) = (Vec::new(), 0);
    for edit in &theirs_edits {
        if mine.iter().any(|ours| ours.same(edit)) {
            continue;
        }
        let touches = edit.touches();
        let met: Vec<PropertyPath> = mine
            .iter()
            .map(Edit::touches)
            .filter(|ours| holds(ours, &touches) || holds(&touches, ours))
            .collect();
        if !met.is_empty() {
            let at = met.into_iter().chain([touches]).min_by_key(|path| path.steps().len()).expect("one");
            clashes.push(markers::Clash { at, why: None });
            continue;
        }
        match lay(&mut merged, edit) {
            Ok(_) => laid += 1,
            Err(why) => clashes.push(markers::Clash { at: touches, why: Some(why.to_string()) }),
        }
    }

    let mut notes = Vec::new();
    // What they changed beside their values (comments, layout), carried by lines onto ours.
    if remade.text() != theirs {
        let carried = lines::merge(remade.text(), merged.text(), theirs);
        let reads_alike = carried.conflicts.is_empty() && open(&carried.text).value() == merged.value();
        if reads_alike {
            merged = open(&carried.text);
        } else {
            notes.push(
                "their edits of comments and layout beside the values they changed weren't carried: they meet \
                 lines ours changed"
                    .to_owned(),
            );
        }
    }

    if clashes.is_empty() {
        let before: BTreeSet<String> =
            ours_document.issues().into_iter().chain(theirs_document.issues()).collect();
        for issue in merged.issues().into_iter().filter(|issue| !before.contains(issue)) {
            notes.push(format!("the merged document's rules say: {issue}"));
        }
        return Merged {
            text: merged.text().to_owned(),
            how: How::ByPath { kind, laid },
            conflicts: Vec::new(),
            notes,
        };
    }
    let roots = [root_of(&merged), root_of(base_document), root_of(theirs_document)];
    let texts = [merged.text(), base, theirs];
    let spelled: Vec<Spelled<'_>> =
        roots.into_iter().zip(texts).filter_map(|(root, text)| Spelled::of(root, text)).collect();
    let Ok([merged_spelled, base_spelled, theirs_spelled]) = <[Spelled<'_>; 3]>::try_from(spelled) else {
        return lines("a side's text doesn't scan".to_owned());
    };
    let (text, conflicts) = markers::mark(&merged_spelled, &base_spelled, &theirs_spelled, clashes);
    Merged { text, how: How::ByPath { kind, laid }, conflicts, notes }
}

/// A merge's conflict left in a document's text: its first marker's line, from 1, and the property path its
/// label names, if it names one (a line merge's and git's own markers don't).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unresolved {
    pub line: usize,
    pub at: Option<PropertyPath>,
}

/// The first unresolved merge conflict in `text`: a line that starts `<<<<<<< `, as this module's markers, git's
/// and `git merge-file`'s do.
#[must_use]
pub fn unresolved(text: &str) -> Option<Unresolved> {
    let (line, marker) = text.lines().enumerate().find(|(_, line)| line.starts_with("<<<<<<< "))?;
    let at = marker.strip_prefix("<<<<<<< ours: ").and_then(|label| label.trim_end().parse().ok());
    Some(Unresolved { line: line + 1, at })
}

/// The version a document's envelope says it's written at, from its text alone, which needn't read.
fn written_version(text: &str) -> Option<u32> {
    let tree = text::scan(text).ok()?;
    tree.field("type")?;
    tree.field("version")?.text(text).parse().ok()
}

/// A document's value, which every side has here: one that doesn't read merges by lines.
fn root_of<T>(document: &Document<T>) -> &dyn PartialReflect
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    document.value().expect("every side reads").as_partial_reflect()
}

/// Merges by lines, saying why. A clean merge that doesn't read as `kind`, where the base did, is a conflict on
/// the whole document, so git leaves it for a person. Where the base doesn't read either (a file the build can't
/// read, a newer version than it knows, a fixture written not to), the line merge's own result stands.
fn by_lines(base: &str, ours: &str, theirs: &str, why: String, kind: Option<Kind>) -> Merged {
    let merged = lines::merge(base, ours, theirs);
    let mut conflicts: Vec<Conflict> =
        merged.conflicts.iter().map(|line| Conflict { at: format!("line {line}"), why: None }).collect();
    let checked = kind.filter(|kind| kind.read(base).is_ok());
    if let (true, Some(kind)) = (conflicts.is_empty(), checked)
        && let Err(error) = kind.read(&merged.text)
    {
        conflicts.push(Conflict {
            at: String::new(),
            why: Some(format!("the line merge doesn't read as a {kind:?} document: {error}")),
        });
    }
    Merged { text: merged.text, how: How::ByLines { why }, conflicts, notes: Vec::new() }
}

/// A value with the text that spells it, and where.
pub(crate) struct Spelled<'a> {
    pub root: &'a dyn PartialReflect,
    pub tree: Node,
    pub text: &'a str,
}

impl<'a> Spelled<'a> {
    fn of(root: &'a dyn PartialReflect, text: &'a str) -> Option<Self> {
        Some(Self { root, tree: text::scan(text).ok()?, text })
    }

    fn body(&self) -> &Node {
        document::body(&self.tree)
    }

    /// The text of the value at `path`, if the file writes it.
    fn value_text(&self, path: &PropertyPath) -> Option<&'a str> {
        Some(self.node(path)?.text(self.text))
    }

    /// [`Self::value_text`], with the lines of comments right above it if it starts its line: a new entry's own
    /// words, which go in with it.
    fn with_comments_above(&self, path: &PropertyPath) -> Option<&'a str> {
        let span = &self.node(path)?.span;
        let mut start = span.start;
        if layout::starts_its_line(self.text, start) {
            let mut line = layout::line_start(self.text, start);
            while line > 0 {
                let above = layout::line_start(self.text, line - 1);
                if !self.text[above..line].trim_start().starts_with("//") {
                    break;
                }
                start = above + layout::indent_at(self.text, above).len();
                line = above;
            }
        }
        Some(&self.text[start..span.end])
    }

    /// The node of the value at `path`, if the file writes it.
    pub fn node(&self, path: &PropertyPath) -> Option<&Node> {
        let (node, taken) = walk::place(self.root, self.body(), self.text, path).ok()?;
        (taken == path.steps().len()).then_some(node)
    }

    /// Where the file writes the entry at `path`: a field with its name, a list's entry, the document's body.
    /// `None` if it doesn't write it.
    pub fn entry(&self, path: &PropertyPath) -> Option<std::ops::Range<usize>> {
        let (node, taken) = walk::place(self.root, self.body(), self.text, path).ok()?;
        if taken < path.steps().len() {
            return None;
        }
        let Some((holder, Step::Field(_))) = path.split_last() else { return Some(node.span.clone()) };
        let (holder, _) = walk::place(self.root, self.body(), self.text, &holder).ok()?;
        let field = holder.fields().iter().find(|field| field.value.span == node.span);
        Some(field.map_or(node.span.start, |field| field.start)..node.span.end)
    }
}

/// One side's change of the base at one path. Each holds its value as the side has it, `entry`: a value read back
/// from its text by reflection lacks the fields the text leaves to their defaults, and isn't the type it was.
enum Edit {
    /// The property at `path` now says `value` (canonical RON), as `spelled` in the side's text, if it writes it.
    Set { path: PropertyPath, value: String, entry: Box<dyn Reflect>, spelled: Option<String> },
    /// A new entry called `key` in the list at `list`, after the entries `after` names (nearest first) in the
    /// side's list. `known` is every key the base's or the side's list has, so an entry the other side added
    /// can be told from them.
    Insert {
        list: PropertyPath,
        key: String,
        value: String,
        entry: Box<dyn Reflect>,
        spelled: Option<String>,
        after: Vec<String>,
        known: Arc<BTreeSet<String>>,
    },
    /// The entry called `key` taken out of the list at `list`.
    Remove { list: PropertyPath, key: String },
}

impl Edit {
    /// The path of what it changes.
    fn touches(&self) -> PropertyPath {
        match self {
            Self::Set { path, .. } => path.clone(),
            Self::Insert { list, key, .. } | Self::Remove { list, key } => list.join(Step::Key(key.clone())),
        }
    }

    /// Whether it's the very edit `other` is: the same change to the same value.
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Set { path, value, .. }, Self::Set { path: other_path, value: other_value, .. }) => {
                path == other_path && value == other_value
            }
            (
                Self::Insert { list, key, value, .. },
                Self::Insert { list: other_list, key: other_key, value: other_value, .. },
            ) => list == other_list && key == other_key && value == other_value,
            (Self::Remove { list, key }, Self::Remove { list: other_list, key: other_key }) => {
                list == other_list && key == other_key
            }
            _ => false,
        }
    }
}

/// Whether `outer` is `inner` or holds it.
fn holds(outer: &PropertyPath, inner: &PropertyPath) -> bool {
    inner.steps().starts_with(outer.steps())
}

/// Lays `edit` on `document`, through its own edits, so its parser and the value check hold it, then spells it as
/// the side did.
fn lay<T>(document: &mut Document<T>, edit: &Edit) -> Result<crate::Outcome, Refused>
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    const LABEL: &str = "merge";
    let (outcome, at, spelled) = match edit {
        Edit::Set { path, entry, spelled, .. } => {
            (document.set(LABEL, path, entry.as_partial_reflect())?, path.clone(), spelled)
        }
        Edit::Insert { list, key, entry, spelled, after, known, .. } => {
            let at = place_for(document, list, after, known)?;
            let outcome = document.insert(LABEL, list, at, entry.as_partial_reflect())?;
            (outcome, list.join(Step::Key(key.clone())), spelled)
        }
        Edit::Remove { list, key } => return document.remove(LABEL, &list.join(Step::Key(key.clone()))),
    };
    if let Some(spelled) = spelled {
        respell(document, LABEL, &at, spelled);
    }
    Ok(outcome)
}

/// Writes the value at `path` as `spelled` says it, if the text then reads as the same document: what a person
/// wrote, comments and all, where the edit wrote it in the file's style.
fn respell<T>(document: &mut Document<T>, label: &str, path: &PropertyPath, spelled: &str)
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let Some(value) = document.value() else { return };
    let Some(written) = Spelled::of(value.as_partial_reflect(), document.text()) else { return };
    let Some(span) = written.node(path).map(|node| node.span.clone()) else { return };
    if &document.text()[span.clone()] == spelled {
        return;
    }
    let text = format!("{}{spelled}{}", &document.text()[..span.start], &document.text()[span.end..]);
    // Spelled another way that reads as something else, it keeps the file's own spelling.
    let _ = document.rewrite(label, text);
}

/// `value` as a value of its own type, whatever reflection holds it as.
fn concrete(value: &dyn PartialReflect, registry: &TypeRegistry) -> Result<Box<dyn Reflect>, ValueTextError> {
    if let Ok(clone) = value.reflect_clone() {
        return Ok(clone);
    }
    let info =
        value.get_represented_type_info().ok_or_else(|| ValueTextError("a value with no type".to_owned()))?;
    registry
        .get_type_data::<bevy_reflect::ReflectFromReflect>(info.type_id())
        .and_then(|from| from.from_reflect(value))
        .ok_or_else(|| ValueTextError(format!("`{}` can't be copied", info.type_path())))
}

/// Where a new entry goes in `document`'s list at `list`: after the nearest entry it followed that's still there,
/// and after any the other side added there, which keeps theirs after ours; first if none is.
fn place_for<T>(
    document: &Document<T>,
    list: &PropertyPath,
    after: &[String],
    known: &BTreeSet<String>,
) -> Result<usize, Refused>
where
    T: Reflect + FromReflect + Typed + GetTypeRegistration + Clone + PartialEq,
{
    let root = document.value().map(|value| value.as_partial_reflect());
    let root = root.ok_or_else(|| Refused::ReadOnly("it doesn't read".to_owned()))?;
    let ReflectRef::List(items) = walk::get(root, list)?.reflect_ref() else {
        return Err(Refused::WrongType { path: list.to_string(), why: "it isn't a list".to_owned() });
    };
    let keys: Vec<Option<&str>> = items.iter().map(walk::name_of).collect();
    let mut at = after
        .iter()
        .find_map(|key| keys.iter().position(|named| *named == Some(key.as_str())))
        .map_or(0, |at| at + 1);
    while keys.get(at).is_some_and(|key| key.is_some_and(|key| !known.contains(key))) {
        at += 1;
    }
    Ok(at)
}

/// The edits that take `base` to the side `side` spells.
fn edits(
    base: &dyn PartialReflect,
    side: &Spelled<'_>,
    registry: &TypeRegistry,
) -> Result<Vec<Edit>, ValueTextError> {
    let mut found = Vec::new();
    Differ { side, registry }.descend(base, side.root, &PropertyPath::root(), &mut found)?;
    Ok(found)
}

struct Differ<'s, 'a> {
    side: &'s Spelled<'a>,
    registry: &'s TypeRegistry,
}

impl Differ<'_, '_> {
    fn descend(
        &self,
        base: &dyn PartialReflect,
        side: &dyn PartialReflect,
        path: &PropertyPath,
        found: &mut Vec<Edit>,
    ) -> Result<(), ValueTextError> {
        let (base, side) = (walk::through(base), walk::through(side));
        if base.reflect_partial_eq(side) == Some(true) {
            return Ok(());
        }
        match (walk::variant(base), walk::variant(side)) {
            (Some(before), Some(after)) if before != after => return self.set(base, side, path, found),
            (Some(_), Some(_)) if walk::is_option(side) => {
                return match (walk::child(base, &Step::Some), walk::child(side, &Step::Some)) {
                    (Some(base), Some(side)) => self.descend(base, side, &path.join(Step::Some), found),
                    _ => self.set(base, side, path, found),
                };
            }
            _ => {}
        }
        if let (ReflectRef::List(before), ReflectRef::List(after)) = (base.reflect_ref(), side.reflect_ref())
        {
            return match (keys_of(before), keys_of(after)) {
                (Some(before_keys), Some(after_keys)) => {
                    self.list((base, before, &before_keys), (side, after, &after_keys), path, found)
                }
                // Entries with no names, or names they share, are one value: a place doesn't say which entry.
                _ => self.set(base, side, path, found),
            };
        }
        match (walk::shape(base), walk::shape(side)) {
            (Shape::Fields(before), Shape::Fields(after)) if before.len() == after.len() => {
                for (before, after) in before.iter().zip(&after) {
                    self.descend(
                        before.value,
                        after.value,
                        &path.join(Step::Field(after.name.to_string())),
                        found,
                    )?;
                }
                Ok(())
            }
            // A tuple's or an array's places are what its items mean (a position's `x`), never a list's.
            (Shape::Items(before), Shape::Items(after)) if before.len() == after.len() => {
                for (at, (before, after)) in before.into_iter().zip(after).enumerate() {
                    self.descend(before, after, &path.join(Step::Index(at)), found)?;
                }
                Ok(())
            }
            _ => self.set(base, side, path, found),
        }
    }

    /// A list of named entries: those gone, those changed, those new, by name.
    fn list(
        &self,
        (base, before, before_keys): (&dyn PartialReflect, &dyn bevy_reflect::list::List, &[&str]),
        (side, after, after_keys): (&dyn PartialReflect, &dyn bevy_reflect::list::List, &[&str]),
        path: &PropertyPath,
        found: &mut Vec<Edit>,
    ) -> Result<(), ValueTextError> {
        let base_at: BTreeMap<&str, usize> =
            before_keys.iter().enumerate().map(|(at, key)| (*key, at)).collect();
        let side_has: BTreeSet<&str> = after_keys.iter().copied().collect();
        let kept_before: Vec<&str> =
            before_keys.iter().copied().filter(|key| side_has.contains(key)).collect();
        let kept_after: Vec<&str> =
            after_keys.iter().copied().filter(|key| base_at.contains_key(key)).collect();
        if kept_before != kept_after {
            // Reordered: the order is the list's, so it's one value.
            return self.set(base, side, path, found);
        }
        for key in before_keys.iter().filter(|key| !side_has.contains(*key)) {
            found.push(Edit::Remove { list: path.clone(), key: (*key).to_owned() });
        }
        let known: Arc<BTreeSet<String>> =
            Arc::new(before_keys.iter().chain(after_keys).map(|key| (*key).to_owned()).collect());
        for (at, key) in after_keys.iter().enumerate() {
            let entry = path.join(Step::Key((*key).to_owned()));
            let side = after.get(at).expect("a key a place");
            match base_at.get(key) {
                Some(&was) => self.descend(before.get(was).expect("a key a place"), side, &entry, found)?,
                None => found.push(Edit::Insert {
                    list: path.clone(),
                    key: (*key).to_owned(),
                    value: write::canonical(side, self.registry)?,
                    entry: concrete(side, self.registry)?,
                    spelled: self.side.with_comments_above(&entry).map(ToOwned::to_owned),
                    after: after_keys[..at].iter().rev().map(|key| (*key).to_owned()).collect(),
                    known: Arc::clone(&known),
                }),
            }
        }
        Ok(())
    }

    fn set(
        &self,
        base: &dyn PartialReflect,
        side: &dyn PartialReflect,
        path: &PropertyPath,
        found: &mut Vec<Edit>,
    ) -> Result<(), ValueTextError> {
        let value = write::canonical(side, self.registry)?;
        if value != write::canonical(base, self.registry)? {
            found.push(Edit::Set {
                path: path.clone(),
                value,
                entry: concrete(side, self.registry)?,
                spelled: self.side.value_text(path).map(ToOwned::to_owned),
            });
        }
        Ok(())
    }
}

/// Each entry's key, if every entry has one no other shares: the list's entries go by name.
fn keys_of(list: &dyn bevy_reflect::list::List) -> Option<Vec<&str>> {
    let keys: Vec<&str> = list.iter().map(walk::name_of).collect::<Option<_>>()?;
    let unique: BTreeSet<&str> = keys.iter().copied().collect();
    (unique.len() == keys.len()).then_some(keys)
}
