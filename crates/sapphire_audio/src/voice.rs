//! [`VoiceRoute`]: which language's line a key plays (M3 A1).
//!
//! The voice language is a setting of its own (`sapphire_loc`'s `LocaleSettings::voice`): a player
//! can hear Japanese and read English. A game asks for a line by its key as a [`LineKey`], hashed once
//! where the key is read, so a frame's lookup compares numbers and allocates nothing.

use std::collections::HashMap;

use sapphire_assets::AssetId;
use sapphire_assets::voice::VoiceLines;

/// A line's key, hashed: what a [`LineRequest`](crate::LineRequest) carries. Two keys of one table
/// that hash alike are refused when it's added, so the hash stands for the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineKey(u64);

impl LineKey {
    /// The key `key` (FNV-1a, 64 bits: the same on every machine and in every run).
    pub const fn of(key: &str) -> Self {
        Self(fnv1a(key.as_bytes()))
    }
}

/// FNV-1a over `bytes`.
pub(crate) const fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut index = 0;
    while index < bytes.len() {
        hash = (hash ^ bytes[index] as u64).wrapping_mul(0x0100_0000_01b3);
        index += 1;
    }
    hash
}

/// Voice lines that can't join a route.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum VoiceRouteError {
    #[error("two tables speak {0:?}")]
    TwoTables(String),
    #[error("the {locale:?} lines `{first}` and `{second}` hash alike: rename one")]
    KeysCollide { locale: String, first: String, second: String },
}

/// One language's lines.
#[derive(Debug)]
struct Language {
    locale: String,
    lines: HashMap<LineKey, AssetId>,
}

/// Every language's voice lines, and the one the player chose. A key the chosen language lacks plays
/// nothing: a missing line is the localizers' to fill, not another language's.
#[derive(Debug, Default)]
pub struct VoiceRoute {
    languages: Vec<Language>,
    /// The language asked for, whether or not a table speaks it yet.
    wanted: String,
    /// Its table in [`Self::languages`].
    chosen: Option<usize>,
}

impl VoiceRoute {
    /// No lines, in no language.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one language's lines. Load-time work.
    pub fn add(&mut self, lines: &VoiceLines) -> Result<(), VoiceRouteError> {
        if self.languages.iter().any(|language| language.locale == lines.locale) {
            return Err(VoiceRouteError::TwoTables(lines.locale.clone()));
        }
        let mut keys: HashMap<LineKey, &str> = HashMap::with_capacity(lines.lines.len());
        for line in &lines.lines {
            if let Some(first) = keys.insert(LineKey::of(&line.key), &line.key) {
                return Err(VoiceRouteError::KeysCollide {
                    locale: lines.locale.clone(),
                    first: first.to_owned(),
                    second: line.key.clone(),
                });
            }
        }
        let lines_by_key = lines.lines.iter().map(|line| (LineKey::of(&line.key), line.cue)).collect();
        self.languages.push(Language { locale: lines.locale.clone(), lines: lines_by_key });
        self.choose();
        Ok(())
    }

    /// Chooses the language lines are spoken in, a BCP 47 tag; whether a table speaks it.
    pub fn set_language(&mut self, locale: &str) -> bool {
        if self.wanted != locale {
            locale.clone_into(&mut self.wanted);
            self.choose();
        }
        self.chosen.is_some()
    }

    /// The language lines are spoken in, if a table speaks the one chosen.
    pub fn language(&self) -> Option<&str> {
        self.chosen.map(|index| self.languages[index].locale.as_str())
    }

    /// The cue that speaks `key` in the chosen language.
    pub fn line(&self, key: LineKey) -> Option<AssetId> {
        self.languages[self.chosen?].lines.get(&key).copied()
    }

    fn choose(&mut self) {
        self.chosen = self.languages.iter().position(|language| language.locale == self.wanted);
    }
}

#[cfg(test)]
mod tests {
    use sapphire_assets::voice::VoiceLine;

    use super::*;

    fn cue(n: u8) -> AssetId {
        format!("00000000-0000-4000-8000-0000000000{n:02x}").parse().unwrap()
    }

    fn lines(locale: &str, keyed: &[(&str, u8)]) -> VoiceLines {
        VoiceLines {
            locale: locale.to_owned(),
            lines: keyed.iter().map(|&(key, n)| VoiceLine { key: key.to_owned(), cue: cue(n) }).collect(),
        }
    }

    #[test]
    fn the_chosen_language_answers_and_no_other() {
        let mut route = VoiceRoute::new();
        route.add(&lines("en", &[("perfect-dodge", 1), ("won", 2)])).unwrap();
        route.add(&lines("ja", &[("perfect-dodge", 3)])).unwrap();
        let dodge = LineKey::of("perfect-dodge");
        assert_eq!(route.line(dodge), None, "nothing is chosen yet");
        assert!(route.set_language("ja"));
        assert_eq!(route.language(), Some("ja"));
        assert_eq!(route.line(dodge), Some(cue(3)));
        assert_eq!(
            route.line(LineKey::of("won")),
            None,
            "Japanese has no `won`, and English doesn't stand in"
        );
        assert!(route.set_language("en"));
        assert_eq!(route.line(LineKey::of("won")), Some(cue(2)));
        assert!(!route.set_language("fr"));
        assert_eq!((route.language(), route.line(dodge)), (None, None));
    }

    #[test]
    fn a_language_chosen_before_its_table_comes_is_kept() {
        let mut route = VoiceRoute::new();
        assert!(!route.set_language("ja"));
        route.add(&lines("ja", &[("hello", 1)])).unwrap();
        assert_eq!(route.language(), Some("ja"));
    }

    #[test]
    fn a_language_twice_is_refused() {
        let mut route = VoiceRoute::new();
        route.add(&lines("en", &[("a", 1)])).unwrap();
        assert_eq!(route.add(&lines("en", &[("b", 2)])), Err(VoiceRouteError::TwoTables("en".to_owned())));
    }

    #[test]
    fn keys_hash_as_fnv1a_does() {
        // FNV-1a's published values: nothing about the hash may depend on the machine or the run.
        assert_eq!(LineKey::of(""), LineKey(0xcbf2_9ce4_8422_2325));
        assert_eq!(LineKey::of("a"), LineKey(0xaf63_dc4c_8601_ec8c));
        assert_ne!(LineKey::of("perfect-dodge"), LineKey::of("perfect_dodge"));
    }
}
