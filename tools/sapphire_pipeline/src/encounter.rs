//! Encounter documents (M3 B2): `.encounter.ron` cooked to `.sencounter`.
//!
//! The document, its checks and the cooked format live in `sapphire_zone`, as a zone's do. This module only
//! places them in the cook: a source with a sidecar like any asset (`importer: Encounter(())`) and no inputs
//! besides itself. A zone that lists one holds it to the zone's own triggers and spawners, on every cook, in
//! `cook.rs`'s `zone_dependencies`.

use std::path::{Path, PathBuf};

use sapphire_zone::{EncounterDocument, ZoneError};
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

/// Reads and checks the document. Bump the version whenever that changes output for the same document.
pub const IMPORTER: Stage = Stage { name: "encounter document", version: 1 };

/// The `.sencounter` writer. Bump the version whenever it changes output for the same document.
pub const COOKER: Stage = Stage { name: "sencounter", version: 1 };

/// What an encounter source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".encounter.ron";

/// Import settings for encounter documents: none yet. A struct, so settings can join without changing
/// existing sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EncounterSettings {}

/// An encounter document that can't cook.
#[derive(Debug, thiserror::Error)]
pub enum EncounterCookError {
    #[error("the document isn't UTF-8")]
    Encoding(#[from] std::str::Utf8Error),
    #[error(transparent)]
    Encounter(#[from] ZoneError),
}

/// Whether `path` names an encounter source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file an encounter source at `relative` writes: `gate.encounter.ron` → `gate.sencounter`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Reads and checks a document's bytes.
pub fn read(bytes: &[u8]) -> Result<EncounterDocument, EncounterCookError> {
    Ok(EncounterDocument::from_ron(std::str::from_utf8(bytes)?)?)
}

/// Cooks a document's bytes.
pub fn cook(bytes: &[u8]) -> Result<Vec<u8>, EncounterCookError> {
    Ok(read(bytes)?.cook()?.to_cooked()?.into_vec())
}

#[cfg(test)]
mod tests {
    use sapphire_zone::ENCOUNTER_EXTENSION;

    use super::*;

    #[test]
    fn a_source_is_known_by_its_whole_suffix() {
        assert!(is_source(Path::new("zones/gate.encounter.ron")));
        assert!(is_source(Path::new("GATE.ENCOUNTER.RON")));
        assert!(!is_source(Path::new("zones/encounter.ron")));
    }

    #[test]
    fn a_source_cooks_beside_itself() {
        let output = output_path(Path::new("zones/gate.encounter.ron"), ENCOUNTER_EXTENSION);
        assert_eq!(output, Path::new("zones/gate.sencounter"));
    }

    #[test]
    fn a_bad_document_says_which_field() {
        let text = br#"(type: "sapphire::zone::EncounterDocument", version: 1, data: (
            starts_on: "gate", faction: 0, tokens: 2, waves: [(when: Start, spawners: ["north"])],
        ))"#;
        let message = cook(text).expect_err("no side").to_string();
        assert!(message.contains("`faction`"), "{message}");
    }
}
