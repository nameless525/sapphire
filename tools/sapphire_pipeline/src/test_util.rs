//! Test helpers.

use std::path::{Path, PathBuf};

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
