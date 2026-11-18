//! `cargo xtask content --watch` (M6 C4): cooks `content/`, then cooks it again each time a source in it changes,
//! until interrupted, so a saved source reaches a running app with no command in between.
//!
//! The watch is `sapphire_watch`'s [`FileWatcher`], as hot reload and live link watch with it: every folder of
//! `content/` but hidden ones (which no cook reads), debounced per file. Each change it reports is one more run
//! of the same cook `cargo xtask content` runs, the pipeline binary on the whole tree through the content cache,
//! so it cooks again only what the change reached and rewrites only the packs that changed. No daemon, and no
//! second cooker: the pipeline is run as a process, as every cook runs it. A cook that fails is printed and the
//! watch goes on, since the next save may mend it. The cook's lock is held for each cook alone, so other runs
//! read or cook the content between saves.
//!
//! What follows from the packs is the apps': a desktop dev app's hot reload remounts a rewritten pack, and
//! `cargo xtask link watch` pushes it to a device (a simulator, an emulator, a phone) that reads this cook.
//!
//! A cook that gives a new source its sidecar writes that sidecar into `content/`, which the watch sees as one
//! more change: one more cook, which finds nothing to do.
//!
//! Each error of the watcher's is printed and cooks too. The platform may have dropped events (FSEvents folds
//! them into a rescan under load) and a save among them would otherwise reach no cook; a cook costs only what
//! changed, so cooking on a doubt is cheap.

use std::ffi::OsStr;
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use sapphire_time::Stopwatch;
use sapphire_watch::{DEBOUNCE, FileWatcher, WatchEvents, WatchId};

use super::{Cook, SOURCE_DIR};
use crate::workspace;

/// How often the watch collects what its watcher reported. The watcher debounces each file by
/// [`DEBOUNCE`] before it does, so this adds at most this much to a save's way to the screen.
const POLL: Duration = Duration::from_millis(20);

/// How many changed sources a cook's line names before it counts the rest.
const NAMED: usize = 4;

/// Cooks `content/` for `cook`, then again on each change, until interrupted; with `report`, each cook prints
/// its phases and slowest stages and assets.
pub fn run(cook: Cook, report: bool) -> anyhow::Result<()> {
    let pipeline = super::pipeline()?;
    super::generate(&pipeline)?;
    let root = workspace::root().join(SOURCE_DIR);
    let mut sources = Sources::new(&root)?;
    let cook_now = |changes: &Changes, folders: usize| {
        let stopwatch = Stopwatch::start();
        match super::cook_alone(&pipeline, &root, cook, report) {
            Ok(_) => eprintln!(
                "{cook}: cooked in {:.2} s{}; watching {folders} folders of {} (Ctrl-C stops)",
                stopwatch.elapsed().as_secs_f64(),
                after(changes),
                root.display()
            ),
            Err(error) => {
                eprintln!(
                    "error: {error:#}\n{cook}: the cook failed{}; save again to cook again",
                    after(changes)
                )
            }
        }
    };
    cook_now(&Changes::default(), sources.folders.len());
    watch(&mut sources, Sources::poll, cook_now, || ControlFlow::Continue(()));
    Ok(())
}

/// The watch's loop: polls `sources` (the command with [`Sources::poll`]), printing the watcher's errors, and
/// runs `cook` once for each poll that found a change or an error, after watching the folders made meanwhile.
/// `go_on` is asked before each poll; the command's never stops (Ctrl-C does), a test's does.
fn watch(
    sources: &mut Sources,
    mut poll: impl FnMut(&mut Sources) -> Changes,
    mut cook: impl FnMut(&Changes, usize),
    mut go_on: impl FnMut() -> ControlFlow<()>,
) {
    while go_on().is_continue() {
        let changes = poll(sources);
        for error in &changes.errors {
            eprintln!("watching: {error}");
        }
        if changes.cooks() {
            sources.watch_new_folders();
            cook(&changes, sources.folders.len());
        } else {
            std::thread::sleep(POLL);
        }
    }
}

/// `" after <sources> changed"`, naming the first few, `" after the watcher's error"` for an error alone (events
/// may have been dropped), or nothing for the first cook.
fn after(changes: &Changes) -> String {
    let changed = &changes.sources;
    if changed.is_empty() {
        return match changes.errors.len() {
            0 => String::new(),
            1 => " after the watcher's error".to_owned(),
            _ => " after the watcher's errors".to_owned(),
        };
    }
    let named: Vec<String> = changed.iter().take(NAMED).map(|path| path.display().to_string()).collect();
    let more = match changed.len().saturating_sub(NAMED) {
        0 => String::new(),
        more => format!(" and {more} more"),
    };
    format!(" after {}{more} changed", named.join(", "))
}

/// The folders of a source tree, watched: each change in one is a source to cook.
struct Sources {
    watcher: FileWatcher,
    root: PathBuf,
    /// Each watched folder, by its path from the root (the root itself is empty).
    folders: Vec<(WatchId, PathBuf)>,
}

/// What changed in a source tree since the last [`Sources::poll`].
#[derive(Debug, Default)]
struct Changes {
    /// Each changed file or folder once, by its path from the root, in the order first seen.
    sources: Vec<PathBuf>,
    /// The watcher's errors: lost events, a watched folder gone.
    errors: Vec<String>,
}

impl Changes {
    /// Whether these call for a cook: any source changed, or any error, since an error may stand for changes
    /// the platform dropped.
    fn cooks(&self) -> bool {
        !self.sources.is_empty() || !self.errors.is_empty()
    }
}

impl Sources {
    /// Watches every folder of the tree at `root` but hidden ones.
    fn new(root: &Path) -> anyhow::Result<Self> {
        let watcher = FileWatcher::new(DEBOUNCE)?;
        let mut sources = Self { watcher, root: root.to_owned(), folders: Vec::new() };
        for folder in
            folders(root).with_context(|| format!("can't list the folders of {}", root.display()))?
        {
            sources.watch(folder)?;
        }
        Ok(sources)
    }

    fn watch(&mut self, folder: PathBuf) -> anyhow::Result<()> {
        let id = self.watcher.watch(&self.root.join(&folder))?;
        if !self.folders.iter().any(|(watched, _)| *watched == id) {
            self.folders.push((id, folder));
        }
        Ok(())
    }

    /// Watches the folders made since the last look, saying why one couldn't be: a source saved into it before
    /// the watch began is cooked all the same, since a cook reads the whole tree.
    fn watch_new_folders(&mut self) {
        let found = match folders(&self.root) {
            Ok(found) => found,
            Err(error) => {
                eprintln!("watching: can't list the folders of {}: {error}", self.root.display());
                return;
            }
        };
        for folder in found {
            if !self.folders.iter().any(|(_, watched)| *watched == folder)
                && let Err(error) = self.watch(folder)
            {
                eprintln!("watching: {error:#}");
            }
        }
    }

    /// What changed since the last poll: names a cook reads (never a hidden one), and the watcher's errors.
    fn poll(&mut self) -> Changes {
        let Self { watcher, root, folders } = self;
        changes(root, folders, watcher.poll())
    }
}

/// What a poll's `events` mean for the tree at `root` whose watched `folders` are given.
///
/// A watched folder that goes is a change too, though no event of its parent's says so: the watcher takes an
/// event naming a folder it watches for news of that folder, and reports it gone as an error. So on an error,
/// each folder no longer there is a change, and is no longer watched here; made again, it is a new folder.
fn changes(root: &Path, folders: &mut Vec<(WatchId, PathBuf)>, events: WatchEvents<'_>) -> Changes {
    let mut changes = Changes { sources: Vec::new(), errors: events.errors.to_vec() };
    for changed in events.changed.iter().filter(|changed| !is_hidden(&changed.name)) {
        let Some((_, folder)) = folders.iter().find(|(watched, _)| *watched == changed.watch) else {
            continue;
        };
        let path = folder.join(&changed.name);
        if !changes.sources.contains(&path) {
            changes.sources.push(path);
        }
    }
    if !changes.errors.is_empty() {
        let gone = folders.extract_if(.., |(_, folder)| !root.join(&*folder).is_dir());
        changes.sources.extend(gone.map(|(_, folder)| folder));
    }
    changes
}

/// Whether a cook leaves `name` out: a hidden file or folder (a dot first), as an editor's swap file and the
/// add-on's file being sent are.
fn is_hidden(name: &OsStr) -> bool {
    name.as_encoded_bytes().first() == Some(&b'.')
}

/// Every folder of the tree at `root` that a cook reads, by its path from the root, the root first (an empty
/// path) and the rest in order: hidden folders, and what is in them, left out.
///
/// A symbolic link is never followed: a cook refuses every link under its root and names it, and a link to a
/// folder above would make this walk endless, before the first cook could say so.
fn folders(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut found = vec![PathBuf::new()];
    let mut next = 0;
    while let Some(folder) = found.get(next).cloned() {
        next += 1;
        let mut inside = Vec::new();
        for entry in fs::read_dir(root.join(&folder))? {
            let entry = entry?;
            // The entry's own type, never what a link names.
            if !is_hidden(&entry.file_name()) && entry.file_type()?.is_dir() {
                inside.push(folder.join(entry.file_name()));
            }
        }
        inside.sort();
        found.extend(inside);
    }
    found.sort();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    /// A folder of its own under the system's temporary folder, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("xtask_content_watch_{name}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn every_folder_a_cook_reads_is_watched() {
        let root = TempDir::new("folders");
        for folder in ["zones/ward/cells", "audio", ".git/objects", "zones/.drafts", "zones/ward/.cache"] {
            fs::create_dir_all(root.0.join(folder)).unwrap();
        }
        fs::write(root.0.join("zones/ward.zone.ron"), "").unwrap();
        let found = folders(&root.0).unwrap();
        let expected = ["", "audio", "zones", "zones/ward", "zones/ward/cells"];
        assert_eq!(found, expected.map(PathBuf::from), "hidden folders and what's in them left out");
    }

    #[test]
    fn hidden_names_cook_nothing() {
        assert!(is_hidden(OsStr::new(".test_character.glb.writing.glb")), "the add-on's export, mid-write");
        assert!(is_hidden(OsStr::new(".DS_Store")));
        assert!(!is_hidden(OsStr::new("test_character.glb")));
        assert!(!is_hidden(OsStr::new("test_character.glb.meta")), "a sidecar's settings are a cook's input");
    }

    #[test]
    fn a_cook_line_names_a_few_changes_and_counts_the_rest() {
        let changed = |sources: Vec<PathBuf>| Changes { sources, errors: Vec::new() };
        assert_eq!(after(&Changes::default()), "", "the first cook follows no change");
        assert_eq!(
            after(&changed(["a.png", "b/c.glb"].map(PathBuf::from).into())),
            " after a.png, b/c.glb changed"
        );
        let many = (0..6).map(|n| PathBuf::from(format!("{n}.ron"))).collect();
        assert_eq!(after(&changed(many)), " after 0.ron, 1.ron, 2.ron, 3.ron and 2 more changed");
        let error = Changes { sources: Vec::new(), errors: vec![DROPPED.to_owned()] };
        assert_eq!(after(&error), " after the watcher's error");
    }

    /// What `sapphire_watch` reports when the platform says it dropped events (FSEvents' rescan).
    const DROPPED: &str = "the platform dropped file events: changes made meanwhile may not be reloaded";

    /// Polls with nothing changed, then with the platform's dropped events alone, then nothing again: the error
    /// is a cook of its own, since a save among the dropped events would otherwise reach none; the quiet polls
    /// are none. Through the watch's own loop, with the watcher's report scripted.
    #[test]
    fn dropped_events_alone_cook_again() {
        let root = TempDir::new("dropped");
        fs::create_dir(root.0.join("characters")).unwrap();
        let mut sources = Sources::new(&root.0).unwrap();
        let dropped = [DROPPED.to_owned()];
        let script: [&[String]; 3] = [&[], &dropped, &[]];
        let (polls, mut cooks) = (Cell::new(0), Vec::new());
        watch(
            &mut sources,
            |sources| {
                let errors = script[polls.get()];
                polls.set(polls.get() + 1);
                changes(&sources.root, &mut sources.folders, WatchEvents { changed: &[], errors })
            },
            |changes, folders| cooks.push((changes.sources.clone(), changes.errors.clone(), folders)),
            || if polls.get() < script.len() { ControlFlow::Continue(()) } else { ControlFlow::Break(()) },
        );
        assert_eq!(
            cooks,
            [(Vec::new(), dropped.to_vec(), 2)],
            "one cook, for the error, both folders still watched"
        );
    }

    /// A link is never followed, as a cook never follows one: a link to a folder above leaves the walk finite,
    /// and the cook names the link.
    #[cfg(unix)]
    #[test]
    fn the_folder_walk_never_follows_a_link() {
        let root = TempDir::new("link");
        fs::create_dir_all(root.0.join("zones/ward")).unwrap();
        std::os::unix::fs::symlink(&root.0, root.0.join("zones/ward/up")).unwrap();
        std::os::unix::fs::symlink(root.0.join("zones"), root.0.join("zones_too")).unwrap();
        let found = folders(&root.0).unwrap();
        assert_eq!(found, ["", "zones", "zones/ward"].map(PathBuf::from), "no link walked into");
    }

    /// The longest the platform test waits: a saturated Mac's FSEvents has delivered nothing for a minute. The
    /// wait ends the moment the event comes (as `sapphire_watch`'s own platform test).
    const PLATFORM_WAIT: Duration = Duration::from_secs(60);

    /// How often it writes the file again while it waits, in case the platform lost the event without saying so.
    const REWRITE_EVERY: Duration = Duration::from_secs(5);

    /// Polls `sources` until `wanted` changes, writing `file` again every few seconds; panics after
    /// [`PLATFORM_WAIT`]. Says nothing of how fast, which only the platform decides.
    fn wait_for(sources: &mut Sources, wanted: &Path, mut write: impl FnMut()) {
        let (since, mut writes, mut errors) = (Stopwatch::start(), 0u32, Vec::new());
        while since.elapsed() < PLATFORM_WAIT {
            if since.elapsed() >= REWRITE_EVERY * writes {
                write();
                writes += 1;
            }
            let changes = sources.poll();
            if changes.sources.iter().any(|source| source == wanted) {
                return;
            }
            errors.extend(changes.errors);
            std::thread::sleep(POLL);
        }
        panic!(
            "{} wasn't reported in {PLATFORM_WAIT:?} ({writes} writes); errors: {errors:?}",
            wanted.display()
        );
    }

    /// A source saved in a folder made after the watch began is a change, by its path from the root, once the
    /// watch has looked for new folders, as it does before each cook: the folder made is itself a change in its
    /// parent, and so a cook. So is the folder going, with its sources.
    /// The watch's whole loop on the platform's own events: a source saved (with an editor's hidden swap file
    /// beside it) is one cook, naming that source alone. The cook here only counts, since the pipeline's own test
    /// (`cook_on_save`) proves what a cook after that save cooks: that asset alone, and its one pack.
    #[test]
    fn a_save_is_one_cook_of_that_source() {
        let root = TempDir::new("loop");
        fs::create_dir(root.0.join("characters")).unwrap();
        let mut sources = Sources::new(&root.0).unwrap();
        let (cooks, since, mut writes) = (RefCell::new(Vec::new()), Stopwatch::start(), 0u32);
        watch(
            &mut sources,
            Sources::poll,
            |changes, _| cooks.borrow_mut().push(changes.sources.clone()),
            || {
                if !cooks.borrow().is_empty() || since.elapsed() >= PLATFORM_WAIT {
                    return ControlFlow::Break(());
                }
                // Again every few seconds, in case the platform lost the event without saying so.
                if since.elapsed() >= REWRITE_EVERY * writes {
                    writes += 1;
                    fs::write(root.0.join("characters/.hero.glb.swp"), [writes as u8]).unwrap();
                    fs::write(root.0.join("characters/hero.glb"), [writes as u8]).unwrap();
                }
                ControlFlow::Continue(())
            },
        );
        let cooks = cooks.into_inner();
        assert_eq!(cooks, [[PathBuf::from("characters/hero.glb")]], "after {writes} writes");
    }

    #[test]
    fn the_platforms_events_reach_a_folder_made_after_the_watch_began_and_its_going() {
        let root = TempDir::new("platform");
        fs::create_dir(root.0.join("characters")).unwrap();
        let mut sources = Sources::new(&root.0).unwrap();
        assert_eq!(sources.folders.len(), 2);

        let folder = root.0.join("characters/new");
        wait_for(&mut sources, Path::new("characters/new"), || {
            let _ = fs::create_dir(&folder);
            fs::write(root.0.join("characters/.swap"), "").unwrap();
        });
        sources.watch_new_folders();
        assert_eq!(sources.folders.len(), 3);
        let mut bytes = 0u8;
        wait_for(&mut sources, Path::new("characters/new/hero.glb"), || {
            bytes += 1;
            fs::write(folder.join("hero.glb"), [bytes]).unwrap();
        });

        // Removed with what's in it: the folder itself is the change, and it is watched no more.
        wait_for(&mut sources, Path::new("characters/new"), || {
            let _ = fs::remove_dir_all(&folder);
        });
        assert_eq!(sources.folders.len(), 2);
    }
}
