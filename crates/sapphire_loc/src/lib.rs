//! Runs localized strings (M3 V2, spec §5.25): an evaluator of our own over cooked tables, since
//! `fluent`'s bundles can't be cooked (`FluentResource::try_new` takes a `String`).
//!
//! - **Owns:** [`Localizer`], which holds [`StringTable`]s and answers keys in the current text
//!   locale: variables, plural selectors, numbers, message and term references and attributes, with
//!   plurals and numbers through ICU4X. The live language switch, with no restart. [`LocaleSettings`],
//!   the text language and the voice language apart (spec §5.15), plain data for `sapphire_save` to
//!   hold. Per-locale font family chains, as names ([`Localizer::font_chain`]), which `sapphire_text`
//!   takes as family lists. A pseudo-localization switch ([`Localizer::set_pseudo`]).
//! - **Does not own:** the table's format and pseudo-localized form (`sapphire_strings`), `.ftl`
//!   sources and every check across locales (`sapphire_pipeline`), packs (an app reads a table's
//!   bytes through `sapphire_assets` and hands over a [`StringTable`]), fonts, or which voice lines
//!   play (`sapphire_audio`, which reads [`Localizer::voice_locale`]).
//!
//! Dates wait for a menu that shows one (M3 plan §3), since `icu_datetime`'s data is 18.75 MB of source.
//!
//! # What a key gives
//!
//! ```
//! use sapphire_loc::{LocaleSettings, Localizer};
//!
//! let loc = Localizer::new(LocaleSettings::default());
//! // No table holds it: the key itself shows, and a stat counts it. Nothing panics.
//! assert_eq!(loc.text("menu-title"), "menu-title");
//! assert_eq!(loc.stats().unknown_keys, 1);
//! ```
//!
//! - [`Localizer::text`] gives a message with no placeables as a `&str`, with no allocation.
//! - [`Localizer::format`] appends any message to a caller's buffer.
//! - [`Localizer::resolve`] does whichever the message needs, for code that doesn't know, such as a
//!   bound UI node. Anything showing text keeps [`Localizer::generation`] and resolves again when it
//!   changes: that's the whole of a language switch to it.
//!
//! A key no table of the chain holds comes back as itself. A variable the caller didn't pass shows
//! as `{$name}`, and a reference that doesn't resolve as `{key}`. Each counts in [`LocStats`] (R9),
//! and a cook refuses any table where a reference wouldn't resolve, so in cooked content only a caller
//! can bring them on.
//!
//! # Bidi isolation marks
//!
//! Fluent's bundles wrap each placeable in U+2068 and U+2069 by default. This doesn't: English and
//! Japanese are both left to right, so a placeable can't reorder its neighbours, and the marks are
//! two more characters a font subset, a shaper and every string comparison would meet for nothing. An
//! RTL locale would make it a table's setting, cooked with the locale, and that's when to add one.
//!
//! # Fallback
//!
//! A table states its chain (`ja` to `en`), and a key it lacks is looked for along it. What's found
//! runs by its own table's rules and looks further only down the chain from there: an English message
//! standing in for a Japanese one takes English plurals and English terms, rather than a sentence of
//! both. A cook lists every key this serves, so what's untranslated is visible.
//!
//! # Cost
//!
//! `tests/costs.rs` prints a lookup's and a format's, and spec §5.25 has what was measured. A lookup
//! doesn't allocate (`tests/allocations.rs`), and a formatted string doesn't once its buffer has
//! grown, for numbers of up to 8 digits: past that `fixed_decimal` holds its digits on the heap.

mod evaluate;
mod localizer;

pub use localizer::{Localizer, UnknownLocale};
pub use sapphire_strings::{StringTable, StringsError, is_framed};
use serde::{Deserialize, Serialize};

/// The languages a player chose, kept apart because people mix them: Japanese voices under English
/// menus. `sapphire_save` keeps this in settings; it's plain data, so that needs no edge here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocaleSettings {
    /// The BCP 47 tag menus and subtitles read in.
    pub text: String,
    /// The BCP 47 tag voices speak in. Whether a game has that voice is the audio route's to know.
    pub voice: String,
}

impl Default for LocaleSettings {
    fn default() -> Self {
        Self { text: "en".to_owned(), voice: "en".to_owned() }
    }
}

/// A value a message is formatted with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    Text(&'a str),
    Integer(i64),
    Float(f64),
}

impl<'a> From<&'a str> for Value<'a> {
    fn from(text: &'a str) -> Self {
        Self::Text(text)
    }
}

impl From<i64> for Value<'_> {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<i32> for Value<'_> {
    fn from(value: i32) -> Self {
        Self::Integer(value.into())
    }
}

impl From<u32> for Value<'_> {
    fn from(value: u32) -> Self {
        Self::Integer(value.into())
    }
}

impl From<f64> for Value<'_> {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

/// A named value: `("count", 3.into())`. A short list searched in order, so none needs allocating.
pub type Arg<'a> = (&'a str, Value<'a>);

/// What the localizer has done and met (rule R9). No row of spec §3.2 budgets it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocStats {
    /// Calls of [`Localizer::text`], [`Localizer::format`] and [`Localizer::resolve`], all told.
    pub lookups: u64,
    /// Keys no table of the chain holds, and references that didn't resolve. They show as themselves.
    pub unknown_keys: u64,
    /// Variables a message used and its caller didn't pass. They show as `{$name}`.
    pub missing_args: u64,
    /// [`Localizer::text`] asked for a message that has placeables. Its key shows: it wants
    /// [`Localizer::resolve`].
    pub not_plain: u64,
    /// Formats stopped at `MAX_DEPTH` or `MAX_STEPS`, showing what they had: a table no cook made.
    pub overruns: u64,
    /// Numbers that aren't finite or are too large to write. They show as `NaN`.
    pub bad_numbers: u64,
    /// Language switches.
    pub switches: u64,
    /// Tables held.
    pub tables: u64,
    /// What they hold on the heap, near enough, with their pseudo-localized forms.
    pub table_bytes: u64,
}
