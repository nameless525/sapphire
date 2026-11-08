//! A combat timeline's edits, in its words: windows, instants, phases and entries of a move, as a
//! [`Document`] of moves takes them, by path.
//!
//! There's no second way to change a file here. Each edit goes through [`Document`], which writes the fewest
//! bytes and reads the result back through `sapphire_moves`' own parser, so nothing else in the file moves:
//! not a comment, not a blank line, not the order of a move's fields. An edit that makes a move wrong goes
//! through all the same, and [`crate::judge`] shows what the document's own check says of it.

use bevy_reflect::PartialReflect;
use sapphire_document::{Document, Outcome, PropertyKind, PropertyPath, Refused, Step, walk};
use sapphire_moves::{MoveDocument, MoveSetDocument};

/// A document of moves, as an editor holds it.
pub type MovesDocument = Document<MoveSetDocument>;

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

    /// The steps from a move to the list.
    fn steps(self) -> Vec<Step> {
        match self {
            Self::RootKeys => vec![Step::Field("motion".to_owned()), Step::Field("root".to_owned())],
            _ => vec![Step::Field(self.key().to_owned())],
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

impl From<Refused> for EditError {
    /// In the words a person reads on the timeline.
    fn from(refused: Refused) -> Self {
        match refused {
            Refused::ReadOnly(why) => Self(format!("the file can't be edited yet: {why}")),
            Refused::WrongType { why, .. } => Self(format!("this edit wouldn't read back: {why}")),
            Refused::Document(why) => Self(format!("this edit wouldn't read back: {why}")),
            other => Self(other.to_string()),
        }
    }
}

/// How a path calls the move at `mv`.
fn move_path(document: &MovesDocument, mv: usize) -> Result<PropertyPath, EditError> {
    let moves = PropertyPath::root().join(Step::Field("moves".to_owned()));
    let count = document.value().ok_or_else(|| unreadable(document))?.moves.len();
    if mv >= count {
        return Err(EditError(format!("the file has no move {mv}")));
    }
    Ok(document.entry_path(&moves, mv)?)
}

/// Why it can't be edited, in [`Refused`]'s words.
fn unreadable(document: &MovesDocument) -> EditError {
    Refused::ReadOnly(document.failure().map_or_else(String::new, |failure| failure.message().to_owned()))
        .into()
}

/// How a path calls a move's track.
fn list_path(document: &MovesDocument, mv: usize, track: Track) -> Result<PropertyPath, EditError> {
    let mut path = move_path(document, mv)?;
    for step in track.steps() {
        path = path.join(step);
    }
    Ok(path)
}

/// How a path calls an entry, as of now.
fn entry_path(document: &MovesDocument, entry: EntryRef) -> Result<PropertyPath, EditError> {
    let count = document
        .value()
        .ok_or_else(|| unreadable(document))?
        .moves
        .get(entry.mv)
        .map_or(0, |mv| entry.track.spans(mv).len());
    if entry.index >= count {
        return Err(EditError(format!("the move has no {} {}", entry.track.key(), entry.index)));
    }
    let list = list_path(document, entry.mv, entry.track)?;
    Ok(document.entry_path(&list, entry.index)?)
}

/// The fields of `at`, set together: one edit, or none.
fn set_numbers(
    document: &mut MovesDocument,
    label: &str,
    at: &PropertyPath,
    fields: &[(&str, u32)],
) -> Result<(), EditError> {
    document.together(label, |document| {
        for (name, value) in fields {
            let field = at.join(Step::Field((*name).to_owned()));
            document.set(label, &field, value)?;
        }
        Ok(())
    })
}

/// Sets a window's ends.
pub fn set_window(
    document: &mut MovesDocument,
    entry: EntryRef,
    from: u32,
    to: u32,
) -> Result<(), EditError> {
    let at = entry_path(document, entry)?;
    set_numbers(document, "edit a window", &at, &[("from", from), ("to", to)])
}

/// Sets an instant's tick.
pub fn set_instant(document: &mut MovesDocument, entry: EntryRef, at: u32) -> Result<(), EditError> {
    let path = entry_path(document, entry)?;
    set_numbers(document, "move an instant", &path, &[("at", at)])
}

/// Sets a move's phases.
pub fn set_phases(document: &mut MovesDocument, mv: usize, phases: Phases) -> Result<(), EditError> {
    let at = move_path(document, mv)?;
    let fields = [("startup", phases.startup), ("active", phases.active), ("recovery", phases.recovery)];
    set_numbers(document, "edit the phases", &at, &fields)
}

/// The tunable values written in an entry, in the order they're written: its numbers, however deep, and bare
/// names. Its ticks are the timeline's, and aren't among them, and nor is what the file leaves out.
pub fn entry_values(document: &MovesDocument, entry: EntryRef) -> Result<Vec<EntryValue>, EditError> {
    let at = entry_path(document, entry)?;
    let properties = document.properties_under(&at)?;
    let text = document.text();
    let mut values: Vec<(usize, EntryValue)> = properties
        .iter()
        .filter_map(|property| {
            let span = property.written.clone()?;
            // What holds other values isn't one itself, which leaves a variant with no fields and a `None`.
            let holds_values = property.kind != PropertyKind::Leaf
                && (property.kind == PropertyKind::List
                    || properties.iter().any(|other| {
                        other.path.steps().len() > property.path.steps().len()
                            && other.path.steps().starts_with(property.path.steps())
                            && other.written.is_some()
                    }));
            if holds_values {
                return None;
            }
            let path = label_of(document, &at, &property.path);
            let value =
                EntryValue { path, literal: text[span.clone()].to_owned(), property: property.path.clone() };
            Some((span.start, value))
        })
        .collect();
    values.sort_by_key(|(start, _)| *start);
    Ok(values
        .into_iter()
        .map(|(_, value)| value)
        .filter(|value| !matches!(value.path.as_str(), "from" | "to" | "at"))
        .collect())
}

/// A property's way down from its entry, as the file calls it: `attack.knockback.distance`, `offset.1`,
/// `allow.0.move`, whatever the field's called in Rust.
fn label_of(document: &MovesDocument, entry: &PropertyPath, path: &PropertyPath) -> String {
    let root = document.last_good().map(|value| value as &dyn PartialReflect);
    let mut above = entry.clone();
    let mut parts = Vec::new();
    for step in &path.steps()[entry.steps().len()..] {
        match step {
            Step::Field(name) => {
                let holder = root.and_then(|root| walk::get(root, &above).ok());
                parts
                    .push(holder.map_or(name.as_str(), |holder| walk::written_name(holder, name)).to_owned());
            }
            Step::Key(name) => parts.push(name.clone()),
            Step::Index(at) => parts.push(at.to_string()),
            Step::Some => {}
        }
        above = above.join(step.clone());
    }
    parts.join(".")
}

/// Replaces one value of an entry with `literal`, at the path [`entry_values`] gave it, spelled as it's
/// typed. A literal the document can't read is refused, and nothing changes.
pub fn set_entry_value(
    document: &mut MovesDocument,
    entry: EntryRef,
    path: &str,
    literal: &str,
) -> Result<(), EditError> {
    let value = entry_values(document, entry)?
        .into_iter()
        .find(|value| value.path == path)
        .ok_or_else(|| EditError(format!("this {} has no `{path}`", entry.track.key())))?;
    document.set_literal(&format!("edit {path}"), &value.property, literal)?;
    Ok(())
}

/// Adds `entry_text` at the end of a move's track, in the file's own layout for it, and returns the new entry.
pub fn insert_entry(
    document: &mut MovesDocument,
    mv: usize,
    track: Track,
    entry_text: &str,
) -> Result<EntryRef, EditError> {
    let list = list_path(document, mv, track)?;
    let count = document.value().and_then(|value| value.moves.get(mv)).map_or(0, |mv| track.spans(mv).len());
    // The timeline extends an authored root track. The generic property inspector can create one first.
    if track == Track::RootKeys
        && document.properties_under(&list)?.first().is_none_or(|list| list.written.is_none())
    {
        return Err(EditError("this move has no root keys to add to".to_owned()));
    }
    let added = document.insert_literal(&format!("add to {}", track.key()), &list, count, entry_text)?;
    debug_assert_eq!(added, Outcome::Changed, "a new entry is always an edit");
    Ok(EntryRef { mv, track, index: count })
}

/// Removes an entry and what only it needs: its comma, and its line if it was all that's on it. A comment
/// beside or above it stays, since a person wrote that.
pub fn delete_entry(document: &mut MovesDocument, entry: EntryRef) -> Result<(), EditError> {
    let at = entry_path(document, entry)?;
    document.remove(&format!("delete from {}", entry.track.key()), &at)?;
    Ok(())
}

/// A value [`entry_values`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryValue {
    /// Its way down from the entry: `attack.knockback.distance`, `shape.radius`, `offset.1`.
    pub path: String,
    /// As the file writes it.
    pub literal: String,
    /// How the document calls it.
    pub property: PropertyPath,
}

impl EntryValue {
    /// Whether it's a number.
    #[must_use]
    pub fn is_number(&self) -> bool {
        self.literal.parse::<f64>().is_ok()
    }
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
    use std::sync::Arc;

    use sapphire_document::{ReadFailure, kinds, registry_of};

    use super::test_files::{ALL, PLAYER, WARRIOR};
    use super::*;

    fn open(source: &str) -> MovesDocument {
        Document::open(source.to_owned(), kinds::moves(), Arc::new(registry_of::<MoveSetDocument>()))
    }

    fn entry(text: &MovesDocument, mv: &str, track: Track, index: usize) -> EntryRef {
        let document = text.value().expect("a document");
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
        let mut text = open(PLAYER);
        assert_eq!(text.failure(), None);
        let movement = |document: &MovesDocument| {
            document.value().expect("a document").fighter.as_ref().expect("a fighter").movement
        };
        let original = movement(&text).expect("the fighter has authored movement");
        let hit = entry(&text, "light_1", Track::Hitboxes, 0);
        set_window(&mut text, hit, 8, 12).expect("the edit");
        assert_ne!(text.text(), PLAYER);
        assert_eq!(section(text.text()), section(PLAYER));
        assert_eq!(movement(&text), Some(original));
    }

    /// The tree finds every value the parser reads, in all the documents there are: this is what lets a
    /// real file be edited at all, and what would fail first on a layout nobody has written yet.
    #[test]
    fn the_tree_agrees_with_the_parser_on_every_real_document() {
        for (name, source) in ALL {
            let text = open(source);
            assert_eq!(text.failure(), None, "{name}");
            assert_eq!(text.text(), source, "{name}");
        }
    }

    #[test]
    fn a_window_moves_where_a_hand_would_move_it_and_nothing_else_does() {
        let mut text = open(PLAYER);
        let hit = entry(&text, "light_1", Track::Hitboxes, 0);
        set_window(&mut text, hit, 8, 12).expect("the edit");
        let hand = by_hand(PLAYER, "light_1", "name: \"light_1_hit\"", "from: 7", "from: 8");
        let hand = by_hand(&hand, "light_1", "name: \"light_1_hit\"", "to: 11", "to: 12");
        assert_eq!(text.text(), hand);
        assert_eq!(text.value(), Some(&MoveSetDocument::parse(&hand).expect("hand edit parses")));
    }

    /// Setting every window of a real file to what it says already changes no byte, which is the round
    /// trip's own test: an edit that re-serialized anything would show here.
    #[test]
    fn setting_every_entry_to_what_it_says_is_no_edit() {
        for (name, source) in ALL {
            let mut text = open(source);
            let document = text.value().expect("a document").clone();
            for (mv_index, mv) in document.moves.iter().enumerate() {
                for track in Track::ALL {
                    for (index, span) in track.spans(mv).into_iter().enumerate() {
                        let entry = EntryRef { mv: mv_index, track, index };
                        if track.is_instant() {
                            set_instant(&mut text, entry, span.from).expect("no edit");
                        } else {
                            set_window(&mut text, entry, span.from, span.to).expect("no edit");
                        }
                    }
                }
                set_phases(&mut text, mv_index, Phases::of(mv)).expect("no edit");
            }
            assert_eq!(text.text(), source, "{name}");
        }
    }

    #[test]
    fn a_cue_moves_and_phases_change_in_place() {
        let mut text = open(PLAYER);
        let swing = entry(&text, "light_1", Track::Cues, 1);
        set_instant(&mut text, swing, 9).expect("the edit");
        let hand = by_hand(PLAYER, "light_1", "name: \"swing\"", "at: 7", "at: 9");
        assert_eq!(text.text(), hand);

        let mv = swing.mv;
        set_phases(&mut text, mv, Phases { startup: 8, active: 3, recovery: 23 }).expect("the edit");
        let hand = by_hand(&hand, "light_1", "startup:", "startup: 7", "startup: 8");
        let hand = by_hand(&hand, "light_1", "active:", "active: 4", "active: 3");
        assert_eq!(text.text(), hand);
    }

    #[test]
    fn a_value_inside_an_entry_changes_by_its_path() {
        let mut text = open(PLAYER);
        let hit = entry(&text, "light_1", Track::Hitboxes, 0);
        let values = entry_values(&text, hit).expect("values");
        let paths: Vec<&str> = values.iter().map(|value| value.path.as_str()).collect();
        assert!(paths.contains(&"attack.knockback.distance"), "{paths:?}");
        assert!(paths.contains(&"shape.radius"), "{paths:?}");
        assert!(!paths.contains(&"from"), "{paths:?}");

        set_entry_value(&mut text, hit, "attack.damage", "9").expect("the edit");
        let hand = by_hand(PLAYER, "light_1", "attack:", "damage: 6", "damage: 9");
        assert_eq!(text.text(), hand);
        assert_eq!(
            text.value().expect("a document").moves[0].hitboxes[0].attack.as_ref().map(|a| a.damage),
            Some(9)
        );

        let before = text.text().to_owned();
        let refused = set_entry_value(&mut text, hit, "attack.damage", "nine").expect_err("not a number");
        assert!(refused.0.contains("wouldn't read back"), "{refused}");
        assert_eq!(text.text(), before);
    }

    #[test]
    fn an_entry_added_to_a_track_follows_that_tracks_layout() {
        let mut text = open(PLAYER);
        let swing = entry(&text, "light_1", Track::Cues, 2);
        let added =
            insert_entry(&mut text, swing.mv, Track::Cues, r#"(name: "new", at: 20)"#).expect("the edit");
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
        let mut text = open(PLAYER);
        let hit = entry(&text, "light_1", Track::Hitboxes, 0);
        let added =
            insert_entry(&mut text, hit.mv, Track::Invulnerable, "(from: 2, to: 5)").expect("the edit");
        assert_eq!(added.index, 0);
        let document = text.value().expect("a document");
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
        let mut text = open(source);
        assert_eq!(text.failure(), None);
        let cue = |index| EntryRef { mv: 0, track: Track::Cues, index };
        delete_entry(&mut text, cue(0)).expect("the edit");
        assert!(text.text().contains("// Stays.\n                (name: \"second\""));
        delete_entry(&mut text, cue(0)).expect("the edit");
        assert!(text.text().contains("// Stays.\n                // Stays too.\n"), "{}", text.text());
        delete_entry(&mut text, EntryRef { mv: 0, track: Track::Parry, index: 1 }).expect("the edit");
        assert!(text.text().contains("parry: [(from: 0, to: 1)],"), "{}", text.text());
        delete_entry(&mut text, EntryRef { mv: 0, track: Track::Armour, index: 0 }).expect("the edit");
        assert!(text.text().contains("armour: [],"), "{}", text.text());
        let added =
            insert_entry(&mut text, 0, Track::Armour, "(from: 1, to: 2, threshold: 3)").expect("the edit");
        assert_eq!(added.index, 0);
        assert!(text.text().contains("armour: [(from: 1, to: 2, threshold: 3)],"), "{}", text.text());
    }

    #[test]
    fn a_file_that_doesnt_parse_keeps_its_last_reading_and_takes_no_edits() {
        let mut text = open(WARRIOR);
        let last = text.value().expect("a document").clone();
        let broken = WARRIOR.replacen("startup:", "startup", 1);
        text.take_disk("someone's edit", broken.clone());
        assert!(matches!(text.failure(), Some(ReadFailure::Parse(_))));
        assert_eq!(text.text(), broken);
        assert_eq!(text.last_good(), Some(&last));
        let refused = set_window(&mut text, EntryRef { mv: 0, track: Track::Hitboxes, index: 0 }, 1, 2)
            .expect_err("nothing to edit");
        assert!(refused.0.contains("can't be edited yet"), "{refused}");
        assert_eq!(text.text(), broken);
    }

    #[test]
    fn crlf_files_keep_their_line_ends() {
        let source = PLAYER.replace('\n', "\r\n");
        let mut text = open(&source);
        assert_eq!(text.failure(), None);
        let hit = entry(&text, "light_1", Track::Cues, 0);
        insert_entry(&mut text, hit.mv, Track::Cues, r#"(name: "new", at: 20)"#).expect("the edit");
        assert!(text.text().contains("at: 11),\r\n                    (name: \"new\", at: 20),\r\n"));
        assert_eq!(text.text().matches('\n').count(), text.text().matches("\r\n").count());
    }
}
