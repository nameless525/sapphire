//! Watching real directories: these wait for the platform's file events, for at most a few
//! seconds each.

use super::*;

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
    for _ in 0..250 {
        let changes = reload.poll();
        if !changes.is_empty() {
            return changes;
        }
        std::thread::sleep(TICK);
    }
    panic!("nothing changed in five seconds");
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
fn a_burst_of_writes_is_one_change() {
    let content = TempDir::new("burst");
    let pack = content.join("shared.spack");
    std::fs::write(&pack, b"0").unwrap();
    let mut reload = HotReload::new(TEST_DEBOUNCE).expect("a file-watching backend");
    reload.watch_packs(&content.0).unwrap();
    // Thirty writes over about half a second, more than three debounce times, but never quiet
    // for one.
    for n in 0..30u8 {
        write_atomically(&pack, &[n]);
        std::thread::sleep(Duration::from_millis(15));
    }
    let first = next(&mut reload);
    assert_eq!(first.packs, ["shared"]);
    let rest = gather(&mut reload, 4 * TEST_DEBOUNCE);
    assert!(rest.is_empty(), "the burst was reported once: then {rest:?}");
    assert!(reload.poll().is_empty());
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
    assert!(reload.poll().is_empty());

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

    // A removed pack is a pack to remount: the remount fails, and the asset server keeps it.
    std::fs::remove_file(content.join("shared.spack")).unwrap();
    assert_eq!(next(&mut reload).packs, ["shared"]);
    // A rename changes both names.
    std::fs::rename(content.join("zone.spack"), content.join("district.spack")).unwrap();
    let mut packs = next(&mut reload).packs;
    packs.sort();
    assert_eq!(packs, ["district", "zone"]);

    // A removed shader keeps the text it had, and says why.
    std::fs::remove_file(shaders.join("outline.wgsl")).unwrap();
    let changes = next(&mut reload);
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
