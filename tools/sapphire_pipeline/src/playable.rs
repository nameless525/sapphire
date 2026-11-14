//! Playable kinds (M5 C8): `.playable.ron` cooked to `.splayable`, as enemy documents are ([`crate::enemy`]).
//!
//! The document, its checks and the cooked format live in `sapphire_moves`, beside the enemy's. This module
//! places them in the cook: a source with a sidecar like any asset (`importer: Playable(())`) that reads
//! nothing besides itself, since its set of moves is the simulation's, compiled in, and its look is IDs.
//! `cook.rs` holds what it names to the right kinds of asset, on every cook.

use std::path::{Path, PathBuf};

use sapphire_assets::rig::ExpressionPreset;
use sapphire_moves::{PlayableDocument, PlayableError};
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

/// Reads and checks the document. Bump the version whenever that changes output for the same document.
pub const IMPORTER: Stage = Stage { name: "playable document", version: 1 };

/// The `.splayable` writer. Bump the version whenever it changes output for the same document.
pub const COOKER: Stage = Stage { name: "splayable", version: 1 };

/// What a playable kind's source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".playable.ron";

/// Import settings for playable kinds: none yet. A struct, so settings can join without changing existing
/// sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayableSettings {}

/// A playable kind's document that can't cook.
#[derive(Debug, thiserror::Error)]
pub enum PlayableCookError {
    #[error("the document isn't UTF-8")]
    Encoding(#[from] std::str::Utf8Error),
    #[error(transparent)]
    Playable(#[from] PlayableError),
    #[error("`look.expression`: {0:?} is no preset expression a rig knows")]
    Expression(String),
}

/// Whether `path` names a playable kind's source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file a source at `relative` writes: `heroine.playable.ron` → `heroine.splayable`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Reads and checks a document's bytes: what it says by itself, and that its face rests on a preset a rig
/// binds.
pub fn read(bytes: &[u8]) -> Result<PlayableDocument, PlayableCookError> {
    let document = PlayableDocument::parse(std::str::from_utf8(bytes)?)?;
    if let Some(rest) = &document.look.expression
        && ExpressionPreset::from_name(&rest.preset).is_none()
    {
        return Err(PlayableCookError::Expression(rest.preset.clone()));
    }
    Ok(document)
}

/// Cooks a document's bytes.
pub fn cook(bytes: &[u8]) -> Result<Vec<u8>, PlayableCookError> {
    Ok(read(bytes)?.compile().to_cooked()?.into_vec())
}

#[cfg(test)]
mod tests {
    use sapphire_moves::{PLAYABLE_EXTENSION, Playable, RIG_SUB_ASSET};

    use super::*;

    #[test]
    fn a_source_is_known_by_its_whole_suffix() {
        assert!(is_source(Path::new("combat/player/heroine.playable.ron")));
        assert!(is_source(Path::new("HEROINE.PLAYABLE.RON")));
        assert!(!is_source(Path::new("combat/playable.ron")));
    }

    #[test]
    fn a_source_cooks_beside_itself() {
        let output = output_path(Path::new("combat/player/heroine.playable.ron"), PLAYABLE_EXTENSION);
        assert_eq!(output, Path::new("combat/player/heroine.splayable"));
    }

    /// A kind's rig is its mesh's sub-asset by the name a VRM's cook gives it.
    #[test]
    fn the_rig_a_kind_names_is_the_one_a_vrm_cooks() {
        assert_eq!(RIG_SUB_ASSET, crate::import::vrm::RIG.name);
    }

    /// The shipped kinds cook, and what they cook reads back.
    #[test]
    fn the_shipped_kinds_cook() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/combat");
        for kind in ["player/heroine.playable.ron", "hero/hero.playable.ron"] {
            let bytes = std::fs::read(root.join(kind)).expect("shipped");
            let cooked = cook(&bytes).unwrap_or_else(|error| panic!("{kind}: {error}"));
            let mut aligned = sapphire_serialize::cooked::AlignedVec::<16>::new();
            aligned.extend_from_slice(&cooked);
            Playable::from_cooked(&aligned).expect("reads");
        }
    }

    #[test]
    fn a_face_resting_on_no_preset_is_refused() {
        let text = include_str!("../../../crates/sapphire_moves/test_content/slice.playable.ron").replacen(
            "\"relaxed\"",
            "\"smug\"",
            1,
        );
        let error = read(text.as_bytes()).expect_err("no such preset").to_string();
        assert!(error.contains("smug"), "{error}");
    }
}
