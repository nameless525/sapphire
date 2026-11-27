//! What's wrong with a document, by document and field (M6 T6): one shape for the three ways the tools find out.
//!
//! A [`Problem`] is a parse failure (the text isn't this kind of document), a check's issue (it reads, and a rule
//! says no) or a cook's refusal, each on its document and, where one is known, the field: a property path as the
//! document's RON spells it, lists by place (`placements[2].scale`, `kinds[0].model`, `rows[1].shake?.strength`).
//! The editor's problems pane lists them, `cargo xtask content-check` prints them the same way ([`report`]), and
//! each names its field in the same spelling whichever of the three found it.
//!
//! # How a problem finds its field
//!
//! Nothing here knows any kind's rules. A message names its field by **the convention the documents' errors
//! already keep**: it starts with the field's path in backticks, then a colon or a space (`` `placements[0].at`:
//! isn't finite ``), at the start of a line or right after the position a RON error starts with (`3:15: `). The
//! path is from the document's root: an error about an entry leads with the entry's place
//! (`` `rigs[2].rig.key.intensity`: ``), since a path after a wrapper's own words (`` preset `dawn`:
//! `key.intensity` ``) is the entry's, not the document's, and isn't read. [`named_field`] reads that, so a new
//! refusal of any kind names its field by writing its message so, and nothing here changes. A RON error says
//! where it stopped instead (`3:15: unexpected …`, ron's line and column), and [`field_at`] finds the field the
//! text holds there: only in the text given, which must be the text that error read. A message that does neither
//! leaves its problem on the whole document, saying why ([`At::Document`]).
//!
//! A cook's refusal arrives with its field already found: the pipeline reads its message the same way when it
//! writes one (`sapphire_pipeline::refusal`).

use std::fmt;
use std::path::{Path, PathBuf};

use crate::kinds::Kind;
use crate::path::PropertyPath;
use crate::text::{self, Node};

/// Which of the tools found it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    /// The text isn't this kind of document.
    Parse,
    /// It reads, and a rule says no.
    Check,
    /// The cook refused it.
    Cook,
}

impl Origin {
    /// Its word in a report.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Check => "check",
            Self::Cook => "cook",
        }
    }
}

/// Where in its document a problem is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum At {
    /// A field, as the document's RON spells its path.
    Field(String),
    /// The document as a whole, and why no field is known.
    Document(String),
}

impl At {
    /// The field, if it's on one.
    #[must_use]
    pub fn field(&self) -> Option<&str> {
        match self {
            Self::Field(field) => Some(field),
            Self::Document(_) => None,
        }
    }
}

/// One thing wrong with one document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The document, as whoever found it names it: open in the editor, or under a cook's source tree.
    pub document: PathBuf,
    pub at: At,
    pub origin: Origin,
    /// What's wrong, then each thing that caused it, a line each.
    pub message: String,
}

impl Problem {
    /// The text `text` of `document` doesn't read: its parser said `failure`.
    #[must_use]
    pub fn parse(document: &Path, text: &str, failure: &str) -> Self {
        Self::found(document, Origin::Parse, failure, Some(text))
    }

    /// A rule of `document`'s kind says `issue` of it.
    #[must_use]
    pub fn check(document: &Path, issue: &str) -> Self {
        Self::found(document, Origin::Check, issue, None)
    }

    /// A rule says `issue` of `document`, whose text is `text`: a position in the issue is found in it.
    #[must_use]
    pub fn check_in(document: &Path, issue: &str, text: &str) -> Self {
        Self::found(document, Origin::Check, issue, Some(text))
    }

    /// A cook refused `document` with `message`, at `field` if it named one. The refusal found its field as it
    /// was made, reading its document's text only where that was what failed (`sapphire_pipeline::refusal`), so a
    /// RON position in its message isn't looked for here: it may be another document's.
    #[must_use]
    pub fn cook(document: &Path, field: Option<&str>, message: &str) -> Self {
        match field {
            Some(field) => Self {
                document: document.to_owned(),
                at: At::Field(field.to_owned()),
                origin: Origin::Cook,
                message: message.to_owned(),
            },
            None => Self::found(document, Origin::Cook, message, None),
        }
    }

    fn found(document: &Path, origin: Origin, message: &str, text: Option<&str>) -> Self {
        Self { document: document.to_owned(), at: locate(message, text), origin, message: message.to_owned() }
    }
}

/// What `kind`'s parser and rules find wrong with the text `text` of `document`: its parse failure, or each issue
/// its rules find. What the pane lists of an open document, and `content-check` of every document of a tree.
#[must_use]
pub fn of_text(document: &Path, kind: Kind, text: &str) -> Vec<Problem> {
    match kind.read(text) {
        Ok(issues) => issues.iter().map(|issue| Problem::check_in(document, issue, text)).collect(),
        Err(failure) => vec![Problem::parse(document, text, &failure)],
    }
}

/// Where `message` says it is: its field by the convention ([`named_field`]), or by a RON error's position in
/// `text`, or else the document, and why.
#[must_use]
pub fn locate(message: &str, text: Option<&str>) -> At {
    if let Some(field) = named_field(message) {
        return At::Field(field.to_owned());
    }
    let Some((line, column)) = position(message) else {
        return At::Document("its message names no field".to_owned());
    };
    let Some(text) = text else {
        return At::Document(format!(
            "its message names no field, and nothing says what text its {line}:{column} is in"
        ));
    };
    match byte_at(text, line, column).and_then(|at| field_at(text, at)) {
        Some(field) => At::Field(field),
        None => At::Document(format!(
            "its text stops reading as RON at {line}:{column}, where no field of the document is"
        )),
    }
}

/// The field a message names by the convention ([module docs](self)): a property path in backticks at the start
/// of one of its lines, or right after a RON position (`3:15: `, `3:15-3:18: `), followed by a colon or a space.
#[must_use]
pub fn named_field(message: &str) -> Option<&str> {
    leads(message).find_map(|part| {
        let inner = part.strip_prefix('`')?;
        let end = inner.find('`')?;
        let (path, after) = (&inner[..end], &inner[end + 1..]);
        let follows = after.starts_with(':') || after.starts_with(' ');
        (follows && !path.is_empty() && path.parse::<PropertyPath>().is_ok()).then_some(path)
    })
}

/// Where a RON error in `message` says it stopped, as ron writes it at the start of a part: its line and column,
/// from 1 (`3:15: …`, or a span, `3:15-3:18: …`).
#[must_use]
pub fn position(message: &str) -> Option<(usize, usize)> {
    parts(message).find_map(|part| {
        let (span, _) = part.split_once(": ")?;
        let start = span.split_once('-').map_or(span, |(start, _)| start);
        let (line, column) = start.split_once(':')?;
        let all_digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
        (all_digits(line) && all_digits(column)).then(|| Some((line.parse().ok()?, column.parse().ok()?)))?
    })
}

/// Each line of `message`, and each part of a line right after a RON position.
fn leads(message: &str) -> impl Iterator<Item = &str> {
    message.lines().flat_map(|line| {
        let mut leads = vec![line.trim_start()];
        let mut from = 0;
        while let Some(at) = line[from..].find(": ") {
            let end = from + at;
            let before = &line[..end];
            let word = before.rsplit(char::is_whitespace).next().unwrap_or(before);
            if is_span(word) {
                leads.push(&line[end + 2..]);
            }
            from = end + 2;
        }
        leads
    })
}

/// Whether `word` is a RON position, `3:15`, or a span, `3:15-3:18`.
fn is_span(word: &str) -> bool {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let place =
        |text: &str| text.split_once(':').is_some_and(|(line, column)| digits(line) && digits(column));
    match word.split_once('-') {
        Some((start, end)) => place(start) && place(end),
        None => place(word),
    }
}

/// Each line of `message`, and each part of a line after a `": "`.
fn parts(message: &str) -> impl Iterator<Item = &str> {
    message.lines().flat_map(|line| {
        let mut parts = vec![line.trim_start()];
        let mut rest = line;
        while let Some(at) = rest.find(": ") {
            rest = &rest[at + 2..];
            parts.push(rest);
        }
        parts
    })
}

/// The byte of `text` at ron's `line` and `column`, each from 1, the column counting characters.
fn byte_at(text: &str, line: usize, column: usize) -> Option<usize> {
    let start: usize = text.split_inclusive('\n').take(line.checked_sub(1)?).map(str::len).sum();
    let rest = text.get(start..)?;
    let offset = rest.char_indices().nth(column.checked_sub(1)?).map_or(rest.len(), |(offset, _)| offset);
    Some(start + offset)
}

/// The field of the document `text` that holds byte `at`, as its RON spells the path: `None` if the text doesn't
/// scan (a syntax error, most likely) or `at` is in no field.
#[must_use]
pub fn field_at(text: &str, at: usize) -> Option<String> {
    let tree = text::scan(text).ok()?;
    let body = tree.field("data").filter(|_| tree.field("type").is_some()).unwrap_or(&tree);
    let mut path = String::new();
    descend(body, text, at, &mut path);
    (!path.is_empty()).then_some(path)
}

fn descend(node: &Node, source: &str, at: usize, path: &mut String) {
    for field in node.fields() {
        if (field.start..field.value.span.end).contains(&at) {
            if !path.is_empty() {
                path.push('.');
            }
            path.push_str(&field.name);
            descend(&field.value, source, at, path);
            return;
        }
    }
    let items = node.items();
    let is_some = items.len() == 1 && node.text(source).starts_with("Some");
    for (index, item) in items.iter().enumerate() {
        if item.span.contains(&at) {
            if is_some {
                path.push('?');
            } else {
                path.push_str(&format!("[{index}]"));
            }
            descend(item, source, at, path);
            return;
        }
    }
}

/// `problems` with each found twice by different tools kept once, the first: a document's parse failure and its
/// cook's refusal at the same field are one fault. Two of one tool at one field are two, and two on the whole
/// document are always two, since nothing says they're the same fault.
#[must_use]
pub fn deduplicated(problems: Vec<Problem>) -> Vec<Problem> {
    let mut kept: Vec<Problem> = Vec::with_capacity(problems.len());
    for problem in problems {
        let seen = problem.at.field().is_some_and(|field| {
            kept.iter().any(|earlier| {
                earlier.origin != problem.origin
                    && earlier.document == problem.document
                    && earlier.at.field() == Some(field)
            })
        });
        if !seen {
            kept.push(problem);
        }
    }
    kept
}

/// `problems` by document, in the order each document first comes, as the pane lists them.
#[must_use]
pub fn grouped(problems: &[Problem]) -> Vec<(&Path, Vec<&Problem>)> {
    let mut groups: Vec<(&Path, Vec<&Problem>)> = Vec::new();
    for problem in problems {
        match groups.iter_mut().find(|(document, _)| *document == problem.document.as_path()) {
            Some((_, group)) => group.push(problem),
            None => groups.push((&problem.document, vec![problem])),
        }
    }
    groups
}

/// A problem's row under its document: `placements[0].scale: why (parse)`, its message on one line, less the
/// field it starts by naming.
impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = self.message.replace('\n', ": ");
        match &self.at {
            At::Field(field) => {
                let named = format!("`{field}`");
                let message = message
                    .strip_prefix(&named)
                    .map_or(message.as_str(), |rest| rest.strip_prefix(':').unwrap_or(rest).trim_start());
                write!(f, "{field}: {message} ({})", self.origin.name())
            }
            At::Document(why) => write!(f, "{message} ({}; no field: {why})", self.origin.name()),
        }
    }
}

/// The problems as the pane lists them: each document, then its rows, indented.
#[must_use]
pub fn report(problems: &[Problem]) -> String {
    let mut text = String::new();
    for (document, rows) in grouped(problems) {
        text.push_str(&document.display().to_string());
        text.push('\n');
        for row in rows {
            text.push_str("  ");
            text.push_str(&row.to_string());
            text.push('\n');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_convention_names_a_field_at_a_line_or_after_a_wrapper() {
        assert_eq!(named_field("`placements[2].scale`: must be positive"), Some("placements[2].scale"));
        assert_eq!(named_field("`columns` must be at least 1"), Some("columns"));
        assert_eq!(named_field("cooking a.zone.ron\n`kinds[0].model`: no mesh"), Some("kinds[0].model"));
        assert_eq!(named_field("the library doesn't read: 3:4: `effects[1]`: bad"), Some("effects[1]"));
        assert_eq!(named_field("the library doesn't read: 3:4-3:9: `effects[1]` bad"), Some("effects[1]"));
        assert_eq!(named_field("preset `dawn`: `key.intensity` must be finite"), None, "an entry's own path");
        assert_eq!(
            named_field("3:4: preset `dawn`: `key.intensity` must be"),
            None,
            "after a wrapper's words"
        );
        assert_eq!(named_field(r#"`moves["light_1"].startup`: zero"#), Some(r#"moves["light_1"].startup"#));
        assert_eq!(
            named_field("move `light_1`, track `hitboxes`: overlaps"),
            None,
            "a name, not a path, leads"
        );
        assert_eq!(named_field("`en-US` isn't a locale tag"), None, "not a path");
        assert_eq!(named_field("`a.b`, then"), None, "no colon or space after it");
        assert_eq!(named_field("nothing here"), None);
    }

    #[test]
    fn a_ron_position_is_read_at_a_part_start() {
        assert_eq!(position("3:15: Unexpected"), Some((3, 15)));
        assert_eq!(position("3:15-3:18: unknown field"), Some((3, 15)));
        assert_eq!(position("cooking x\nthe zone doesn't read: 12:4: missing field"), Some((12, 4)));
        assert_eq!(position("at 10:30 the light: changed"), None);
    }

    const ZONE: &str = "(\n    type: \"sapphire::zone::ZoneDocument\",\n    version: 3,\n    data: (\n        \
                        placements: [\n            (prefab: \"a\", at: (0.0, 1.0, 2.0)),\n            (prefab: \"b\", \
                        at: (0.0, 1.0, 2.0), shade: Some((tint: 2))),\n        ],\n    ),\n)\n";

    #[test]
    fn a_position_finds_the_field_that_holds_it() {
        let at = ZONE.find("1.0, 2.0)),").unwrap();
        assert_eq!(field_at(ZONE, at).as_deref(), Some("placements[0].at[1]"));
        let at = ZONE.find("tint").unwrap();
        assert_eq!(field_at(ZONE, at).as_deref(), Some("placements[1].shade?.tint"));
        let at = ZONE.find("\"b\"").unwrap();
        assert_eq!(field_at(ZONE, at).as_deref(), Some("placements[1].prefab"));
        assert_eq!(field_at(ZONE, 0), None, "the envelope is no field");
        assert_eq!(field_at("(a: [", 3), None, "a text that doesn't scan");
        // ron's line 6, column 37, is the `1.0` of the first placement.
        let at = byte_at(ZONE, 6, 37).unwrap();
        assert_eq!(&ZONE[at..at + 3], "1.0");
        assert_eq!(locate("6:37: expected a string", Some(ZONE)), At::Field("placements[0].at[1]".into()));
        assert!(matches!(locate("1:1: Unexpected", Some(ZONE)), At::Document(_)));
        assert!(matches!(locate("schema version 9 is newer", Some(ZONE)), At::Document(_)));
    }

    #[test]
    fn one_fault_found_twice_is_one_row_and_two_rules_at_one_field_are_two() {
        let document = Path::new("a.zone.ron");
        let parse = Problem::parse(document, ZONE, "`placements[0].scale`: is zero");
        let cook = Problem::cook(document, Some("placements[0].scale"), "cooking a.zone.ron\nis zero");
        let first = Problem::check(document, "`grid`: too small");
        let second = Problem::check(document, "`grid`: not square");
        let unplaced = Problem::check(document, "the grid is lopsided");
        let refused = Problem::cook(document, None, "cooking a.zone.ron\nits prefab is gone");
        let kept = deduplicated(vec![
            parse.clone(),
            first.clone(),
            second.clone(),
            cook,
            unplaced.clone(),
            refused.clone(),
        ]);
        assert_eq!(kept, vec![parse, first, second, unplaced, refused], "two on the document are two");
        let text = report(&kept);
        assert!(text.starts_with("a.zone.ron\n  placements[0].scale: is zero (parse)\n"), "{text}");
    }
}
