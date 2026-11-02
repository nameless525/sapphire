//! Edits to a document of moves, made in its text.
//!
//! Every edit here is a list of [`Splice`]s, the bytes it replaces and with what, found through
//! [`crate::text`]'s tree, so nothing else in the file moves: not a comment, not a blank line, not the
//! order of a move's fields. [`MovesText`] re-reads the text through `sapphire_moves`' own parser after
//! each one, and holds the tree to that parse (`agreement`), so what the tree found and what the
//! document means can't drift apart unseen. There's no second copy of what a document may say: an edit
//! that makes it wrong goes through, and [`crate::file`] shows what the document's own check says of it.

use std::ops::Range;

use sapphire_moves::{MoveDocument, MoveSetDocument};

use crate::text::{self, Field, Kind, Node};

/// Bytes to replace, and with what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    pub range: Range<usize>,
    pub text: String,
}

/// `text` with `splices` applied: they don't overlap, and may come in any order.
#[must_use]
pub fn apply(text: &str, splices: &[Splice]) -> String {
    let mut sorted: Vec<&Splice> = splices.iter().collect();
    sorted.sort_by_key(|splice| splice.range.start);
    let mut out = String::with_capacity(text.len() + 64);
    let mut at = 0;
    for splice in sorted {
        out.push_str(&text[at..splice.range.start]);
        out.push_str(&splice.text);
        at = splice.range.end;
    }
    out.push_str(&text[at..]);
    out
}

/// A row of the timeline: a list in a move's document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Track {
    Hitboxes,
    Hurtboxes,
    Invulnerable,
    PerfectDodge,
    Parry,
    Armour,
    Cancels,
    Warps,
    Cues,
    /// A hand-keyed move's root keys.
    RootKeys,
}

impl Track {
    pub const ALL: [Self; 10] = [
        Self::Hitboxes,
        Self::Hurtboxes,
        Self::Invulnerable,
        Self::PerfectDodge,
        Self::Parry,
        Self::Armour,
        Self::Cancels,
        Self::Warps,
        Self::Cues,
        Self::RootKeys,
    ];

    /// Its field in a move, and what `sapphire_moves::Issue::track` calls it. Root keys are `motion`'s.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Hitboxes => "hitboxes",
            Self::Hurtboxes => "hurtboxes",
            Self::Invulnerable => "invulnerable",
            Self::PerfectDodge => "perfect_dodge",
            Self::Parry => "parry",
            Self::Armour => "armour",
            Self::Cancels => "cancels",
            Self::Warps => "warps",
            Self::Cues => "cues",
            Self::RootKeys => "motion",
        }
    }

    /// Whether its entries are one tick (`at`) rather than a window (`from`, `to`).
    #[must_use]
    pub const fn is_instant(self) -> bool {
        matches!(self, Self::Cues | Self::RootKeys)
    }

    /// The list in a move's tree. `None` where the move's text doesn't write one.
    fn list(self, mv: &Node) -> Option<&Node> {
        match self {
            Self::RootKeys => mv.field("motion")?.field("root"),
            _ => mv.field(self.key()),
        }
    }

    /// Each entry's ticks, as the parsed document has them, in the file's order.
    #[must_use]
    pub fn spans(self, mv: &MoveDocument) -> Vec<TickSpan> {
        let spans =
            |list: Vec<(u32, u32)>| list.into_iter().map(|(from, to)| TickSpan { from, to }).collect();
        let instants = |list: Vec<u32>| list.into_iter().map(TickSpan::instant).collect();
        match self {
            Self::Hitboxes => spans(mv.hitboxes.iter().map(|entry| (entry.from, entry.to)).collect()),
            Self::Hurtboxes => spans(mv.hurtboxes.iter().map(|entry| (entry.from, entry.to)).collect()),
            Self::Invulnerable => spans(mv.invulnerable.iter().map(|entry| (entry.from, entry.to)).collect()),
            Self::PerfectDodge => {
                spans(mv.perfect_dodge.iter().map(|entry| (entry.from, entry.to)).collect())
            }
            Self::Parry => spans(mv.parry.iter().map(|entry| (entry.from, entry.to)).collect()),
            Self::Armour => spans(mv.armour.iter().map(|entry| (entry.from, entry.to)).collect()),
            Self::Cancels => spans(mv.cancels.iter().map(|entry| (entry.from, entry.to)).collect()),
            Self::Warps => spans(mv.warps.iter().map(|entry| (entry.from, entry.to)).collect()),
            Self::Cues => instants(mv.cues.iter().map(|entry| entry.at).collect()),
            Self::RootKeys => instants(mv.motion.root.iter().map(|entry| entry.at).collect()),
        }
    }
}

/// An entry's ticks: `from` up to but not including `to`. An instant takes one tick's cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickSpan {
    pub from: u32,
    pub to: u32,
}

impl TickSpan {
    #[must_use]
    pub const fn instant(at: u32) -> Self {
        Self { from: at, to: at.saturating_add(1) }
    }
}

/// One entry of one track of one move, by its place in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntryRef {
    pub mv: usize,
    pub track: Track,
    pub index: usize,
}

/// A move's three phases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Phases {
    pub startup: u32,
    pub active: u32,
    pub recovery: u32,
}

impl Phases {
    #[must_use]
    pub fn of(mv: &MoveDocument) -> Self {
        Self { startup: mv.startup, active: mv.active, recovery: mv.recovery }
    }
}

/// Why an edit wasn't made. The text is as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditError(pub String);

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A document's text and what it says: the one thing an editor changes.
#[derive(Clone, Debug)]
pub struct MovesText {
    text: String,
    parsed: Result<Parsed, ReadFailure>,
    /// The last time the text said something: what the timeline shows, read-only, while it doesn't.
    last_good: Option<Parsed>,
}

/// What the text says, and where.
#[derive(Clone, Debug)]
pub struct Parsed {
    pub document: MoveSetDocument,
    tree: Node,
}

/// Why the text can't be edited right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadFailure {
    /// It isn't a document of moves: RON's error, with its line. A person's edit, most likely.
    Parse(String),
    /// It's a document, and the editor can't find its way around the text: a bug here. The file reads,
    /// and stays as it is.
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

impl MovesText {
    #[must_use]
    pub fn new(text: String) -> Self {
        let parsed = read(&text);
        let last_good = parsed.as_ref().ok().cloned();
        Self { text, parsed, last_good }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Why the text can't be edited, if it can't.
    #[must_use]
    pub fn failure(&self) -> Option<&ReadFailure> {
        self.parsed.as_ref().err()
    }

    /// What the text says now.
    #[must_use]
    pub fn document(&self) -> Option<&MoveSetDocument> {
        self.parsed.as_ref().ok().map(|parsed| &parsed.document)
    }

    /// What the text last said, for showing while it says nothing.
    #[must_use]
    pub fn last_good(&self) -> Option<&MoveSetDocument> {
        self.last_good.as_ref().map(|parsed| &parsed.document)
    }

    /// Replaces the text whole: an undo, or the file changing under the editor.
    pub fn set_text(&mut self, text: String) {
        self.parsed = read(&text);
        if let Ok(parsed) = &self.parsed {
            self.last_good = Some(parsed.clone());
        }
        self.text = text;
    }

    fn parsed(&self) -> Result<&Parsed, EditError> {
        self.parsed
            .as_ref()
            .map_err(|failure| EditError(format!("the file can't be edited yet: {}", failure.message())))
    }

    fn move_node(&self, parsed: &Parsed, mv: usize) -> Result<Node, EditError> {
        let moves = parsed.tree.field("data").and_then(|data| data.field("moves"));
        moves
            .and_then(|moves| moves.items().get(mv))
            .cloned()
            .ok_or_else(|| EditError(format!("the file has no move {mv}")))
    }

    fn apply_splices(&mut self, splices: &[Splice]) -> Result<(), EditError> {
        let next = apply(&self.text, splices);
        match read(&next) {
            Ok(parsed) => {
                self.last_good = Some(parsed.clone());
                self.parsed = Ok(parsed);
                self.text = next;
                Ok(())
            }
            Err(failure) => Err(EditError(format!("this edit wouldn't read back: {}", failure.message()))),
        }
    }

    fn entry_node(&self, entry: EntryRef) -> Result<Node, EditError> {
        let parsed = self.parsed()?;
        let mv = self.move_node(parsed, entry.mv)?;
        entry
            .track
            .list(&mv)
            .and_then(|list| list.items().get(entry.index))
            .cloned()
            .ok_or_else(|| EditError(format!("the move has no {} {}", entry.track.key(), entry.index)))
    }

    /// Sets a window's ends. Reads back what it wrote, so a place the tree got wrong can't land.
    pub fn set_window(&mut self, entry: EntryRef, from: u32, to: u32) -> Result<(), EditError> {
        let node = self.entry_node(entry)?;
        let mut splices = Vec::with_capacity(2);
        for (name, value) in [("from", from), ("to", to)] {
            let field = node
                .field(name)
                .ok_or_else(|| EditError(format!("this {} has no `{name}`", entry.track.key())))?;
            if field.text(&self.text).parse::<u32>().ok() != Some(value) {
                splices.push(Splice { range: field.span.clone(), text: value.to_string() });
            }
        }
        self.checked(&splices, entry, TickSpan { from, to })
    }

    /// Sets an instant's tick.
    pub fn set_instant(&mut self, entry: EntryRef, at: u32) -> Result<(), EditError> {
        let node = self.entry_node(entry)?;
        let field =
            node.field("at").ok_or_else(|| EditError(format!("this {} has no `at`", entry.track.key())))?;
        let splice = Splice { range: field.span.clone(), text: at.to_string() };
        self.checked(&[splice], entry, TickSpan::instant(at))
    }

    /// Applies `splices`, and holds the entry to what they were to make of it.
    fn checked(&mut self, splices: &[Splice], entry: EntryRef, want: TickSpan) -> Result<(), EditError> {
        let before = self.text.clone();
        self.apply_splices(splices)?;
        let got = self
            .document()
            .and_then(|document| entry.track.spans(document.moves.get(entry.mv)?).get(entry.index).copied());
        if got != Some(want) {
            self.set_text(before);
            return Err(EditError(format!(
                "this edit of {} {} read back as {got:?}, not {want:?}: left as it was",
                entry.track.key(),
                entry.index
            )));
        }
        Ok(())
    }

    /// Sets a move's phases.
    pub fn set_phases(&mut self, mv: usize, phases: Phases) -> Result<(), EditError> {
        let parsed = self.parsed()?;
        let node = self.move_node(parsed, mv)?;
        let mut splices = Vec::with_capacity(3);
        for (name, value) in
            [("startup", phases.startup), ("active", phases.active), ("recovery", phases.recovery)]
        {
            let field = node.field(name).ok_or_else(|| EditError(format!("this move has no `{name}`")))?;
            if field.text(&self.text).parse::<u32>().ok() != Some(value) {
                splices.push(Splice { range: field.span.clone(), text: value.to_string() });
            }
        }
        self.apply_splices(&splices)?;
        let got = self.document().and_then(|document| document.moves.get(mv)).map(Phases::of);
        if got != Some(phases) {
            return Err(EditError(format!("this edit of a move's phases read back as {got:?}")));
        }
        Ok(())
    }

    /// The tunable values written in an entry, in the order they're written: its numbers, however deep,
    /// and bare names. Its ticks are the timeline's, and aren't among them.
    pub fn entry_values(&self, entry: EntryRef) -> Result<Vec<EntryValue>, EditError> {
        let node = self.entry_node(entry)?;
        let mut values = Vec::new();
        collect_values(&node, &self.text, "", &mut values);
        values.retain(|value| !matches!(value.path.as_str(), "from" | "to" | "at"));
        Ok(values)
    }

    /// Replaces one value of an entry with `literal`, at the path [`Self::entry_values`] gave it. A
    /// literal the document can't read is refused, and nothing changes.
    pub fn set_entry_value(&mut self, entry: EntryRef, path: &str, literal: &str) -> Result<(), EditError> {
        let node = self.entry_node(entry)?;
        let target = find_path(&node, path)
            .ok_or_else(|| EditError(format!("this {} has no `{path}`", entry.track.key())))?;
        let splice = Splice { range: target.span.clone(), text: literal.to_owned() };
        self.apply_splices(&[splice])
    }

    /// Adds `entry_text` at the end of a move's track, in the file's own layout for it, and returns the
    /// new entry.
    pub fn insert_entry(&mut self, mv: usize, track: Track, entry_text: &str) -> Result<EntryRef, EditError> {
        let parsed = self.parsed()?;
        let node = self.move_node(parsed, mv)?;
        let count = parsed.document.moves.get(mv).map_or(0, |mv| track.spans(mv).len());
        let splices = match track.list(&node) {
            Some(list) => match list.items().last() {
                Some(last) => after_last_item(&self.text, last, entry_text),
                None => into_empty_list(&self.text, list, entry_text),
            },
            None if track == Track::RootKeys => {
                return Err(EditError("this move has no root keys to add to".to_owned()));
            }
            None => new_list(&self.text, &node, track, entry_text)?,
        };
        self.apply_splices(&splices)?;
        Ok(EntryRef { mv, track, index: count })
    }

    /// Removes an entry and what only it needs: its comma, and its line if it was all that's on it. A
    /// comment beside or above it stays, since a person wrote that.
    pub fn delete_entry(&mut self, entry: EntryRef) -> Result<(), EditError> {
        let parsed = self.parsed()?;
        let mv = self.move_node(parsed, entry.mv)?;
        let list = entry
            .track
            .list(&mv)
            .ok_or_else(|| EditError(format!("the move has no {}", entry.track.key())))?;
        let items = list.items();
        let node = items
            .get(entry.index)
            .ok_or_else(|| EditError(format!("the move has no {} {}", entry.track.key(), entry.index)))?;
        let previous = entry.index.checked_sub(1).and_then(|index| items.get(index));
        let splice = removal(&self.text, node, previous);
        self.apply_splices(&[splice])
    }
}

/// A value [`MovesText::entry_values`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryValue {
    /// Its way down from the entry: `attack.knockback.distance`, `shape.radius`, `offset.1`.
    pub path: String,
    /// As the file writes it.
    pub literal: String,
}

impl EntryValue {
    /// Whether it's a number.
    #[must_use]
    pub fn is_number(&self) -> bool {
        self.literal.parse::<f64>().is_ok()
    }
}

fn collect_values(node: &Node, source: &str, path: &str, out: &mut Vec<EntryValue>) {
    let child_path = |name: &str| if path.is_empty() { name.to_owned() } else { format!("{path}.{name}") };
    match &node.kind {
        Kind::Atom => {
            out.push(EntryValue { path: path.to_owned(), literal: node.text(source).to_owned() });
        }
        Kind::Struct(fields) => {
            for field in fields {
                collect_values(&field.value, source, &child_path(&field.name), out);
            }
        }
        Kind::Seq(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_values(item, source, &child_path(&index.to_string()), out);
            }
        }
        Kind::Map(_) => {}
    }
}

fn find_path<'a>(node: &'a Node, path: &str) -> Option<&'a Node> {
    let mut at = node;
    for part in path.split('.').filter(|part| !part.is_empty()) {
        at = match part.parse::<usize>() {
            Ok(index) => at.items().get(index)?,
            Err(_) => at.field(part)?,
        };
    }
    Some(at)
}

/// The line's start before `at`.
fn line_start(source: &str, at: usize) -> usize {
    source[..at].rfind('\n').map_or(0, |newline| newline + 1)
}

/// Whether only spaces come before `at` on its line.
fn starts_its_line(source: &str, at: usize) -> bool {
    source[line_start(source, at)..at].chars().all(|c| c == ' ' || c == '\t')
}

/// The file's newline, as its first line ends.
fn newline_of(source: &str) -> &'static str {
    match source.find('\n') {
        Some(at) if at > 0 && source.as_bytes()[at - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// The spaces a line starts with.
fn indent_at(source: &str, at: usize) -> &str {
    let start = line_start(source, at);
    let rest = &source[start..];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

/// Past a comma straight after `at`, over spaces and comments. `None` if there's no comma.
fn comma_after(source: &str, at: usize) -> Option<usize> {
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

fn after_last_item(source: &str, last: &Node, entry_text: &str) -> Vec<Splice> {
    let comma = comma_after(source, last.span.end);
    let after = comma.unwrap_or(last.span.end);
    let mut splices = Vec::with_capacity(2);
    if starts_its_line(source, last.span.start)
        && let Some(end) = end_of_line_after(source, after)
    {
        if comma.is_none() {
            splices.push(Splice { range: last.span.end..last.span.end, text: ",".to_owned() });
        }
        let (newline, indent) = (newline_of(source), indent_at(source, last.span.start));
        splices.push(Splice { range: end..end, text: format!("{newline}{indent}{entry_text},") });
    } else if comma.is_some() {
        splices.push(Splice { range: after..after, text: format!(" {entry_text}") });
    } else {
        splices.push(Splice { range: after..after, text: format!(", {entry_text}") });
    }
    splices
}

fn into_empty_list(source: &str, list: &Node, entry_text: &str) -> Vec<Splice> {
    let inside = list.span.start + 1..list.span.end - 1;
    if source[inside.clone()].trim().is_empty() {
        vec![Splice { range: inside, text: entry_text.to_owned() }]
    } else {
        // A comment in it: it stays.
        let at = inside.start;
        vec![Splice { range: at..at, text: format!("{entry_text},") }]
    }
}

/// A new `track: [entry],` field, after the field a move's document writes before it.
fn new_list(source: &str, mv: &Node, track: Track, entry_text: &str) -> Result<Vec<Splice>, EditError> {
    const ORDER: [&str; 14] = [
        "name",
        "startup",
        "active",
        "recovery",
        "motion",
        "hitboxes",
        "hurtboxes",
        "invulnerable",
        "perfect_dodge",
        "parry",
        "armour",
        "cancels",
        "warps",
        "cues",
    ];
    let place = ORDER.iter().position(|name| *name == track.key()).unwrap_or(ORDER.len());
    let before: Option<&Field> = ORDER[..place]
        .iter()
        .rev()
        .find_map(|name| mv.fields().iter().find(|field| field.name == *name))
        .or_else(|| mv.fields().last());
    let before = before.ok_or_else(|| EditError("a move with no fields".to_owned()))?;
    let (key, newline) = (track.key(), newline_of(source));
    let comma = comma_after(source, before.value.span.end);
    let after = comma.unwrap_or(before.value.span.end);
    let mut splices = Vec::with_capacity(2);
    if starts_its_line(source, before.start)
        && let Some(end) = end_of_line_after(source, after)
    {
        if comma.is_none() {
            let at = before.value.span.end;
            splices.push(Splice { range: at..at, text: ",".to_owned() });
        }
        let indent = indent_at(source, before.start);
        splices.push(Splice {
            range: end..end,
            text: format!("{newline}{indent}{key}: [{newline}{indent}    {entry_text},{newline}{indent}],"),
        });
    } else if comma.is_some() {
        splices.push(Splice { range: after..after, text: format!(" {key}: [{entry_text}],") });
    } else {
        splices.push(Splice { range: after..after, text: format!(", {key}: [{entry_text}]") });
    }
    Ok(splices)
}

/// What removes `node` from its list.
fn removal(source: &str, node: &Node, previous: Option<&Node>) -> Splice {
    let comma = comma_after(source, node.span.end);
    let after = comma.unwrap_or(node.span.end);
    if starts_its_line(source, node.span.start) {
        let rest = source[after..].find('\n').map_or(source.len(), |newline| after + newline);
        if source[after..rest].trim().is_empty() {
            let end = (rest + 1).min(source.len());
            return Splice { range: line_start(source, node.span.start)..end, text: String::new() };
        }
    }
    if comma.is_some() {
        let spaces = source[after..].len() - source[after..].trim_start_matches(' ').len();
        return Splice { range: node.span.start..after + spaces, text: String::new() };
    }
    // The last in its list, with no comma of its own: the one before it goes too.
    match previous {
        Some(previous) => Splice { range: previous.span.end..node.span.end, text: String::new() },
        None => Splice { range: node.span.clone(), text: String::new() },
    }
}

/// Reads a text: the document it says, and the tree of where it says it, held to each other.
fn read(text: &str) -> Result<Parsed, ReadFailure> {
    let document = MoveSetDocument::parse(text).map_err(|error| ReadFailure::Parse(error_chain(&error)))?;
    let tree = text::scan(text).map_err(|error| {
        ReadFailure::Scan(format!(
            "the editor stopped reading the file's text at byte {}: {}. It parses, so this is the \
             editor's bug: edit it by hand for now",
            error.at, error.what
        ))
    })?;
    agreement(&document, &tree, text).map_err(ReadFailure::Scan)?;
    Ok(Parsed { document, tree })
}

/// Holds the tree to the parse: every value an edit can reach says the same in both, or the text is
/// read-only and says why. So a tree that's wrong about the file fails here, before any edit.
fn agreement(document: &MoveSetDocument, tree: &Node, text: &str) -> Result<(), String> {
    let moves = tree.field("data").and_then(|data| data.field("moves")).map(Node::items).unwrap_or_default();
    if moves.len() != document.moves.len() {
        return Err(format!(
            "the editor reads {} moves in the text, and the document has {}",
            moves.len(),
            document.moves.len()
        ));
    }
    let number = |node: Option<&Node>| node.and_then(|node| node.text(text).parse::<u32>().ok());
    for (node, mv) in moves.iter().zip(&document.moves) {
        let name = node.field("name").and_then(|name| name.string(text));
        if name != Some(mv.name.as_str()) {
            return Err(format!("the editor reads move `{}` as {name:?}", mv.name));
        }
        for (field, value) in [("startup", mv.startup), ("active", mv.active), ("recovery", mv.recovery)] {
            if number(node.field(field)) != Some(value) {
                return Err(format!("the editor can't read move `{}`'s `{field}`", mv.name));
            }
        }
        for track in Track::ALL {
            let spans = track.spans(mv);
            let items = track.list(node).map(Node::items).unwrap_or_default();
            if items.len() != spans.len() {
                return Err(format!(
                    "the editor reads {} entries in move `{}`'s {}, and the document has {}",
                    items.len(),
                    mv.name,
                    track.key(),
                    spans.len()
                ));
            }
            for (item, span) in items.iter().zip(&spans) {
                let agrees = if track.is_instant() {
                    number(item.field("at")) == Some(span.from)
                } else {
                    number(item.field("from")) == Some(span.from) && number(item.field("to")) == Some(span.to)
                };
                if !agrees {
                    return Err(format!("the editor can't read a {} of move `{}`", track.key(), mv.name));
                }
            }
        }
    }
    Ok(())
}

/// An error with its causes, as the sandbox words a refusal.
pub(crate) fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[cfg(test)]
pub(crate) mod test_files {
    pub const PLAYER: &str = include_str!("../../../content/combat/player/player.moves.ron");
    pub const WARRIOR: &str = include_str!("../../../content/combat/enemy/warrior.moves.ron");
    pub const SLICE: &str = include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron");
    pub const ALL: [(&str, &str); 3] = [("player", PLAYER), ("warrior", WARRIOR), ("slice", SLICE)];
}

#[cfg(test)]
mod tests {
    use super::test_files::{ALL, PLAYER, WARRIOR};
    use super::*;

    fn entry(text: &MovesText, mv: &str, track: Track, index: usize) -> EntryRef {
        let document = text.document().expect("a document");
        let mv = document.moves.iter().position(|found| found.name == mv).expect("the move");
        EntryRef { mv, track, index }
    }

    /// What a person would write by hand: the one `old` at the start of the first line holding `after`,
    /// after a move's `name` line, replaced with `new`.
    fn by_hand(text: &str, mv: &str, after: &str, old: &str, new: &str) -> String {
        let start = text.find(&format!("name: \"{mv}\",")).expect("the move");
        let line = start + text[start..].find(after).expect("the line");
        let at = line + text[line..].find(old).expect("the value");
        format!("{}{new}{}", &text[..at], &text[at + old.len()..])
    }

    /// A fighter's `movement` is no row, so an edit of a move goes around it, byte for byte, and the file
    /// still says what the parser reads of it.
    #[test]
    fn an_edit_leaves_a_fighters_movement_alone() {
        const SECTION: &str = "movement: (";
        let section = |text: &str| {
            let start = text.find(SECTION).expect("the section");
            text[start..start + text[start..].find("\n        ),").expect("its end")].to_owned()
        };
        let mut text = MovesText::new(PLAYER.to_owned());
        assert_eq!(text.failure(), None);
        let hit = entry(&text, "light_1", Track::Hitboxes, 0);
        text.set_window(hit, 8, 12).expect("the edit");
        assert_ne!(text.text(), PLAYER);
        assert_eq!(section(text.text()), section(PLAYER));
        let movement = text.document().expect("a document").fighter.as_ref().expect("a fighter").movement;
        assert_eq!(movement, Some(sapphire_moves::ControllerProfile::default()));
    }

    /// The tree finds every value the parser reads, in all the documents there are: this is what lets a
    /// real file be edited at all, and what would fail first on a layout nobody has written yet.
    #[test]
    fn the_tree_agrees_with_the_parser_on_every_real_document() {
        for (name, source) in ALL {
            let text = MovesText::new(source.to_owned());
            assert_eq!(text.failure(), None, "{name}");
            assert_eq!(text.text(), source, "{name}");
        }
    }

    #[test]
    fn a_window_moves_where_a_hand_would_move_it_and_nothing_else_does() {
        let mut text = MovesText::new(PLAYER.to_owned());
        let hit = entry(&text, "light_1", Track::Hitboxes, 0);
        text.set_window(hit, 8, 12).expect("the edit");
        let hand = by_hand(PLAYER, "light_1", "name: \"light_1_hit\"", "from: 7", "from: 8");
        let hand = by_hand(&hand, "light_1", "name: \"light_1_hit\"", "to: 11", "to: 12");
        assert_eq!(text.text(), hand);
        assert_eq!(text.document(), Some(&MoveSetDocument::parse(&hand).expect("hand edit parses")));
    }

    /// Setting every window of a real file to what it says already changes no byte, which is the round
    /// trip's own test: an edit that re-serialized anything would show here.
    #[test]
    fn setting_every_entry_to_what_it_says_is_no_edit() {
        for (name, source) in ALL {
            let mut text = MovesText::new(source.to_owned());
            let document = text.document().expect("a document").clone();
            for (mv_index, mv) in document.moves.iter().enumerate() {
                for track in Track::ALL {
                    for (index, span) in track.spans(mv).into_iter().enumerate() {
                        let entry = EntryRef { mv: mv_index, track, index };
                        if track.is_instant() {
                            text.set_instant(entry, span.from).expect("no edit");
                        } else {
                            text.set_window(entry, span.from, span.to).expect("no edit");
                        }
                    }
                }
                text.set_phases(mv_index, Phases::of(mv)).expect("no edit");
            }
            assert_eq!(text.text(), source, "{name}");
        }
    }

    #[test]
    fn a_cue_moves_and_phases_change_in_place() {
        let mut text = MovesText::new(PLAYER.to_owned());
        let swing = entry(&text, "light_1", Track::Cues, 1);
        text.set_instant(swing, 9).expect("the edit");
        let hand = by_hand(PLAYER, "light_1", "name: \"swing\"", "at: 7", "at: 9");
        assert_eq!(text.text(), hand);

        let mv = swing.mv;
        text.set_phases(mv, Phases { startup: 8, active: 3, recovery: 23 }).expect("the edit");
        let hand = by_hand(&hand, "light_1", "startup:", "startup: 7", "startup: 8");
        let hand = by_hand(&hand, "light_1", "active:", "active: 4", "active: 3");
        assert_eq!(text.text(), hand);
    }

    #[test]
    fn a_value_inside_an_entry_changes_by_its_path() {
        let mut text = MovesText::new(PLAYER.to_owned());
        let hit = entry(&text, "light_1", Track::Hitboxes, 0);
        let values = text.entry_values(hit).expect("values");
        let paths: Vec<&str> = values.iter().map(|value| value.path.as_str()).collect();
        assert!(paths.contains(&"attack.knockback.distance"), "{paths:?}");
        assert!(paths.contains(&"shape.radius"), "{paths:?}");
        assert!(!paths.contains(&"from"), "{paths:?}");

        text.set_entry_value(hit, "attack.damage", "9").expect("the edit");
        let hand = by_hand(PLAYER, "light_1", "attack:", "damage: 6", "damage: 9");
        assert_eq!(text.text(), hand);
        assert_eq!(
            text.document().expect("a document").moves[0].hitboxes[0].attack.as_ref().map(|a| a.damage),
            Some(9)
        );

        let before = text.text().to_owned();
        let refused = text.set_entry_value(hit, "attack.damage", "nine").expect_err("not a number");
        assert!(refused.0.contains("wouldn't read back"), "{refused}");
        assert_eq!(text.text(), before);
    }

    #[test]
    fn an_entry_added_to_a_track_follows_that_tracks_layout() {
        let mut text = MovesText::new(PLAYER.to_owned());
        let swing = entry(&text, "light_1", Track::Cues, 2);
        let added = text.insert_entry(swing.mv, Track::Cues, r#"(name: "new", at: 20)"#).expect("the edit");
        assert_eq!(added.index, 3);
        let hand = by_hand(
            PLAYER,
            "light_1",
            "(name: \"step\", at: 11),",
            "(name: \"step\", at: 11),",
            "(name: \"step\", at: 11),\n                    (name: \"new\", at: 20),",
        );
        assert_eq!(text.text(), hand);
    }

    #[test]
    fn a_track_a_move_doesnt_write_appears_in_the_documents_order() {
        let mut text = MovesText::new(PLAYER.to_owned());
        let hit = entry(&text, "light_1", Track::Hitboxes, 0);
        let added = text.insert_entry(hit.mv, Track::Invulnerable, "(from: 2, to: 5)").expect("the edit");
        assert_eq!(added.index, 0);
        let document = text.document().expect("a document");
        assert_eq!(document.moves[hit.mv].invulnerable[0].to, 5);
        let at_hitboxes = text.text().find("hitboxes:").expect("hitboxes");
        let at_invulnerable = text.text().find("invulnerable:").expect("invulnerable");
        let at_cancels = text.text().find("cancels:").expect("cancels");
        assert!(at_hitboxes < at_invulnerable && at_invulnerable < at_cancels);
        assert!(
            text.text().contains(
                "                invulnerable: [\n                    (from: 2, to: 5),\n                ],\n"
            ),
            "the file's own layout"
        );
    }

    #[test]
    fn a_deleted_entry_takes_its_line_and_leaves_comments() {
        let source = "(\n    type: \"sapphire::moves::MoveSetDocument\",\n    version: 1,\n    data: (\n        intents: [],\n        moves: [(\n            name: \"a\",\n            startup: 1,\n            active: 1,\n            recovery: 8,\n            cues: [\n                // Stays.\n                (name: \"first\", at: 0),\n                (name: \"second\", at: 1), // Stays too.\n                (name: \"third\", at: 2),\n            ],\n            parry: [(from: 0, to: 1), (from: 2, to: 3)],\n            armour: [(from: 0, to: 1, threshold: 2)],\n        )],\n    ),\n)\n";
        let mut text = MovesText::new(source.to_owned());
        assert_eq!(text.failure(), None);
        let cue = |index| EntryRef { mv: 0, track: Track::Cues, index };
        text.delete_entry(cue(0)).expect("the edit");
        assert!(text.text().contains("// Stays.\n                (name: \"second\""));
        text.delete_entry(cue(0)).expect("the edit");
        assert!(text.text().contains("// Stays.\n                // Stays too.\n"), "{}", text.text());
        text.delete_entry(EntryRef { mv: 0, track: Track::Parry, index: 1 }).expect("the edit");
        assert!(text.text().contains("parry: [(from: 0, to: 1)],"), "{}", text.text());
        text.delete_entry(EntryRef { mv: 0, track: Track::Armour, index: 0 }).expect("the edit");
        assert!(text.text().contains("armour: [],"), "{}", text.text());
        let added = text.insert_entry(0, Track::Armour, "(from: 1, to: 2, threshold: 3)").expect("the edit");
        assert_eq!(added.index, 0);
        assert!(text.text().contains("armour: [(from: 1, to: 2, threshold: 3)],"), "{}", text.text());
    }

    #[test]
    fn a_file_that_doesnt_parse_keeps_its_last_reading_and_takes_no_edits() {
        let mut text = MovesText::new(WARRIOR.to_owned());
        let last = text.document().expect("a document").clone();
        let broken = WARRIOR.replacen("startup:", "startup", 1);
        text.set_text(broken.clone());
        assert!(matches!(text.failure(), Some(ReadFailure::Parse(_))));
        assert_eq!(text.text(), broken);
        assert_eq!(text.last_good(), Some(&last));
        let refused = text
            .set_window(EntryRef { mv: 0, track: Track::Hitboxes, index: 0 }, 1, 2)
            .expect_err("nothing to edit");
        assert!(refused.0.contains("can't be edited yet"), "{refused}");
        assert_eq!(text.text(), broken);
    }

    #[test]
    fn crlf_files_keep_their_line_ends() {
        let source = PLAYER.replace('\n', "\r\n");
        let mut text = MovesText::new(source.clone());
        assert_eq!(text.failure(), None);
        let hit = entry(&text, "light_1", Track::Cues, 0);
        text.insert_entry(hit.mv, Track::Cues, r#"(name: "new", at: 20)"#).expect("the edit");
        assert!(text.text().contains("at: 11),\r\n                    (name: \"new\", at: 20),\r\n"));
        assert_eq!(text.text().matches('\n').count(), text.text().matches("\r\n").count());
    }
}
