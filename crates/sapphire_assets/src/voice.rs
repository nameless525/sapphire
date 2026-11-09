//! Voice lines (`.voice.ron` documents, cooked to `.svoice`; M3 A1, spec §5.15): one language's lines,
//! each a cue on the [`Voice`](crate::cue::Bus::Voice) bus, keyed as strings are.
//!
//! ```ron
//! (
//!     type: "sapphire::assets::VoiceLines",
//!     version: 1,
//!     data: (
//!         locale: "ja",
//!         lines: [
//!             (key: "perfect-dodge", cue: "4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c"),
//!         ],
//!     ),
//! )
//! ```
//!
//! A game asks for a line by its key, and the voice language a player chose (`sapphire_loc`'s
//! `LocaleSettings::voice`, apart from the text's) says which table answers: that's
//! `sapphire_audio`'s `VoiceRoute`. A key reads as a Fluent message's does, so a line and its subtitle
//! can share one. The cook checks that each line is a cue on the voice bus.

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{AlignedVec, CookedError};
use serde::{Deserialize, Serialize};

use crate::AssetId;
use crate::score::{DocumentProblem, read_document};

/// Conventional file extension for cooked voice lines.
pub const VOICE_LINES_EXTENSION: &str = "svoice";

/// The most lines one table may hold.
pub const MAX_LINES: usize = 4_096;

/// One language's voice lines. See the [module docs](self).
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
pub struct VoiceLines {
    /// The BCP 47 tag the lines are spoken in.
    pub locale: String,
    /// At least one.
    pub lines: Vec<VoiceLine>,
}

impl Versioned for VoiceLines {
    const VERSION: u32 = 1;
}

/// One line.
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct VoiceLine {
    /// What a game asks for: a Fluent identifier, a letter then letters, digits, `-` and `_`.
    pub key: String,
    /// The cue it plays, by asset ID.
    pub cue: AssetId,
}

/// Voice lines that can't be read.
#[derive(Debug, thiserror::Error)]
pub enum VoiceLinesError {
    #[error(transparent)]
    Document(#[from] DocumentProblem),
    #[error(transparent)]
    Cooked(#[from] CookedError),
}

/// Whether `key` is a Fluent identifier.
pub fn is_line_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `tag` could be a BCP 47 tag: subtags of letters and digits between hyphens. Whether a
/// game has the language is the route's to say.
fn is_locale_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .split('-')
            .all(|part| (1..=8).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric()))
}

impl VoiceLines {
    /// Reads and checks a voice-line document.
    pub fn from_document(text: &str) -> Result<Self, VoiceLinesError> {
        let lines: Self = read_document(text)?;
        lines.check()?;
        Ok(lines)
    }

    /// Checks every value, naming the first that's wrong.
    pub fn check(&self) -> Result<(), DocumentProblem> {
        let field = |field, problem: String| Err(DocumentProblem::Field { field, problem });
        if !is_locale_tag(&self.locale) {
            return field("locale", format!("{:?} isn't a language tag", self.locale));
        }
        if self.lines.is_empty() || self.lines.len() > MAX_LINES {
            return field("lines", format!("{} lines isn't from 1 to {MAX_LINES}", self.lines.len()));
        }
        for (index, line) in self.lines.iter().enumerate() {
            if !is_line_key(&line.key) {
                return field("lines", format!("lines[{index}]'s key {:?} isn't a message's", line.key));
            }
            if self.lines[..index].iter().any(|earlier| earlier.key == line.key) {
                return field("lines", format!("`{}` is keyed twice", line.key));
            }
        }
        Ok(())
    }

    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every value.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedVoiceLines, VoiceLinesError> {
        let lines = sapphire_serialize::access::<Self>(bytes)?;
        lines.to_owned_lines().check()?;
        Ok(lines)
    }
}

impl ArchivedVoiceLines {
    /// The lines as plain data.
    pub fn to_owned_lines(&self) -> VoiceLines {
        VoiceLines {
            locale: self.locale.as_str().to_owned(),
            lines: self
                .lines
                .iter()
                .map(|line| VoiceLine { key: line.key.as_str().to_owned(), cue: line.cue })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUE: &str = "4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c";

    fn document(locale: &str, lines: &str) -> String {
        format!(
            "(\n    type: \"sapphire::assets::VoiceLines\",\n    version: 1,\n    data: (\n        locale: \
             {locale:?},\n        lines: [{lines}],\n    ),\n)\n"
        )
    }

    fn line(key: &str) -> String {
        format!("(key: {key:?}, cue: {CUE:?})")
    }

    #[test]
    fn lines_read_and_cook_back() {
        let lines = VoiceLines::from_document(&document("ja", &line("perfect-dodge"))).unwrap();
        assert_eq!(lines.locale, "ja");
        assert_eq!(lines.lines[0].cue, CUE.parse().unwrap());
        let cooked = lines.to_cooked().unwrap();
        assert_eq!(VoiceLines::read(&cooked).unwrap().to_owned_lines(), lines);
    }

    #[test]
    fn keys_read_as_a_messages_do() {
        assert!(is_line_key("fight-won_2"));
        for bad in ["", "2nd", "-dash", "has space", "kana-か"] {
            assert!(!is_line_key(bad), "{bad:?}");
            let message = VoiceLines::from_document(&document("en", &line(bad))).unwrap_err().to_string();
            assert!(message.contains("`lines`"), "{bad:?}: {message}");
        }
    }

    #[test]
    fn a_key_twice_and_a_tag_that_isnt_are_refused() {
        let twice = format!("{}, {}", line("hello"), line("hello"));
        let message = VoiceLines::from_document(&document("en", &twice)).unwrap_err().to_string();
        assert!(message.contains("`hello` is keyed twice"), "{message}");
        for bad in ["", "en_US", "en--us", "toolongsubtag"] {
            let message = VoiceLines::from_document(&document(bad, &line("hello"))).unwrap_err().to_string();
            assert!(message.contains("`locale`"), "{bad:?}: {message}");
        }
        assert!(VoiceLines::from_document(&document("zh-Hant-TW", &line("hello"))).is_ok());
        assert!(VoiceLines::from_document(&document("en", "")).unwrap_err().to_string().contains("`lines`"));
    }
}
