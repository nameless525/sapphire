//! Localized strings as plain data (M3 V2, spec §5.25): the cooked `.sloc` table, its checks, the
//! characters it can show and its pseudo-localized form.
//!
//! - **Owns:** [`StringTable`], one locale's messages, terms and attributes as flat patterns, with
//!   the locale's fallback chain and font family names. It's what the cooker writes, what packs
//!   check ([`StringTable::read`]) and what `sapphire_loc` runs.
//! - **Does not own:** parsing `.ftl` sources and the checks across locales (`sapphire_pipeline`),
//!   running a pattern, plurals and numbers (`sapphire_loc`, which links ICU4X; nothing here does, so
//!   a pack or a cook doesn't), or fonts themselves (`sapphire_text`, which takes the family names).
//!
//! Strings never enter the simulation: nothing in `[simulation]` reaches this crate.
//!
//! # The format
//!
//! `postcard` after the header `sapphire_serialize::save` writes, so the bytes carry the type's name
//! and version and a length, and an older version is refused rather than upgraded: cook again. It
//! decodes to owned vectors, flat ones, so a hostile file can't nest its way down the stack, and every
//! index is checked before [`StringTable::read`] returns one. Reading it isn't in place as rkyv's
//! formats are, because a language switch is rare and a table is small. What a pattern can still do
//! when its references are read, loop or fan out, is bounded where it runs ([`MAX_DEPTH`],
//! [`MAX_STEPS`]), and a cook refuses any pattern that would reach those bounds.

mod pseudo;
mod table;

pub use pseudo::{
    EXPANSION_DENOMINATOR, EXPANSION_NUMERATOR, FRAME_CLOSE, FRAME_OPEN, characters as pseudo_characters,
    is_framed, pseudolocalize,
};
pub use table::{
    ArgValue, Entry, Expression, FontChain, MAX_DEPTH, MAX_FRACTION_DIGITS, MAX_STEPS, NUMBER_CHARACTERS,
    NumberFormat, Part, Pattern, Run, STRINGS_EXTENSION, Select, Span, StringTable, StringsError, TermArg,
    Variant, VariantKey,
};
