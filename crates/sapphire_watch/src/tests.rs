//! Real filesystem events: continuous writes must still be reported before they stop.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sapphire_time::Stopwatch;

use super::*;

/// A directory of its own under the system's temporary directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("sapphire_watch_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a temporary directory");
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const TEST_DEBOUNCE: Duration = Duration::from_millis(150);
const TICK: Duration = Duration::from_millis(20);

/// Writes `path` every 5 ms, and reads `read` as often, until `stop` or ten seconds have passed.
fn noise(path: PathBuf, read: PathBuf, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let (since, mut n) = (Stopwatch::start(), 0u32);
        while !stop.load(Ordering::Relaxed) && since.elapsed() < Duration::from_secs(10) {
            // The directory goes when the test ends, perhaps before this thread sees `stop`.
            let _ = std::fs::write(&path, n.to_le_bytes());
            let _ = std::fs::read(&read);
            n = n.wrapping_add(1);
            std::thread::sleep(Duration::from_millis(5));
        }
    })
}

#[test]
fn a_file_written_all_the_time_is_still_reported() {
    const LONGEST: Duration = Duration::from_millis(600);
    let dir = TempDir::new("writer");
    let mut watcher = FileWatcher::with_max_wait(TEST_DEBOUNCE, LONGEST).expect("a file-watching backend");
    watcher.watch(&dir.0).unwrap();
    std::thread::sleep(4 * TEST_DEBOUNCE);
    let _ = watcher.poll();

    // Two seconds of writes every 5 ms, never quiet for a debounce time: reported every 0.6 s.
    let stop = Arc::new(AtomicBool::new(false));
    let writer = noise(dir.join("log.txt"), dir.join("missing"), Arc::clone(&stop));
    let (since, mut reports) = (Stopwatch::start(), 0);
    while since.elapsed() < Duration::from_secs(2) {
        reports += watcher.poll().changed.iter().filter(|changed| changed.name == "log.txt").count();
        std::thread::sleep(TICK);
    }
    let noisy = !writer.is_finished();
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
    assert!(noisy);
    assert!(reports >= 2, "reported {reports} times in two seconds of writes");
}

/// Polls until `found` finds what it looks for in a report, for at most ten seconds.
fn until<T>(watcher: &mut FileWatcher, mut found: impl FnMut(&WatchEvents<'_>) -> Option<T>) -> T {
    let since = Stopwatch::start();
    while since.elapsed() < Duration::from_secs(10) {
        if let Some(value) = found(&watcher.poll()) {
            return value;
        }
        std::thread::sleep(TICK);
    }
    panic!("not reported in ten seconds");
}

/// Polls until a change of `name` is reported, for at most ten seconds.
fn until_changed(watcher: &mut FileWatcher, name: &str) {
    until(watcher, |events| events.changed.iter().any(|changed| changed.name == name).then_some(()));
}

/// A watcher of a directory of its own, in `root`, with what it reports from before the test
/// taken: the watcher, the watch and the directory.
fn watching(root: &TempDir) -> (FileWatcher, WatchId, PathBuf) {
    let dir = root.join("watched");
    std::fs::create_dir(&dir).unwrap();
    let mut watcher = FileWatcher::new(TEST_DEBOUNCE).expect("a file-watching backend");
    let watch = watcher.watch(&dir).unwrap();
    std::thread::sleep(4 * TEST_DEBOUNCE);
    let _ = watcher.poll();
    (watcher, watch, dir)
}

/// The watch works: a file written now is reported.
fn assert_still_watching(watcher: &mut FileWatcher, dir: &Path) {
    std::thread::sleep(4 * TEST_DEBOUNCE);
    let _ = watcher.poll();
    std::fs::write(dir.join("written_later"), b"later").unwrap();
    until_changed(watcher, "written_later");
}

/// Review DV-12. Each backend treats a directory that goes in its own way: inotify drops the
/// watch and reports it, Windows' drops it and reports nothing, and FSEvents goes on watching
/// the path. So the test drops the watch itself: on every platform, only watching the directory
/// again can report what comes after.
#[test]
fn a_watched_directory_gone_is_reported_once_and_watched_again_when_it_is_back() {
    let root = TempDir::new("gone");
    let (mut watcher, watch, dir) = watching(&root);
    let shown = watcher.dir(watch).display().to_string();

    std::fs::remove_dir_all(&dir).unwrap();
    watcher.drop_backend_watch(watch);
    let error = until(&mut watcher, |events| events.errors.first().cloned());
    assert!(error.contains(&shown), "{error} names {shown}");

    // Once, however long it stays gone.
    let since = Stopwatch::start();
    while since.elapsed() < Duration::from_millis(1500) {
        let events = watcher.poll();
        assert!(events.errors.is_empty(), "{:?}", events.errors);
        std::thread::sleep(TICK);
    }

    // Back, a file in it before anything watches it again: nothing would report it otherwise.
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("written_meanwhile"), b"meanwhile").unwrap();
    until_changed(&mut watcher, "written_meanwhile");
    assert_still_watching(&mut watcher, &dir);
}

/// A directory removed and made again between polls is never seen gone, but on Linux the watch
/// went with the old one.
#[test]
fn a_watched_directory_made_again_at_once_is_watched_again() {
    // `notify` can't watch through this on Windows: when its read fails and the directory is
    // there again, it goes on reading the deleted one, and nothing says so.
    if cfg!(windows) {
        return;
    }
    let root = TempDir::new("made_again");
    let (mut watcher, watch, dir) = watching(&root);

    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("written_meanwhile"), b"meanwhile").unwrap();
    // Whatever the backend reported has reached the watcher.
    std::thread::sleep(4 * TEST_DEBOUNCE);
    watcher.drop_backend_watch(watch);

    until_changed(&mut watcher, "written_meanwhile");
    assert_still_watching(&mut watcher, &dir);
}

/// An event naming a watched directory that is still there is no error: its permissions changed,
/// or FSEvents is reporting its creation, just before the watch began.
#[test]
fn a_watched_directory_changed_in_place_is_no_error() {
    let root = TempDir::new("in_place");
    let (mut watcher, _, dir) = watching(&root);

    let readable = std::fs::metadata(&dir).unwrap().permissions();
    let mut read_only = readable.clone();
    read_only.set_readonly(true);
    std::fs::set_permissions(&dir, read_only).unwrap();
    std::thread::sleep(4 * TEST_DEBOUNCE);
    assert!(watcher.poll().errors.is_empty());
    std::fs::set_permissions(&dir, readable).unwrap();
    std::thread::sleep(4 * TEST_DEBOUNCE);
    assert!(watcher.poll().errors.is_empty());

    assert_still_watching(&mut watcher, &dir);
}
