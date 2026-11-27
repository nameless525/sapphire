//! An effects library, cooked (`.seffects`, M6 N1, spec §5.16): what a game's cues and sword trails play. The
//! document is `sapphire_vfx`'s `EffectLibrary` (`<name>.effects.ron`), whose schema and checks are that
//! crate's: a cook reads a library written at any version it knows, checks it whole, and keeps it here as the
//! document `sapphire_vfx` writes back at its current version, so the runtime reads one version and never
//! upgrades, and every value holds to the bit (a float is written as the shortest text that reads back to it).
//!
//! The library is kept as its document, not as an archive of its own, because its schema is the effects
//! crate's and grows with M6's effects (forces, events, painted sheets, stateless emitters): one schema, read
//! by one reader, with no second copy here to keep in step. A library is a few tens of kilobytes, read once
//! when a fight starts. What this checks by itself is the envelope, the archive and that the document is an
//! effects library ([`Effects::read`]); the rest is checked as `sapphire_vfx` reads it, which the cook has
//! already done once.
//!
//! Beside the document, the cook lists the painted sheets its emitters name (M6 N4a,
//! [`crate::effect_sheet`]), sorted and once each, which a library depends on: they load before it does, and
//! its effects atlas is theirs.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::AssetId;
use sapphire_serialize::cooked::{AlignedVec, CookedError};
use sapphire_serialize::{DocumentError, document};

/// What a cooked effects library's file name ends with.
pub const EFFECTS_EXTENSION: &str = "seffects";

/// The stable name of the library's type, `sapphire_vfx::EffectLibrary`, as its document's envelope says it:
/// this crate can't link the effects crate, so it holds the name as text, and the pipeline's tests hold it to
/// the type.
pub const LIBRARY_TYPE: &str = "sapphire::vfx::EffectLibrary";

/// An effects library, cooked. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[type_path = "sapphire::assets"]
#[rkyv(derive(Debug))]
pub struct Effects {
    /// The library's document as `sapphire_vfx` writes it, at the version the cook read it with.
    pub library: String,
    /// The sheets its emitters name, sorted, each once (M6 N4a): its dependencies.
    pub sheets: Vec<AssetId>,
}

impl Versioned for Effects {
    /// 1: M6 N1. 2: M6 N4a, the sheets its emitters name.
    const VERSION: u32 = 2;
}

/// An effects library that can't be read.
#[derive(Debug, thiserror::Error)]
pub enum EffectsError {
    #[error(transparent)]
    Cooked(#[from] CookedError),
    /// Its document doesn't read as a document, or isn't an effects library. Boxed, since a document's error
    /// is large and every asset's error carries this one.
    #[error("its library: {0}")]
    Library(Box<DocumentError>),
    /// Its sheets aren't sorted and each once, as the cook lists them.
    #[error("its sheets aren't sorted, each once")]
    Sheets,
}

impl From<DocumentError> for EffectsError {
    fn from(error: DocumentError) -> Self {
        Self::Library(Box::new(error))
    }
}

impl Effects {
    /// Writes it: the same bytes for the same library, every time.
    pub fn to_cooked(&self) -> Result<AlignedVec, EffectsError> {
        Ok(sapphire_serialize::to_cooked(self)?)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and that the document it holds is an
    /// effects library.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedEffects, EffectsError> {
        let effects = sapphire_serialize::access::<Self>(bytes)?;
        let found = document::type_name(effects.library.as_str())?;
        if found != LIBRARY_TYPE {
            return Err(DocumentError::WrongType { expected: LIBRARY_TYPE, found }.into());
        }
        if !effects.sheets.is_sorted_by(|a, b| a < b) {
            return Err(EffectsError::Sheets);
        }
        Ok(effects)
    }
}

impl ArchivedEffects {
    /// The library's document, for `sapphire_vfx` to read.
    #[must_use]
    pub fn library(&self) -> &str {
        self.library.as_str()
    }

    /// The sheets its emitters name, sorted, each once.
    #[must_use]
    pub fn sheets(&self) -> &[AssetId] {
        self.sheets.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(type_name: &str) -> Effects {
        Effects {
            library: format!("(type: \"{type_name}\", version: 5, data: (effects: []))"),
            sheets: Vec::new(),
        }
    }

    #[test]
    fn a_library_reads_back_as_it_was_written() {
        let effects = library(LIBRARY_TYPE);
        let bytes = effects.to_cooked().expect("cooked");
        assert_eq!(bytes.as_slice(), effects.to_cooked().expect("cooked again").as_slice());
        assert_eq!(Effects::read(&bytes).expect("reads").library(), effects.library);
    }

    /// The asset server checks what it can without the effects crate: a document of another type, or none, is
    /// refused when it's decoded rather than when a fight reads it.
    #[test]
    fn what_isnt_an_effects_library_is_refused() {
        let other = library("sapphire::cues::CueTable").to_cooked().expect("cooked");
        let error = Effects::read(&other).expect_err("another type").to_string();
        assert!(error.contains("sapphire::cues::CueTable"), "{error}");
        let garbled =
            Effects { library: "(type: ".to_owned(), sheets: Vec::new() }.to_cooked().expect("cooked");
        assert!(Effects::read(&garbled).is_err());
    }

    /// Its sheets read back as the cook listed them, and a list out of order or with a repeat is refused.
    #[test]
    fn its_sheets_are_sorted_and_each_once() {
        let [a, b] = [AssetId::from_bytes([1; 16]), AssetId::from_bytes([2; 16])];
        let with =
            |sheets: Vec<AssetId>| Effects { sheets, ..library(LIBRARY_TYPE) }.to_cooked().expect("cooked");
        assert_eq!(Effects::read(&with(vec![a, b])).expect("reads").sheets(), [a, b]);
        assert!(matches!(Effects::read(&with(vec![b, a])), Err(EffectsError::Sheets)));
        assert!(matches!(Effects::read(&with(vec![a, a])), Err(EffectsError::Sheets)));
    }
}
