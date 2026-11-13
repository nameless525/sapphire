//! The watcher's own logic, on a test's file events and time ([`Script`]): the debounce per file,
//! a burst coalesced, a file written all the time, directories gone and back. No platform,
//! thread or clock decides what these see, so they hold however late a busy machine delivers its
//! file events (a saturated Mac has delivered FSEvents more than ten seconds late). One test waits
//! for the platform's own events.

use std::path::{Path, PathBuf};

use notify::event::{AccessKind, CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind, RenameMode};
use notify::{Event, EventKind};
use sapphire_time::Stopwatch;

use super::*;
use crate::watch::{MAX_WAIT, Script};

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
/// How often the tests poll, as an app polls once a frame.
const TICK: Duration = Duration::from_millis(20);

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

fn event(kind: EventKind, paths: &[&Path]) -> notify::Result<Event> {
    Ok(paths.iter().fold(Event::new(kind), |event, path| event.add_path(path.to_path_buf())))
}

fn write(path: &Path) -> notify::Result<Event> {
    event(EventKind::Modify(ModifyKind::Data(DataChange::Content)), &[path])
}

fn read(path: &Path) -> notify::Result<Event> {
    event(EventKind::Access(AccessKind::Read), &[path])
}

/// A watcher of a directory of its own in `root`, on a script's events and time.
fn scripted(root: &TempDir, max_wait: Duration) -> (FileWatcher, WatchId, Script, PathBuf) {
    let dir = root.join("watched");
    std::fs::create_dir(&dir).unwrap();
    let (mut watcher, script) = FileWatcher::scripted(TEST_DEBOUNCE, max_wait);
    let watch = watcher.watch(&dir).unwrap();
    assert_eq!(script.take_calls(), [(watcher.dir(watch).to_owned(), true)]);
    (watcher, watch, script, dir)
}

/// A poll that reported something: when, on the script's time, and the names and errors.
#[derive(Debug, PartialEq)]
struct Poll {
    at: Duration,
    changed: Vec<String>,
    errors: Vec<String>,
}

fn poll(watcher: &mut FileWatcher, script: &Script) -> Option<Poll> {
    let events = watcher.poll();
    let changed: Vec<String> =
        events.changed.iter().map(|changed| changed.name.to_string_lossy().into_owned()).collect();
    let errors = events.errors.to_vec();
    (!changed.is_empty() || !errors.is_empty()).then(|| Poll { at: script.now(), changed, errors })
}

/// Runs the script from `start` to `end` a millisecond at a time: `platform` sends what the
/// platform reports at each millisecond from `start`, and the watcher is polled every [`TICK`].
/// Returns the polls that reported something, with their times from `start`.
fn play(
    watcher: &mut FileWatcher,
    script: &Script,
    start: Duration,
    end: Duration,
    mut platform: impl FnMut(u64, &Script),
) -> Vec<Poll> {
    let mut polls = Vec::new();
    for t in 0..=(end - start).as_millis() as u64 {
        script.set_time(start + ms(t));
        platform(t, script);
        if t % TICK.as_millis() as u64 == 0
            && let Some(poll) = poll(watcher, script)
        {
            polls.push(Poll { at: poll.at - start, ..poll });
        }
    }
    polls
}

fn changed(at: u64, names: &[&str]) -> Poll {
    Poll { at: ms(at), changed: names.iter().map(|name| (*name).to_owned()).collect(), errors: Vec::new() }
}

#[test]
fn a_file_written_all_the_time_is_still_reported() {
    // Five seconds of writes every 5 ms, never quiet for a debounce time, with a longest wait of
    // 0.6 s: reported every 0.6 s, and a debounce time after the writes stop.
    let root = TempDir::new("writer");
    let (mut watcher, _, script, dir) = scripted(&root, ms(600));
    let log = dir.join("log.txt");
    let polls = play(&mut watcher, &script, Duration::ZERO, ms(5200), |t, script| {
        if t % 5 == 0 && t < 5000 {
            script.send(write(&log));
        }
    });
    let mut expected: Vec<Poll> = (1..=8).map(|n| changed(600 * n, &["log.txt"])).collect();
    expected.push(changed(5160, &["log.txt"]));
    assert_eq!(polls, expected);
}

/// DT-006: one debounce for every watched file waited for all of them to be quiet, so a file
/// written every few milliseconds held back every other change for as long as it lasted. The
/// quiet file is reported a debounce time after its change, the busy one only at the longest
/// wait, and files read (some platforms report reads) never.
fn a_busy_file_and_a_quiet_one(
    watcher: &mut FileWatcher,
    script: &Script,
    dir: &Path,
    start: Duration,
) -> Vec<Poll> {
    let (noise, target, other) = (dir.join("noise.tmp"), dir.join("target.ron"), dir.join("other.spack"));
    play(watcher, script, start, start + ms(2100), |t, script| {
        if t % 5 == 0 {
            script.send(write(&noise));
            script.send(read(&other));
        }
        if t == 300 {
            script.send(write(&target));
        }
    })
}

#[test]
fn a_file_written_all_the_time_never_holds_back_a_quiet_one() {
    let root = TempDir::new("noise");
    let (mut watcher, _, script, dir) = scripted(&root, MAX_WAIT);
    let polls = a_busy_file_and_a_quiet_one(&mut watcher, &script, &dir, Duration::ZERO);
    assert_eq!(polls, [changed(460, &["target.ron"]), changed(2000, &["noise.tmp"])]);
}

#[test]
fn a_burst_of_writes_is_one_change() {
    // Thirty writes of a pack 15 ms apart, each to a temporary file renamed into place, as the
    // pipeline writes packs: both names once, a debounce time after the last.
    let root = TempDir::new("burst");
    let (mut watcher, _, script, dir) = scripted(&root, MAX_WAIT);
    let (pack, temp) = (dir.join("shared.spack"), dir.join("shared.spack.tmp"));
    let polls = play(&mut watcher, &script, Duration::ZERO, ms(1000), |t, script| {
        if t % 15 == 0 && t < 450 {
            script.send(event(EventKind::Create(CreateKind::File), &[&temp]));
            script.send(write(&temp));
            script.send(event(EventKind::Modify(ModifyKind::Name(RenameMode::Both)), &[&temp, &pack]));
        }
    });
    assert_eq!(polls, [changed(600, &["shared.spack.tmp", "shared.spack"])]);
}

#[test]
fn events_delivered_late_are_debounced_from_when_they_come() {
    // The watcher sees when events come, not when the files were written: a burst whose events
    // all come at once after a stall is one change, a debounce time after they came.
    let root = TempDir::new("late");
    let (mut watcher, _, script, dir) = scripted(&root, MAX_WAIT);
    let pack = dir.join("shared.spack");
    let polls = play(&mut watcher, &script, Duration::ZERO, ms(4000), |t, script| {
        if t == 3000 {
            (0..30).for_each(|_| script.send(write(&pack)));
        }
    });
    assert_eq!(polls, [changed(3160, &["shared.spack"])]);

    // A burst whose delivery stalls for longer than the debounce time mid-way can't be told from
    // two bursts, and is two changes: a second reload of the same file, never a lost one.
    let polls = play(&mut watcher, &script, ms(4000), ms(5000), |t, script| {
        if t % 10 == 0 && (t <= 100 || (400..=500).contains(&t)) {
            script.send(write(&pack));
        }
    });
    assert_eq!(polls, [changed(260, &["shared.spack"]), changed(660, &["shared.spack"])]);
}

/// The tests above read no real time: with the script's time far ahead of where a watcher
/// starts, they see the same; with it stalled, nothing comes due however often the watcher is
/// polled and however many events come meanwhile; and jumped far ahead, everything waiting comes
/// due in the one poll.
#[test]
fn what_is_reported_depends_on_the_scripts_time_alone() {
    let far = Duration::from_secs(1 << 30);
    let root = TempDir::new("far");
    let (mut watcher, _, script, dir) = scripted(&root, MAX_WAIT);
    let polls = a_busy_file_and_a_quiet_one(&mut watcher, &script, &dir, far);
    assert_eq!(polls, [changed(460, &["target.ron"]), changed(2000, &["noise.tmp"])]);

    let root = TempDir::new("stalled");
    let (mut watcher, _, script, dir) = scripted(&root, MAX_WAIT);
    let (noise, target) = (dir.join("noise.tmp"), dir.join("target.ron"));
    script.set_time(ms(300));
    script.send(write(&target));
    script.set_time(ms(440));
    for _ in 0..10_000 {
        script.send(write(&noise));
        assert_eq!(poll(&mut watcher, &script), None);
    }
    script.set_time(far);
    assert_eq!(
        poll(&mut watcher, &script),
        Some(Poll { at: far, ..changed(0, &["target.ron", "noise.tmp"]) })
    );
    assert_eq!(poll(&mut watcher, &script), None);
    assert!(script.take_calls().is_empty(), "the directory is still the one watched");
}

/// Review DV-12. Each backend treats a directory that goes in its own way: inotify drops the
/// watch and reports it, Windows' drops it and reports nothing, and FSEvents goes on watching
/// the path. So the watcher looks at each directory every second, and reports it gone once.
#[test]
fn a_watched_directory_gone_is_reported_once_and_watched_again_when_it_is_back() {
    let root = TempDir::new("gone");
    let (mut watcher, watch, script, dir) = scripted(&root, MAX_WAIT);
    let shown = watcher.dir(watch).display().to_string();

    // Noticed at the next look, a second after the watcher began, though no event says so.
    std::fs::remove_dir_all(&dir).unwrap();
    script.set_time(ms(999));
    assert_eq!(poll(&mut watcher, &script), None);
    script.set_time(ms(1000));
    let gone = poll(&mut watcher, &script).expect("reported");
    assert!(gone.changed.is_empty() && gone.errors.len() == 1 && gone.errors[0].contains(&shown), "{gone:?}");

    // Once, however long it stays gone.
    assert_eq!(play(&mut watcher, &script, ms(1000), ms(11_000), |_, _| {}), []);

    // Back, with a file in it before anything watches it again: nothing would report it
    // otherwise. The backend is asked to watch it again.
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("written_meanwhile"), b"meanwhile").unwrap();
    let back = play(&mut watcher, &script, ms(11_000), ms(12_000), |_, _| {});
    assert_eq!(back, [changed(1000, &["written_meanwhile"])]);
    let canonical = watcher.dir(watch).to_owned();
    assert_eq!(script.take_calls(), [(canonical.clone(), false), (canonical, true)]);
}

/// The platform naming a watched directory has the watcher look at it then, not at the next
/// second: the name comes out of the debounce as a file's does.
#[test]
fn a_watched_directory_gone_is_reported_when_an_event_names_it() {
    let root = TempDir::new("named");
    let (mut watcher, watch, script, dir) = scripted(&root, MAX_WAIT);
    let canonical = watcher.dir(watch).to_owned();
    std::fs::remove_dir_all(&dir).unwrap();
    let polls = play(&mut watcher, &script, Duration::ZERO, ms(500), |t, script| {
        if t == 10 {
            script.send(event(EventKind::Remove(RemoveKind::Folder), &[&canonical]));
        }
    });
    let shown = canonical.display().to_string();
    assert!(
        matches!(polls.as_slice(), [Poll { at, changed, errors }]
            if *at == ms(160) && changed.is_empty() && errors.len() == 1 && errors[0].contains(&shown)),
        "{polls:?}"
    );
}

/// A directory removed and made again between polls is never seen gone, but on Linux the watch
/// went with the old one: the event naming it has the watcher look, and it is another directory.
#[test]
fn a_watched_directory_made_again_at_once_is_watched_again() {
    // `notify` can't watch through this on Windows: when its read fails and the directory is
    // there again, it goes on reading the deleted one, and nothing says so.
    if cfg!(windows) {
        return;
    }
    let root = TempDir::new("made_again");
    let (mut watcher, watch, script, dir) = scripted(&root, MAX_WAIT);
    let canonical = watcher.dir(watch).to_owned();
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("written_meanwhile"), b"meanwhile").unwrap();
    let polls = play(&mut watcher, &script, Duration::ZERO, ms(500), |t, script| {
        if t == 10 {
            script.send(event(EventKind::Create(CreateKind::Folder), &[&canonical]));
        }
    });
    assert_eq!(polls, [changed(160, &["written_meanwhile"])]);
    assert_eq!(script.take_calls(), [(canonical.clone(), false), (canonical, true)]);
}

/// An event naming a watched directory that is still there is no error: its permissions changed,
/// or FSEvents is reporting its creation, just before the watch began.
#[test]
fn a_watched_directory_changed_in_place_is_no_error() {
    let root = TempDir::new("in_place");
    let (mut watcher, watch, script, dir) = scripted(&root, MAX_WAIT);
    let canonical = watcher.dir(watch).to_owned();
    let permissions =
        || event(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Permissions)), &[&canonical]);
    let readable = std::fs::metadata(&dir).unwrap().permissions();
    let mut read_only = readable.clone();
    read_only.set_readonly(true);
    std::fs::set_permissions(&dir, read_only).unwrap();
    let polls = play(&mut watcher, &script, Duration::ZERO, ms(500), |t, script| {
        if t == 10 {
            script.send(permissions());
        }
    });
    std::fs::set_permissions(&dir, readable).unwrap();
    assert_eq!(polls, []);
    // Nor once it is as it was, nor at the next looks.
    let polls = play(&mut watcher, &script, ms(500), ms(2500), |t, script| {
        if t == 10 {
            script.send(permissions());
        }
    });
    assert_eq!(polls, []);
    assert!(script.take_calls().is_empty(), "the directory is still the one watched");
}

/// The longest [`the_platforms_events_reach_the_watcher`] waits: a saturated Mac's FSEvents has
/// delivered nothing for more than ten seconds, once for a minute. The wait ends the moment the
/// event comes.
const PLATFORM_WAIT: Duration = Duration::from_secs(60);

/// How often [`the_platforms_events_reach_the_watcher`] writes the file again while it waits, in
/// case the platform lost the event without saying so.
const REWRITE_EVERY: Duration = Duration::from_secs(5);

/// The platform's own events reach the watcher: a file written once the watch began is reported.
/// Nothing is in the directory before then, so nothing from before can stand in for the writes;
/// and nothing here says how fast or how often, which only the platform decides.
#[test]
fn the_platforms_events_reach_the_watcher() {
    let root = TempDir::new("platform");
    let dir = root.join("watched");
    std::fs::create_dir(&dir).unwrap();
    let mut watcher = FileWatcher::new(TEST_DEBOUNCE).expect("a file-watching backend");
    watcher.watch(&dir).unwrap();
    let file = dir.join("written");

    let (since, mut writes, mut dropped, mut errors) = (Stopwatch::start(), 0u32, false, Vec::new());
    while since.elapsed() < PLATFORM_WAIT {
        // Again every few seconds, and at once if the platform said it dropped events.
        if dropped || since.elapsed() >= REWRITE_EVERY * writes {
            std::fs::write(&file, writes.to_le_bytes()).unwrap();
            writes += 1;
        }
        let events = watcher.poll();
        if events.changed.iter().any(|changed| changed.name == "written") {
            return;
        }
        dropped = events.errors.iter().any(|error| error.contains("dropped file events"));
        errors.extend_from_slice(events.errors);
        std::thread::sleep(TICK);
    }
    panic!("{writes} writes weren't reported in {PLATFORM_WAIT:?}; errors meanwhile: {errors:?}");
}
