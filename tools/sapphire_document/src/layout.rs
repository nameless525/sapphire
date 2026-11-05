//! Splices in a file's own layout: what puts a new field or entry where a person would have, what takes one
//! out, and nothing else in the file moves. A new line takes the indent of the line above it, a list that
//! ends in a comma goes on ending in one, and a comment stays where it is, since a person wrote it.

use std::ops::Range;

use crate::text::{self, Node};

/// Bytes to replace, and with what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    pub range: Range<usize>,
    pub text: String,
}

/// `text` with `splices` applied, or `None` if ranges overlap or aren't valid UTF-8 byte ranges. Those at one place go in the order given.
#[must_use]
pub fn apply(text: &str, splices: &[Splice]) -> Option<String> {
    let mut sorted: Vec<&Splice> = splices.iter().collect();
    sorted.sort_by_key(|splice| splice.range.start);
    let mut out = String::with_capacity(text.len() + 64);
    let mut at = 0;
    for splice in sorted {
        if splice.range.start < at || text.get(splice.range.clone()).is_none() {
            return None;
        }
        out.push_str(&text[at..splice.range.start]);
        out.push_str(&splice.text);
        at = splice.range.end;
    }
    out.push_str(&text[at..]);
    Some(out)
}

/// The line's start before `at`.
#[must_use]
pub fn line_start(source: &str, at: usize) -> usize {
    source[..at].rfind('\n').map_or(0, |newline| newline + 1)
}

/// Whether only spaces come before `at` on its line.
#[must_use]
pub fn starts_its_line(source: &str, at: usize) -> bool {
    source[line_start(source, at)..at].chars().all(|c| c == ' ' || c == '\t')
}

/// The file's newline, as its first line ends.
#[must_use]
pub fn newline_of(source: &str) -> &'static str {
    match source.find('\n') {
        Some(at) if at > 0 && source.as_bytes()[at - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// The spaces a line starts with.
#[must_use]
pub fn indent_at(source: &str, at: usize) -> &str {
    let start = line_start(source, at);
    let rest = &source[start..];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

/// Past a comma straight after `at`, over spaces and comments. `None` if there's no comma.
#[must_use]
pub fn comma_after(source: &str, at: usize) -> Option<usize> {
    let next = text::skip_trivia(source, at);
    (source.as_bytes().get(next) == Some(&b',')).then_some(next + 1)
}

/// Where a new line after `at` goes: the end of its line, if only spaces or a comment are left on it.
fn end_of_line_after(source: &str, at: usize) -> Option<usize> {
    let end = source[at..].find('\n').map_or(source.len(), |newline| at + newline);
    let line = source[at..end].trim_end_matches('\r').trim();
    let carriage = usize::from(source[..end].ends_with('\r'));
    (line.is_empty() || line.starts_with("//")).then_some(end - carriage)
}

/// The bytes between a compound value's brackets, past the name it may start with. `None` for an atom.
#[must_use]
pub fn inside(source: &str, node: &Node) -> Option<Range<usize>> {
    let open = source[node.span.clone()].find(['(', '[', '{'])? + node.span.start;
    Some(open + 1..node.span.end - 1)
}

/// `entry` after a value that runs `start..end`, on a line of its own if the value has one.
#[must_use]
pub fn insert_after(source: &str, start: usize, end: usize, entry: &str) -> Vec<Splice> {
    let comma = comma_after(source, end);
    let after = comma.unwrap_or(end);
    let mut splices = Vec::with_capacity(2);
    if starts_its_line(source, start)
        && let Some(line_end) = end_of_line_after(source, after)
    {
        if comma.is_none() {
            splices.push(Splice { range: end..end, text: ",".to_owned() });
        }
        let (newline, indent) = (newline_of(source), indent_at(source, start));
        splices.push(Splice { range: line_end..line_end, text: format!("{newline}{indent}{entry},") });
    } else if comma.is_some() {
        splices.push(Splice { range: after..after, text: format!(" {entry},") });
    } else {
        splices.push(Splice { range: after..after, text: format!(", {entry}") });
    }
    splices
}

/// `entry` before everything else in a compound value, whose first item starts at `first`.
#[must_use]
pub fn insert_first(source: &str, inside: &Range<usize>, first: usize, entry: &str) -> Splice {
    if starts_its_line(source, first)
        && let Some(line_end) = end_of_line_after(source, inside.start)
    {
        let (newline, indent) = (newline_of(source), indent_at(source, first));
        return Splice { range: line_end..line_end, text: format!("{newline}{indent}{entry},") };
    }
    Splice { range: inside.start..inside.start, text: format!("{entry}, ") }
}

/// `entry` in a compound value with nothing in it.
#[must_use]
pub fn into_empty(source: &str, inside: Range<usize>, entry: &str) -> Splice {
    if source[inside.clone()].trim().is_empty() {
        Splice { range: inside, text: entry.to_owned() }
    } else {
        // A comment in it: it stays.
        let at = inside.start;
        Splice { range: at..at, text: format!("{entry},") }
    }
}

/// What removes the value at `start..end` from what holds it, and what only it needs: its comma, and its
/// line if it was all that's on it. `previous_end` is where the item before it ends. A comment beside or
/// above it stays, since a person wrote that.
#[must_use]
pub fn removal(source: &str, start: usize, end: usize, previous_end: Option<usize>) -> Splice {
    let comma = comma_after(source, end);
    let after = comma.unwrap_or(end);
    // A comma may follow comments. They belong to the surrounding layout, not the removed
    // value, and must survive even when one contiguous splice spans them.
    let trailing = comma.map_or("", |after| &source[end..after - 1]);
    let keep_trailing = !text::comments(trailing).is_empty();
    if starts_its_line(source, start) && !keep_trailing {
        let rest = source[after..].find('\n').map_or(source.len(), |newline| after + newline);
        if source[after..rest].trim().is_empty() {
            let line_end = (rest + 1).min(source.len());
            return Splice { range: line_start(source, start)..line_end, text: String::new() };
        }
    }
    if comma.is_some() {
        let spaces = source[after..].len() - source[after..].trim_start_matches(' ').len();
        return Splice {
            range: start..after + spaces,
            text: if keep_trailing { trailing.to_owned() } else { String::new() },
        };
    }
    // The last in its list, with no comma of its own: the one before it goes too.
    match previous_end {
        Some(previous_end) => {
            let between = &source[previous_end..start];
            let kept = if text::comments(between).is_empty() {
                String::new()
            } else if let Some(comma) = comma_after(source, previous_end) {
                format!("{}{}", &source[previous_end..comma - 1], &source[comma..start])
            } else {
                between.to_owned()
            };
            Splice { range: previous_end..end, text: kept }
        }
        None => Splice { range: start..end, text: String::new() },
    }
}

/// A list's entry and what goes with it when it changes place.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Slot {
    /// All that's replaced: from the comments above it to the end of its line, or only its value if it
    /// shares a line.
    range: Range<usize>,
    /// The comment lines above it, and its indent.
    lead: String,
    value: String,
    /// A comment beside it, with the space before it.
    beside: String,
    has_comma: bool,
}

/// Where the comment lines directly above `at` start: `at`'s line start, if there are none. A blank line
/// ends them, because a comment past one speaks for what follows it, not for this entry alone.
fn comments_above(source: &str, at: usize) -> usize {
    let mut start = line_start(source, at);
    while start > 0 {
        let previous = line_start(source, start - 1);
        if !source[previous..start].trim_start().starts_with("//") {
            break;
        }
        start = previous;
    }
    start
}

/// The slots of `items`, or `None` if any shares its line: those trade only their values.
fn line_slots(source: &str, items: &[Node]) -> Option<Vec<Slot>> {
    items
        .iter()
        .map(|item| {
            if !starts_its_line(source, item.span.start) {
                return None;
            }
            let comma = comma_after(source, item.span.end);
            let after = comma.unwrap_or(item.span.end);
            let line_end = end_of_line_after(source, after)?;
            let start = comments_above(source, item.span.start);
            Some(Slot {
                range: start..line_end,
                lead: source[start..item.span.start].to_owned(),
                value: source[item.span.clone()].to_owned(),
                beside: source[after..line_end].to_owned(),
                has_comma: comma.is_some(),
            })
        })
        .collect()
}

/// What moves the entry at `from` to `to`, with the entries between shifting over. An entry takes the
/// comments above and beside it along, as long as each is on a line of its own; a place keeps whether it
/// ends in a comma, so a list that leaves off its last one goes on leaving it off.
#[must_use]
pub fn reorder(source: &str, items: &[Node], from: usize, to: usize) -> Vec<Splice> {
    let (low, high) = (from.min(to), from.max(to));
    let order: Vec<usize> = {
        let mut order: Vec<usize> = (low..=high).collect();
        let moved = order.remove(from - low);
        order.insert(to - low, moved);
        order
    };
    let window = &items[low..=high];
    if let Some(slots) = line_slots(source, window) {
        return slots
            .iter()
            .zip(&order)
            .map(|(place, source_index)| {
                let entry = &slots[source_index - low];
                let comma = if place.has_comma { "," } else { "" };
                Splice {
                    range: place.range.clone(),
                    text: format!("{}{}{comma}{}", entry.lead, entry.value, entry.beside),
                }
            })
            .collect();
    }
    window
        .iter()
        .zip(&order)
        .map(|(place, source_index)| Splice {
            range: place.span.clone(),
            text: source[items[*source_index].span.clone()].to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::scan;

    fn spliced(source: &str, splices: &[Splice]) -> String {
        apply(source, splices).expect("no overlap")
    }

    #[test]
    fn overlapping_splices_are_refused() {
        let splices =
            [Splice { range: 0..4, text: String::new() }, Splice { range: 2..3, text: String::new() }];
        assert_eq!(apply("abcdef", &splices), None);
    }

    #[test]
    fn a_new_entry_follows_the_layout_it_lands_in() {
        for (source, want) in [
            ("[\n    1,\n    2, // two\n]", "[\n    1,\n    2, // two\n    3,\n]"),
            ("[\n    1,\n    2 // two\n]", "[\n    1,\n    2, // two\n    3,\n]"),
            ("[1, 2]", "[1, 2, 3]"),
            ("[1, 2,]", "[1, 2, 3,]"),
        ] {
            let list = scan(source).expect("a list");
            let last = list.items().last().expect("an item");
            let got = spliced(source, &insert_after(source, last.span.start, last.span.end, "3"));
            assert_eq!(got, want);
        }
    }

    #[test]
    fn an_entry_goes_in_first_past_a_comment_on_the_open_line() {
        let source = "[ // the list\n    2,\n]";
        let list = scan(source).expect("a list");
        let at = inside(source, &list).expect("brackets");
        let got = spliced(source, &[insert_first(source, &at, list.items()[0].span.start, "1")]);
        assert_eq!(got, "[ // the list\n    1,\n    2,\n]");
        let inline = "[2, 3]";
        let list = scan(inline).expect("a list");
        let at = inside(inline, &list).expect("brackets");
        assert_eq!(
            spliced(inline, &[insert_first(inline, &at, list.items()[0].span.start, "1")]),
            "[1, 2, 3]"
        );
    }

    #[test]
    fn taking_out_an_entry_leaves_the_comments() {
        for (source, at, want) in [
            ("[\n    // above\n    1,\n    2,\n]", 0, "[\n    // above\n    2,\n]"),
            ("[\n    1, // beside\n    2,\n]", 0, "[\n    // beside\n    2,\n]"),
            ("[1, 2, 3]", 1, "[1, 3]"),
            ("[1, 2, 3]", 2, "[1, 2]"),
            ("[1]", 0, "[]"),
        ] {
            let list = scan(source).expect("a list");
            let item = &list.items()[at];
            let previous = at.checked_sub(1).map(|at| list.items()[at].span.end);
            let got = spliced(source, &[removal(source, item.span.start, item.span.end, previous)]);
            assert_eq!(got, want, "{source}");
        }
    }

    #[test]
    fn an_entry_takes_its_comments_along_and_a_place_keeps_its_comma() {
        let source = "[\n    // one\n    1, // uno\n\n    // two\n    2,\n    3\n]";
        let list = scan(source).expect("a list");
        let got = spliced(source, &reorder(source, list.items(), 0, 2));
        assert_eq!(got, "[\n    // two\n    2,\n\n    3,\n    // one\n    1 // uno\n]");
        let inline = "[1, /* two */ 2, 3]";
        let list = scan(inline).expect("a list");
        assert_eq!(spliced(inline, &reorder(inline, list.items(), 2, 0)), "[3, /* two */ 1, 2]");
    }
}
