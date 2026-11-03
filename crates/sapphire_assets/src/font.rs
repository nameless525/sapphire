//! Cooked fonts and font chains (M3 U3, spec §5.17, §5.27).
//!
//! A [`Font`] (`.sfont`) is a font file cut down to the characters its pack can show, with what the
//! shaper can't read from the file: how far its baseline shifts, and the license it ships under,
//! which a credits screen can show and a build's notices repeat. Its axes survive the cut, so a
//! variable font keeps its weights.
//!
//! [`FontChains`] (`.sfontc`, cooked from a `.fontchains.ron` document) says which font files each
//! use of type, such as `body` or `display`, may need:
//!
//! ```ron
//! (
//!     type: "sapphire::assets::FontChains",
//!     version: 2,
//!     data: (
//!         chains: [
//!             (name: "display", fonts: ["4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c", "0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b"]),
//!         ],
//!     ),
//! )
//! ```
//!
//! **It's a list of what to load, and the order isn't read.** What a language shows a role in, first
//! choice first, is the family list in its locale document (`LocaleDocument::fonts`, spec §5.25),
//! which `sapphire_ui` passes to the shaper. Shaping knows a string's locale and not a run's script,
//! and `fontique`'s fallback table has no entry for hiragana or katakana with a locale, so a list by
//! script couldn't be chosen from. A cook refuses a locale's family that no font of its role's chain
//! carries (`sapphire_pipeline::ui`). Chains hold IDs, like cues and graphs, so cooking them reads no
//! font.
//!
//! Both are plain data, so the cooker writes them without linking a shaper, and `sapphire_text` takes
//! the file's bytes with no edge to this crate. They go in a pack of their own ([`FONTS_PACK`]), so a
//! change of font leaves the shared pack alone and what shows no text mounts none of it.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use sapphire_serialize::cooked::{AlignedVec, CookedError};
use serde::{Deserialize, Serialize};

use crate::AssetId;

/// Conventional file extension for cooked fonts.
pub const FONT_EXTENSION: &str = "sfont";

/// Conventional file extension for cooked font chains.
pub const FONT_CHAINS_EXTENSION: &str = "sfontc";

/// The pack font chains go in, as do the fonts a sidecar sends there.
pub const FONTS_PACK: &str = "fonts";

/// The furthest a face's baseline may shift, in ems: past this it's a typing mistake.
const MAX_BASELINE_SHIFT: f32 = 1.0;

/// A cooked font. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::assets"]
pub struct Font {
    /// The family a style names it by, as the file itself says.
    pub family: String,
    /// The pack it goes in.
    pub pack: String,
    /// How far down its lines' baselines move, in ems (`FaceOptions` in `sapphire_text`).
    pub baseline_shift: f32,
    /// The license the font ships under, as its authors wrote it.
    pub license: String,
    /// The font file, cut down.
    pub file: Vec<u8>,
}

impl Versioned for Font {
    const VERSION: u32 = 1;
}

/// A font or font chains that can't be read.
#[derive(Debug, thiserror::Error)]
pub enum FontError {
    /// The document isn't a font chains document at all: the wrong type or a version out of reach.
    #[error(transparent)]
    Document(Box<DocumentError>),
    /// Malformed RON, or a value of the wrong kind. `text` is the document's own line, which names
    /// the field.
    #[error("line {line}, column {column}: {message}: `{text}`")]
    Malformed { line: usize, column: usize, message: String, text: String },
    /// A value that parsed but can't be used.
    #[error("{field}: {problem}")]
    Field { field: String, problem: String },
    #[error("a cooked font with no {field}")]
    Empty { field: &'static str },
    #[error("a cooked font whose file isn't a font file")]
    NotAFont,
    #[error("a baseline shift of {shift} ems")]
    BaselineShift { shift: f32 },
    #[error(transparent)]
    Cooked(#[from] CookedError),
}

impl Font {
    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and the fields. Whether the file
    /// inside shapes is the shaper's to find out, as the cook's tests prove it can.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedFont, FontError> {
        let font = sapphire_serialize::access::<Self>(bytes)?;
        let fields = [("family", &font.family), ("pack", &font.pack), ("license", &font.license)];
        if let Some((field, _)) = fields.iter().find(|(_, value)| value.is_empty()) {
            return Err(FontError::Empty { field });
        }
        let tag = font.file.get(..4).unwrap_or_default();
        if tag != [0, 1, 0, 0] && tag != *b"OTTO" {
            return Err(FontError::NotAFont);
        }
        let shift = font.baseline_shift.to_native();
        if !shift.is_finite() || shift.abs() > MAX_BASELINE_SHIFT {
            return Err(FontError::BaselineShift { shift });
        }
        Ok(font)
    }
}

/// A use of type, such as `body` or `display`, and the font files it may need.
#[derive(
    Clone, Debug, PartialEq, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct FontChain {
    pub name: String,
    /// By asset ID: at least one.
    pub fonts: Vec<AssetId>,
}

/// A project's font chains. See the [module docs](self).
#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    TypePath,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
pub struct FontChains {
    pub chains: Vec<FontChain>,
}

impl Versioned for FontChains {
    const VERSION: u32 = 2;
}

impl FontChains {
    /// Reads and checks a document.
    pub fn from_document(text: &str) -> Result<Self, FontError> {
        let chains = sapphire_serialize::from_ron::<Self>(text).map_err(|error| match error {
            DocumentError::Parse(spanned) => {
                let line = spanned.span.start.line;
                let quoted = text.lines().nth(line.saturating_sub(1)).unwrap_or_default().trim();
                FontError::Malformed {
                    line,
                    column: spanned.span.start.col,
                    message: spanned.code.to_string(),
                    text: quoted.to_owned(),
                }
            }
            other => FontError::Document(Box::new(other)),
        })?;
        chains.check()?;
        Ok(chains)
    }

    /// Checks that every chain can answer, naming the first that can't.
    pub fn check(&self) -> Result<(), FontError> {
        let field =
            |field: String, problem: &str| Err(FontError::Field { field, problem: problem.to_owned() });
        if self.chains.is_empty() {
            return field("chains".to_owned(), "a document names at least one chain");
        }
        for (index, chain) in self.chains.iter().enumerate() {
            if chain.name.is_empty() {
                return field(format!("chains[{index}].name"), "a chain has a name");
            }
            if self.chains[..index].iter().any(|earlier| earlier.name == chain.name) {
                return field(format!("chain {}", chain.name), "two chains have this name");
            }
            if chain.fonts.is_empty() {
                return field(format!("chain {}", chain.name), "names no font");
            }
        }
        Ok(())
    }

    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every chain.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedFontChains, FontError> {
        let chains = sapphire_serialize::access::<Self>(bytes)?;
        chains.to_owned_chains().check()?;
        Ok(chains)
    }
}

impl ArchivedFont {
    /// The font as plain data.
    pub fn to_owned_font(&self) -> Font {
        Font {
            family: self.family.to_string(),
            pack: self.pack.to_string(),
            baseline_shift: self.baseline_shift.to_native(),
            license: self.license.to_string(),
            file: self.file.to_vec(),
        }
    }
}

impl ArchivedFontChains {
    /// The chains as plain data.
    pub fn to_owned_chains(&self) -> FontChains {
        FontChains {
            chains: self
                .chains
                .iter()
                .map(|chain| FontChain {
                    name: chain.name.to_string(),
                    fonts: chain.fonts.iter().copied().collect(),
                })
                .collect(),
        }
    }

    /// The fonts `chain` names, if there's a chain of that name.
    pub fn fonts(&self, chain: &str) -> Option<&[AssetId]> {
        let chain = self.chains.iter().find(|candidate| candidate.name == chain)?;
        Some(chain.fonts.as_slice())
    }

    /// Every font any chain names, which are the assets the chains depend on.
    pub fn font_ids(&self) -> impl Iterator<Item = AssetId> + '_ {
        self.chains.iter().flat_map(|chain| chain.fonts.iter().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c";

    fn document(chains: &str) -> String {
        format!(
            "(\n    type: \"sapphire::assets::FontChains\",\n    version: 2,\n    data: (\n        chains: [{chains}],\n    ),\n)\n"
        )
    }

    fn font() -> Font {
        Font {
            family: "M PLUS 1".to_owned(),
            pack: FONTS_PACK.to_owned(),
            baseline_shift: 0.125,
            license: "the license".to_owned(),
            file: vec![0, 1, 0, 0, 9],
        }
    }

    #[test]
    fn a_font_reads_back_as_written() {
        let cooked = font().to_cooked().unwrap();
        let read = Font::read(&cooked).unwrap();
        assert_eq!(read.family, "M PLUS 1");
        assert_eq!(read.pack, FONTS_PACK);
        assert_eq!(read.baseline_shift.to_native(), 0.125);
        assert_eq!(read.license, "the license");
        assert_eq!(read.file.as_slice(), [0, 1, 0, 0, 9]);
    }

    #[test]
    fn a_font_missing_a_field_is_refused_by_name() {
        let refused = |font: Font| Font::read(&font.to_cooked().unwrap()).unwrap_err().to_string();
        assert!(refused(Font { family: String::new(), ..font() }).contains("family"));
        assert!(refused(Font { pack: String::new(), ..font() }).contains("pack"));
        assert!(refused(Font { license: String::new(), ..font() }).contains("license"));
        assert!(refused(Font { file: b"wOFF".to_vec(), ..font() }).contains("isn't a font"));
        assert!(refused(Font { baseline_shift: 3.0, ..font() }).contains("baseline"));
        assert!(refused(Font { baseline_shift: f32::NAN, ..font() }).contains("baseline"));
    }

    #[test]
    fn chains_answer_by_use() {
        let text = document(&format!("(name: \"display\", fonts: [\"{ID}\", \"{ID}\"])"));
        let chains = FontChains::from_document(&text).unwrap();
        let cooked = chains.to_cooked().unwrap();
        let read = FontChains::read(&cooked).unwrap();
        assert_eq!(read.to_owned_chains(), chains);
        let id: AssetId = ID.parse().unwrap();
        assert_eq!(read.fonts("display"), Some([id, id].as_slice()));
        assert_eq!(read.fonts("body"), None);
        assert_eq!(read.font_ids().count(), 2);
    }

    #[test]
    fn a_bad_document_says_what_is_wrong_with_it() {
        let error = |chains: &str| FontChains::from_document(&document(chains)).unwrap_err().to_string();
        assert!(error("").contains("at least one chain"));
        let empty = error("(name: \"body\", fonts: [])");
        assert!(empty.contains("chain body") && empty.contains("names no font"), "{empty}");
        let same = format!("(name: \"body\", fonts: [\"{ID}\"]), (name: \"body\", fonts: [\"{ID}\"])");
        assert!(error(&same).contains("two chains"));
        assert!(error("(nme: \"body\")").contains("line"));
    }
}
