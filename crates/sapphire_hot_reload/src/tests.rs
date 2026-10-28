//! Watching real directories: these wait for the platform's file events, for at most a few
//! seconds each (longer on a loaded machine, whose sleeps run long).
//!
//! They check what holds however the machine is loaded: a pause in the writes, or in the
//! delivery of their events, as long as the debounce time may end a burst; and FSEvents may
//! report files written just before a watch began, in its first batches or folded into a later
//! one. So bursts are checked to coalesce, not to be reported exactly once, and each change to be
//! reported, not to be reported alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sapphire_time::Stopwatch;

use super::*;
// The public watcher contract promises delivery within two seconds of continuous writes.
const MAX_WAIT: Duration = Duration::from_secs(2);

/// A directory of its own under the system's temporary directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("sapphire_hot_reload_{name}_{}", std::process::id()));
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

/// Short, so the tests are quick; the real one is [`DEBOUNCE`].
const TEST_DEBOUNCE: Duration = Duration::from_millis(150);
const TICK: Duration = Duration::from_millis(20);

/// Polls until something changed, for at most five seconds.
fn next(reload: &mut HotReload) -> Changes {
    next_where(reload, |_| true)
}

/// Polls until a change `wanted` takes comes, for at most five seconds, skipping others.
fn next_where(reload: &mut HotReload, wanted: impl Fn(&Changes) -> bool) -> Changes {
    for _ in 0..250 {
        let changes = reload.poll();
        if !changes.is_empty() && wanted(&changes) {
            return changes;
        }
        std::thread::sleep(TICK);
    }
    panic!("no such change in five seconds");
}

/// Polls until every pack in `expected` was reported, for at most five seconds, and returns
/// every pack reported meanwhile: others may come too, the expected ones must.
fn packs_reported(reload: &mut HotReload, expected: &[&str]) -> Vec<String> {
    let mut packs = Vec::new();
    for _ in 0..250 {
        packs.extend(reload.poll().packs);
        if expected.iter().all(|pack| packs.iter().any(|reported| reported == pack)) {
            return packs;
        }
        std::thread::sleep(TICK);
    }
    panic!("{expected:?} weren't reported in five seconds; {packs:?} were");
}

/// Polls for `wait`, and returns everything that changed in that time.
fn gather(reload: &mut HotReload, wait: Duration) -> Vec<Changes> {
    let mut all = Vec::new();
    for _ in 0..wait.as_millis() / TICK.as_millis() {
        let changes = reload.poll();
        if !changes.is_empty() {
            all.push(changes);
        }
        std::thread::sleep(TICK);
    }
    all
}

/// Writes `bytes` to `path` as the pipeline writes packs: to a temporary file, renamed into place.
fn write_atomically(path: &Path, bytes: &[u8]) {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    std::fs::write(&temp, bytes).expect("write");
    std::fs::rename(&temp, path).expect("rename");
}

#[test]
fn a_burst_of_writes_is_coalesced() {
    const WRITES: u8 = 30;
    let content = TempDir::new("burst");
    let pack = content.join("shared.spack");
    std::fs::write(&pack, b"0").unwrap();
    let mut reload = HotReload::new(TEST_DEBOUNCE).expect("a file-watching backend");
    reload.watch_packs(&content.0).unwrap();
    let _ = gather(&mut reload, 4 * TEST_DEBOUNCE);
    // Thirty writes over about half a second, more than three debounce times, never quiet for one
    // unless the machine pauses this thread; each such pause may end a burst.
    let (mut since_write, mut pauses) = (Stopwatch::start(), 0);
    for n in 0..WRITES {
        write_atomically(&pack, &[n]);
        if since_write.lap() >= TEST_DEBOUNCE && n > 0 {
            pauses += 1;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    let mut reports = vec![next(&mut reload)];
    reports.extend(gather(&mut reload, 4 * TEST_DEBOUNCE));
    assert!(
        reports.iter().all(|changes| changes.packs == ["shared"] && changes.errors.is_empty()),
        "{reports:?}"
    );
    // Once on an idle machine; a report per write would be thirty. A loaded machine may also stall
    // delivering the events, which this thread can't see: a few times at most in half a second.
    let most = 1 + pauses + usize::from(WRITES / 10);
    assert!(
        reports.len() <= most,
        "{WRITES} writes with {pauses} pauses were reported {} times, more than {most}: {reports:?}",
        reports.len()
    );
}

#[test]
fn packs_shaders_and_documents_are_reported_as_what_they_are() {
    let content = TempDir::new("kinds_content");
    let shaders = TempDir::new("kinds_shaders");
    let documents = TempDir::new("kinds_documents");
    for file in ShaderFile::ALL {
        std::fs::write(shaders.join(file.file_name()), file.compiled_in()).unwrap();
    }
    // Two documents in one directory, as the default day and light rigs are.
    let (day, rigs) = (documents.join("day.ron"), documents.join("rigs.ron"));
    std::fs::write(&day, "(keys: [])").unwrap();
    std::fs::write(&rigs, "(rigs: [])").unwrap();
    let mut reload = HotReload::new(TEST_DEBOUNCE).expect("a file-watching backend");
    reload.watch_packs(&content.0).unwrap();
    reload.watch_shaders(&shaders.0).unwrap();
    let document = reload.watch_document(&day).unwrap();
    let other_document = reload.watch_document(&rigs).unwrap();
    assert_ne!(document, other_document);
    // The files just written may be reported once the watch begins.
    let _ = gather(&mut reload, 4 * TEST_DEBOUNCE);

    // A pack lands; the temporary file it was written to is no pack.
    write_atomically(&content.join("shared.spack"), b"pack");
    let changes = next(&mut reload);
    assert_eq!(
        (changes.packs.as_slice(), changes.shaders.is_none()),
        (["shared".to_owned()].as_slice(), true)
    );

    // A shader: every file is read again, the changed one with its new text.
    let sky = format!("{}\n// edited\n", ShaderFile::Sky.compiled_in());
    std::fs::write(shaders.join("sky.wgsl"), &sky).unwrap();
    let changes = next(&mut reload);
    let sources = changes.shaders.expect("new shader sources");
    assert_eq!(sources.get(ShaderFile::Sky), sky);
    assert_eq!(sources.get(ShaderFile::Frame), ShaderFile::Frame.compiled_in());
    assert!(
        changes.packs.is_empty() && changes.errors.is_empty(),
        "{:?} {:?}",
        changes.packs,
        changes.errors
    );

    // A document, with its text, and not its neighbor.
    std::fs::write(&day, "(keys: [()])").unwrap();
    let changes = next(&mut reload);
    assert_eq!(changes.documents, [(document, "(keys: [()])".to_owned())]);
    std::fs::write(&rigs, "(rigs: [()])").unwrap();
    let changes = next(&mut reload);
    assert_eq!(changes.documents, [(other_document, "(rigs: [()])".to_owned())]);

    // Files that are none of these.
    std::fs::write(content.join("notes.txt"), "x").unwrap();
    std::fs::write(shaders.join("notes.wgsl"), "x").unwrap();
    std::fs::write(documents.join("other.ron"), "x").unwrap();
    assert!(gather(&mut reload, 4 * TEST_DEBOUNCE).is_empty());
}

#[test]
fn removed_and_renamed_files_are_changes() {
    let content = TempDir::new("removed_content");
    let shaders = TempDir::new("removed_shaders");
    for file in ShaderFile::ALL {
        std::fs::write(shaders.join(file.file_name()), file.compiled_in()).unwrap();
    }
    std::fs::write(content.join("shared.spack"), b"pack").unwrap();
    std::fs::write(content.join("zone.spack"), b"pack").unwrap();
    let mut reload = HotReload::new(TEST_DEBOUNCE).expect("a file-watching backend");
    reload.watch_packs(&content.0).unwrap();
    reload.watch_shaders(&shaders.0).unwrap();
    let _ = gather(&mut reload, 4 * TEST_DEBOUNCE);

    // A removed pack is a pack to remount: the remount fails, and the asset server keeps it.
    std::fs::remove_file(content.join("shared.spack")).unwrap();
    packs_reported(&mut reload, &["shared"]);
    // A rename changes both names.
    std::fs::rename(content.join("zone.spack"), content.join("district.spack")).unwrap();
    packs_reported(&mut reload, &["district", "zone"]);

    // A removed shader keeps the text it had, and says why.
    std::fs::remove_file(shaders.join("outline.wgsl")).unwrap();
    let changes = next_where(&mut reload, |changes| changes.shaders.is_some());
    let sources = changes.shaders.expect("shader sources");
    assert_eq!(sources.get(ShaderFile::Outline), ShaderFile::Outline.compiled_in());
    assert!(changes.errors.iter().any(|error| error.contains("outline.wgsl")), "{:?}", changes.errors);
}

#[test]
fn directories_are_watched_once_and_missing_ones_fail() {
    let dir = TempDir::new("once");
    let mut watcher = FileWatcher::new(TEST_DEBOUNCE).expect("a file-watching backend");
    let first = watcher.watch(&dir.0).unwrap();
    assert_eq!(watcher.watch(&dir.join(".")).unwrap(), first, "the same directory, spelled otherwise");
    assert!(matches!(watcher.watch(&dir.join("missing")), Err(WatchError::Io { .. })));
    let mut reload = HotReload::new(TEST_DEBOUNCE).unwrap();
    assert!(reload.watch_document(Path::new("")).is_err());
}

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
fn a_file_written_all_the_time_never_holds_back_a_quiet_one() {
    // DT-006: one debounce for every watched directory waited for all of them to be quiet, so a
    // file written every few milliseconds held back every other change for as long as it lasted.
    let dir = TempDir::new("noise");
    let (target, read) = (dir.join("target.ron"), dir.join("read.spack"));
    std::fs::write(&target, "(old)").unwrap();
    std::fs::write(&read, "pack").unwrap();
    let mut reload = HotReload::new(TEST_DEBOUNCE).expect("a file-watching backend");
    let document = reload.watch_document(&target).unwrap();
    let _ = gather(&mut reload, 4 * TEST_DEBOUNCE);

    // A writer, and a reader (some platforms report reads), in the same directory.
    let stop = Arc::new(AtomicBool::new(false));
    let writer = noise(dir.join("noise.tmp"), read, Arc::clone(&stop));
    std::thread::sleep(2 * TEST_DEBOUNCE);
    let since = Stopwatch::start();
    std::fs::write(&target, "(new)").unwrap();
    let changes = next_where(&mut reload, |changes| !changes.documents.is_empty());
    let latency = since.elapsed();
    let noisy = !writer.is_finished();
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
    assert_eq!(changes.documents, [(document, "(new)".to_owned())]);
    assert!(noisy, "reported only once the writer stopped, {latency:?} after the change");
    // About the debounce time; a loaded machine may take longer to deliver the events.
    assert!(latency < MAX_WAIT, "reported {latency:?} after the change");
}
