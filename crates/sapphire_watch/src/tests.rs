//! Real filesystem events: continuous writes must still be reported before they stop.

use std::path::PathBuf;
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
