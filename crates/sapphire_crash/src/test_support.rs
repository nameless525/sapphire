//! Helpers shared by the unit tests.

use std::fs;
use std::path::{Path, PathBuf};

/// A scratch directory under the system temp dir, removed on drop.
pub(crate) struct ScratchDir(PathBuf);

impl ScratchDir {
    /// `name` must be unique among the crate's tests, which run in parallel in one process.
    pub(crate) fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("sapphire_crash-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create scratch dir");
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
