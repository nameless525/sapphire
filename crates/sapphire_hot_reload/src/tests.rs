//! Hot reload's own logic, on the watcher's events made here: what a changed file is (a pack, a
//! shader source, a document, or none of these), with the files read as they are. When files
//! are reported, debounced, is `sapphire_watch`'s logic, tested there on a test's events and
//! time. So no platform or clock decides what these see, and they hold however late a busy
//! machine delivers its file events. One test waits for the platform's own events.

use sapphire_time::Stopwatch;

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

/// What hot reload makes of the watcher reporting `changed`, each a file's watch and name.
fn changes(reload: &mut HotReload, changed: &[(WatchId, &str)]) -> Changes {
    let changed: Vec<Changed> =
        changed.iter().map(|(watch, name)| Changed { watch: *watch, name: (*name).into() }).collect();
    reload.watched.changes(WatchEvents { changed: &changed, errors: &[] })
}

/// Writes `bytes` to `path` as the pipeline writes packs: to a temporary file, renamed into place.
fn write_atomically(path: &Path, bytes: &[u8]) {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    std::fs::write(&temp, bytes).expect("write");
    std::fs::rename(&temp, path).expect("rename");
}

/// Hot reload watching a content root, a shader directory holding every shader file, and two
/// documents in one directory, as the default day and light rigs are.
struct Watching {
    reload: HotReload,
    content: TempDir,
    shaders: TempDir,
    documents: TempDir,
    packs_watch: WatchId,
    shaders_watch: WatchId,
    documents_watch: WatchId,
    day: DocumentId,
    rigs: DocumentId,
}

fn watching(name: &str) -> Watching {
    let content = TempDir::new(&format!("{name}_content"));
    let shaders = TempDir::new(&format!("{name}_shaders"));
    let documents = TempDir::new(&format!("{name}_documents"));
    for file in ShaderFile::ALL {
        std::fs::write(shaders.join(file.file_name()), file.compiled_in()).unwrap();
    }
    std::fs::write(documents.join("day.ron"), "(keys: [])").unwrap();
    std::fs::write(documents.join("rigs.ron"), "(rigs: [])").unwrap();
    let mut reload = HotReload::new(TEST_DEBOUNCE).expect("a file-watching backend");
    reload.watch_packs(&content.0).unwrap();
    reload.watch_shaders(&shaders.0).unwrap();
    let day = reload.watch_document(&documents.join("day.ron")).unwrap();
    let rigs = reload.watch_document(&documents.join("rigs.ron")).unwrap();
    assert_ne!(day, rigs);
    let packs_watch = reload.watched.packs.expect("packs watched");
    let shaders_watch = reload.watched.shaders.as_ref().expect("shaders watched").0;
    let documents_watch = reload.watched.documents[0].0;
    assert_eq!(documents_watch, reload.watched.documents[1].0, "one directory, watched once");
    Watching { reload, content, shaders, documents, packs_watch, shaders_watch, documents_watch, day, rigs }
}

#[test]
fn packs_shaders_and_documents_are_reported_as_what_they_are() {
    let Watching {
        mut reload,
        content,
        shaders,
        documents,
        packs_watch,
        shaders_watch,
        documents_watch,
        day,
        rigs,
    } = watching("kinds");

    // A pack lands; the temporary file it was written to is no pack.
    write_atomically(&content.join("shared.spack"), b"pack");
    let changes_made =
        changes(&mut reload, &[(packs_watch, "shared.spack.tmp"), (packs_watch, "shared.spack")]);
    assert_eq!(changes_made.packs, ["shared"]);
    assert!(
        changes_made.shaders.is_none() && changes_made.documents.is_empty() && changes_made.errors.is_empty()
    );

    // A shader: every file is read again, the changed one with its new text.
    let sky = format!("{}\n// edited\n", ShaderFile::Sky.compiled_in());
    std::fs::write(shaders.join("sky.wgsl"), &sky).unwrap();
    let changes_made = changes(&mut reload, &[(shaders_watch, "sky.wgsl")]);
    let sources = changes_made.shaders.expect("new shader sources");
    assert_eq!(sources.get(ShaderFile::Sky), sky);
    assert_eq!(sources.get(ShaderFile::Frame), ShaderFile::Frame.compiled_in());
    assert!(
        changes_made.packs.is_empty() && changes_made.documents.is_empty() && changes_made.errors.is_empty()
    );

    // A document, with its text, and not its neighbor.
    std::fs::write(documents.join("day.ron"), "(keys: [()])").unwrap();
    let changes_made = changes(&mut reload, &[(documents_watch, "day.ron")]);
    assert_eq!(changes_made.documents, [(day, "(keys: [()])".to_owned())]);
    std::fs::write(documents.join("rigs.ron"), "(rigs: [()])").unwrap();
    let changes_made = changes(&mut reload, &[(documents_watch, "rigs.ron")]);
    assert_eq!(changes_made.documents, [(rigs, "(rigs: [()])".to_owned())]);

    // All three in one report, each as what it is.
    write_atomically(&content.join("zone.spack"), b"pack");
    let changes_made = changes(
        &mut reload,
        &[(documents_watch, "day.ron"), (shaders_watch, "frame.wgsl"), (packs_watch, "zone.spack")],
    );
    assert_eq!(changes_made.packs, ["zone"]);
    assert_eq!(changes_made.shaders.expect("shader sources").get(ShaderFile::Sky), sky);
    assert_eq!(changes_made.documents, [(day, "(keys: [()])".to_owned())]);
    assert!(changes_made.errors.is_empty(), "{:?}", changes_made.errors);

    // Files that are none of these: what a file is depends on its directory as well as its name.
    std::fs::write(content.join("notes.txt"), "x").unwrap();
    std::fs::write(shaders.join("notes.wgsl"), "x").unwrap();
    std::fs::write(documents.join("other.ron"), "x").unwrap();
    let none = changes(
        &mut reload,
        &[
            (packs_watch, "notes.txt"),
            (packs_watch, "sky.wgsl"),
            (packs_watch, "day.ron"),
            (shaders_watch, "notes.wgsl"),
            (shaders_watch, "shared.spack"),
            (documents_watch, "other.ron"),
            (documents_watch, "zone.spack"),
        ],
    );
    assert!(none.is_empty(), "{none:?}");
}

#[test]
fn removed_and_renamed_files_are_changes() {
    let Watching {
        mut reload, content, shaders, documents, packs_watch, shaders_watch, documents_watch, ..
    } = watching("removed");

    // A removed pack is a pack to remount: the remount fails, and the asset server keeps it. A
    // rename changes both names.
    std::fs::write(content.join("zone.spack"), b"pack").unwrap();
    std::fs::rename(content.join("zone.spack"), content.join("district.spack")).unwrap();
    let changes_made = changes(&mut reload, &[(packs_watch, "zone.spack"), (packs_watch, "district.spack")]);
    assert_eq!(changes_made.packs, ["zone", "district"]);

    // A removed shader keeps the text it had, and says why.
    std::fs::remove_file(shaders.join("outline.wgsl")).unwrap();
    let changes_made = changes(&mut reload, &[(shaders_watch, "outline.wgsl")]);
    let sources = changes_made.shaders.expect("shader sources");
    assert_eq!(sources.get(ShaderFile::Outline), ShaderFile::Outline.compiled_in());
    assert!(
        changes_made.errors.iter().any(|error| error.contains("outline.wgsl")),
        "{:?}",
        changes_made.errors
    );

    // A removed document says why, and brings no text.
    std::fs::remove_file(documents.join("day.ron")).unwrap();
    let changes_made = changes(&mut reload, &[(documents_watch, "day.ron")]);
    assert!(changes_made.documents.is_empty());
    assert!(changes_made.errors.iter().any(|error| error.contains("day.ron")), "{:?}", changes_made.errors);

    // The watcher's own errors come through, saying whose they are.
    let errors = ["the watched directory is gone".to_owned()];
    let changes_made = reload.watched.changes(WatchEvents { changed: &[], errors: &errors });
    assert_eq!(changes_made.errors, ["file watching: the watched directory is gone"]);
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
    // A name alone is a file where the app runs, as `--watch-document x.moves.ron` names one.
    reload.watch_document(Path::new("Cargo.toml")).expect("a file here");
}

/// The longest [`the_platforms_events_reach_hot_reload`] waits: a saturated Mac's FSEvents has
/// delivered nothing for more than ten seconds, once for a minute. The wait ends the moment the
/// events come.
const PLATFORM_WAIT: Duration = Duration::from_secs(60);

/// How often [`the_platforms_events_reach_hot_reload`] writes the files again while it waits, in
/// case the platform lost the events without saying so.
const REWRITE_EVERY: Duration = Duration::from_secs(5);

/// The platform's own events reach hot reload: a pack landing and a document saved once the
/// watches began are reported, the document with its text. Nothing is in either directory before
/// then, so nothing from before can stand in for these writes; and nothing here says how fast or
/// how often, which only the platform decides.
#[test]
fn the_platforms_events_reach_hot_reload() {
    let content = TempDir::new("platform_content");
    let documents = TempDir::new("platform_documents");
    let day = documents.join("day.ron");
    let mut reload = HotReload::new(TEST_DEBOUNCE).expect("a file-watching backend");
    reload.watch_packs(&content.0).unwrap();
    let document = reload.watch_document(&day).unwrap();

    let (since, mut writes, mut dropped) = (Stopwatch::start(), 0u32, false);
    let (mut pack, mut text, mut errors) = (false, false, Vec::new());
    while since.elapsed() < PLATFORM_WAIT {
        // Again every few seconds, and at once if the platform said it dropped events.
        if dropped || since.elapsed() >= REWRITE_EVERY * writes {
            write_atomically(&content.join("shared.spack"), b"pack");
            std::fs::write(&day, "(keys: [])").unwrap();
            writes += 1;
        }
        let changes = reload.poll();
        pack |= changes.packs.iter().any(|pack| pack == "shared");
        text |= changes.documents.contains(&(document, "(keys: [])".to_owned()));
        if pack && text {
            return;
        }
        dropped = changes.errors.iter().any(|error| error.contains("dropped file events"));
        errors.extend(changes.errors);
        std::thread::sleep(TICK);
    }
    panic!(
        "in {PLATFORM_WAIT:?} of {writes} writes, the pack was reported: {pack}, the document: {text}; errors \
         meanwhile: {errors:?}"
    );
}
