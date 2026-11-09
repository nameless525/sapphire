//! What the game keeps on disk, written off the frame's path: the player's settings and the save alike.
//!
//! [`Kept`] takes a snapshot on the frame, which is only bytes, and writes it on a job. One write is in
//! flight at a time, because two could land out of order and the older would win; what's asked for during a
//! write waits for it, and only the newest of those is written. [`Kept::finish`] is what a quit calls.
//!
//! Loading stays with the store: a settings file always gives settings and a save may give none, and the
//! caller says what each means.

use std::fmt;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_save::{ProgressStore, SettingsFile, Snapshot};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::LOG_TARGET;

/// A file a [`Kept`] writes: it turns a value into bytes on the frame, and writes them anywhere.
pub trait Store: Clone + Send + 'static {
    type Value;
    /// For the logs: "settings", "progress".
    const WHAT: &'static str;

    /// The value as bytes, ready to go to a job.
    fn snapshot(&self, value: &Self::Value) -> Result<Snapshot<Self::Value>, String>;

    /// Puts the bytes on disk. It can stall as long as the storage does.
    fn write(&self, snapshot: &Snapshot<Self::Value>) -> Result<(), String>;
}

impl<T: Versioned + Serialize + DeserializeOwned + TypePath + Default + 'static> Store for SettingsFile<T> {
    type Value = T;
    const WHAT: &'static str = "settings";

    fn snapshot(&self, value: &T) -> Result<Snapshot<T>, String> {
        SettingsFile::snapshot(self, value).map_err(|error| error.to_string())
    }

    fn write(&self, snapshot: &Snapshot<T>) -> Result<(), String> {
        SettingsFile::write(self, snapshot).map_err(|error| error.to_string())
    }
}

impl<T: Versioned + Serialize + DeserializeOwned + TypePath + 'static> Store for ProgressStore<T> {
    type Value = T;
    const WHAT: &'static str = "progress";

    fn snapshot(&self, value: &T) -> Result<Snapshot<T>, String> {
        ProgressStore::snapshot(self, value).map_err(|error| error.to_string())
    }

    fn write(&self, snapshot: &Snapshot<T>) -> Result<(), String> {
        ProgressStore::write(self, snapshot).map(drop).map_err(|error| error.to_string())
    }
}

/// What a [`Kept`] has done.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeptStats {
    /// Values asked to be kept.
    pub asked: u32,
    /// Writes that reached the disk.
    pub written: u32,
    /// Writes, or snapshots, that failed, each logged. What's in memory is still what the game goes on.
    pub failed: u32,
    /// Values a newer one overtook before they were written.
    pub overtaken: u32,
}

/// A store and the one write it has in flight. See the module docs.
pub struct Kept<S: Store> {
    store: S,
    written: Option<Receiver<Result<(), String>>>,
    /// What was asked for during a write.
    newest: Option<Snapshot<S::Value>>,
    stats: KeptStats,
}

impl<S: Store> fmt::Debug for Kept<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Kept")
            .field("what", &S::WHAT)
            .field("in_flight", &self.written.is_some())
            .field("waiting", &self.newest.is_some())
            .field("stats", &self.stats)
            .finish()
    }
}

impl<S: Store> Kept<S> {
    #[must_use]
    pub fn new(store: S) -> Self {
        Self { store, written: None, newest: None, stats: KeptStats::default() }
    }

    #[must_use]
    pub fn store(&self) -> &S {
        &self.store
    }

    #[must_use]
    pub fn stats(&self) -> KeptStats {
        self.stats
    }

    /// Whether a write is in flight or waiting.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.written.is_some() || self.newest.is_some()
    }

    /// Keeps `value`, on a job once any write in flight has reported back.
    pub fn keep(&mut self, value: &S::Value) {
        self.stats.asked += 1;
        match self.store.snapshot(value) {
            Ok(snapshot) => {
                if self.newest.replace(snapshot).is_some() {
                    self.stats.overtaken += 1;
                }
            }
            Err(error) => {
                self.stats.failed += 1;
                tracing::error!(target: LOG_TARGET, what = S::WHAT, %error, "can't be written");
            }
        }
        self.poll();
    }

    /// Once a frame: hears how a write went, and starts the next.
    pub fn poll(&mut self) {
        if let Some(written) = &self.written {
            match written.try_recv() {
                Ok(result) => self.report(result),
                Err(TryRecvError::Empty) => return,
                // The job died, and its panic has been logged.
                Err(TryRecvError::Disconnected) => self.stats.failed += 1,
            }
            self.written = None;
        }
        let Some(snapshot) = self.newest.take() else { return };
        let (report, written) = mpsc::channel();
        let store = self.store.clone();
        sapphire_jobs::spawn_async(move || {
            let _ = report.send(store.write(&snapshot));
        });
        self.written = Some(written);
    }

    /// Waits for what's in flight and writes what's left, here: the process is on its way out.
    pub fn finish(&mut self) {
        if let Some(written) = self.written.take() {
            match written.recv() {
                Ok(result) => self.report(result),
                Err(_) => self.stats.failed += 1,
            }
        }
        if let Some(snapshot) = self.newest.take() {
            let result = self.store.write(&snapshot);
            self.report(result);
        }
    }

    fn report(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => self.stats.written += 1,
            Err(error) => {
                self.stats.failed += 1;
                tracing::error!(target: LOG_TARGET, what = S::WHAT, %error, "wasn't written");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use serde::Deserialize;

    use super::*;

    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TypePath)]
    #[type_path = "game::flow::tests"]
    struct Kept1 {
        value: u32,
    }

    impl Versioned for Kept1 {
        const VERSION: u32 = 1;
    }

    /// A directory of its own, gone with the test.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("game-flow-kept-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a directory");
            Self(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn counted(writes: &Arc<AtomicU32>) -> impl Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync + 'static {
        let writes = Arc::clone(writes);
        move |path, bytes| {
            writes.fetch_add(1, Ordering::Relaxed);
            std::fs::write(path, bytes)
        }
    }

    /// The newest stays and an older can't follow it: a value asked for during a write waits for it, and one
    /// overtaken before its turn isn't written at all.
    #[test]
    fn what_is_asked_for_last_is_what_stays_in_settings_and_saves_alike() {
        let dir = Dir::new("newest");
        let writes = Arc::new(AtomicU32::new(0));
        let mut settings = Kept::new(SettingsFile::<Kept1>::new(&dir.0, "settings", counted(&writes)));
        for value in [10, 20, 30] {
            settings.keep(&Kept1 { value });
        }
        settings.finish();
        let stats = settings.stats();
        assert_eq!((stats.asked, stats.failed), (3, 0));
        assert_eq!(stats.written + stats.overtaken, 3, "{stats:?}");
        assert!(writes.load(Ordering::Relaxed) <= 2, "what was overtaken isn't written");
        assert!(!settings.busy());
        assert_eq!(settings.store().load().value, Kept1 { value: 30 });

        let mut progress = Kept::new(ProgressStore::<Kept1>::new(&dir.0, "progress", counted(&writes)));
        for value in [1, 2, 3, 4] {
            progress.keep(&Kept1 { value });
            progress.poll();
        }
        progress.finish();
        let loaded = progress.store().load().expect("loads").expect("a save");
        assert_eq!(loaded.value, Kept1 { value: 4 });
    }

    /// A write that fails is logged and counted, and the next one is still tried.
    #[test]
    fn a_failed_write_is_counted_and_the_next_still_goes() {
        let dir = Dir::new("failed");
        let failing = Arc::new(AtomicBool::new(true));
        let fails = Arc::clone(&failing);
        let replace = move |path: &Path, bytes: &[u8]| {
            if fails.load(Ordering::Relaxed) {
                return Err(io::Error::other("the disk is full"));
            }
            std::fs::write(path, bytes)
        };
        let mut kept = Kept::new(ProgressStore::<Kept1>::new(&dir.0, "progress", replace));
        kept.keep(&Kept1 { value: 1 });
        kept.finish();
        assert_eq!(kept.stats().failed, 1);
        assert!(kept.store().load().expect("loads").is_none(), "nothing reached the disk");

        failing.store(false, Ordering::Relaxed);
        kept.keep(&Kept1 { value: 2 });
        kept.finish();
        assert_eq!((kept.stats().failed, kept.stats().written), (1, 1));
        assert_eq!(kept.store().load().expect("loads").expect("a save").value, Kept1 { value: 2 });
    }
}
