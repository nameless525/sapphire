//! `RonPatch`: from a value before and a value after to the fewest bytes that change the text.
//!
//! It walks the two values and the text together and writes only what differs: a number's bytes, a field
//! the file left out, an entry. A nested edit stays nested, so a comment inside a struct survives a change
//! beside it. What can't be walked is replaced whole and written new: another variant of an enum, or a list
//! of another length set in one go. [`crate::document`] holds whatever comes of this to the document's own
//! parser, so a mistake here refuses an edit and can't corrupt a file.

use bevy_reflect::{PartialReflect, TypeRegistry};

use crate::layout::{self, Splice};
use crate::path::Step;
use crate::text::{Kind, Node};
use crate::walk::{self, FieldOf, Shape};
use crate::write::{self, Style, ValueTextError};

/// Plans the splices for one edit of one document.
pub struct RonPatch<'a> {
    source: &'a str,
    registry: &'a TypeRegistry,
    /// Whether this file leaves `Some(..)` out, as most of ours do. A file that says it anywhere gets it
    /// said in what's new.
    implicit_some: bool,
    splices: Vec<Splice>,
}

impl<'a> RonPatch<'a> {
    #[must_use]
    pub fn new(source: &'a str, registry: &'a TypeRegistry) -> Self {
        Self { source, registry, implicit_some: !crate::text::has_explicit_some(source), splices: Vec::new() }
    }

    /// What it's planned so far.
    #[must_use]
    pub fn into_splices(self) -> Vec<Splice> {
        self.splices
    }

    /// Plans taking `node`, whose value was `old`, to `new`.
    pub fn diff(
        &mut self,
        node: &Node,
        old: &dyn PartialReflect,
        new: &dyn PartialReflect,
    ) -> Result<(), ValueTextError> {
        let (old, new) = (walk::through(old), walk::through(new));
        if write::canonical(old, self.registry)? == write::canonical(new, self.registry)? {
            return Ok(());
        }
        self.diff_changed(node, old, new)
    }

    /// Descends a pair already known to differ canonically.
    fn diff_changed(
        &mut self,
        node: &Node,
        old: &dyn PartialReflect,
        new: &dyn PartialReflect,
    ) -> Result<(), ValueTextError> {
        if let (Some(before), Some(after)) = (walk::variant(old), walk::variant(new)) {
            if before != after {
                return self.replace(node, new);
            }
            if walk::is_option(new) {
                let inner = walk::inside_some(node, self.source);
                return match (walk::child(old, &Step::Some), walk::child(new, &Step::Some)) {
                    (Some(old), Some(new)) => self.diff(inner, old, new),
                    _ => self.replace(node, new),
                };
            }
        }
        match (walk::shape(old), walk::shape(new)) {
            (Shape::Fields(before), Shape::Fields(after)) if before.len() == after.len() => {
                self.fields(node, &before, &after, new)
            }
            (Shape::Items(before), Shape::Items(after))
                if before.len() == after.len() && node.items().len() == after.len() =>
            {
                for (at, (old, new)) in before.into_iter().zip(after).enumerate() {
                    self.diff(&node.items()[at], old, new)?;
                }
                Ok(())
            }
            _ => self.replace(node, new),
        }
    }

    fn fields(
        &mut self,
        node: &Node,
        before: &[FieldOf<'_>],
        after: &[FieldOf<'_>],
        new: &dyn PartialReflect,
    ) -> Result<(), ValueTextError> {
        match &node.kind {
            Kind::Struct(_) => {}
            // `()`: every field left out, so those that aren't now go in.
            Kind::Seq(items) if items.is_empty() => {}
            // A struct that serde writes as a tuple.
            Kind::Seq(items) if items.len() == after.len() => {
                for (at, (old, new)) in before.iter().zip(after).enumerate() {
                    self.diff(&items[at], old.value, new.value)?;
                }
                return Ok(());
            }
            _ => return self.replace(node, new),
        }
        for (at, (old, field)) in before.iter().zip(after).enumerate() {
            if write::canonical(old.value, self.registry)? == write::canonical(field.value, self.registry)? {
                continue;
            }
            match node.field(&field.written) {
                Some(text) if walk::is_none(field.value) => self.remove_field(node, text, &field.written),
                Some(text) => {
                    self.diff_changed(text, walk::through(old.value), walk::through(field.value))?
                }
                None => self.insert_field(node, &after[..at], field)?,
            }
        }
        Ok(())
    }

    /// Writes `new` over all of `node`. What it held goes, comments too.
    pub fn replace(&mut self, node: &Node, new: &dyn PartialReflect) -> Result<(), ValueTextError> {
        let text = write::styled(new, self.registry, self.style_at(node.span.start))?;
        self.splices.push(Splice { range: node.span.clone(), text });
        Ok(())
    }

    fn style_at(&self, at: usize) -> Style<'a> {
        Style {
            implicit_some: self.implicit_some,
            newline: layout::newline_of(self.source),
            indent: layout::indent_at(self.source, at),
        }
    }

    /// A field the file left out, after the nearest field declared before it that the file writes. Comes
    /// first if none is: the comments above what's now second stay with it.
    fn insert_field(
        &mut self,
        node: &Node,
        declared_before: &[FieldOf<'_>],
        field: &FieldOf<'_>,
    ) -> Result<(), ValueTextError> {
        let written = node.fields();
        let anchor = declared_before
            .iter()
            .rev()
            .find_map(|earlier| written.iter().find(|field| field.name == earlier.written));
        let reference = anchor.or(written.first()).map_or(node.span.start, |field| field.start);
        let style = self.style_at(reference);
        let value = write::styled(field.value, self.registry, style)?;
        let entry = format!("{}: {value}", field.written);
        let inside = layout::inside(self.source, node)
            .ok_or_else(|| ValueTextError(format!("`{}` has no brackets to add to", field.written)))?;
        match (anchor, written.first()) {
            (Some(anchor), _) => {
                let splices = layout::insert_after(self.source, anchor.start, anchor.value.span.end, &entry);
                for splice in splices {
                    // Two new fields after one that lacks its comma want it once.
                    if !self.splices.contains(&splice) {
                        self.splices.push(splice);
                    }
                }
            }
            (None, Some(first)) => {
                self.splices.push(layout::insert_first(self.source, &inside, first.start, &entry));
            }
            (None, None) => self.splices.push(layout::into_empty(self.source, inside, &entry)),
        }
        Ok(())
    }

    fn remove_field(&mut self, node: &Node, text: &Node, written: &str) {
        let fields = node.fields();
        let at = fields.iter().position(|field| field.name == written);
        let previous = at.and_then(|at| at.checked_sub(1)).map(|at| fields[at].value.span.end);
        let start = at.map_or(text.span.start, |at| fields[at].start);
        self.splices.push(layout::removal(self.source, start, text.span.end, previous));
    }

    /// A new entry in a written list, between the entries around `at`.
    pub fn insert_item(
        &mut self,
        list: &Node,
        at: usize,
        value: &dyn PartialReflect,
    ) -> Result<(), ValueTextError> {
        let items = list.items();
        let reference = items.get(at.saturating_sub(1)).map_or(list.span.start, |item| item.span.start);
        let entry = write::styled(value, self.registry, self.style_at(reference))?;
        self.insert_text(list, at, &entry)
    }

    /// Adds an entry to a written list as `entry` spells it, on the list's own layout.
    pub fn insert_text(&mut self, list: &Node, at: usize, entry: &str) -> Result<(), ValueTextError> {
        let items = list.items();
        let inside = layout::inside(self.source, list)
            .ok_or_else(|| ValueTextError("a list with no brackets".to_owned()))?;
        match at.checked_sub(1).and_then(|before| items.get(before)) {
            Some(before) => self.splices.extend(layout::insert_after(
                self.source,
                before.span.start,
                before.span.end,
                entry,
            )),
            None => match items.first() {
                Some(first) => {
                    self.splices.push(layout::insert_first(self.source, &inside, first.span.start, entry));
                }
                None => self.splices.push(layout::into_empty(self.source, inside, entry)),
            },
        }
        Ok(())
    }

    /// A list the file leaves out, holding one entry, among the fields of the struct `node` as a person would
    /// add it: on lines of its own if its neighbour has one. `parent` is that struct's new value, `name` the
    /// list's field, and `entry` the entry's spelling, if it has one yet.
    pub fn new_list(
        &mut self,
        node: &Node,
        parent: &dyn PartialReflect,
        name: &str,
        value: &dyn PartialReflect,
        entry: Option<&str>,
    ) -> Result<(), ValueTextError> {
        let Shape::Fields(fields) = walk::shape(walk::through(parent)) else {
            return Err(ValueTextError(format!("`{name}` isn't a field of a struct")));
        };
        let at = fields
            .iter()
            .position(|field| field.name == name)
            .ok_or_else(|| ValueTextError(format!("`{name}` isn't a field of this struct")))?;
        let written = node.fields();
        let anchor = fields[..at]
            .iter()
            .rev()
            .find_map(|earlier| written.iter().find(|field| field.name == earlier.written))
            .or(written.last());
        let key = &fields[at].written;
        let Some(anchor) = anchor else {
            let entry = match entry {
                Some(entry) => entry.to_owned(),
                None => write::styled(value, self.registry, self.style_at(node.span.start))?,
            };
            let inside = layout::inside(self.source, node)
                .ok_or_else(|| ValueTextError(format!("`{name}` has no brackets to add to")))?;
            self.splices.push(layout::into_empty(self.source, inside, &format!("{key}: [{entry}]")));
            return Ok(());
        };
        let own_lines = layout::starts_its_line(self.source, anchor.start);
        let indent = layout::indent_at(self.source, anchor.start);
        let inner = format!("{indent}    ");
        let style = Style { indent: &inner, ..self.style_at(anchor.start) };
        let entry = match entry {
            Some(entry) => entry.to_owned(),
            None => write::styled(value, self.registry, style)?,
        };
        let newline = layout::newline_of(self.source);
        let text = if own_lines {
            format!("{key}: [{newline}{inner}{entry},{newline}{indent}]")
        } else {
            format!("{key}: [{entry}]")
        };
        self.splices.extend(layout::insert_after(self.source, anchor.start, anchor.value.span.end, &text));
        Ok(())
    }

    /// Takes the entry at `at` out of a written list.
    pub fn remove_item(&mut self, list: &Node, at: usize) {
        let items = list.items();
        let previous = at.checked_sub(1).map(|before| items[before].span.end);
        self.splices.push(layout::removal(self.source, items[at].span.start, items[at].span.end, previous));
    }

    /// Moves the entry at `from` to `to` in a written list.
    pub fn reorder_item(&mut self, list: &Node, from: usize, to: usize) {
        self.splices.extend(layout::reorder(self.source, list.items(), from, to));
    }
}
