//! Test helpers.

use std::path::{Path, PathBuf};

use sapphire_assets::material::AnimeCharacterMaterial;
use sapphire_serialize::cooked::AlignedVec;

use crate::mesh::ImportedMaterial;

/// An imported material slot named `name`, with the default generated material.
pub(crate) fn material(name: &str) -> ImportedMaterial {
    ImportedMaterial { name: name.to_owned(), generated: AnimeCharacterMaterial::default() }
}

/// The file at `path`, in memory aligned as cooked data must be.
pub(crate) fn read_aligned(path: &Path) -> AlignedVec {
    let mut bytes = AlignedVec::new();
    bytes.extend_from_slice(&std::fs::read(path).expect("read a cooked file"));
    bytes
}

/// A uniquely named scratch directory that is removed on drop.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("sapphire_pipeline-{}-{name}", std::process::id()));
        // A previous run with the same PID may have left it behind.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
