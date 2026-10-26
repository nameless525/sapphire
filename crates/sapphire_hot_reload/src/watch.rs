//! [`FileWatcher`]: directories watched for changed files, debounced.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crate::LOG_TARGET;

/// A watched directory, from [`FileWatcher::watch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WatchId(usize);

/// A file that changed in a watched directory: created, written, removed, or renamed from or to
/// its name. Whoever reads it finds out which.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changed {
    pub watch: WatchId,
    pub name: OsString,
}

/// What changed since the last [`FileWatcher::poll`].
#[derive(Clone, Copy, Debug)]
pub struct WatchEvents<'a> {
    /// Each changed file once, in the order first seen.
    pub changed: &'a [Changed],
    /// The backend's errors: lost events, a watched directory gone.
    pub errors: &'a [String],
}

/// Why a watch couldn't start.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error(
        "this build has no file-watching backend (sapphire_hot_reload's `notify` feature, which apps \
         turn on for desktop targets)"
    )]
    Unsupported,
    #[error("can't watch {path:?}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("watching {path:?} failed: {message}")]
    Backend { path: PathBuf, message: String },
}

/// What the backend sends from its thread: one burst's changed paths, or errors.
enum Report {
    Changed(Vec<PathBuf>),
    Errors(Vec<String>),
}

/// Watches directories, not recursively, for files that change, through `notify` 8.2 and a
/// trailing-edge debounce: a burst of events is reported once the watched files have been quiet
/// for the debounce time, however long the burst, so an editor's save or a cook's write to a
/// temporary file and rename into place comes out as one change per name. The debounce runs on
/// a thread of its own that blocks while nothing happens; [`Self::poll`] collects what it
/// reported, and allocates nothing while nothing changed.
pub struct FileWatcher {
    backend: backend::Backend,
    receiver: Receiver<Report>,
    /// Watched directories, canonical, by [`WatchId`].
    dirs: Vec<PathBuf>,
    changed: Vec<Changed>,
    errors: Vec<String>,
}

impl std::fmt::Debug for FileWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileWatcher").field("dirs", &self.dirs).finish_non_exhaustive()
    }
}

impl FileWatcher {
    /// A watcher that reports a burst once its files have been quiet for `debounce`. Fails with
    /// [`WatchError::Unsupported`] in builds without the backend.
    pub fn new(debounce: Duration) -> Result<Self, WatchError> {
        let (sender, receiver) = mpsc::channel();
        let backend = backend::Backend::new(debounce, sender)?;
        Ok(Self { backend, receiver, dirs: Vec::new(), changed: Vec::new(), errors: Vec::new() })
    }

    /// Watches the files in directory `dir`, not its subdirectories. A directory watched already
    /// keeps its ID.
    pub fn watch(&mut self, dir: &Path) -> Result<WatchId, WatchError> {
        // Events name canonical paths on some platforms (FSEvents resolves symbolic links such
        // as macOS's /var), so directories are compared canonical.
        let dir =
            std::fs::canonicalize(dir).map_err(|source| WatchError::Io { path: dir.to_owned(), source })?;
        if let Some(index) = self.dirs.iter().position(|watched| *watched == dir) {
            return Ok(WatchId(index));
        }
        self.backend.watch(&dir).map_err(|message| WatchError::Backend { path: dir.clone(), message })?;
        tracing::debug!(target: LOG_TARGET, dir = %dir.display(), "watching");
        self.dirs.push(dir);
        Ok(WatchId(self.dirs.len() - 1))
    }

    /// The directory `watch` is, canonical.
    pub fn dir(&self, watch: WatchId) -> &Path {
        &self.dirs[watch.0]
    }

    /// What changed since the last poll.
    pub fn poll(&mut self) -> WatchEvents<'_> {
        self.changed.clear();
        self.errors.clear();
        while let Ok(report) = self.receiver.try_recv() {
            match report {
                Report::Changed(paths) => {
                    for path in paths {
                        if let Some(changed) = self.locate(&path)
                            && !self.changed.contains(&changed)
                        {
                            self.changed.push(changed);
                        }
                    }
                }
                Report::Errors(errors) => self.errors.extend(errors),
            }
        }
        WatchEvents { changed: &self.changed, errors: &self.errors }
    }

    /// The watched directory `path` is directly in, and its name there.
    fn locate(&self, path: &Path) -> Option<Changed> {
        let (parent, name) = (path.parent()?, path.file_name()?);
        // A file's directory is still there when the file is gone; canonical, as `watch` keeps
        // them.
        let parent = std::fs::canonicalize(parent).ok()?;
        let index = self.dirs.iter().position(|dir| *dir == parent)?;
        Some(Changed { watch: WatchId(index), name: name.to_owned() })
    }
}

#[cfg(feature = "notify")]
mod backend {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::time::Duration;

    use notify::event::{AccessKind, AccessMode};
    use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

    use super::{Report, WatchError};

    /// `notify`'s recommended watcher for the platform (FSEvents, inotify,
    /// ReadDirectoryChangesW), whose events a thread of its own debounces.
    pub(super) struct Backend(RecommendedWatcher);

    impl Backend {
        pub(super) fn new(debounce: Duration, reports: Sender<Report>) -> Result<Self, WatchError> {
            let backend_error = |message: String| WatchError::Backend { path: PathBuf::new(), message };
            let (events, received) = mpsc::channel();
            let watcher = notify::recommended_watcher(move |event| {
                // A closed channel means the debounce thread is gone, and the watcher with it.
                let _ = events.send(event);
            })
            .map_err(|error| backend_error(error.to_string()))?;
            std::thread::Builder::new()
                .name("sapphire-hot-reload".into())
                .spawn(move || debounce_events(&received, debounce, &reports))
                .map_err(|error| backend_error(error.to_string()))?;
            Ok(Self(watcher))
        }

        pub(super) fn watch(&mut self, dir: &Path) -> Result<(), String> {
            self.0.watch(dir, RecursiveMode::NonRecursive).map_err(|error| error.to_string())
        }
    }

    /// Waits for an event, gathers the burst it starts until no event comes for `debounce`, and
    /// reports the burst's paths, each once. Returns once the watcher is dropped, which closes
    /// `events`, or the [`super::FileWatcher`], which closes `reports`.
    fn debounce_events(
        events: &Receiver<notify::Result<Event>>,
        debounce: Duration,
        reports: &Sender<Report>,
    ) {
        while let Ok(first) = events.recv() {
            let (mut paths, mut errors) = (Vec::new(), Vec::new());
            gather(first, &mut paths, &mut errors);
            loop {
                match events.recv_timeout(debounce) {
                    Ok(event) => gather(event, &mut paths, &mut errors),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            let sent = (paths.is_empty() || reports.send(Report::Changed(paths)).is_ok())
                && (errors.is_empty() || reports.send(Report::Errors(errors)).is_ok());
            if !sent {
                return;
            }
        }
    }

    /// Adds an event's paths, or its error, to the burst. Reading a file isn't a change (the IO
    /// thread reads packs all the time), but closing one written is.
    fn gather(event: notify::Result<Event>, paths: &mut Vec<PathBuf>, errors: &mut Vec<String>) {
        match event {
            Ok(event) => {
                let write = !matches!(event.kind, EventKind::Access(kind) if kind != AccessKind::Close(AccessMode::Write));
                for path in event.paths.into_iter().filter(|_| write) {
                    if !paths.contains(&path) {
                        paths.push(path);
                    }
                }
            }
            Err(error) => errors.push(error.to_string()),
        }
    }
}

#[cfg(not(feature = "notify"))]
mod backend {
    use std::path::Path;
    use std::sync::mpsc::Sender;
    use std::time::Duration;

    use super::{Report, WatchError};

    /// No backend: builds without the `notify` feature (phones) watch nothing.
    pub(super) enum Backend {}

    impl Backend {
        pub(super) fn new(_debounce: Duration, _reports: Sender<Report>) -> Result<Self, WatchError> {
            Err(WatchError::Unsupported)
        }

        pub(super) fn watch(&mut self, _dir: &Path) -> Result<(), String> {
            match *self {}
        }
    }
}
