//! A keep stopped at every write (M4 F3), as P3's install is: a session played with each of its writes in turn
//! failing, dying, or dying part way through, must leave files that agree.
//!
//! - **Dying** (the process killed there, so nothing after it reaches the disk): the session's document is the
//!   one the last whole keep wrote, or none if the first keep never finished, and every run it names replays
//!   to the checksums the device kept, with as many rows of frames as it counts. A write that dies part way
//!   leaves part of its bytes: an append's first half, or a replace's stray temp.
//! - **Failing** (an error, and the process goes on): the next keep writes it again, and the session ends whole,
//!   its document the one a session with no failures ends with.
//!
//! `keep_kill` kills a real process instead, at moments that aren't chosen.

mod common;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Plain, Played, Scratch, Settings, about, every_run_replays};
use sapphire_playtest::{DOCUMENT_FILE, Disk, Ending, Session};

/// A frame of 100 ms, so a session keeps every 100 frames.
const FRAME: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    /// The process dies as the write begins.
    Dies,
    /// The process dies with part of the write done.
    DiesPartWay,
    /// The write fails part way, and the process goes on.
    Fails,
}

#[derive(Default)]
struct Log {
    /// Writes so far.
    writes: usize,
    dead: bool,
    /// Each document written, as written.
    documents: Vec<String>,
}

/// A disk whose write numbered `at` meets `fault`, and which logs the documents it writes.
struct Faulty {
    fault: Option<(usize, Fault)>,
    log: Mutex<Log>,
}

impl Faulty {
    fn new(fault: Option<(usize, Fault)>) -> Arc<Self> {
        Arc::new(Self { fault, log: Mutex::new(Log::default()) })
    }

    /// Whether this write goes ahead, and if not, how much of it is done first.
    fn fate(&self) -> Result<(), Fault> {
        let mut log = self.log.lock().unwrap();
        let number = log.writes;
        log.writes += 1;
        if log.dead {
            return Err(Fault::Dies);
        }
        match self.fault {
            Some((at, fault)) if at == number => {
                log.dead = fault != Fault::Fails;
                Err(fault)
            }
            _ => Ok(()),
        }
    }
}

fn failed() -> io::Error {
    io::Error::other("the fault")
}

impl Disk for Faulty {
    fn replace(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        match self.fate() {
            Ok(()) => {
                Plain.replace(path, bytes)?;
                if path.ends_with(DOCUMENT_FILE) {
                    self.log.lock().unwrap().documents.push(String::from_utf8(bytes.to_vec()).unwrap());
                }
                Ok(())
            }
            Err(Fault::Dies) => Err(failed()),
            // `durable_replace`'s temp, written part way: never the file itself.
            Err(Fault::DiesPartWay | Fault::Fails) => {
                fs::write(path.with_added_extension("tmp"), &bytes[..bytes.len() / 2])?;
                Err(failed())
            }
        }
    }

    fn append(&self, path: &Path, at: u64, bytes: &[u8]) -> io::Result<()> {
        match self.fate() {
            Ok(()) => Plain.append(path, at, bytes),
            Err(Fault::Dies) => Err(failed()),
            Err(Fault::DiesPartWay | Fault::Fails) => {
                Plain.append(path, at, &bytes[..bytes.len() / 2])?;
                Err(failed())
            }
        }
    }
}

/// A session of two runs: settings, a mark and a change of settings in the first, which ends; the second sent
/// to the background, then quit. Each keep is written before the next. Gives the session's directory.
fn play(sessions: &Path, disk: Arc<Faulty>) -> PathBuf {
    let mut session = Session::begin(sessions, about(Some(1_794_344_673)), disk).unwrap();
    let dir = session.dir().to_owned();
    session.settings(&Settings { volume: 80 });
    let mut first = Played::new();
    first.play_with(&mut session, 250, FRAME, true);
    session.mark(Some("there"));
    session.settings(&Settings { volume: 40 });
    first.play_with(&mut session, 100, FRAME, true);
    let (replay, trail) = first.so_far();
    session.run_ended(replay, trail);
    let mut second = Played::new();
    second.play_with(&mut session, 230, FRAME, true);
    session.left_foreground();
    session.flush(Some(second.so_far()));
    second.play_with(&mut session, 40, FRAME, true);
    session.end(Some(second.so_far()));
    dir
}

#[test]
fn a_keep_stopped_at_any_write_leaves_files_that_agree() {
    let scratch = Scratch::new("faults");
    let whole = Faulty::new(None);
    let dir = play(&scratch.sessions(), Arc::clone(&whole));
    let (writes, documents) = {
        let log = whole.log.lock().unwrap();
        (log.writes, log.documents.clone())
    };
    let last = documents.last().unwrap().clone();
    let kept = every_run_replays(&dir);
    assert_eq!(kept.document.runs.len(), 2);
    assert!(matches!(kept.document.ended, Some(Ending::Quit { .. })));
    assert!(documents.len() >= 8, "{} keeps", documents.len());
    fs::remove_dir_all(scratch.sessions()).unwrap();

    let (mut dead, mut part_way, mut none_kept, mut failed) = (0, 0, 0, 0);
    for at in 0..writes {
        for fault in [Fault::Dies, Fault::DiesPartWay, Fault::Fails] {
            let disk = Faulty::new(Some((at, fault)));
            let dir = play(&scratch.sessions(), Arc::clone(&disk));
            let written = disk.log.lock().unwrap().documents.clone();
            let what = format!("write {at} of {writes}, {fault:?}");
            let on_disk = fs::read_to_string(dir.join(DOCUMENT_FILE)).ok();
            if fault == Fault::Fails {
                assert_eq!(
                    on_disk.as_deref(),
                    Some(&*last),
                    "{what}: the session ends as one with no failure"
                );
                every_run_replays(&dir);
                failed += 1;
            } else {
                // The documents it wrote are a session with no fault's, up to the fault.
                assert_eq!(written[..], documents[..written.len()], "{what}");
                assert_eq!(on_disk.as_ref(), written.last(), "{what}: the last whole keep's document");
                if on_disk.is_some() {
                    every_run_replays(&dir);
                    if fault == Fault::Dies { dead += 1 } else { part_way += 1 }
                } else {
                    none_kept += 1;
                }
            }
            fs::remove_dir_all(scratch.sessions()).unwrap();
        }
    }
    eprintln!(
        "{writes} writes in {} keeps: {dead} died there, {part_way} died part way, {none_kept} before the first \
         keep was whole, {failed} failed and went on",
        documents.len()
    );
    assert!(none_kept <= 2 * 4, "only the first keep's writes leave nothing");
}
