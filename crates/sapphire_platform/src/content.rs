//! Read-only access to packaged content, and to the files a patch placed over it.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::Arc;

use sdl3_sys::error::SDL_ClearError;
use sdl3_sys::iostream::{
    SDL_CloseIO, SDL_GetIOSize, SDL_GetIOStatus, SDL_IO_SEEK_CUR, SDL_IO_SEEK_END, SDL_IO_SEEK_SET,
    SDL_IO_STATUS_ERROR, SDL_IOFromFile, SDL_IOStream, SDL_ReadIO, SDL_SeekIO,
};

use crate::sdl;

/// Environment variable that replaces the default content root on desktop, so
/// development builds can run against a cooked-content directory in the repository.
pub const CONTENT_DIR_ENV: &str = "SAPPHIRE_CONTENT_DIR";

/// Errors from [`ContentFs`].
#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    #[error("invalid content path {path:?}: {reason}")]
    InvalidPath { path: String, reason: &'static str },
    #[error("cannot open content file {path:?}: {message}")]
    Open { path: String, message: String },
    #[error("cannot read content file {path:?}")]
    Read {
        path: String,
        #[source]
        source: io::Error,
    },
}

/// The read-only content root shipped with the app (the `pkg://` root of spec §5.1).
///
/// Paths must mean the same thing on every platform: relative and `/`-separated, with
/// components made only of lowercase `a-z`, `0-9`, `.`, `_` and `-`, not ending in `.`,
/// and not a Windows device name (`con`, `nul`, `com1`, ... with or without an
/// extension). Anything else is rejected with [`ContentError::InvalidPath`], so a path
/// that works on a case-insensitive desktop can't fail on Android, and no path escapes
/// the root. Content is resolved against:
///
/// - **Desktop:** `$SAPPHIRE_CONTENT_DIR/` if set, otherwise `content/` next to the
///   executable (`SDL_GetBasePath`). Inside a macOS `.app` that is
///   `Contents/Resources/content/`.
/// - **iOS:** `content/` in the app bundle.
/// - **Android:** `content/` inside the APK's assets. SDL first looks for the same
///   relative path under the app's internal storage directory and only then in the
///   APK, so the engine must never create a `content/` directory in internal storage
///   unless it means to override packaged files.
///
/// **Patches (M4 P4):** [`Self::with_files_over`] places files over the content by path: a patch's packs and
/// documents, and its manifest, each a file in a writable directory of its own. A path placed over is read from
/// its file, never from the root, by every reader of the content (the IO thread, a game's documents, the release
/// manifest), so what a patch installed is what mounts.
///
/// Opening files is safe from any thread, so the IO thread can share a clone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentFs {
    /// SDL path prefix, always ending in a separator.
    root: String,
    /// Files placed over the root's, by content path: absolute paths.
    over: Arc<BTreeMap<String, PathBuf>>,
}

impl ContentFs {
    pub(crate) fn new(mut root: String) -> Self {
        if !root.ends_with(['/', '\\']) {
            root.push('/');
        }
        Self { root, over: Arc::default() }
    }

    /// The same content with `files` placed over it: each content path (`shared.spack`, `release.smanifest`,
    /// `documents/ui/menus.ron`) read from its file, an absolute path, instead of the root's. Only these: what
    /// was placed over this content before isn't kept. A path content can't name ([`Self::open`]'s rules), such
    /// as `NOTICES.txt`, is left out, since nothing reads it here; a file that isn't absolute is refused.
    pub fn with_files_over(
        &self,
        files: impl IntoIterator<Item = (String, PathBuf)>,
    ) -> Result<Self, ContentError> {
        let mut over = BTreeMap::new();
        for (path, file) in files {
            if validate_path(&path).is_err() {
                continue;
            }
            if !file.is_absolute() {
                return Err(ContentError::InvalidPath {
                    path,
                    reason: "a file placed over content is absolute",
                });
            }
            over.insert(path, file);
        }
        Ok(Self { root: self.root.clone(), over: Arc::new(over) })
    }

    /// The file placed over `path`, if one is.
    #[must_use]
    pub fn file_over(&self, path: &str) -> Option<&Path> {
        self.over.get(path).map(PathBuf::as_path)
    }

    /// Whether any file is placed over the content.
    #[must_use]
    pub fn has_files_over(&self) -> bool {
        !self.over.is_empty()
    }

    /// The root SDL resolves content paths against: a directory on desktop and iOS, an
    /// APK-asset-relative prefix on Android.
    pub fn root(&self) -> &str {
        &self.root
    }

    /// Opens a content file for streaming reads.
    pub fn open(&self, path: &str) -> Result<ContentFile, ContentError> {
        validate_path(path)?;
        let full = match self.file_over(path) {
            // An absolute path, which SDL opens as a plain file everywhere, Android included.
            Some(file) => sdl::c_string_lossy(&file.to_string_lossy()),
            None => sdl::c_string_lossy(&format!("{}{path}", self.root)),
        };
        // SAFETY: both arguments are valid C strings; SDL_IOFromFile is safe to call
        // from any thread.
        let stream = unsafe { SDL_IOFromFile(full.as_ptr(), c"rb".as_ptr()) };
        let stream = NonNull::new(stream)
            .ok_or_else(|| ContentError::Open { path: path.to_owned(), message: sdl::last_error() })?;
        Ok(ContentFile { stream })
    }

    /// Reads a whole content file.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, ContentError> {
        let mut file = self.open(path)?;
        let read_error = |source| ContentError::Read { path: path.to_owned(), source };
        let len = file.len().map_err(read_error)?;
        let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
        file.read_to_end(&mut bytes).map_err(read_error)?;
        Ok(bytes)
    }
}

/// Windows device names, reserved in every directory and with any extension.
const RESERVED_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
    "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Accepts only paths that name the same file on every platform's file system and in
/// APK assets: relative, `/`-separated, with components of lowercase ASCII letters,
/// digits, `.`, `_` and `-` that neither end in `.` nor use a Windows reserved name.
/// That also means a path can never escape the content root.
pub(crate) fn validate_path(path: &str) -> Result<(), ContentError> {
    let reason = if path.is_empty() {
        Some("path is empty")
    } else if path.starts_with('/') {
        Some("absolute paths are not allowed")
    } else {
        path.split('/').find_map(component_error)
    };
    match reason {
        Some(reason) => Err(ContentError::InvalidPath { path: path.to_owned(), reason }),
        None => Ok(()),
    }
}

fn component_error(component: &str) -> Option<&'static str> {
    let allowed = |c: char| matches!(c, 'a'..='z' | '0'..='9' | '.' | '_' | '-');
    if component.is_empty() {
        Some("path has an empty component")
    } else if !component.chars().all(allowed) {
        Some("components may only contain a-z, 0-9, '.', '_' and '-'")
    } else if component.ends_with('.') {
        // Also rejects `.` and `..`.
        Some("components may not end in '.'")
    } else if RESERVED_NAMES.contains(&component.split('.').next().unwrap_or(component)) {
        Some("components may not use a reserved Windows device name")
    } else {
        None
    }
}

/// An open content file. Implements [`Read`] and [`Seek`]; closed on drop.
///
/// It may move to another thread (the IO thread), but is not shared between threads.
///
/// This is the portable path, and the one that reads from inside the APK on Android.
/// Positioned reads and memory-mapping of pack files belong to the async IO layer
/// (M1); `ContentFile` stays its fallback for APK assets.
#[derive(Debug)]
pub struct ContentFile {
    stream: NonNull<SDL_IOStream>,
}

// SAFETY: SDL allows an SDL_IOStream to be used from any thread as long as no two
// threads use it at once. `ContentFile` is not `Sync` and every operation takes
// `&mut self`, so uses are serialized.
unsafe impl Send for ContentFile {}

impl ContentFile {
    /// The file's size in bytes.
    pub fn len(&mut self) -> io::Result<u64> {
        // SAFETY: `self.stream` is open until drop.
        let size = unsafe { SDL_GetIOSize(self.stream.as_ptr()) };
        u64::try_from(size).map_err(|_| io::Error::other(sdl::last_error()))
    }

    /// Whether the file is empty.
    pub fn is_empty(&mut self) -> io::Result<bool> {
        Ok(self.len()? == 0)
    }
}

impl Read for ContentFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        // SAFETY: `self.stream` is open and `buf` is valid for `buf.len()` bytes of writes.
        let n = unsafe { SDL_ReadIO(self.stream.as_ptr(), buf.as_mut_ptr().cast::<c_void>(), buf.len()) };
        // SAFETY: as above.
        if n == 0 && unsafe { SDL_GetIOStatus(self.stream.as_ptr()) } == SDL_IO_STATUS_ERROR {
            return Err(io::Error::other(sdl::last_error()));
        }
        Ok(n)
    }
}

impl Seek for ContentFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let (offset, whence) = match pos {
            SeekFrom::Start(offset) => (
                i64::try_from(offset)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "seek offset overflows i64"))?,
                SDL_IO_SEEK_SET,
            ),
            SeekFrom::Current(offset) => (offset, SDL_IO_SEEK_CUR),
            SeekFrom::End(offset) => (offset, SDL_IO_SEEK_END),
        };
        // SAFETY: `self.stream` is open until drop.
        let position = unsafe { SDL_SeekIO(self.stream.as_ptr(), offset, whence) };
        u64::try_from(position).map_err(|_| io::Error::other(sdl::last_error()))
    }
}

impl Drop for ContentFile {
    fn drop(&mut self) {
        // SAFETY: we own the stream and close it exactly once. Closing a read-only
        // stream can't lose data, so its result is irrelevant.
        unsafe { SDL_CloseIO(self.stream.as_ptr()) };
    }
}

/// Creates SDL's Android `AAssetManager` on the calling thread (the SDL thread, from
/// `Platform::new`). SDL creates it lazily on the first file open, without
/// synchronization, so two first opens racing on different threads could both build
/// it. No-op elsewhere.
pub(crate) fn warm_up() {
    if cfg!(target_os = "android") {
        // A relative path SDL looks up in internal storage first and then, failing that,
        // in the APK, which is the step that creates the asset manager. The file doesn't
        // exist, so the open fails; only its side effect matters.
        // SAFETY: valid C strings; SDL_IOFromFile is safe to call from any thread.
        let stream = unsafe { SDL_IOFromFile(c"sapphire_asset_manager_probe".as_ptr(), c"rb".as_ptr()) };
        if !stream.is_null() {
            // SAFETY: the stream was just opened and is closed once.
            unsafe { SDL_CloseIO(stream) };
        }
        SDL_ClearError();
    }
}

/// The default content root for this platform (see [`ContentFs`]).
pub(crate) fn default_root(base_path: Option<String>) -> String {
    if cfg!(target_os = "android") {
        return String::from("content/");
    }
    // An empty override is ignored rather than turned into the filesystem root.
    if !cfg!(target_os = "ios")
        && let Some(dir) = std::env::var_os(CONTENT_DIR_ENV).filter(|dir| !dir.is_empty())
    {
        return dir.to_string_lossy().into_owned();
    }
    // An unknown base path leaves a relative root, which SDL resolves against the
    // working directory.
    format!("{}content/", base_path.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_portable_relative_paths() {
        for path in [
            "boot.pack",
            "ui/fonts/main-bold_2.ttf",
            "a/b.c/d-e_f",
            "samples/mannequin.smesh",
            "com10/console.txt",
        ] {
            assert!(validate_path(path).is_ok(), "{path}");
        }
    }

    #[test]
    fn rejects_escaping_and_ambiguous_paths() {
        for path in [
            "",
            "/etc/passwd",
            "../secret",
            "a/../../b",
            "a/..",
            "./a",
            "a/.",
            "a//b",
            "a/",
            "a\\..\\b",
            "C:/Windows",
            "content://com.example/file",
            "a\0b",
        ] {
            assert!(
                matches!(validate_path(path), Err(ContentError::InvalidPath { .. })),
                "{path:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_paths_that_differ_across_file_systems() {
        for path in [
            // Case-insensitive on desktop, case-sensitive in APKs and on Linux.
            "Boot.pack",
            "ui/Fonts/main.ttf",
            // Characters with platform-specific meaning or normalization.
            "a b",
            "caf\u{e9}.txt",
            "a?b",
            "a*b",
            "a<b",
            "a|b",
            "a\"b",
            // Windows strips trailing dots and spaces.
            "boot.",
            "dir./file",
            "trailing ",
            // Windows device names, with or without an extension.
            "con",
            "nul.txt",
            "aux/file",
            "maps/prn.tar.gz",
            "com1",
            "com9.log",
            "lpt1",
            "lpt9.bin",
        ] {
            assert!(
                matches!(validate_path(path), Err(ContentError::InvalidPath { .. })),
                "{path:?} should be rejected"
            );
        }
    }

    #[test]
    fn root_always_ends_with_a_separator() {
        assert_eq!(ContentFs::new(String::from("/data/content")).root(), "/data/content/");
        assert_eq!(ContentFs::new(String::from("content/")).root(), "content/");
    }

    #[test]
    fn reads_and_seeks_files_through_sdl() {
        let dir = std::env::temp_dir().join(format!("sapphire_platform_content_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).expect("create temp content dir");
        std::fs::write(dir.join("sub/data.bin"), b"0123456789").expect("write temp content file");

        let fs = ContentFs::new(dir.to_string_lossy().into_owned());
        assert_eq!(fs.read("sub/data.bin").expect("read"), b"0123456789");

        let mut file = fs.open("sub/data.bin").expect("open");
        assert_eq!(file.len().expect("len"), 10);
        assert_eq!(file.seek(SeekFrom::End(-3)).expect("seek"), 7);
        let mut tail = String::new();
        file.read_to_string(&mut tail).expect("read tail");
        assert_eq!(tail, "789");

        assert!(matches!(fs.read("sub/missing.bin"), Err(ContentError::Open { .. })));
        assert!(matches!(fs.read("../data.bin"), Err(ContentError::InvalidPath { .. })));
        // Windows can't delete a directory holding an open file.
        drop(file);
        std::fs::remove_dir_all(&dir).expect("remove temp content dir");
    }

    /// A patch's file is what every reader gets for its path, and the root's file for every other path.
    #[test]
    fn files_placed_over_content_are_read_in_its_place() {
        let dir = std::env::temp_dir().join(format!("sapphire_platform_over_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("root/documents")).expect("create temp content dir");
        std::fs::create_dir_all(dir.join("patch")).expect("create temp patch dir");
        std::fs::write(dir.join("root/shared.spack"), b"shipped pack").expect("write");
        std::fs::write(dir.join("root/documents/menus.ron"), b"shipped menus").expect("write");
        std::fs::write(dir.join("patch/0a1b"), b"patched pack").expect("write");

        let shipped = ContentFs::new(dir.join("root").to_string_lossy().into_owned());
        let patched = shipped
            .with_files_over([("shared.spack".to_owned(), dir.join("patch/0a1b"))])
            .expect("a pack placed over");
        assert_eq!(patched.root(), shipped.root());
        assert!(patched.has_files_over() && !shipped.has_files_over());
        assert_eq!(patched.read("shared.spack").expect("read"), b"patched pack");
        let mut reader = patched.open_reader("shared.spack").expect("open");
        let mut bytes = [0; 7];
        reader.read_exact_at(5, &mut bytes).expect("a positioned read");
        assert_eq!(&bytes, b"ed pack");
        assert_eq!(patched.read("documents/menus.ron").expect("read"), b"shipped menus");
        assert_eq!(shipped.read("shared.spack").expect("read"), b"shipped pack");

        let placed = |path: &str, file: PathBuf| shipped.with_files_over([(path.to_owned(), file)]);
        let unnamed = placed("NOTICES.txt", dir.join("patch/0a1b")).expect("left out");
        assert!(!unnamed.has_files_over(), "a path content can't name is nothing to read");
        let relative = placed("shared.spack", PathBuf::from("0a1b"));
        assert!(matches!(relative, Err(ContentError::InvalidPath { .. })));
        drop(reader);
        std::fs::remove_dir_all(&dir).expect("remove temp dirs");
    }
}
