//! Writing a person's files: all of it or none of it, and over nothing unchecked.
//!
//! A save goes through a [`Temporary`], renamed over the file, and a new file through [`create_whole`], which
//! links it into place because a link, unlike a rename, fails rather than replace what's there.
//! [`move_whole`] is the same for a move. These are the rules the cooker's own sidecars keep
//! (`sapphire_pipeline::meta`).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::{fs, io};

/// How many names a save tries for its temporary. Each one taken is someone else's file.
pub const TEMPORARY_NAMES: u32 = 1024;

/// A file of one save's own, written whole beside the one it's to replace and renamed over it: all of it
/// or none of it, keeping the file's mode. Only a name nothing's at will do, so whatever is there already,
/// another save's temporary, a person's file or a link to anything, is left alone, and another editor's save
/// can't write into this one or rename it away. Dropped before it replaces anything, it's gone, and only it.
#[derive(Debug)]
pub struct Temporary {
    pub path: PathBuf,
    replaced: bool,
}

impl Temporary {
    /// Writes `text` beside `target`.
    pub fn beside(target: &Path, text: &str) -> io::Result<Self> {
        Self::beside_bytes(target, text.as_bytes())
    }

    /// Writes arbitrary bytes beside `target`, under the same ownership and replacement rules.
    pub fn beside_bytes(target: &Path, bytes: &[u8]) -> io::Result<Self> {
        let (mut file, temporary) = Self::create(target)?;
        if let Ok(metadata) = fs::metadata(target) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(temporary)
    }

    fn create(target: &Path) -> io::Result<(fs::File, Self)> {
        for attempt in 0..TEMPORARY_NAMES {
            let path = temporary_path(target, attempt);
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((file, Self { path, replaced: false })),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "no name left for a temporary beside it"))
    }

    /// Renames it over `target`.
    pub fn replace(mut self, target: &Path) -> io::Result<()> {
        fs::rename(&self.path, target)?;
        self.replaced = true;
        Ok(())
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.replaced {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// A hidden sibling, which the cook and a watcher's documents pass over. The process's ID keeps two editors
/// off each other's names, for speed: `create_new` is what makes a name this save's own.
#[must_use]
pub fn temporary_path(target: &Path, attempt: u32) -> PathBuf {
    let name = target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    target.with_file_name(format!(".{name}.{}.{attempt}.sapphire_editor", std::process::id()))
}

/// A new file holding `bytes`, whole or not at all, which never replaces one: `false` if something's there
/// already. Killed mid-write, it leaves at most a hidden file, which scans pass over. Requires hard
/// links: a filesystem without atomic no-replace publication fails without exposing a partial file.
pub fn create_whole(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    create_whole_with(path, bytes, |source, destination| fs::hard_link(source, destination))
}

fn create_whole_with(
    path: &Path,
    bytes: &[u8],
    publish: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<bool> {
    let (mut file, temporary) = Temporary::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    match publish(&temporary.path, path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        // Without atomic no-replace publication, fail with the original destination untouched.
        // A create_new/write fallback would expose partial bytes to another editor or cook.
        Err(error) => Err(error),
    }
}

/// Moves a regular file to a name nothing's at: `false`, and nothing moved, if something's there.
/// Requires hard links in the same filesystem; unsupported moves fail without a replacing rename.
/// If removing the source fails after linking, both names remain and the error is returned.
pub fn move_whole(from: &Path, to: &Path) -> io::Result<bool> {
    let kind = fs::symlink_metadata(from)?.file_type();
    if !kind.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "only regular files can be moved without replacement",
        ));
    }
    match fs::hard_link(from, to) {
        Ok(()) => fs::remove_file(from).map(|()| true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        // A check followed by rename could overwrite a file created between the two operations.
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_publication_leaves_no_partial_destination_or_owned_temporary() {
        let target =
            std::env::temp_dir().join(format!("sapphire_document_publication_{}.ron", std::process::id()));
        let result = create_whole_with(&target, b"whole bytes", |staged, target| {
            assert_eq!(fs::read(staged).expect("staged"), b"whole bytes");
            assert!(!target.exists(), "no partially written destination");
            Err(io::Error::new(io::ErrorKind::Unsupported, "no hard links"))
        });
        assert_eq!(result.expect_err("publication refused").kind(), io::ErrorKind::Unsupported);
        assert!(!target.exists());
        assert!(!temporary_path(&target, 0).exists());
    }
}
