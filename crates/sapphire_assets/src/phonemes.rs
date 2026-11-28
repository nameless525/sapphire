//! Phoneme timings (M6 U14, spec §5.15): when each sound of a voiced line is said, in a sidecar beside the
//! line's master, `content/vo/<locale>/<key>.phonemes.ron` ([`PHONEMES_SUFFIX`]), which a studio's aligner
//! writes offline from the take and its text. Nothing in the engine aligns: a sidecar is read only by the cook
//! of the line's sound, which maps it through its language's [`VisemeTable`] into the visemes of the sound's
//! [`MouthCurve`](crate::mouth::MouthCurve) ([`Sound::with_phonemes`]), so the face reads the same 60 Hz
//! curve, in the same format, as before.
//!
//! ```ron
//! (
//!     type: "sapphire::assets::PhonemeTimings",
//!     version: 1,
//!     data: (
//!         locale: "en",
//!         phonemes: [
//!             (phoneme: "sil", from_ms: 0, to_ms: 100),
//!             (phoneme: "T", from_ms: 100, to_ms: 135),
//!             (phoneme: "EH1", from_ms: 135, to_ms: 265),
//!         ],
//!     ),
//! )
//! ```
//!
//! - **Times are whole milliseconds** from the sound's first sample, each interval from `from_ms` up to
//!   `to_ms`, in order, none overlapping another, none past the sound's end. A gap between two is silence. An
//!   aligner's seconds convert by rounding to the millisecond, which is finer than a curve's frame.
//! - **Phonemes are spelled as the language's table spells them** ([`VisemeTable::builtin`]): ARPAbet for
//!   English, as CMUdict and the common English aligners write it, where a vowel's trailing stress digit is
//!   ignored; romanized phonemes for Japanese, as Japanese segmentation kits write them. `sil`, `sp` and `pau`
//!   are silence in every language. A phoneme the table hasn't is refused by name and place.
//! - **The mapping keeps the envelope.** A frame's `open` is still the analysis' loudness
//!   ([`MouthCurve::analyze`](crate::mouth::MouthCurve::analyze)), which is right in every language and which
//!   the mix's ducking follows. Only the five visemes change: each
//!   frame takes the [`Shape`] of the phoneme said at it, scaled by that frame's `open`, so the mouth's shape is
//!   the aligner's and its size the voice's; a frame no phoneme covers says nothing.
//! - **Intervals land on frames** by [`frame_of`]: the nearest frame to each end, a half rounded up. A phoneme
//!   covers its start's frame up to, not including, its end's, so a mouth leads or trails its sound by at most
//!   half a frame (8.3 ms). A phoneme shorter than a frame can round to none; smoothing, attack, release and a
//!   minimum hold are the per-language tables' (M6 A24), which replace [`VisemeTable::builtin`].
//!
//! **The same bytes on every machine.** The sidecar holds integers, each end's frame is integer arithmetic,
//! and a viseme is `(open × share + 127) / 255` on integers, from the `open` byte the analysis already holds to
//! the same bytes everywhere (`mouth`'s module docs) and a share from a table compiled in. Nothing here is a
//! float, so a curve from a sidecar is as portable as the curve it starts from.

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use crate::mouth::{FULL, MOUTH_RATE, VISEMES};
use crate::score::{DocumentProblem, read_document};
use crate::sound::Sound;
use crate::voice::is_locale_tag;

/// What a line's phoneme sidecar's file name is: its master's key and this, beside the master.
pub const PHONEMES_SUFFIX: &str = ".phonemes.ron";

/// The most phonemes one line's timings may hold: twenty a second for ten minutes, a sound's longest.
pub const MAX_PHONEMES: usize = 12_000;

/// What every table reads as silence: an aligner's marks for a pause and for nothing said.
pub const SILENCES: [&str; 3] = ["sil", "sp", "pau"];

/// One phoneme's mouth: how much of the frame's envelope each viseme takes, `aa`, `ih`, `ou`, `ee` and `oh`,
/// out of [`FULL`], together at most [`FULL`]. All zero is a shut mouth: a pause, or lips pressed together.
pub type Shape = [u8; VISEMES];

/// One line's phoneme timings, in one language. See the [module docs](self).
#[derive(Reflect, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
pub struct PhonemeTimings {
    /// The BCP 47 tag of the language the line is said in: which table maps it.
    pub locale: String,
    /// In order, at least one.
    pub phonemes: Vec<Phoneme>,
}

impl Versioned for PhonemeTimings {
    const VERSION: u32 = 1;
}

/// One phoneme said, from `from_ms` up to `to_ms`.
#[derive(Reflect, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phoneme {
    /// As its language's table spells it.
    pub phoneme: String,
    pub from_ms: u32,
    pub to_ms: u32,
}

/// Timings that can't be read, or can't be said by the sound they're for.
#[derive(Debug, thiserror::Error)]
pub enum PhonemeError {
    #[error(transparent)]
    Document(#[from] DocumentProblem),
    #[error("`phonemes[{index}]` is `{phoneme}`, which the {language} viseme table hasn't")]
    Unknown { index: usize, phoneme: String, language: String },
    #[error("`phonemes[{index}]` ends at {to_ms} ms, past the sound's end at {length_ms} ms")]
    PastTheEnd { index: usize, to_ms: u32, length_ms: u32 },
    #[error("the timings are in {locale}, and the viseme table is {language}'s")]
    Language { locale: String, language: String },
    #[error(
        "the sound has no mouth curve for timings to shape: its sidecar needs `importer: Sound((mouth: true))`"
    )]
    NoMouth,
}

impl PhonemeTimings {
    /// Reads and checks a sidecar.
    pub fn from_document(text: &str) -> Result<Self, PhonemeError> {
        let timings: Self = read_document(text)?;
        timings.check()?;
        Ok(timings)
    }

    /// Checks every value, naming the first that's wrong by its field and place.
    pub fn check(&self) -> Result<(), DocumentProblem> {
        let field = |field, problem: String| Err(DocumentProblem::Field { field, problem });
        if !is_locale_tag(&self.locale) {
            return field("locale", format!("{:?} isn't a language tag", self.locale));
        }
        if self.phonemes.is_empty() || self.phonemes.len() > MAX_PHONEMES {
            return field(
                "phonemes",
                format!("{} phonemes isn't from 1 to {MAX_PHONEMES}", self.phonemes.len()),
            );
        }
        let mut last_end = 0;
        for (index, phoneme) in self.phonemes.iter().enumerate() {
            let Phoneme { phoneme: name, from_ms, to_ms } = phoneme;
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '_') {
                return field(
                    "phonemes",
                    format!("phonemes[{index}]'s phoneme {name:?} isn't a phoneme's name"),
                );
            }
            if from_ms >= to_ms {
                return field(
                    "phonemes",
                    format!(
                        "phonemes[{index}] (`{name}`) is from {from_ms} ms to {to_ms} ms: it must end after it starts"
                    ),
                );
            }
            if *from_ms < last_end {
                return field(
                    "phonemes",
                    format!(
                        "phonemes[{index}] (`{name}`) starts at {from_ms} ms, before the one before it ends at {last_end} ms"
                    ),
                );
            }
            last_end = *to_ms;
        }
        Ok(())
    }
}

/// The curve frame nearest `ms` milliseconds into a sound, a half rounded up: where an interval's end lands.
#[must_use]
pub const fn frame_of(ms: u32) -> usize {
    ((ms as u64 * MOUTH_RATE as u64 + 500) / 1_000) as usize
}

/// One language's phonemes as mouth shapes: what [`Sound::with_phonemes`] maps timings through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisemeTable {
    /// The language it speaks, by its primary subtag: `en`, `ja`. A timing's locale `en-GB` reads as `en`.
    pub language: String,
    /// Each phoneme of the language's alphabet and its shape. [`SILENCES`] needn't be listed.
    pub shapes: Vec<(String, Shape)>,
}

/// A language's primary subtag, in lowercase: `en` of `en-GB`.
fn primary(locale: &str) -> String {
    locale.split('-').next().unwrap_or_default().to_ascii_lowercase()
}

impl VisemeTable {
    /// The engine's own table for `locale`'s language, if it has one: English and Japanese (see
    /// [`ENGLISH`] and [`JAPANESE`]). A proposal to see the timings through, not a phonetician's choice; M6 A24's
    /// per-language documents replace it.
    #[must_use]
    pub fn builtin(locale: &str) -> Option<Self> {
        let language = primary(locale);
        let shapes = match language.as_str() {
            "en" => ENGLISH,
            "ja" => JAPANESE,
            _ => return None,
        };
        Some(Self {
            language,
            shapes: shapes.iter().map(|&(name, shape)| (name.to_owned(), shape)).collect(),
        })
    }

    /// The shape `phoneme` is said with: a silence's is shut, and a trailing stress digit (ARPAbet's 0, 1 and 2)
    /// is read past.
    #[must_use]
    pub fn shape(&self, phoneme: &str) -> Option<Shape> {
        if SILENCES.contains(&phoneme) {
            return Some([0; VISEMES]);
        }
        let find = |name: &str| self.shapes.iter().find(|(known, _)| known == name).map(|&(_, shape)| shape);
        find(phoneme).or_else(|| find(phoneme.strip_suffix(['0', '1', '2'])?))
    }
}

impl Sound {
    /// The sound with its mouth curve's visemes said by `timings` through `table` (see the [module docs](self)):
    /// the cook's stage for a line with a phoneme sidecar. The envelope stays the analysis'.
    pub fn with_phonemes(
        mut self,
        timings: &PhonemeTimings,
        table: &VisemeTable,
    ) -> Result<Self, PhonemeError> {
        if primary(&timings.locale) != table.language {
            return Err(PhonemeError::Language {
                locale: timings.locale.clone(),
                language: table.language.clone(),
            });
        }
        // The sound's length, a millisecond rounded up: an aligner's last interval ends at the take's end.
        let length_ms = (u64::from(self.frames) * 1_000).div_ceil(u64::from(self.sample_rate.max(1))) as u32;
        let mut said = Vec::with_capacity(timings.phonemes.len());
        for (index, phoneme) in timings.phonemes.iter().enumerate() {
            if phoneme.to_ms > length_ms {
                return Err(PhonemeError::PastTheEnd { index, to_ms: phoneme.to_ms, length_ms });
            }
            let shape = table.shape(&phoneme.phoneme).ok_or_else(|| PhonemeError::Unknown {
                index,
                phoneme: phoneme.phoneme.clone(),
                language: table.language.clone(),
            })?;
            said.push((frame_of(phoneme.from_ms), frame_of(phoneme.to_ms), shape));
        }
        let curve = self.mouth.as_mut().ok_or(PhonemeError::NoMouth)?;
        let mut phonemes = said.iter().peekable();
        for (at, frame) in curve.frames.iter_mut().enumerate() {
            while phonemes.next_if(|&&(_, to, _)| to <= at).is_some() {}
            let shape = match phonemes.peek() {
                Some(&&(from, _, shape)) if from <= at => shape,
                _ => [0; VISEMES],
            };
            frame.visemes = shape.map(|share| shared(frame.open, share));
        }
        Ok(self)
    }
}

/// `share` of `open`, both out of [`FULL`], rounded to the nearest.
fn shared(open: u8, share: u8) -> u8 {
    ((u32::from(open) * u32::from(share) + u32::from(FULL) / 2) / u32::from(FULL)) as u8
}

/// A shape that is one viseme, `share` of the envelope.
const fn one(viseme: usize, share: u8) -> Shape {
    let mut shape = [0; VISEMES];
    shape[viseme] = share;
    shape
}

/// A shape between two visemes: a diphthong's, from the first toward the second.
const fn two(first: usize, first_share: u8, second: usize, second_share: u8) -> Shape {
    let mut shape = [0; VISEMES];
    shape[first] = first_share;
    shape[second] = second_share;
    shape
}

const AA: usize = 0;
const IH: usize = 1;
const OU: usize = 2;
const EE: usize = 3;
const OH: usize = 4;
/// Lips together.
const SHUT: Shape = [0; VISEMES];

/// English, in ARPAbet without stress: the vowels on the viseme nearest each, a diphthong split between its
/// ends, and each consonant as the partly open mouth it's said with, the lips shut for `B`, `M` and `P`.
pub const ENGLISH: &[(&str, Shape)] = &[
    ("AA", one(AA, 255)),
    ("AE", two(AA, 150, EE, 105)),
    ("AH", one(AA, 170)),
    ("AO", one(OH, 255)),
    ("AW", two(AA, 150, OU, 105)),
    ("AY", two(AA, 150, IH, 105)),
    ("EH", one(EE, 255)),
    ("ER", two(OH, 110, IH, 70)),
    ("EY", two(EE, 150, IH, 105)),
    ("IH", one(IH, 210)),
    ("IY", one(IH, 255)),
    ("OW", two(OH, 150, OU, 105)),
    ("OY", two(OH, 150, IH, 105)),
    ("UH", one(OU, 190)),
    ("UW", one(OU, 255)),
    ("B", SHUT),
    ("M", SHUT),
    ("P", SHUT),
    ("F", one(IH, 60)),
    ("V", one(IH, 60)),
    ("TH", one(IH, 80)),
    ("DH", one(IH, 80)),
    ("S", one(IH, 90)),
    ("Z", one(IH, 90)),
    ("T", one(IH, 90)),
    ("D", one(IH, 90)),
    ("N", one(IH, 90)),
    ("L", one(AA, 90)),
    ("SH", one(OU, 110)),
    ("ZH", one(OU, 110)),
    ("CH", one(OU, 110)),
    ("JH", one(OU, 110)),
    ("K", one(AA, 80)),
    ("G", one(AA, 80)),
    ("NG", one(AA, 80)),
    ("HH", one(AA, 100)),
    ("R", one(OU, 100)),
    ("W", one(OU, 200)),
    ("Y", one(IH, 150)),
];

/// Japanese, romanized: its five vowels one to one on VRM's five, long or short, the moraic nasal `N` nearly
/// shut, the geminate `q` and `b`, `m` and `p` shut, and each other consonant partly open.
pub const JAPANESE: &[(&str, Shape)] = &[
    ("a", one(AA, 255)),
    ("i", one(IH, 255)),
    ("u", one(OU, 255)),
    ("e", one(EE, 255)),
    ("o", one(OH, 255)),
    ("a:", one(AA, 255)),
    ("i:", one(IH, 255)),
    ("u:", one(OU, 255)),
    ("e:", one(EE, 255)),
    ("o:", one(OH, 255)),
    ("N", one(OU, 40)),
    ("q", SHUT),
    ("b", SHUT),
    ("by", SHUT),
    ("m", SHUT),
    ("my", SHUT),
    ("p", SHUT),
    ("py", SHUT),
    ("k", one(AA, 70)),
    ("ky", one(IH, 70)),
    ("g", one(AA, 70)),
    ("gy", one(IH, 70)),
    ("s", one(IH, 70)),
    ("sh", one(IH, 70)),
    ("z", one(IH, 70)),
    ("j", one(IH, 70)),
    ("t", one(IH, 70)),
    ("ts", one(IH, 70)),
    ("ch", one(IH, 70)),
    ("d", one(IH, 70)),
    ("dy", one(IH, 70)),
    ("n", one(IH, 70)),
    ("ny", one(IH, 70)),
    ("h", one(AA, 70)),
    ("hy", one(IH, 70)),
    ("f", one(OU, 100)),
    ("r", one(IH, 70)),
    ("ry", one(IH, 70)),
    ("y", one(IH, 100)),
    ("w", one(OU, 150)),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mouth::{MouthCurve, MouthFrame};

    fn document(locale: &str, phonemes: &str) -> String {
        format!(
            "(\n    type: \"sapphire::assets::PhonemeTimings\",\n    version: 1,\n    data: (\n        locale: \
             \"{locale}\",\n        phonemes: [\n{phonemes}\n        ],\n    ),\n)\n"
        )
    }

    fn phoneme(name: &str, from_ms: u32, to_ms: u32) -> String {
        format!("            (phoneme: \"{name}\", from_ms: {from_ms}, to_ms: {to_ms}),")
    }

    /// A sound `frames` long at 24 kHz with a curve held fully open throughout.
    fn open_sound(frames: u32) -> Sound {
        let curve = MouthCurve {
            frames: vec![
                MouthFrame { open: FULL, visemes: [51; VISEMES] };
                MouthCurve::frames_for(frames, 24_000)
            ],
        };
        Sound { sample_rate: 24_000, channels: 1, frames, vorbis: Vec::new(), mouth: Some(curve) }
    }

    #[test]
    fn every_shape_shares_no_more_than_the_envelope() {
        for (language, table) in [("en", ENGLISH), ("ja", JAPANESE)] {
            for (index, &(name, shape)) in table.iter().enumerate() {
                let total: u32 = shape.iter().map(|&share| u32::from(share)).sum();
                assert!(total <= u32::from(FULL), "{language} `{name}`: {shape:?}");
                assert!(
                    !table[..index].iter().any(|&(earlier, _)| earlier == name),
                    "{language} `{name}` twice"
                );
                assert!(!SILENCES.contains(&name), "{language} `{name}` is a silence already");
            }
        }
    }

    #[test]
    fn a_table_reads_its_language_and_past_a_stress_digit() {
        let english = VisemeTable::builtin("en-GB").unwrap();
        assert_eq!(english.language, "en");
        assert_eq!(english.shape("AA1"), Some(one(AA, 255)));
        assert_eq!(english.shape("AA"), english.shape("AA0"));
        assert_eq!(english.shape("M"), Some(SHUT));
        assert_eq!(english.shape("sp"), Some(SHUT));
        assert_eq!(english.shape("a"), None, "Japanese's spelling isn't English's");
        assert_eq!(english.shape("AA3"), None);
        let japanese = VisemeTable::builtin("ja").unwrap();
        assert_eq!(japanese.shape("o:"), Some(one(OH, 255)));
        assert!(VisemeTable::builtin("fr").is_none());
    }

    #[test]
    fn intervals_land_on_the_nearest_frame() {
        assert_eq!(frame_of(0), 0);
        // A frame is 16.67 ms: 8 ms is nearer frame 0, 9 ms frame 1.
        assert_eq!((frame_of(8), frame_of(9)), (0, 1));
        assert_eq!(frame_of(25), 2, "25 ms is 1.5 frames, a half rounded up");
        assert_eq!(frame_of(1_000), 60);
        assert_eq!(frame_of(u32::MAX), 257_698_038, "no overflow");
    }

    /// The row's proof: each phoneme's shape starts at its start's frame and stops at its end's, scaled by the
    /// envelope; a gap and the time around the timings say nothing, and the envelope is kept.
    #[test]
    fn each_phoneme_lands_at_its_frame() {
        let text =
            [phoneme("sil", 0, 50), phoneme("HH", 50, 90), phoneme("AY1", 90, 300), phoneme("M", 400, 450)];
        let timings = PhonemeTimings::from_document(&document("en", &text.join("\n"))).unwrap();
        let table = VisemeTable::builtin("en").unwrap();
        // Half a second: 31 frames.
        let sound = open_sound(12_000).with_phonemes(&timings, &table).unwrap();
        let curve = sound.mouth.unwrap();
        assert_eq!(curve.frames.len(), 31);
        let shape_at = |frame: usize| curve.frames[frame].visemes;
        // 50 ms is frame 3, 90 ms frame 5 (5.4), 300 ms frame 18, 400 ms frame 24, 450 ms frame 27.
        for frame in 0..31 {
            let expected = match frame {
                3..=4 => one(AA, 100),
                5..=17 => two(AA, 150, IH, 105),
                _ => SHUT,
            };
            assert_eq!(shape_at(frame), expected, "frame {frame}");
            assert_eq!(curve.frames[frame].open, FULL, "frame {frame}: the envelope is the analysis'");
        }
        for phoneme in &timings.phonemes {
            let at = frame_of(phoneme.from_ms);
            let shape = table.shape(&phoneme.phoneme).unwrap();
            assert_eq!(shape_at(at), shape, "{phoneme:?} at frame {at}");
            if at > 0 && shape != SHUT {
                assert_ne!(shape_at(at - 1), shape, "{phoneme:?} starts at frame {at}, not before");
            }
        }
    }

    #[test]
    fn a_shape_takes_its_share_of_the_envelope() {
        assert_eq!(shared(FULL, FULL), FULL);
        assert_eq!(shared(0, FULL), 0);
        assert_eq!(shared(128, 128), 64);
        assert_eq!(shared(200, 105), 82, "82.35 rounds down");
        assert_eq!(shared(1, 128), 1, "0.502 rounds up");
        let timings = PhonemeTimings::from_document(&document("ja", &phoneme("a", 0, 500))).unwrap();
        let mut sound = open_sound(12_000);
        sound.mouth.as_mut().unwrap().frames[10].open = 100;
        let curve =
            sound.with_phonemes(&timings, &VisemeTable::builtin("ja").unwrap()).unwrap().mouth.unwrap();
        assert_eq!(curve.frames[10].visemes, [100, 0, 0, 0, 0]);
        assert_eq!(curve.frames[30].visemes, SHUT, "the frame past the end is past the phoneme's");
    }

    #[test]
    fn timings_that_cant_be_said_are_refused_by_field() {
        let refused = |locale: &str, phonemes: &[String]| {
            PhonemeTimings::from_document(&document(locale, &phonemes.join("\n"))).unwrap_err().to_string()
        };
        for (locale, phonemes, why) in [
            ("en", vec![phoneme("AA", 10, 10)], "phonemes[0] (`AA`) is from 10 ms to 10 ms"),
            (
                "en",
                vec![phoneme("AA", 0, 100), phoneme("B", 90, 120)],
                "phonemes[1] (`B`) starts at 90 ms, before",
            ),
            ("en", vec![phoneme("A A", 0, 100)], "phonemes[0]'s phoneme \"A A\""),
            ("en", vec![phoneme("", 0, 100)], "phonemes[0]'s phoneme \"\""),
            ("en_US", vec![phoneme("AA", 0, 100)], "`locale`"),
            ("en", vec![], "0 phonemes"),
        ] {
            let error = refused(locale, &phonemes);
            assert!(error.contains(why), "{phonemes:?}: {error}");
        }
        let unknown = PhonemeTimings::from_document(&document(
            "en",
            "(phoneme: \"AA\", from_ms: 0, to_ms: 9, stress: 1)",
        ));
        assert!(unknown.unwrap_err().to_string().contains("stress"), "unknown fields are refused");

        let table = VisemeTable::builtin("en").unwrap();
        let said = |text: &str, table: &VisemeTable| {
            let timings = PhonemeTimings::from_document(text).unwrap();
            open_sound(12_000).with_phonemes(&timings, table).unwrap_err().to_string()
        };
        let past = said(&document("en", &phoneme("AA", 400, 501)), &table);
        assert!(past.contains("`phonemes[0]` ends at 501 ms, past the sound's end at 500 ms"), "{past}");
        let typo =
            said(&document("en", &[phoneme("AA", 0, 100), phoneme("QQ", 100, 200)].join("\n")), &table);
        assert!(typo.contains("`phonemes[1]` is `QQ`, which the en viseme table hasn't"), "{typo}");
        let other = said(&document("ja", &phoneme("a", 0, 100)), &table);
        assert!(other.contains("in ja, and the viseme table is en's"), "{other}");
        let mut silent = open_sound(12_000);
        silent.mouth = None;
        let timings = PhonemeTimings::from_document(&document("en", &phoneme("AA", 0, 100))).unwrap();
        assert!(matches!(silent.with_phonemes(&timings, &table), Err(PhonemeError::NoMouth)));
    }
}
