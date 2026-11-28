//! Conflict markers at a value: where two sides clash, the merged text holds each side's spelling of the entry or
//! field they clash on, as git's diff3 style marks a conflict, so a person keeps one by deleting the rest.
//!
//! ```text
//!         startup: 4,
//! <<<<<<< ours: moves["light_1"].active
//!         active: 3,
//! ||||||| base
//!         active: 2,
//! =======
//!         active: 5,
//! >>>>>>> theirs
//!         recovery: 9,
//! ```
//!
//! A marker spans the whole lines of what it marks where that stands on lines of its own, so each section is a
//! side's lines as it wrote them, comments too; otherwise it's set on lines of its own inside the line. A side
//! that doesn't have the entry has an empty section. What ours took out has no place in our text, so its marker
//! goes where the entry stood, after the nearest entry before it that's still there, or, failing that, at what
//! holds it. The `<<<<<<<` lines aren't RON, so the file doesn't read until each is resolved.

use std::ops::Range;

use bevy_reflect::ReflectRef;

use super::{Conflict, Spelled, holds};
use crate::layout;
use crate::path::{PropertyPath, Step};
use crate::walk;

/// Where two sides clash, and why if it's more than both changing it.
#[derive(Clone, Debug)]
pub(crate) struct Clash {
    pub at: PropertyPath,
    pub why: Option<String>,
}

/// The bytes of the merged text a marker replaces.
#[derive(Clone, Debug)]
struct Region {
    range: Range<usize>,
    /// Whether it's whole lines.
    lines: bool,
    /// Whether a comma follows what it marks, inside it: each side's section ends in one too.
    comma: bool,
}

/// `merged`'s text with a marker at each clash, and the conflicts they are.
pub(crate) fn mark(
    merged: &Spelled<'_>,
    base: &Spelled<'_>,
    theirs: &Spelled<'_>,
    clashes: Vec<Clash>,
) -> (String, Vec<Conflict>) {
    let mut clashes = outermost(clashes);
    let placed = loop {
        let mut placed: Vec<(Clash, Region)> = clashes
            .iter()
            .map(|clash| {
                let (at, region) = locate(merged, base, theirs, &clash.at);
                (Clash { at, why: clash.why.clone() }, region)
            })
            .collect();
        placed.sort_by_key(|(_, region)| region.range.start);
        let overlap = placed.windows(2).position(|pair| pair[1].1.range.start < pair[0].1.range.end);
        let Some(first) = overlap else { break placed };
        // Two markers in one place are one, at what holds both.
        let (a, b) = (&placed[first].0, &placed[first + 1].0);
        let shared = a.at.steps().iter().zip(b.at.steps()).take_while(|(a, b)| a == b).count();
        let joined = Clash { at: a.at.prefix(shared), why: a.why.clone().or_else(|| b.why.clone()) };
        placed.drain(first..first + 2);
        clashes = outermost(placed.into_iter().map(|(clash, _)| clash).chain([joined]).collect());
    };
    let newline = layout::newline_of(merged.text);
    let mut splices = Vec::with_capacity(placed.len());
    let mut conflicts = Vec::with_capacity(placed.len());
    for (clash, region) in placed {
        let indent = layout::indent_at(merged.text, region.range.start);
        let ours = &merged.text[region.range.clone()];
        let [base, theirs] = [base, theirs].map(|side| section(side, &clash.at, &region, indent, newline));
        let label =
            if clash.at.steps().is_empty() { "the document".to_owned() } else { clash.at.to_string() };
        let why = clash.why.as_deref().map_or(String::new(), |why| format!(" ({})", one_line(why)));
        let text = if region.lines {
            let ours = ending(ours, newline);
            format!(
                "<<<<<<< ours: {label}{newline}{ours}||||||| base{newline}{base}======={newline}{theirs}>>>>>>> \
                 theirs{why}{newline}"
            )
        } else {
            let start = region.range.start;
            let lead = if start == 0 || merged.text[..start].ends_with('\n') { "" } else { newline };
            format!(
                "{lead}<<<<<<< ours: {label}{newline}{ours}{newline}||||||| base{newline}{base}{newline}======={newline}\
                 {theirs}{newline}>>>>>>> theirs{why}{newline}"
            )
        };
        splices.push(layout::Splice { range: region.range, text });
        conflicts.push(Conflict { at: clash.at.to_string(), why: clash.why });
    }
    let text = layout::apply(merged.text, &splices).expect("markers apart, on character edges");
    (text, conflicts)
}

/// The clashes with none inside another, each at an entry or field rather than inside an `Option`.
fn outermost(clashes: Vec<Clash>) -> Vec<Clash> {
    let mut clashes: Vec<Clash> =
        clashes.into_iter().map(|clash| Clash { at: past_options(clash.at), why: clash.why }).collect();
    clashes.sort_by_key(|clash| clash.at.steps().len());
    let mut kept: Vec<Clash> = Vec::with_capacity(clashes.len());
    for clash in clashes {
        match kept.iter_mut().find(|outer| holds(&outer.at, &clash.at)) {
            Some(outer) => outer.why = outer.why.take().or(clash.why),
            None => kept.push(clash),
        }
    }
    kept
}

/// `path` without the `?` steps it ends in: an `Option`'s field is marked whole.
fn past_options(mut path: PropertyPath) -> PropertyPath {
    while let Some((holder, Step::Some)) = path.split_last() {
        path = holder;
    }
    path
}

/// Where the marker for a clash at `path` goes in `merged`, and the path it ends up marking: `path`, or what
/// holds it if the merged text writes neither it nor a place for it.
fn locate(
    merged: &Spelled<'_>,
    base: &Spelled<'_>,
    theirs: &Spelled<'_>,
    path: &PropertyPath,
) -> (PropertyPath, Region) {
    let mut path = path.clone();
    loop {
        if let Some(entry) = merged.entry(&path) {
            return (path, widen(merged.text, &entry));
        }
        if let Some(at) = gone_at(merged, base, theirs, &path) {
            return (path, Region { range: at..at, lines: true, comma: true });
        }
        // The document's body is always written, so this ends.
        let (holder, _) = path.split_last().expect("the body is written");
        path = past_options(holder);
    }
}

/// Where an entry the merged text doesn't have would stand: the start of the line after the nearest entry
/// before it, in their list or the base's, that's still there, or of the list's first line. Only for a list
/// whose entries stand on lines of their own, each with its comma.
fn gone_at(
    merged: &Spelled<'_>,
    base: &Spelled<'_>,
    theirs: &Spelled<'_>,
    path: &PropertyPath,
) -> Option<usize> {
    let (list, Step::Key(key)) = path.split_last()? else { return None };
    let written = merged.node(&list)?;
    for side in [theirs, base] {
        let Ok(value) = walk::get(side.root, &list) else { continue };
        let ReflectRef::List(items) = value.reflect_ref() else { continue };
        let keys: Vec<Option<&str>> = items.iter().map(walk::name_of).collect();
        let Some(at) = keys.iter().position(|named| *named == Some(key.as_str())) else { continue };
        for before in keys[..at].iter().rev().flatten() {
            if let Some(entry) = merged.entry(&list.join(Step::Key((*before).to_owned()))) {
                let region = widen(merged.text, &entry);
                return (region.lines && region.comma).then_some(region.range.end);
            }
        }
    }
    let first = written.items().first()?;
    layout::starts_its_line(merged.text, first.span.start)
        .then(|| layout::line_start(merged.text, first.span.start))
}

/// The marker's bytes for an entry at `entry`: its whole lines if it stands on lines of its own (its comma and
/// a comment after it on its last line included), else it and its comma.
fn widen(text: &str, entry: &Range<usize>) -> Region {
    let comma = layout::comma_after(text, entry.end);
    let after = comma.unwrap_or(entry.end);
    let line_end = text[after..].find('\n').map_or(text.len(), |newline| after + newline + 1);
    let rest = text[after..line_end].trim();
    if layout::starts_its_line(text, entry.start) && (rest.is_empty() || rest.starts_with("//")) {
        Region { range: layout::line_start(text, entry.start)..line_end, lines: true, comma: comma.is_some() }
    } else {
        Region { range: entry.start..after, lines: false, comma: comma.is_some() }
    }
}

/// A side's spelling of the entry at `path`, laid out as `region` is: empty if the side hasn't one.
fn section(side: &Spelled<'_>, path: &PropertyPath, region: &Region, indent: &str, newline: &str) -> String {
    let Some(entry) = side.entry(path) else { return String::new() };
    let text = side.text;
    if !region.lines {
        let comma = if region.comma { "," } else { "" };
        return format!("{}{comma}", &text[entry]);
    }
    let own = widen(text, &entry);
    let mut section = String::new();
    if own.lines {
        section.push_str(&text[own.range.start..entry.end]);
    } else {
        section.push_str(indent);
        section.push_str(&text[entry.clone()]);
    }
    if region.comma && !own.comma {
        section.push(',');
    }
    if own.lines {
        section.push_str(&text[entry.end..own.range.end]);
    } else if own.comma {
        section.push(',');
    }
    ending(&section, newline)
}

/// `text`, ending on a newline unless it's empty.
fn ending(text: &str, newline: &str) -> String {
    if text.is_empty() || text.ends_with('\n') { text.to_owned() } else { format!("{text}{newline}") }
}

/// `text` on one line, for a marker's label.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
