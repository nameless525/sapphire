//! Foliage documents (M6 G1a): `.foliage.ron` cooked to `.sfoliage`, the kinds a zone's scatter sets draw.
//!
//! The document, its checks and placement are `sapphire_zone`'s ([`sapphire_zone::foliage`]). This module places
//! the document in the cook: a source with a sidecar like any asset (`importer: Foliage(())`) whose own asset is
//! its kinds, each with the mesh it draws, which the cook holds to a static mesh of the tree. A zone names a
//! foliage document by its sidecar's ID (`foliage`), and its scatter sets, a sub-asset a cell, cook with the
//! zone ([`crate::zone::scatter`]), since where a plant stands is the zone's ground.

use std::path::{Path, PathBuf};

use sapphire_zone::{Foliage, FoliageDocument, ZoneError};
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

/// Reads and checks the document. Bump the version whenever that changes output for the same document.
pub const IMPORTER: Stage = Stage { name: "foliage document", version: 1 };

/// The `.sfoliage` writer. Bump the version whenever it changes output for the same document.
pub const COOKER: Stage = Stage { name: "sfoliage", version: 1 };

/// What a foliage document's source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".foliage.ron";

/// Import settings for foliage documents: none yet. A struct, so settings can join without changing existing
/// sidecars. Not the mesh importer's `foliage` settings (M1 W6), which say how a plant's mesh shades.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FoliageDocumentSettings {}

/// A foliage document that can't cook.
#[derive(Debug, thiserror::Error)]
pub enum FoliageCookError {
    #[error("the document isn't UTF-8")]
    Encoding(#[from] std::str::Utf8Error),
    /// The document doesn't read, or a field of it is wrong: the error names it.
    #[error(transparent)]
    Document(#[from] ZoneError),
}

/// Whether `path` names a foliage document's source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file a source at `relative` writes: `grove.foliage.ron` → `grove.sfoliage`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Reads and checks a document's bytes.
pub fn read(bytes: &[u8]) -> Result<FoliageDocument, FoliageCookError> {
    Ok(FoliageDocument::from_ron(std::str::from_utf8(bytes)?)?)
}

/// Cooks a document's bytes: its kinds, read again through the runtime's check.
pub fn cook(bytes: &[u8]) -> Result<Vec<u8>, FoliageCookError> {
    let cooked = read(bytes)?.cook().to_cooked()?;
    Foliage::read(&cooked)?;
    Ok(cooked.into_vec())
}

#[cfg(test)]
mod tests {
    use sapphire_zone::FOLIAGE_EXTENSION;

    use super::*;

    const GROVE: &[u8] = include_bytes!("../../../crates/sapphire_zone/testdata/grove.foliage.ron");

    #[test]
    fn a_source_is_known_by_its_whole_suffix() {
        assert!(is_source(Path::new("wood/grove.foliage.ron")));
        assert!(is_source(Path::new("GROVE.FOLIAGE.RON")));
        assert!(!is_source(Path::new("foliage.ron")));
        assert_eq!(
            output_path(Path::new("wood/grove.foliage.ron"), FOLIAGE_EXTENSION),
            Path::new("wood/grove.sfoliage")
        );
    }

    #[test]
    fn a_document_cooks_to_its_kinds_and_the_same_bytes_twice() {
        let cooked = cook(GROVE).expect("cooks");
        assert_eq!(cooked, cook(GROVE).expect("cooks"));
        let mut aligned: sapphire_serialize::cooked::AlignedVec =
            sapphire_serialize::cooked::AlignedVec::new();
        aligned.extend_from_slice(&cooked);
        let foliage = Foliage::read(&aligned).expect("reads");
        let names: Vec<&str> = foliage.kinds.iter().map(|kind| kind.name.as_str()).collect();
        assert_eq!(names, ["oak", "fern", "clump"]);
        assert_eq!(foliage.meshes().len(), 3);
    }

    #[test]
    fn a_malformed_document_is_refused_by_its_field() {
        let text = std::str::from_utf8(GROVE).unwrap().replace("spacing: 4.0", "spacing: -4.0");
        let error = cook(text.as_bytes()).expect_err("refused").to_string();
        assert!(error.contains("rules[0].spacing"), "{error}");
    }
}
