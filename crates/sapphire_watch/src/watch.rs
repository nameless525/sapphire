//! [`FileWatcher`]: directories watched for changed files, debounced.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sapphire_time::Stopwatch;

use crate::LOG_TARGET;

/// A watched directory, from [`FileWatcher::watch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WatchId(usize);

/// A file that changed in a watched directory: created, written, removed, or renamed from or to
/// its name, or any file in a directory watched again, which may have. Whoever reads it finds out
/// which.
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
        "this build has no file-watching backend (sapphire_watch's `notify` feature, which apps \
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

/// The longest a file written without pause waits to be reported: [`FileWatcher`] reports it
/// this long after its first change, quiet or not (DT-006). Twenty debounce times at
/// [`crate::DEBOUNCE`], several times longer than an editor's save or a cook's write of a pack.
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(2);

/// How often [`FileWatcher::poll`] looks at the watched directories themselves, since a backend
/// may not say that one went.
const CHECK_DIRS_EVERY: Duration = Duration::from_secs(1);

/// Watches directories, not recursively, for files that change, through `notify` 8.2 and a
/// trailing-edge debounce per file: a file is reported once it has been quiet for the debounce
/// time, so an editor's save or a cook's write to a temporary file and rename into place comes
/// out as one change per name, and changes to other files never hold it back. A file written
/// without pause is still reported every two seconds (`MAX_WAIT`). Files that come due
/// together are reported together; a cook's packs, written one after another, may come in
/// separate reports. Errors, and the platform saying it dropped events, are reported at once.
/// The debounce runs on a thread of its own that blocks while nothing waits to be reported;
/// [`Self::poll`] collects what it reported, and allocates nothing while nothing changed.
///
/// A watched directory that goes is reported once, as an error naming it (review DV-12). Backends
/// treat that differently: inotify reports it and drops the watch, Windows' drops the watch and
/// reports nothing, and FSEvents goes on watching the path. So [`Self::poll`] looks at each
/// directory every second, and at once when an event names one. When one is back, or is another
/// directory made under the same name, it is watched again and every file in it is reported as
/// changed, since nothing watched it meanwhile.
pub struct FileWatcher {
    backend: backend::Backend,
    /// Watched directories, canonical, by [`WatchId`].
    dirs: Vec<PathBuf>,
    /// What was last found of each of `dirs`.
    states: Vec<DirState>,
    changed: Vec<Changed>,
    errors: Vec<String>,
    /// When to look at the directories next, on this clock.
    clock: Stopwatch,
    next_check: Duration,
}

/// What [`FileWatcher::poll`] last found of a watched directory.
#[derive(Debug, Default)]
struct DirState {
    /// When it was made, where the platform says: another directory under the same name differs.
    created: Option<SystemTime>,
    /// An event named it since the last look.
    named: bool,
    /// Why it isn't being watched, as last reported, so it is reported once.
    problem: Option<String>,
}

impl DirState {
    fn of(dir: &Path) -> Self {
        let created = std::fs::metadata(dir).and_then(|metadata| metadata.created()).ok();
        Self { created, ..Self::default() }
    }

    /// Reports `problem`, unless it is the one reported last.
    fn report(&mut self, problem: String, errors: &mut Vec<String>) {
        if self.problem.as_ref() != Some(&problem) {
            errors.push(problem.clone());
            self.problem = Some(problem);
        }
    }
}

impl std::fmt::Debug for FileWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileWatcher").field("dirs", &self.dirs).finish_non_exhaustive()
    }
}

impl FileWatcher {
    /// A watcher that reports a file once it has been quiet for `debounce`, or two seconds (or
    /// `debounce`, if longer) after it first changed if it never is. Fails with
    /// [`WatchError::Unsupported`] in builds without the backend.
    pub fn new(debounce: Duration) -> Result<Self, WatchError> {
        Self::with_max_wait(debounce, MAX_WAIT)
    }

    /// [`Self::new`] with another longest wait, for tests.
    pub(crate) fn with_max_wait(debounce: Duration, max_wait: Duration) -> Result<Self, WatchError> {
        let backend = backend::Backend::new(debounce, max_wait.max(debounce))?;
        Ok(Self {
            backend,
            dirs: Vec::new(),
            states: Vec::new(),
            changed: Vec::new(),
            errors: Vec::new(),
            clock: Stopwatch::start(),
            next_check: CHECK_DIRS_EVERY,
        })
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
        self.states.push(DirState::of(&dir));
        self.dirs.push(dir);
        Ok(WatchId(self.dirs.len() - 1))
    }

    /// The directory `watch` is, canonical.
    pub fn dir(&self, watch: WatchId) -> &Path {
        &self.dirs[watch.0]
    }

    /// Has the backend stop watching `watch`, as Linux's does by itself when the directory goes.
    #[cfg(all(test, feature = "notify"))]
    pub(crate) fn drop_backend_watch(&mut self, watch: WatchId) {
        self.backend.unwatch(&self.dirs[watch.0]);
    }

    /// What changed since the last poll.
    pub fn poll(&mut self) -> WatchEvents<'_> {
        self.changed.clear();
        self.errors.clear();
        let (dirs, states) = (&self.dirs, &mut self.states);
        let (changed, errors) = (&mut self.changed, &mut self.errors);
        let mut named = false;
        self.backend.drain(
            |path| {
                if let Some(index) = dirs.iter().position(|dir| *dir == path) {
                    states[index].named = true;
                    named = true;
                } else if let Some(path) = locate(dirs, &path)
                    && !changed.contains(&path)
                {
                    changed.push(path);
                }
            },
            |error| errors.push(error),
        );
        let now = self.clock.elapsed();
        if named || now >= self.next_check {
            self.next_check = now + CHECK_DIRS_EVERY;
            self.check_dirs();
        }
        WatchEvents { changed: &self.changed, errors: &self.errors }
    }

    /// Looks at each watched directory: reports those gone, and watches again those back or made
    /// again, reporting every file in them.
    fn check_dirs(&mut self) {
        for (index, (dir, state)) in self.dirs.iter().zip(&mut self.states).enumerate() {
            let named = std::mem::take(&mut state.named);
            let Some(metadata) = std::fs::metadata(dir).ok().filter(std::fs::Metadata::is_dir) else {
                let problem = format!("the watched directory {} is gone", dir.display());
                state.report(problem, &mut self.errors);
                continue;
            };
            let created = metadata.created().ok();
            let same = match (created, state.created) {
                (Some(now), Some(then)) => now == then,
                // Nothing says which directory this is, so take an event naming it to mean another.
                _ => !named,
            };
            if same && state.problem.is_none() {
                continue;
            }
            self.backend.unwatch(dir);
            if let Err(message) = self.backend.watch(dir) {
                let problem = format!("can't watch {} again: {message}", dir.display());
                state.report(problem, &mut self.errors);
                continue;
            }
            tracing::info!(target: LOG_TARGET, dir = %dir.display(), "watching again");
            *state = DirState { created, ..DirState::default() };
            let watch = WatchId(index);
            match std::fs::read_dir(dir) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let file = Changed { watch, name: entry.file_name() };
                        if !self.changed.contains(&file) {
                            self.changed.push(file);
                        }
                    }
                }
                Err(error) => self.errors.push(format!("can't read {}: {error}", dir.display())),
            }
        }
    }
}

/// The directory of `dirs` that `path` is directly in, and its name there.
fn locate(dirs: &[PathBuf], path: &Path) -> Option<Changed> {
    let (parent, name) = (path.parent()?, path.file_name()?);
    // A file's directory is still there when the file is gone; canonical, as `watch` keeps them.
    let parent = std::fs::canonicalize(parent).ok()?;
    let index = dirs.iter().position(|dir| *dir == parent)?;
    Some(Changed { watch: WatchId(index), name: name.to_owned() })
}

#[cfg(feature = "notify")]
mod backend {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::time::Duration;

    use notify::event::{AccessKind, AccessMode};
    use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
    use sapphire_time::Stopwatch;

    use super::WatchError;

    /// What the debounce thread sends: paths that came due together, or errors.
    enum Report {
        Changed(Vec<PathBuf>),
        Errors(Vec<String>),
    }

    /// `notify`'s recommended watcher for the platform (FSEvents, inotify,
    /// ReadDirectoryChangesW), whose events a thread of its own debounces into reports.
    pub(super) struct Backend {
        watcher: RecommendedWatcher,
        reports: Receiver<Report>,
    }

    impl Backend {
        pub(super) fn new(debounce: Duration, max_wait: Duration) -> Result<Self, WatchError> {
            let backend_error = |message: String| WatchError::Backend { path: PathBuf::new(), message };
            let (events, received) = mpsc::channel();
            let (sender, reports) = mpsc::channel();
            let watcher = notify::recommended_watcher(move |event| {
                // A closed channel means the debounce thread is gone, and the watcher with it.
                let _ = events.send(event);
            })
            .map_err(|error| backend_error(error.to_string()))?;
            let debounce = Debounce::new(debounce, max_wait);
            std::thread::Builder::new()
                .name("sapphire-hot-reload".into())
                .spawn(move || debounce_events(&received, debounce, &sender))
                .map_err(|error| backend_error(error.to_string()))?;
            Ok(Self { watcher, reports })
        }

        pub(super) fn watch(&mut self, dir: &Path) -> Result<(), String> {
            self.watcher.watch(dir, RecursiveMode::NonRecursive).map_err(|error| error.to_string())
        }

        /// Stops watching `dir`, if it still is: a backend may have stopped by itself when the
        /// directory went, which is the only way this fails.
        pub(super) fn unwatch(&mut self, dir: &Path) {
            let _ = self.watcher.unwatch(dir);
        }

        /// Hands each path and error reported since the last call to `changed` and `error`, in
        /// the order they came.
        pub(super) fn drain(&mut self, mut changed: impl FnMut(PathBuf), mut error: impl FnMut(String)) {
            while let Ok(report) = self.reports.try_recv() {
                match report {
                    Report::Changed(paths) => paths.into_iter().for_each(&mut changed),
                    Report::Errors(errors) => errors.into_iter().for_each(&mut error),
                }
            }
        }
    }

    /// Feeds the watcher's events to `debounce` and sends what comes due. Blocks while nothing
    /// waits to be reported, and otherwise wakes when the next path is due. Returns once the
    /// watcher is dropped, which closes `events`, or the [`super::FileWatcher`], which closes
    /// `reports`.
    fn debounce_events(
        events: &Receiver<notify::Result<Event>>,
        mut debounce: Debounce,
        reports: &Sender<Report>,
    ) {
        // The thread's own clock: only the debounce's waits are measured with it (rule R2).
        let clock = Stopwatch::start();
        loop {
            let event = match debounce.next_due() {
                None => match events.recv() {
                    Ok(event) => Some(event),
                    Err(_) => return,
                },
                Some(due) => match events.recv_timeout(due.saturating_sub(clock.elapsed())) {
                    Ok(event) => Some(event),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
            };
            let now = clock.elapsed();
            let errors = event.and_then(|event| debounce.add(event, now));
            let sent = errors.is_none_or(|errors| reports.send(Report::Errors(errors)).is_ok())
                && debounce.take_due(now).is_none_or(|paths| reports.send(Report::Changed(paths)).is_ok());
            if !sent {
                return;
            }
        }
    }

    /// The debounce per path (DT-006): changed paths waiting to be reported, each due once it
    /// has been quiet for the debounce time, or the longest wait after its first change if it
    /// never is. Times are on the debounce thread's clock. Each path waits on its own, so
    /// changes to other files, however many, never hold one back.
    pub(super) struct Debounce {
        debounce: Duration,
        max_wait: Duration,
        /// In the order first seen.
        pending: Vec<Pending>,
    }

    /// A changed path, and when it changed first and last since it was last reported.
    struct Pending {
        path: PathBuf,
        first: Duration,
        last: Duration,
    }

    impl Debounce {
        pub(super) fn new(debounce: Duration, max_wait: Duration) -> Self {
            Self { debounce, max_wait, pending: Vec::new() }
        }

        /// Takes an event that came at `now`: each path a change names waits again for the
        /// debounce time. Reading a file isn't a change (the IO thread reads packs all the time),
        /// and moves no deadline, but closing one written is. Returns what to report at once:
        /// the backend's errors, and the platform saying it dropped events (which may have been
        /// changes nothing will report).
        pub(super) fn add(&mut self, event: notify::Result<Event>, now: Duration) -> Option<Vec<String>> {
            let event = match event {
                Ok(event) => event,
                Err(error) => return Some(vec![error.to_string()]),
            };
            if event.need_rescan() {
                let paths: Vec<String> = event.paths.iter().map(|path| path.display().to_string()).collect();
                return Some(vec![format!(
                    "the platform dropped file events{}{}: changes made meanwhile may not be reloaded",
                    if paths.is_empty() { "" } else { " in " },
                    paths.join(", ")
                )]);
            }
            let write = !matches!(event.kind, EventKind::Access(kind) if kind != AccessKind::Close(AccessMode::Write));
            for path in event.paths.into_iter().filter(|_| write) {
                match self.pending.iter_mut().find(|pending| pending.path == path) {
                    Some(pending) => pending.last = now,
                    None => self.pending.push(Pending { path, first: now, last: now }),
                }
            }
            None
        }

        /// When the next path is due, if any is waiting.
        pub(super) fn next_due(&self) -> Option<Duration> {
            self.pending.iter().map(|pending| self.due(pending)).min()
        }

        /// The paths due at `now`, in the order first seen, if any are.
        pub(super) fn take_due(&mut self, now: Duration) -> Option<Vec<PathBuf>> {
            let (debounce, max_wait) = (self.debounce, self.max_wait);
            let due: Vec<PathBuf> = self
                .pending
                .extract_if(.., |pending| Self::due_at(pending, debounce, max_wait) <= now)
                .map(|pending| pending.path)
                .collect();
            (!due.is_empty()).then_some(due)
        }

        fn due(&self, pending: &Pending) -> Duration {
            Self::due_at(pending, self.debounce, self.max_wait)
        }

        fn due_at(pending: &Pending, debounce: Duration, max_wait: Duration) -> Duration {
            (pending.last + debounce).min(pending.first + max_wait)
        }
    }

    #[cfg(test)]
    mod tests {
        use notify::event::{CreateKind, DataChange, Flag, ModifyKind};

        use super::*;

        const DEBOUNCE: Duration = Duration::from_millis(100);
        const MAX_WAIT: Duration = Duration::from_secs(2);

        fn ms(ms: u64) -> Duration {
            Duration::from_millis(ms)
        }

        fn write(path: &str) -> notify::Result<Event> {
            Ok(Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content))).add_path(path.into()))
        }

        fn read(path: &str) -> notify::Result<Event> {
            Ok(Event::new(EventKind::Access(AccessKind::Read)).add_path(path.into()))
        }

        fn paths(names: &[&str]) -> Option<Vec<PathBuf>> {
            Some(names.iter().map(PathBuf::from).collect())
        }

        #[test]
        fn each_path_is_due_once_it_is_quiet() {
            // DT-006: a file written every 5 ms next to one written once. The quiet one is due a
            // debounce time after its change, not once the other stops.
            let mut debounce = Debounce::new(DEBOUNCE, MAX_WAIT);
            assert_eq!(debounce.next_due(), None, "nothing waits: the thread blocks");
            for t in (0..1000).step_by(5) {
                if t == 300 {
                    assert_eq!(debounce.add(write("/w/target.ron"), ms(t)), None);
                }
                debounce.add(write("/w/noise.tmp"), ms(t));
                debounce.add(read("/w/other.spack"), ms(t));
                let due = debounce.take_due(ms(t));
                assert_eq!(due, (t == 400).then(|| paths(&["/w/target.ron"])).flatten(), "at {t} ms");
            }
            // The noise stops, and is due a debounce time after its last write.
            assert_eq!(debounce.next_due(), Some(ms(995 + 100)));
            assert_eq!(debounce.take_due(ms(1094)), None);
            assert_eq!(debounce.take_due(ms(1095)), paths(&["/w/noise.tmp"]));
            assert_eq!(debounce.next_due(), None);
        }

        #[test]
        fn a_burst_of_one_file_is_one_report_and_reads_change_nothing() {
            let mut debounce = Debounce::new(DEBOUNCE, MAX_WAIT);
            // A write to a temporary file renamed into place: both names, once.
            debounce.add(
                Ok(Event::new(EventKind::Create(CreateKind::File)).add_path("/w/a.spack.tmp".into())),
                ms(0),
            );
            debounce.add(write("/w/a.spack.tmp"), ms(20));
            let rename = Event::new(EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::Both)))
                .add_path("/w/a.spack.tmp".into())
                .add_path("/w/a.spack".into());
            debounce.add(Ok(rename), ms(40));
            // Reads, which the IO thread does all the time, move no deadline and wait for nothing.
            debounce.add(read("/w/a.spack"), ms(100));
            debounce.add(read("/w/b.spack"), ms(130));
            assert_eq!(debounce.take_due(ms(139)), None);
            assert_eq!(debounce.take_due(ms(140)), paths(&["/w/a.spack.tmp", "/w/a.spack"]));
            assert_eq!(debounce.next_due(), None, "reads wait for nothing");
            // Closing a file written is a change.
            let closed = Event::new(EventKind::Access(AccessKind::Close(AccessMode::Write)))
                .add_path("/w/b.spack".into());
            debounce.add(Ok(closed), ms(200));
            assert_eq!(debounce.take_due(ms(300)), paths(&["/w/b.spack"]));
        }

        #[test]
        fn a_file_written_without_pause_is_reported_every_longest_wait() {
            let mut debounce = Debounce::new(DEBOUNCE, MAX_WAIT);
            let mut reported = Vec::new();
            for t in (0..5000).step_by(10) {
                debounce.add(write("/w/log.txt"), ms(t));
                if debounce.take_due(ms(t)).is_some() {
                    reported.push(t);
                }
            }
            assert_eq!(reported, [2000, 4010], "two seconds after its first change each time");
        }

        #[test]
        fn errors_and_dropped_events_are_reported_at_once() {
            let mut debounce = Debounce::new(DEBOUNCE, MAX_WAIT);
            let error = notify::Error::generic("gone");
            assert_eq!(debounce.add(Err(error), ms(0)), Some(vec!["gone".to_owned()]));
            let dropped = Event::new(EventKind::Other).set_flag(Flag::Rescan);
            let report = debounce.add(Ok(dropped), ms(0)).expect("reported");
            assert!(report[0].contains("dropped file events"), "{report:?}");
            let dropped = Event::new(EventKind::Other).set_flag(Flag::Rescan).add_path("/w".into());
            let report = debounce.add(Ok(dropped), ms(0)).expect("reported");
            assert!(report[0].contains("events in /w:"), "{report:?}");
            assert_eq!(debounce.next_due(), None, "no path waits for them");
        }
    }
}

#[cfg(not(feature = "notify"))]
mod backend {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use super::WatchError;

    /// No backend: builds without the `notify` feature (phones) watch nothing. It is
    /// uninhabited, so no [`super::FileWatcher`] exists in them and nothing is ever reported.
    pub(super) enum Backend {}

    impl Backend {
        pub(super) fn new(_debounce: Duration, _max_wait: Duration) -> Result<Self, WatchError> {
            Err(WatchError::Unsupported)
        }

        pub(super) fn watch(&mut self, _dir: &Path) -> Result<(), String> {
            match *self {}
        }

        pub(super) fn unwatch(&mut self, _dir: &Path) {
            match *self {}
        }

        pub(super) fn drain(&mut self, _changed: impl FnMut(PathBuf), _error: impl FnMut(String)) {
            match *self {}
        }
    }
}
