//! The table: one locale's strings, flat. See the [crate docs](crate).

use std::collections::BTreeSet;

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::{SaveError, from_save, to_save};
use serde::{Deserialize, Serialize};

/// Conventional file extension for cooked tables.
pub const STRINGS_EXTENSION: &str = "sloc";

/// How deep a format may go through references and selects: past it, it stops and says so. A cook
/// refuses a pattern that would.
pub const MAX_DEPTH: u32 = 32;

/// How many patterns and parts one format may visit, which is what stops a small hostile table
/// fanning out to billions (`a = {b}{b}`, `b = {c}{c}`, and so on). A cook refuses a pattern that
/// would reach it.
pub const MAX_STEPS: u32 = 4096;

/// The most fraction digits a number may ask for. ICU4X takes far more, and nobody reads them.
pub const MAX_FRACTION_DIGITS: u8 = 20;

/// What the numbers of every locale we ship are written in, which [`StringTable::characters`] adds to
/// a table's own: a font subset has to hold them though no string does. `sapphire_loc`'s tests hold
/// English and Japanese to it; a locale whose numbers use others adds them here.
pub const NUMBER_CHARACTERS: &str = "0123456789,.-";

/// A piece of [`StringTable::pool`], in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub len: u32,
}

/// A piece of one of the table's lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub start: u32,
    pub len: u32,
}

impl Run {
    fn range(self) -> std::ops::Range<usize> {
        self.start as usize..(self.start as usize).saturating_add(self.len as usize)
    }
}

/// One message, term or attribute of a table.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// `id` for a message, `id.attribute` for its attribute, and `-id` and `-id.attribute` for a term.
    pub key: Span,
    pub pattern: Pattern,
}

/// What a message says.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Pattern {
    /// Text with nothing to work out: what a lookup returns with no allocation.
    Plain(Span),
    /// Parts of [`StringTable::parts`], in order.
    Parts(Run),
}

/// A piece of a [`Pattern::Parts`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Part {
    Text(Span),
    Place(Expression),
    /// An index into [`StringTable::selects`].
    Select(u32),
}

/// A value a part shows or a select chooses by.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Expression {
    /// `$name`, shown as it comes.
    Variable(Span),
    /// `NUMBER($name, …)`.
    NumberVariable { name: Span, format: NumberFormat },
    /// A number in the source: `{ 1000 }`.
    Number { value: f64, format: NumberFormat },
    /// A message or a message's attribute, by key, with the caller's variables.
    Message(Span),
    /// A term or a term's attribute, by key, with only the arguments it's given here: a [`Run`] of
    /// [`StringTable::args`].
    Term { key: Span, args: Run },
}

/// How a number is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumberFormat {
    pub min_fraction: u8,
    pub max_fraction: u8,
    pub grouping: bool,
}

impl Default for NumberFormat {
    /// As Fluent's `NUMBER` defaults: grouped, with at most three fraction digits.
    fn default() -> Self {
        Self { min_fraction: 0, max_fraction: 3, grouping: true }
    }
}

/// `{ selector -> [key] pattern *[other] pattern }`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Select {
    pub selector: Expression,
    /// A [`Run`] of [`StringTable::variants`], never empty.
    pub variants: Run,
    /// Which of them, counted from the run's start, is `*`.
    pub default: u32,
}

/// One arm of a [`Select`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub key: VariantKey,
    pub pattern: Pattern,
}

/// What a [`Variant`] matches.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum VariantKey {
    /// A plural category for a number, and the same text for a string.
    Name(Span),
    /// A number exactly: `[0]`.
    Number(f64),
}

/// An argument given to a term: `-realm(case: "inner")`. Fluent lets a term be given only literals.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TermArg {
    pub name: Span,
    pub value: ArgValue,
}

/// See [`TermArg`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ArgValue {
    Text(Span),
    Number { value: f64, format: NumberFormat },
}

/// The font family names a locale shows one role of its text in, first choice first, as
/// `sapphire_text` takes family lists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontChain {
    /// What the text is for: `body`, `display`. A role is only a name the UI's documents use.
    pub role: String,
    pub families: Vec<String>,
}

/// One locale's strings. See the [crate docs](crate).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::strings"]
pub struct StringTable {
    /// The locale's BCP 47 tag.
    pub locale: String,
    /// Where a key this table lacks is looked for, in order: the chain itself, not a first step of one.
    pub fallback: Vec<String>,
    pub fonts: Vec<FontChain>,
    /// All the table's text, keys and literals together.
    pub pool: String,
    /// Ascending by key, which is how a lookup finds one, with no repeats.
    pub entries: Vec<Entry>,
    pub parts: Vec<Part>,
    pub selects: Vec<Select>,
    pub variants: Vec<Variant>,
    pub args: Vec<TermArg>,
}

impl Versioned for StringTable {
    const VERSION: u32 = 1;
}

/// Bytes the runtime would refuse.
#[derive(Debug, thiserror::Error)]
pub enum StringsError {
    #[error("not a string table")]
    Format(#[from] SaveError),
    #[error("{what} {index} is out of bounds")]
    OutOfBounds { what: &'static str, index: usize },
    #[error("entry {index} is out of key order, or repeats the key before it")]
    KeyOrder { index: usize },
    #[error("the table names no locale")]
    NoLocale,
    #[error("select {index} has no variants, or its default isn't one")]
    Select { index: usize },
    #[error("a number format asks for {min}..{max} fraction digits")]
    NumberFormat { min: u8, max: u8 },
    #[error("a number isn't finite")]
    NotFinite,
}

impl StringTable {
    /// The cooked bytes, as [`Self::read`] reads them.
    pub fn to_cooked(&self) -> Result<Vec<u8>, StringsError> {
        Ok(to_save(self)?)
    }

    /// Reads cooked bytes and checks everything the table points at, so a lookup or a walk of what
    /// comes back can't index out of bounds or split a character, whatever the bytes held.
    pub fn read(bytes: &[u8]) -> Result<Self, StringsError> {
        let table: Self = from_save(bytes)?;
        table.validate()?;
        Ok(table)
    }

    fn validate(&self) -> Result<(), StringsError> {
        if self.locale.is_empty() {
            return Err(StringsError::NoLocale);
        }
        for (index, entry) in self.entries.iter().enumerate() {
            self.check_span("entry key", index, entry.key)?;
            self.check_pattern("entry", index, entry.pattern)?;
            if index > 0 && self.str(self.entries[index - 1].key) >= self.str(entry.key) {
                return Err(StringsError::KeyOrder { index });
            }
        }
        for (index, part) in self.parts.iter().enumerate() {
            match *part {
                Part::Text(span) => self.check_span("part", index, span)?,
                Part::Place(expression) => self.check_expression(index, expression)?,
                Part::Select(select) => {
                    if select as usize >= self.selects.len() {
                        return Err(StringsError::OutOfBounds { what: "part", index });
                    }
                }
            }
        }
        for (index, select) in self.selects.iter().enumerate() {
            self.check_expression(index, select.selector)?;
            let known = self.variants.get(select.variants.range()).is_some();
            if !known || select.default >= select.variants.len {
                return Err(StringsError::Select { index });
            }
        }
        for (index, variant) in self.variants.iter().enumerate() {
            match variant.key {
                VariantKey::Name(span) => self.check_span("variant key", index, span)?,
                VariantKey::Number(value) if !value.is_finite() => return Err(StringsError::NotFinite),
                VariantKey::Number(_) => {}
            }
            self.check_pattern("variant", index, variant.pattern)?;
        }
        for (index, arg) in self.args.iter().enumerate() {
            self.check_span("argument", index, arg.name)?;
            match arg.value {
                ArgValue::Text(span) => self.check_span("argument", index, span)?,
                ArgValue::Number { value, format } => check_number(value, format)?,
            }
        }
        Ok(())
    }

    fn check_span(&self, what: &'static str, index: usize, span: Span) -> Result<(), StringsError> {
        let end = (span.start as usize).saturating_add(span.len as usize);
        if self.pool.get(span.start as usize..end).is_none() {
            return Err(StringsError::OutOfBounds { what, index });
        }
        Ok(())
    }

    fn check_pattern(&self, what: &'static str, index: usize, pattern: Pattern) -> Result<(), StringsError> {
        match pattern {
            Pattern::Plain(span) => self.check_span(what, index, span),
            Pattern::Parts(run) if self.parts.get(run.range()).is_none() => {
                Err(StringsError::OutOfBounds { what, index })
            }
            Pattern::Parts(_) => Ok(()),
        }
    }

    fn check_expression(&self, index: usize, expression: Expression) -> Result<(), StringsError> {
        match expression {
            Expression::Variable(span) | Expression::Message(span) => {
                self.check_span("expression", index, span)
            }
            Expression::NumberVariable { name, format } => {
                check_format(format)?;
                self.check_span("expression", index, name)
            }
            Expression::Number { value, format } => check_number(value, format),
            Expression::Term { key, args } => {
                if self.args.get(args.range()).is_none() {
                    return Err(StringsError::OutOfBounds { what: "term arguments", index });
                }
                self.check_span("expression", index, key)
            }
        }
    }

    /// The text of `span`. A table from [`Self::read`] holds no span this can't serve; a table still
    /// being built gets nothing rather than a panic.
    pub fn str(&self, span: Span) -> &str {
        let end = (span.start as usize).saturating_add(span.len as usize);
        self.pool.get(span.start as usize..end).unwrap_or_default()
    }

    /// The parts of `run`.
    pub fn parts_of(&self, run: Run) -> &[Part] {
        self.parts.get(run.range()).unwrap_or_default()
    }

    /// The variants of `run`.
    pub fn variants_of(&self, run: Run) -> &[Variant] {
        self.variants.get(run.range()).unwrap_or_default()
    }

    /// The arguments of `run`.
    pub fn args_of(&self, run: Run) -> &[TermArg] {
        self.args.get(run.range()).unwrap_or_default()
    }

    /// The entry of `key`, by binary search.
    pub fn entry(&self, key: &str) -> Option<&Entry> {
        let found = self.entries.binary_search_by(|entry| self.str(entry.key).cmp(key)).ok()?;
        self.entries.get(found)
    }

    /// The font family names of `role`, first choice first.
    pub fn font_chain(&self, role: &str) -> Option<&[String]> {
        self.fonts.iter().find(|chain| chain.role == role).map(|chain| chain.families.as_slice())
    }

    /// Every character this table can show: its literals, and what numbers are written in
    /// ([`NUMBER_CHARACTERS`]). Not keys or variable names, which only show when something's wrong, nor
    /// what a caller's variables hold, such as a name a player typed. For the font cook (M3 U3), which
    /// subsets a pack's fonts to its tables.
    pub fn characters(&self) -> BTreeSet<char> {
        let mut characters: BTreeSet<char> = NUMBER_CHARACTERS.chars().collect();
        let mut add = |span: Span| characters.extend(self.str(span).chars());
        let add_pattern = |pattern: Pattern, add: &mut dyn FnMut(Span)| {
            if let Pattern::Plain(span) = pattern {
                add(span);
            }
        };
        for entry in &self.entries {
            add_pattern(entry.pattern, &mut add);
        }
        for variant in &self.variants {
            add_pattern(variant.pattern, &mut add);
        }
        for part in &self.parts {
            if let Part::Text(span) = *part {
                add(span);
            }
        }
        for arg in &self.args {
            if let ArgValue::Text(span) = arg.value {
                add(span);
            }
        }
        characters
    }

    /// What the table holds on the heap, near enough, for a stat.
    pub fn heap_bytes(&self) -> usize {
        use std::mem::size_of;
        self.pool.capacity()
            + self.entries.capacity() * size_of::<Entry>()
            + self.parts.capacity() * size_of::<Part>()
            + self.selects.capacity() * size_of::<Select>()
            + self.variants.capacity() * size_of::<Variant>()
            + self.args.capacity() * size_of::<TermArg>()
    }
}

fn check_format(format: NumberFormat) -> Result<(), StringsError> {
    if format.min_fraction > format.max_fraction || format.max_fraction > MAX_FRACTION_DIGITS {
        return Err(StringsError::NumberFormat { min: format.min_fraction, max: format.max_fraction });
    }
    Ok(())
}

fn check_number(value: f64, format: NumberFormat) -> Result<(), StringsError> {
    if !value.is_finite() {
        return Err(StringsError::NotFinite);
    }
    check_format(format)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `count = { $n -> [one] one *[other] many }`, `plain = hi`, and `-term = { $n }`, through every
    /// list a table has.
    fn table() -> StringTable {
        let pool = "count$nonemanyplainhi-termcasein".to_owned();
        let span = |text: &str| Span { start: pool.find(text).unwrap() as u32, len: text.len() as u32 };
        let variable = span("$n");
        let n = Span { start: variable.start + 1, len: 1 };
        StringTable {
            locale: "en".to_owned(),
            fallback: vec![],
            fonts: vec![FontChain { role: "body".to_owned(), families: vec!["M PLUS 1".to_owned()] }],
            entries: vec![
                Entry { key: span("-term"), pattern: Pattern::Parts(Run { start: 1, len: 1 }) },
                Entry { key: span("count"), pattern: Pattern::Parts(Run { start: 0, len: 1 }) },
                Entry { key: span("plain"), pattern: Pattern::Plain(span("hi")) },
            ],
            parts: vec![
                Part::Select(0),
                Part::Place(Expression::Variable(n)),
                Part::Place(Expression::Term { key: span("-term"), args: Run { start: 0, len: 1 } }),
            ],
            selects: vec![Select {
                selector: Expression::Variable(n),
                variants: Run { start: 0, len: 2 },
                default: 1,
            }],
            variants: vec![
                Variant { key: VariantKey::Name(span("one")), pattern: Pattern::Plain(span("one")) },
                Variant { key: VariantKey::Number(2.0), pattern: Pattern::Plain(span("many")) },
            ],
            args: vec![TermArg { name: span("case"), value: ArgValue::Text(span("in")) }],
            pool,
        }
    }

    #[test]
    fn a_table_reads_back_and_finds_its_keys() {
        let table = table();
        let read = StringTable::read(&table.to_cooked().unwrap()).unwrap();
        assert_eq!(read, table);
        assert_eq!(read.entry("plain").map(|entry| entry.pattern), Some(table.entries[2].pattern));
        assert_eq!(read.entry("count").map(|entry| entry.key), Some(table.entries[1].key));
        assert!(read.entry("nothing").is_none());
        assert_eq!(read.font_chain("body").unwrap(), ["M PLUS 1"]);
    }

    /// Every way of spoiling a table's parts, each of which must be refused, since it's read from a
    /// pack. They're tables spoiled before writing, so each gets past the header and postcard.
    #[test]
    fn a_table_pointing_outside_itself_is_refused() {
        let huge = Span { start: u32::MAX, len: u32::MAX };
        let huge_run = Run { start: u32::MAX, len: u32::MAX };
        type Spoil = fn(&mut StringTable, Span, Run);
        let cases: Vec<(&str, Spoil)> = vec![
            ("no locale", |table, _, _| table.locale.clear()),
            ("entry key", |table, span, _| table.entries[0].key = span),
            ("plain pattern", |table, span, _| table.entries[2].pattern = Pattern::Plain(span)),
            ("parts pattern", |table, _, run| table.entries[0].pattern = Pattern::Parts(run)),
            ("key order", |table, _, _| table.entries.swap(0, 1)),
            ("a repeated key", |table, _, _| table.entries[1].key = table.entries[0].key),
            ("text part", |table, span, _| table.parts[1] = Part::Text(span)),
            ("variable", |table, span, _| table.parts[1] = Part::Place(Expression::Variable(span))),
            ("message", |table, span, _| table.parts[1] = Part::Place(Expression::Message(span))),
            ("select index", |table, _, _| table.parts[0] = Part::Select(9)),
            ("variants", |table, _, run| table.selects[0].variants = run),
            ("no variants", |table, _, _| {
                table.selects[0].variants.len = 0;
                table.selects[0].default = 0;
            }),
            ("default", |table, _, _| table.selects[0].default = 2),
            ("selector", |table, span, _| table.selects[0].selector = Expression::Variable(span)),
            ("variant key", |table, span, _| table.variants[0].key = VariantKey::Name(span)),
            ("variant number", |table, _, _| table.variants[1].key = VariantKey::Number(f64::NAN)),
            ("variant pattern", |table, _, run| table.variants[0].pattern = Pattern::Parts(run)),
            ("term key", |table, span, _| {
                table.parts[2] = Part::Place(Expression::Term { key: span, args: Run::default() });
            }),
            ("term args", |table, _, run| {
                let key = table.entries[0].key;
                table.parts[2] = Part::Place(Expression::Term { key, args: run });
            }),
            ("arg name", |table, span, _| table.args[0].name = span),
            ("arg text", |table, span, _| table.args[0].value = ArgValue::Text(span)),
            ("arg number", |table, _, _| {
                let format = NumberFormat::default();
                table.args[0].value = ArgValue::Number { value: f64::INFINITY, format };
            }),
            ("number format order", |table, _, _| {
                let format = NumberFormat { min_fraction: 3, max_fraction: 1, grouping: true };
                table.parts[1] = Part::Place(Expression::Number { value: 1.0, format });
            }),
            ("number format size", |table, _, _| {
                let format = NumberFormat { min_fraction: 0, max_fraction: 200, grouping: true };
                table.parts[1] = Part::Place(Expression::Number { value: 1.0, format });
            }),
            ("number", |table, _, _| {
                let format = NumberFormat::default();
                table.parts[1] = Part::Place(Expression::Number { value: f64::NAN, format });
            }),
        ];
        let mut read_anyway = Vec::new();
        for (name, spoil) in cases {
            let mut spoiled = table();
            spoil(&mut spoiled, huge, huge_run);
            if StringTable::read(&spoiled.to_cooked().unwrap()).is_ok() {
                read_anyway.push(name);
            }
        }
        assert_eq!(read_anyway, Vec::<&str>::new());
    }

    #[test]
    fn a_span_splitting_a_character_is_refused() {
        let mut table = table();
        table.pool.push('語');
        let start = table.pool.len() as u32 - 2;
        table.entries[2].pattern = Pattern::Plain(Span { start, len: 1 });
        assert!(matches!(
            StringTable::read(&table.to_cooked().unwrap()),
            Err(StringsError::OutOfBounds { what: "entry", index: 2 })
        ));
    }

    /// Whatever the bytes, reading returns: truncated at every length, a byte changed at every place,
    /// and lengths claiming far more than the file holds.
    #[test]
    fn hostile_bytes_never_panic() {
        let good = table().to_cooked().unwrap();
        for len in 0..good.len() {
            assert!(StringTable::read(&good[..len]).is_err(), "truncated to {len}");
        }
        let mut accepted = 0;
        for at in 0..good.len() {
            for flip in [0x01, 0x80, 0xff] {
                let mut bytes = good.clone();
                bytes[at] ^= flip;
                if let Ok(read) = StringTable::read(&bytes) {
                    // What gets through is a table every index of which holds.
                    for entry in &read.entries {
                        let _ = read.str(entry.key);
                    }
                    accepted += 1;
                }
            }
        }
        // Text bytes changed for other text, mostly: not a check of the format's strength, only that the
        // loop above read something.
        assert!(accepted > 0);

        let mut trailing = good.clone();
        trailing.push(0);
        assert!(StringTable::read(&trailing).is_err(), "a byte past the payload");
        assert!(StringTable::read(&[0xff; 4096]).is_err());

        // Every list's length claiming about 4 GiB, past the header: refused for want of bytes, without
        // trying to hold them first.
        let mut claims = good[..32].to_vec();
        claims.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x0f]);
        claims.extend_from_slice(&[0; 64]);
        let length = claims.len() as u64 - 32;
        claims[24..32].copy_from_slice(&length.to_le_bytes());
        assert!(StringTable::read(&claims).is_err());
    }

    #[test]
    fn characters_are_what_shows_and_what_numbers_need() {
        let characters = table().characters();
        for shown in ['h', 'i', 'o', 'n', 'e', 'm', 'a', 'y', '0', '9', ',', '.', '-'] {
            assert!(characters.contains(&shown), "{shown}");
        }
        // Only in keys and names.
        for hidden in ['c', 'u', 't', 'p', 'l', '$'] {
            assert!(!characters.contains(&hidden), "{hidden}");
        }
    }
}
