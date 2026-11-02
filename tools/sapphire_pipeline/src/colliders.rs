//! Collider documents (M2 D3): `.colliders.ron` cooked to `.scol`, a zone's static collision.
//!
//! The document, its checks and the cooked format all live in `sapphire_colliders`, a plain-data
//! crate the runtime reads too. This module only places them in the cook: a source with a sidecar
//! like any asset (`importer: Colliders(())`) and no inputs besides itself.
//!
//! The tool links no collision library: `sapphire_physics` builds its trees from the cooked set at
//! load, and a cook only checks values. That keeps a physics change from rebuilding the cooker
//! (M1 A8's rule), and the set's order and bits free of anything a library does.

use std::path::{Path, PathBuf};

use sapphire_colliders::{ColliderDocument, ColliderError};
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

/// Reads and checks the document. Bump the version whenever that changes output for the same
/// document.
pub const IMPORTER: Stage = Stage { name: "collider document", version: 1 };

/// The `.scol` writer. Bump the version whenever it changes output for the same document.
pub const COOKER: Stage = Stage { name: "scol", version: 1 };

/// What a collider source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".colliders.ron";

/// Import settings for collider documents: none yet. A struct, so settings can join without
/// changing existing sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColliderSettings {}

/// A collider document that can't cook.
#[derive(Debug, thiserror::Error)]
pub enum ColliderCookError {
    #[error("the document isn't UTF-8")]
    Encoding(#[from] std::str::Utf8Error),
    #[error(transparent)]
    Colliders(#[from] ColliderError),
}

/// Whether `path` names a collider source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file a collider source at `relative` writes: `arena.colliders.ron` → `arena.scol`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Cooks a document's bytes.
pub fn cook(bytes: &[u8]) -> Result<Vec<u8>, ColliderCookError> {
    let document = ColliderDocument::from_ron(std::str::from_utf8(bytes)?)?;
    Ok(document.cook()?.to_cooked()?.into_vec())
}

#[cfg(test)]
mod tests {
    use sapphire_colliders::{COLLIDERS_EXTENSION, ColliderSet};

    use super::*;

    #[test]
    fn a_source_is_known_by_its_whole_suffix() {
        assert!(is_source(Path::new("zones/arena.colliders.ron")));
        assert!(is_source(Path::new("ARENA.COLLIDERS.RON")));
        assert!(!is_source(Path::new("zones/arena.ron")));
        assert!(!is_source(Path::new("zones/colliders.ron")));
    }

    #[test]
    fn a_source_cooks_beside_itself() {
        let output = output_path(Path::new("zones/arena.colliders.ron"), COLLIDERS_EXTENSION);
        assert_eq!(output, Path::new("zones/arena.scol"));
    }

    #[test]
    fn what_cooks_reads_back() {
        let document = r#"(
            type: "sapphire::colliders::ColliderDocument",
            version: 1,
            data: (colliders: [(name: "ground", shape: Box(half_extents: (5.0, 0.5, 5.0)), position: (0.0, -0.5, 0.0))]),
        )"#;
        let bytes = cook(document.as_bytes()).unwrap();
        let mut aligned: sapphire_serialize::cooked::AlignedVec = Default::default();
        aligned.extend_from_slice(&bytes);
        assert_eq!(ColliderSet::read(&aligned).unwrap().id_of("ground"), Some(0));
    }
}
