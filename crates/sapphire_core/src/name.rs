//! [`Name`]: a 64-bit string ID (spec §5.3).
//!
//! Names replace strings wherever the engine compares identifiers at runtime: animation events,
//! move IDs, sockets, cue keys. They hash with XXH3-64, so the same text yields the same ID in
//! `const` context, at runtime, in tools and on every platform.
//!
//! Debug builds keep a reverse table (hash → text) for [`fmt::Display`] and [`fmt::Debug`] and
//! panic on a hash collision. A `const fn` cannot write to that table, so [`Name::new`] does not
//! register its text; [`Name::intern`] does. A name made only with [`Name::new`] therefore prints
//! as `Name(0x…)` until the same text is interned somewhere, typically by the data that refers to
//! it. Release builds have no table and always print the hash.

use std::fmt;

/// A hashed string identifier. Equality, ordering and hashing all use the 64-bit hash only.
///
/// ```
/// use sapphire_core::Name;
///
/// const ATTACK: Name = Name::new("attack");
/// assert_eq!(Name::intern("attack"), ATTACK);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Name(u64);

impl Name {
    /// Hashes `text` at compile time or runtime. Does not record the text for debug printing.
    #[must_use]
    pub const fn new(text: &str) -> Self {
        Self(xxhash_rust::const_xxh3::xxh3_64(text.as_bytes()))
    }

    /// Hashes `text` and, in debug builds, records it so the name prints as text.
    ///
    /// Use this for names built from data at load time. It takes a lock in debug builds, so keep
    /// it out of per-frame code.
    ///
    /// # Panics
    ///
    /// In debug builds, if a different text was already interned with the same hash.
    #[must_use]
    pub fn intern(text: &str) -> Self {
        // The runtime XXH3 is faster than the const version and produces identical hashes.
        let name = Self(xxhash_rust::xxh3::xxh3_64(text.as_bytes()));
        #[cfg(debug_assertions)]
        debug_table::insert(name.0, text);
        name
    }

    /// Rebuilds a name from a stored hash, for example when deserializing.
    #[must_use]
    pub const fn from_u64(hash: u64) -> Self {
        Self(hash)
    }

    /// The 64-bit hash.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl From<Name> for u64 {
    fn from(name: Name) -> Self {
        name.0
    }
}

impl fmt::Display for Name {
    /// The interned text in debug builds when known; otherwise `Name(0x…)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(debug_assertions)]
        if let Some(result) = debug_table::with_text(self.0, |text| f.write_str(text)) {
            return result;
        }
        write!(f, "Name({:#018x})", self.0)
    }
}

impl fmt::Debug for Name {
    /// `Name("text")` in debug builds when known; otherwise `Name(0x…)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(debug_assertions)]
        if let Some(result) = debug_table::with_text(self.0, |text| write!(f, "Name({text:?})")) {
            return result;
        }
        write!(f, "Name({:#018x})", self.0)
    }
}

#[cfg(debug_assertions)]
mod debug_table {
    use std::collections::HashMap;
    use std::sync::{OnceLock, PoisonError, RwLock};

    type Table = RwLock<HashMap<u64, Box<str>>>;

    fn table() -> &'static Table {
        static TABLE: OnceLock<Table> = OnceLock::new();
        TABLE.get_or_init(Table::default)
    }

    pub(super) fn insert(hash: u64, text: &str) {
        let existing = {
            // A panic elsewhere while holding the lock cannot corrupt a map of plain strings.
            let mut map = table().write().unwrap_or_else(PoisonError::into_inner);
            match map.get(&hash) {
                Some(existing) if &**existing != text => existing.clone(),
                Some(_) => return,
                None => {
                    map.insert(hash, text.into());
                    return;
                }
            }
        };
        panic!("Name hash collision: {existing:?} and {text:?} both hash to {hash:#018x}; rename one");
    }

    /// Runs `f` with the interned text, or returns `None` if `hash` was never interned.
    pub(super) fn with_text<R>(hash: u64, f: impl FnOnce(&str) -> R) -> Option<R> {
        let map = table().read().unwrap_or_else(PoisonError::into_inner);
        map.get(&hash).map(|text| f(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ATTACK: Name = Name::new("attack");

    #[test]
    fn const_and_runtime_hashes_agree() {
        // Lengths straddle every XXH3 code path boundary (0, 1–3, 4–8, 9–16, 17–128, 129–240, 241+).
        for len in [0, 1, 3, 4, 8, 9, 16, 17, 128, 129, 240, 241, 1000] {
            let text: String = (0..len).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
            assert_eq!(Name::new(&text), Name::intern(&text), "length {len}");
        }
    }

    #[test]
    fn hash_is_stable() {
        // XXH3-64 with seed 0 of the empty input; a change here breaks every cooked asset.
        assert_eq!(Name::new("").as_u64(), 0x2d06_8005_38d3_94c2);
        assert_eq!(Name::from_u64(ATTACK.as_u64()), ATTACK);
        assert_eq!(u64::from(ATTACK), ATTACK.as_u64());
    }

    #[test]
    fn distinct_texts_differ() {
        assert_ne!(Name::new("attack"), Name::new("Attack"));
        assert_ne!(Name::new("attack"), Name::new("attack "));
    }

    #[test]
    fn unknown_names_print_their_hash() {
        let name = Name::new("never interned anywhere in this test binary");
        let expected = format!("Name({:#018x})", name.as_u64());
        assert_eq!(name.to_string(), expected);
        assert_eq!(format!("{name:?}"), expected);
    }

    #[test]
    #[cfg(debug_assertions)]
    fn interned_names_print_their_text() {
        let name = Name::intern("dodge");
        assert_eq!(name.to_string(), "dodge");
        assert_eq!(format!("{name:?}"), r#"Name("dodge")"#);
        // A const name prints as text once the same text has been interned.
        const DODGE: Name = Name::new("dodge");
        assert_eq!(DODGE.to_string(), "dodge");
    }

    #[test]
    #[cfg(debug_assertions)]
    fn collisions_panic_in_debug() {
        debug_table::insert(0xdead_beef, "first");
        debug_table::insert(0xdead_beef, "first");
        let result = std::panic::catch_unwind(|| debug_table::insert(0xdead_beef, "second"));
        assert!(result.is_err());
    }
}
