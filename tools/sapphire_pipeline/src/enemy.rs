//! Enemy documents (M3 B3): `.enemy.ron` cooked to `.senemy`.
//!
//! The document, its checks and the cooked format live in `sapphire_moves`, beside the brains it compiles.
//! This module places them in the cook: a source with a sidecar like any asset (`importer: Enemy(())`) whose
//! one input besides itself is the `.moves.ron` it names, since each brain is checked against that document
//! by name. `cook.rs` holds what it names to the right kinds of asset, on every cook.

use std::path::{Path, PathBuf};

use sapphire_moves::{EnemyDocument, EnemyError, MoveSetDocument, MoveSetError};
use serde::{Deserialize, Serialize};

use crate::cache::Stage;

/// Reads and checks the document. Bump the version whenever that changes output for the same document.
pub const IMPORTER: Stage = Stage { name: "enemy document", version: 1 };

/// The `.senemy` writer. Bump the version whenever it changes output for the same documents.
pub const COOKER: Stage = Stage { name: "senemy", version: 1 };

/// What an enemy source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".enemy.ron";

/// Import settings for enemy documents: none yet. A struct, so settings can join without changing existing
/// sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnemySettings {}

/// An enemy document that can't cook.
#[derive(Debug, thiserror::Error)]
pub enum EnemyCookError {
    #[error("the document isn't UTF-8")]
    Encoding(#[from] std::str::Utf8Error),
    #[error(transparent)]
    Enemy(#[from] EnemyError),
    #[error("its moves, {}", path.display())]
    Moves {
        path: PathBuf,
        #[source]
        source: MoveSetError,
    },
}

/// Whether `path` names an enemy source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file an enemy source at `relative` writes: `warrior.enemy.ron` → `warrior.senemy`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Reads and checks a document's bytes.
pub fn read(bytes: &[u8]) -> Result<EnemyDocument, EnemyCookError> {
    Ok(EnemyDocument::parse(std::str::from_utf8(bytes)?)?)
}

/// Cooks a document's bytes against its moves document's, read from `moves_path`.
pub fn cook(bytes: &[u8], moves_path: &Path, moves: &[u8]) -> Result<Vec<u8>, EnemyCookError> {
    let document = read(bytes)?;
    let moves_error = |source| EnemyCookError::Moves { path: moves_path.to_owned(), source };
    let text = std::str::from_utf8(moves)?;
    let moves = MoveSetDocument::parse(text).map_err(moves_error)?;
    Ok(document.compile(&moves)?.to_cooked()?.into_vec())
}

#[cfg(test)]
mod tests {
    use sapphire_moves::{ENEMY_EXTENSION, Enemy};

    use super::*;

    #[test]
    fn a_source_is_known_by_its_whole_suffix() {
        assert!(is_source(Path::new("combat/warrior.enemy.ron")));
        assert!(is_source(Path::new("WARRIOR.ENEMY.RON")));
        assert!(!is_source(Path::new("combat/enemy.ron")));
    }

    #[test]
    fn a_source_cooks_beside_itself() {
        let output = output_path(Path::new("combat/warrior.enemy.ron"), ENEMY_EXTENSION);
        assert_eq!(output, Path::new("combat/warrior.senemy"));
    }

    /// The shipped warrior cooks against its shipped moves, and what it cooks reads back.
    #[test]
    fn the_warrior_cooks_against_its_moves() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/combat/enemy");
        let enemy = std::fs::read(root.join("warrior.enemy.ron")).expect("shipped");
        let moves_path = root.join("warrior.moves.ron");
        let moves = std::fs::read(&moves_path).expect("shipped");
        let cooked = cook(&enemy, &moves_path, &moves).expect("cooks");
        let mut aligned = sapphire_serialize::cooked::AlignedVec::<16>::new();
        aligned.extend_from_slice(&cooked);
        let warrior = Enemy::from_cooked(&aligned).expect("reads");
        assert_eq!(warrior.mind.brains().len(), 1);
        assert_eq!(warrior.mind.phases().len(), 4);
    }
}
