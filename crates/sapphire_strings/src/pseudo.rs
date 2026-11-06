//! Pseudo-localization (M3 V2): every message accented, longer and framed.
//!
//! Three things show on a screen in this mode:
//!
//! - **What's too short:** text grows by [`EXPANSION_NUMERATOR`]/[`EXPANSION_DENOMINATOR`] of its
//!   length, about what a long language takes, by doubling vowels and then padding.
//! - **What's clipped:** a message opens with [`FRAME_OPEN`] and closes with [`FRAME_CLOSE`], so one
//!   missing its end was cut off.
//! - **What isn't in the tables:** a string that's not framed never came through a message. So the way
//!   to find a hard-coded string is to run a screen in this mode and test each string given to layout
//!   with [`is_framed`] (`sapphire_ui`'s text, exit test 4's walk): the frame is unlikely in anything
//!   typed by hand, an unknown key shows bare too, and a whole string is checked before line breaking
//!   can split it. A reference nested in a message gets a frame inside the frame, which is harmless;
//!   a term doesn't, since it's only ever part of a message. HUD digits that bypass text have no
//!   strings to find.
//!
//! The frame and padding are ASCII, in every font. The accents are all Latin-1 and Latin Extended-A:
//! a subset meant to show this mode adds [`characters`]. Text in other scripts keeps its characters
//! and is padded.

use std::collections::BTreeSet;

use crate::table::{Part, Pattern, Run, Span, StringTable};

/// What opens a pseudo-localized message.
pub const FRAME_OPEN: &str = "[!! ";

/// What closes a pseudo-localized message.
pub const FRAME_CLOSE: &str = " !!]";

/// Text grows by at least this over [`EXPANSION_DENOMINATOR`] of its characters, before the frame.
pub const EXPANSION_NUMERATOR: usize = 2;

/// See [`EXPANSION_NUMERATOR`].
pub const EXPANSION_DENOMINATOR: usize = 5;

/// What pads text once its vowels have been doubled.
const PADDING: char = '~';

const fn accented(letter: char) -> char {
    match letter {
        'a' => 'å',
        'c' => 'ç',
        'd' => 'ð',
        'e' => 'é',
        'g' => 'ĝ',
        'h' => 'ĥ',
        'i' => 'î',
        'j' => 'ĵ',
        'k' => 'ķ',
        'l' => 'ļ',
        'n' => 'ñ',
        'o' => 'ö',
        'r' => 'ŕ',
        's' => 'š',
        't' => 'ţ',
        'u' => 'û',
        'w' => 'ŵ',
        'y' => 'ý',
        'z' => 'ž',
        'A' => 'Å',
        'C' => 'Ç',
        'D' => 'Ð',
        'E' => 'É',
        'G' => 'Ĝ',
        'H' => 'Ĥ',
        'I' => 'Î',
        'J' => 'Ĵ',
        'K' => 'Ķ',
        'L' => 'Ļ',
        'N' => 'Ñ',
        'O' => 'Ö',
        'R' => 'Ŕ',
        'S' => 'Š',
        'T' => 'Ţ',
        'U' => 'Û',
        'W' => 'Ŵ',
        'Y' => 'Ý',
        'Z' => 'Ž',
        other => other,
    }
}

/// `text`, accented and longer, with no frame.
pub fn pseudolocalize(text: &str) -> String {
    let count = text.chars().count();
    let mut extra = (count * EXPANSION_NUMERATOR).div_ceil(EXPANSION_DENOMINATOR);
    let mut out = String::with_capacity(text.len() + extra * 2);
    for letter in text.chars() {
        out.push(accented(letter));
        if extra > 0 && matches!(letter, 'a' | 'e' | 'i' | 'o' | 'u' | 'A' | 'E' | 'I' | 'O' | 'U') {
            out.push(accented(letter));
            extra -= 1;
        }
    }
    out.extend(std::iter::repeat_n(PADDING, extra));
    out
}

/// Whether `text` is a whole pseudo-localized message: not hard-coded, not an unknown key and not
/// clipped by whatever made it.
pub fn is_framed(text: &str) -> bool {
    text.starts_with(FRAME_OPEN) && text.ends_with(FRAME_CLOSE)
}

/// Every character this mode adds to English: what a subset needs to show it.
pub fn characters() -> BTreeSet<char> {
    let letters = ('a'..='z').chain('A'..='Z').map(accented);
    letters.chain(FRAME_OPEN.chars()).chain(FRAME_CLOSE.chars()).chain([PADDING]).collect()
}

impl StringTable {
    /// This table as pseudo-localized display text. Its keys, numbers, selector arguments and variant
    /// keys stay. A selector that reads a term must read its original semantic table, as
    /// `sapphire_loc` does: the same term can also be shown as transformed display text.
    /// A message gets a frame, a term doesn't.
    pub fn pseudolocalized(&self) -> StringTable {
        let mut table = self.clone();
        let added = |pool: &mut String, text: String| {
            let span = Span { start: pool.len() as u32, len: text.len() as u32 };
            pool.push_str(&text);
            span
        };
        for index in 0..table.parts.len() {
            if let Part::Text(span) = table.parts[index] {
                let text = pseudolocalize(self.str(span));
                table.parts[index] = Part::Text(added(&mut table.pool, text));
            }
        }
        for index in 0..table.variants.len() {
            if let Pattern::Plain(span) = table.variants[index].pattern {
                let text = pseudolocalize(self.str(span));
                table.variants[index].pattern = Pattern::Plain(added(&mut table.pool, text));
            }
        }
        let open = added(&mut table.pool, FRAME_OPEN.to_owned());
        let close = added(&mut table.pool, FRAME_CLOSE.to_owned());
        for index in 0..table.entries.len() {
            let entry = table.entries[index];
            let is_term = self.str(entry.key).starts_with('-');
            table.entries[index].pattern = match entry.pattern {
                Pattern::Plain(span) => {
                    let text = pseudolocalize(self.str(span));
                    let text = if is_term { text } else { [FRAME_OPEN, text.as_str(), FRAME_CLOSE].concat() };
                    Pattern::Plain(added(&mut table.pool, text))
                }
                Pattern::Parts(run) if is_term => Pattern::Parts(run),
                // A message's parts must lie together, so the frame takes a copy of them.
                Pattern::Parts(run) => {
                    let start = table.parts.len() as u32;
                    table.parts.push(Part::Text(open));
                    table.parts.extend_from_within(run.start as usize..(run.start + run.len) as usize);
                    table.parts.push(Part::Text(close));
                    Pattern::Parts(Run { start, len: run.len + 2 })
                }
            };
        }
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Entry, Expression};

    #[test]
    fn text_grows_by_the_stated_factor_at_least() {
        for text in ["Settings", "Go", "rhythm", "A long sentence, as a menu might hold.", "x"] {
            let out = pseudolocalize(text);
            let (before, after) = (text.chars().count(), out.chars().count());
            assert!(after * EXPANSION_DENOMINATOR >= before * (EXPANSION_DENOMINATOR + EXPANSION_NUMERATOR));
            assert_ne!(out, text);
        }
        assert_eq!(pseudolocalize("Save"), "Šååvéé");
        // Kana keep their characters, and still grow.
        assert_eq!(pseudolocalize("セーブ"), "セーブ~~");
    }

    #[test]
    fn the_frame_tells_a_message_from_anything_else() {
        assert!(is_framed("[!! Šåvé !!]"));
        for text in ["Save", "[!! Šåvé", "Šåvé !!]", "[Save]", ""] {
            assert!(!is_framed(text), "{text}");
        }
    }

    #[test]
    fn a_tables_messages_are_framed_and_its_terms_and_keys_arent() {
        let pool = "title-brandBrandYou $n".to_owned();
        let span = |start, len| Span { start, len };
        let table = StringTable {
            locale: "en".to_owned(),
            pool,
            entries: vec![
                Entry { key: span(5, 6), pattern: Pattern::Plain(span(11, 5)) },
                Entry { key: span(0, 5), pattern: Pattern::Parts(Run { start: 0, len: 2 }) },
            ],
            parts: vec![Part::Text(span(16, 4)), Part::Place(Expression::Variable(span(21, 1)))],
            ..StringTable::default()
        };
        let pseudo = table.pseudolocalized();
        let read = StringTable::read(&pseudo.to_cooked().unwrap()).unwrap();
        assert_eq!(read.str(read.entries[0].key), "-brand");
        let Pattern::Plain(term) = read.entries[0].pattern else { panic!("a plain term stays plain") };
        assert!(!is_framed(read.str(term)));
        let Pattern::Parts(run) = read.entries[1].pattern else { panic!("a message with parts keeps them") };
        let parts = read.parts_of(run);
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], Part::Text(read_span(&read, FRAME_OPEN)));
        assert_eq!(parts[3], Part::Text(read_span(&read, FRAME_CLOSE)));
    }

    fn read_span(table: &StringTable, text: &str) -> Span {
        Span { start: table.pool.find(text).unwrap() as u32, len: text.len() as u32 }
    }

    #[test]
    fn its_characters_cover_what_it_makes_of_english() {
        let characters = characters();
        let made = format!("{FRAME_OPEN}{}{FRAME_CLOSE}", pseudolocalize("The quick brown fox JUMPS over"));
        for letter in made.chars().filter(|letter| *letter != ' ') {
            assert!(characters.contains(&letter), "{letter}");
        }
    }
}
