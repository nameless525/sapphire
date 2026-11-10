//! Where a download keeps what it fetched until it's installed.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use sapphire_assets::manifest;

use crate::hex;

/// A download's staging directory, held by one patcher at a time:
///
/// ```text
/// <dir>/release.smanifest        the release being fetched, as the host signed it, written durably
/// <dir>/tables/<header hash>     a changed pack's header and table of contents: the bytes before its data
/// <dir>/blocks/<blake3>          one block's stored bytes
/// <dir>/files/<blake3>           one file outside the packs
/// <dir>/lock                     held while it's open
/// ```
///
/// Hashes are in hex. That is all a download's state: the manifest says what's being fetched, and every
/// other file is named by its hash, so whether it's whole is a hash away. Nothing here is trusted for being
/// here: a download accepts the manifest again from the host on every run and hashes each file before it
/// counts, and an installer checks what it assembles. A file is written beside its place (`<name>.part`)
/// and renamed into it, so a killed process leaves a `.part`, which the next open removes, never a torn
/// file under a hash. A power cut can tear one (only the manifest is flushed), and its hash then fails.
#[derive(Debug)]
pub struct Staging {
    dir: PathBuf,
    /// Held until dropped; the OS lets go of it if the process dies.
    _lock: File,
}

/// A staging directory that can't be used.
#[derive(Debug, thiserror::Error)]
pub enum StagingError {
    #[error("{} is held by another patcher", dir.display())]
    Busy { dir: PathBuf },
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

const TABLES: &str = "tables";
const BLOCKS: &str = "blocks";
const FILES: &str = "files";
const LOCK: &str = "lock";
const PART: &str = "part";

impl Staging {
    /// Opens `dir`, making it if it isn't there, and holds it. Fails with [`StagingError::Busy`] while
    /// another `Staging` holds it, in this process or another.
    pub fn open(dir: &Path) -> Result<Self, StagingError> {
        let io_error = |path: &Path| {
            let path = path.to_owned();
            move |source| StagingError::Io { path, source }
        };
        for sub in [TABLES, BLOCKS, FILES] {
            fs::create_dir_all(dir.join(sub)).map_err(io_error(&dir.join(sub)))?;
        }
        let lock_path = dir.join(LOCK);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(io_error(&lock_path))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(StagingError::Busy { dir: dir.to_owned() }),
            Err(TryLockError::Error(source)) => return Err(StagingError::Io { path: lock_path, source }),
        }
        let staging = Self { dir: dir.to_owned(), _lock: lock };
        for sub in [TABLES, BLOCKS, FILES] {
            staging.remove_where(sub, |name| name.ends_with(&format!(".{PART}")))?;
        }
        Ok(staging)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where the release being fetched is kept.
    pub fn manifest_path(&self) -> PathBuf {
        self.dir.join(manifest::FILE_NAME)
    }

    /// The staged manifest's bytes, if one is staged: as the host signed them, to be checked again by
    /// whatever reads them.
    pub fn manifest(&self) -> Result<Option<Vec<u8>>, StagingError> {
        let path = self.manifest_path();
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(StagingError::Io { path, source }),
        }
    }

    /// Where the tables of the pack whose header hash is `header_hash` are kept.
    pub fn table_path(&self, header_hash: &[u8; 32]) -> PathBuf {
        self.dir.join(TABLES).join(hex(header_hash))
    }

    /// Where a block whose stored bytes hash to `hash` is kept.
    pub fn block_path(&self, hash: &[u8; 32]) -> PathBuf {
        self.dir.join(BLOCKS).join(hex(hash))
    }

    /// Where a file outside the packs whose blake3 is `hash` is kept.
    pub fn file_path(&self, hash: &[u8; 32]) -> PathBuf {
        self.dir.join(FILES).join(hex(hash))
    }

    /// Stages `bytes` as the release being fetched, through `sapphire_platform::durable_replace`, unless they
    /// are staged already.
    pub(crate) fn set_manifest(&self, bytes: &[u8]) -> Result<(), StagingError> {
        if self.manifest()?.as_deref() == Some(bytes) {
            return Ok(());
        }
        let path = self.manifest_path();
        sapphire_platform::durable_replace(&path, bytes).map_err(|source| StagingError::Io { path, source })
    }

    /// What `parse` makes of the bytes at `path`, if they're there and it takes them. Bytes it refuses are
    /// removed, so they're fetched again.
    pub(crate) fn read_checked<T>(
        &self,
        path: &Path,
        parse: impl FnOnce(&[u8]) -> Option<T>,
    ) -> Result<Option<T>, StagingError> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(StagingError::Io { path: path.to_owned(), source }),
        };
        let parsed = parse(&bytes);
        if parsed.is_none() {
            let shown = path.display();
            tracing::warn!(target: "sapphire::patch", path = %shown, "a staged file isn't whole");
            fs::remove_file(path).map_err(|source| StagingError::Io { path: path.to_owned(), source })?;
        }
        Ok(parsed)
    }

    /// Writes `bytes`, already checked, to `path`: beside it first, then renamed into place.
    pub(crate) fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), StagingError> {
        let part = path.with_added_extension(PART);
        let io_error = |source| StagingError::Io { path: path.to_owned(), source };
        fs::write(&part, bytes).map_err(io_error)?;
        fs::rename(&part, path).map_err(io_error)
    }

    /// Removes every staged table, block and file whose hash isn't in its set.
    pub(crate) fn keep_only(
        &self,
        tables: &HashSet<[u8; 32]>,
        blocks: &HashSet<[u8; 32]>,
        files: &HashSet<[u8; 32]>,
    ) -> Result<(), StagingError> {
        for (sub, keep) in [(TABLES, tables), (BLOCKS, blocks), (FILES, files)] {
            let kept: HashSet<String> = keep.iter().map(hex).collect();
            self.remove_where(sub, |name| !kept.contains(name))?;
        }
        Ok(())
    }

    /// Removes everything staged: what a download does when the install is already the host's release.
    pub(crate) fn clear(&self) -> Result<(), StagingError> {
        self.clear_with(|path| fs::remove_file(path))
    }

    /// [`Self::clear`], removing each file through `remove`: the manifest last, so a staging that still names
    /// a release is one an installer can still finish, or clear.
    pub(crate) fn clear_with(
        &self,
        mut remove: impl FnMut(&Path) -> io::Result<()>,
    ) -> Result<(), StagingError> {
        for sub in [TABLES, BLOCKS, FILES] {
            self.remove_where_with(sub, |_| true, &mut remove)?;
        }
        let path = self.manifest_path();
        match remove(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                Err(StagingError::Io { path, source: error })
            }
            _ => Ok(()),
        }
    }

    /// Removes each file in `sub` whose name `remove` picks.
    fn remove_where(&self, sub: &str, pick: impl Fn(&str) -> bool) -> Result<(), StagingError> {
        self.remove_where_with(sub, pick, &mut |path: &Path| fs::remove_file(path))
    }

    fn remove_where_with(
        &self,
        sub: &str,
        pick: impl Fn(&str) -> bool,
        remove: &mut impl FnMut(&Path) -> io::Result<()>,
    ) -> Result<(), StagingError> {
        let dir = self.dir.join(sub);
        let io_error = |path: &Path| {
            let path = path.to_owned();
            move |source| StagingError::Io { path, source }
        };
        for entry in fs::read_dir(&dir).map_err(io_error(&dir))? {
            let path = entry.map_err(io_error(&dir))?.path();
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            if pick(name) {
                remove(&path).map_err(io_error(&path))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of the test's own, removed afterward.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("sapphire_patch-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn one_patcher_holds_a_staging_directory_at_a_time() {
        let dir = TestDir::new("held");
        let first = Staging::open(&dir.0).expect("opens");
        assert!(matches!(Staging::open(&dir.0), Err(StagingError::Busy { .. })));
        drop(first);
        Staging::open(&dir.0).expect("opens once let go");
    }

    #[test]
    fn opening_removes_what_a_kill_left_half_written_and_reading_removes_what_fails_its_check() {
        let dir = TestDir::new("leftovers");
        let staging = Staging::open(&dir.0).expect("opens");
        let good = *blake3::hash(b"good").as_bytes();
        let bad = *blake3::hash(b"bad").as_bytes();
        staging.write(&staging.block_path(&good), b"good").expect("writes");
        fs::write(staging.block_path(&bad), b"torn").expect("writes");
        fs::write(staging.block_path(&bad).with_added_extension(PART), b"half").expect("writes");
        drop(staging);

        let staging = Staging::open(&dir.0).expect("opens");
        assert!(!staging.block_path(&bad).with_added_extension(PART).exists());
        let check =
            |hash: [u8; 32]| move |bytes: &[u8]| (*blake3::hash(bytes).as_bytes() == hash).then_some(());
        assert_eq!(staging.read_checked(&staging.block_path(&good), check(good)).expect("reads"), Some(()));
        assert_eq!(staging.read_checked(&staging.block_path(&bad), check(bad)).expect("reads"), None);
        assert!(!staging.block_path(&bad).exists(), "a file that fails its hash goes");

        staging.keep_only(&HashSet::new(), &HashSet::new(), &HashSet::new()).expect("prunes");
        assert!(!staging.block_path(&good).exists());
    }
}
