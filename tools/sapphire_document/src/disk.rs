//! Writing a person's files: all of it or none of it, and over nothing unchecked.
//!
//! A save goes through a [`Temporary`], renamed over the file, and a new file through [`create_whole`], which
//! links it into place because a link, unlike a rename, fails rather than replace what's there.
//! [`move_whole`] is the same for a move. These are the rules the cooker's own sidecars keep
//! (`sapphire_pipeline::meta`).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
        let (mut file, temporary) = Self::create(target)?;
        if let Ok(metadata) = fs::metadata(target) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(text.as_bytes())?;
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
/// already. Killed mid-write, it leaves at most a hidden file, which scans pass over.
pub fn create_whole(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let count = COUNT.fetch_add(1, Ordering::Relaxed);
    let staged = path.with_file_name(format!(".{name}.{}-{count}.tmp", std::process::id()));
    let linked = fs::write(&staged, bytes).map(|()| fs::hard_link(&staged, path));
    // The staged file is only ever a copy: its link, if any, now holds the bytes.
    let _ = fs::remove_file(&staged);
    match linked? {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        // A file system without hard links: `create_new` never replaces a file either, but shows it before
        // its bytes are written.
        Err(_) => match fs::OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => file.write_all(bytes).map(|()| true),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error),
        },
    }
}

/// Moves a file to a name nothing's at: `false`, and nothing moved, if something's there.
pub fn move_whole(from: &Path, to: &Path) -> io::Result<bool> {
    let is_link = fs::symlink_metadata(from)?.is_symlink();
    if !is_link {
        match fs::hard_link(from, to) {
            Ok(()) => return fs::remove_file(from).map(|()| true),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
            Err(_) => {}
        }
    }
    // A link stays a link, and a file system without hard links has only this: a look, then a rename.
    if fs::symlink_metadata(to).is_ok() {
        return Ok(false);
    }
    fs::rename(from, to).map(|()| true)
}
