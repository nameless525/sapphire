//! Move set documents (`.moves.ron`) cook to `.smoves` (M2 G2, spec §5.27): frame data in ticks and by
//! name, as a person or the timeline editor writes it, checked and resolved into what the simulation
//! reads ([`sapphire_moves`]).
//!
//! A cook names everything wrong with a document at once, by move and track, so a typo in frame data
//! says where it is. A move's tables are baked from the keys in its document; M2 N1's clips will reach
//! this same cook through [`MoveSetDocument::compile_with`], and their files join the asset's key then.

use std::path::{Path, PathBuf};

use sapphire_moves::{MoveSetDocument, MoveSetError};
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

/// Reading the document, as cache keys know it. Bump the version whenever its output changes for the
/// same file.
pub const IMPORTER: Stage = Stage { name: "move set document", version: 1 };

/// Checking and resolving it, and baking its keyed tables. Bump the version whenever they change
/// output for the same document.
pub const COOKER: Stage = Stage { name: "smoves", version: 1 };

/// What a move set source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".moves.ron";

/// Import settings for move sets: none yet. A struct, so settings can join without changing existing
/// sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MoveSetSettings {}

/// A document that didn't cook.
#[derive(Debug, thiserror::Error)]
pub enum MoveSetCookError {
    #[error("the document isn't UTF-8")]
    NotUtf8(#[source] std::str::Utf8Error),
    #[error(transparent)]
    Moves(#[from] MoveSetError),
}

/// Whether `path` names a move set source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file a move set source at `relative` writes: `fighter.moves.ron` → `fighter.smoves`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Cooks a document's bytes.
pub fn cook(bytes: &[u8]) -> Result<Vec<u8>, MoveSetCookError> {
    let text = std::str::from_utf8(bytes).map_err(MoveSetCookError::NotUtf8)?;
    let set = MoveSetDocument::parse(text)?.compile()?;
    Ok(set.to_cooked()?.into_vec())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sapphire_assets::AssetKind;
    use sapphire_moves::{MOVE_SET_EXTENSION, MoveSet};
    use sapphire_serialize::cooked::AlignedVec;

    use super::*;
    use crate::cook::{Cook, CookError, CookOptions, CookStatus, cook_dir};

    const SLICE: &str = include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron");

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sapphire_moves_cook_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn aligned(bytes: &[u8]) -> AlignedVec {
        let mut aligned = AlignedVec::new();
        aligned.extend_from_slice(bytes);
        aligned
    }

    #[test]
    fn sources_are_known_by_their_suffix() {
        assert!(is_source(Path::new("content/fighter.moves.ron")));
        assert!(is_source(Path::new("FIGHTER.MOVES.RON")));
        assert!(!is_source(Path::new("fighter.ron")));
        assert!(!is_source(Path::new("fighter.material.ron")));
        assert_eq!(
            output_path(Path::new("characters/fighter.moves.ron"), MOVE_SET_EXTENSION),
            Path::new("characters/fighter.smoves")
        );
    }

    /// Through the whole cook: a sidecar, an output by the source's name, the cache, and bytes the
    /// simulation reads, identical to compiling the document directly.
    #[test]
    fn a_document_cooks_through_a_content_tree_and_its_cache() {
        let root = temp_dir("tree");
        let (content, out, cache) = (root.join("content"), root.join("out"), root.join("cache"));
        fs::create_dir_all(content.join("fighter")).unwrap();
        fs::write(content.join("fighter/slice.moves.ron"), SLICE).unwrap();
        let options = CookOptions { cook: Cook::Desktop, cache: Some(&cache), locked: false };

        let report = cook_dir(&content, &out, &options).unwrap();
        assert_eq!(report.assets.len(), 1);
        assert_eq!(report.assets[0].status, CookStatus::Cooked);
        assert_eq!(report.created_sidecars.len(), 1);
        let sidecar = fs::read_to_string(&report.created_sidecars[0]).unwrap();
        assert!(sidecar.contains("MoveSet"), "{sidecar}");
        let output = out.join("fighter/slice.smoves");
        assert_eq!(report.assets[0].output, output);

        let cooked = aligned(&fs::read(&output).unwrap());
        let set = MoveSet::from_cooked(&cooked).unwrap();
        let direct = MoveSetDocument::parse(SLICE).unwrap().compile().unwrap();
        assert_eq!(set, direct);
        assert_eq!(*cooked, *direct.to_cooked().unwrap());
        assert_eq!(AssetKind::MoveSet.references(&cooked).unwrap(), []);

        // Nothing changed: the output stands. A locked run finds the sidecar it needs.
        let locked = CookOptions { locked: true, ..options };
        let again = cook_dir(&content, &out, &locked).unwrap();
        assert_eq!(again.assets[0].status, CookStatus::UpToDate);

        // An edit cooks again, and a stale output goes with its source.
        let edited = SLICE.replacen("startup: 5", "startup: 6", 1);
        fs::write(content.join("fighter/slice.moves.ron"), edited).unwrap();
        let cooked_again = cook_dir(&content, &out, &locked).unwrap();
        assert_eq!(cooked_again.assets[0].status, CookStatus::Cooked);
        let cooked = aligned(&fs::read(&output).unwrap());
        assert_eq!(MoveSet::from_cooked(&cooked).unwrap().moves()[0].startup, 6);

        fs::remove_dir_all(root).unwrap();
    }

    /// The cook's error names the file, and the document's own errors name the move and track.
    #[test]
    fn a_bad_document_fails_the_cook_by_name() {
        let root = temp_dir("bad");
        let (content, out) = (root.join("content"), root.join("out"));
        fs::create_dir_all(&content).unwrap();
        let bad = SLICE.replacen("from: 10, to: 24", "from: 10, to: 40", 1);
        fs::write(content.join("bad.moves.ron"), bad).unwrap();
        let options = CookOptions { cook: Cook::Mobile, cache: None, locked: false };
        let error = cook_dir(&content, &out, &options).unwrap_err();
        assert!(matches!(error, CookError::MoveSet { .. }), "{error:?}");
        assert!(error.to_string().contains("bad.moves.ron"), "{error}");
        let source = std::error::Error::source(&error).unwrap().to_string();
        assert!(source.contains("light_1") && source.contains("cancels"), "{source}");
        assert!(source.contains("10..40"), "{source}");

        fs::write(content.join("bad.moves.ron"), "( this isn't RON").unwrap();
        let error = cook_dir(&content, &out, &options).unwrap_err();
        assert!(matches!(error, CookError::MoveSet { .. }), "{error:?}");

        fs::write(content.join("bad.moves.ron"), [0xff_u8, 0xfe]).unwrap();
        let error = cook_dir(&content, &out, &options).unwrap_err();
        assert!(matches!(error, CookError::MoveSet { .. }), "{error:?}");

        fs::remove_dir_all(root).unwrap();
    }

    /// Packs hold a move set like any other asset.
    #[test]
    fn a_cooked_set_packs() {
        let root = temp_dir("pack");
        let (content, out, cache) = (root.join("content"), root.join("out"), root.join("cache"));
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("slice.moves.ron"), SLICE).unwrap();
        let options = CookOptions { cook: Cook::Desktop, cache: Some(&cache), locked: false };
        let report = cook_dir(&content, &out, &options).unwrap();
        assert_eq!(crate::pack::kind(&report.assets[0].output), Some(AssetKind::MoveSet));
        assert_eq!(crate::pack::codec(AssetKind::MoveSet), sapphire_assets::pack::Codec::Lz4);
        fs::remove_dir_all(root).unwrap();
    }
}
