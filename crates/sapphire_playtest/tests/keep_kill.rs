//! A real kill (M4 F3): a helper process plays a session, keeping as a game does (on a job, every 100 frames of
//! 100 ms), run after run, until this one kills it. After every kill the session must read as of its last whole
//! keep, every run it names replaying to the checksums the device kept.
//!
//! **Aimed kills.** The helper writes through a disk of this test's own ([`Aimed`]), which at a chosen place
//! inside a keep marks a file for this process and waits there to be killed. So each place between a keep's
//! writes ([`Place`]) is killed at a few times on any machine and under any load, and every aimed kill must
//! leave on the disk, past the document, what its place has written of the keep. The helper ends each run with
//! its disk held, as a slow one would, so the run's end overtakes the record waiting and goes in one batch with
//! the next run's first (M4 F4's order).
//!
//! **Random kills** land where nobody aimed, after the helper's first whole keep, and assert only what holds
//! after any kill. How many happened to land mid-keep is printed, since the machine's speed decides it.
//!
//! **Against `keep_faults`,** which stops a flushed session at each write in turn in this process, so covers
//! each place here but the one between two runs' records: what the real kill adds is the writes on a job while
//! the frames go on, so batches gather and records overtake, and a process that dies there. `Child::kill` is
//! `SIGKILL` on Unix and `TerminateProcess` on Windows, so nothing runs in the helper after it, and another
//! process reads what the OS kept. The disk writes as `common::Plain`, without the flushes only a power cut
//! needs.
//!
//! Its own `main` (`harness = false`), because the test binary is also the helper.

#![allow(clippy::print_stdout, reason = "with no harness, this says how the kills landed")]

mod common;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use common::{Plain, Played, Scratch, about, every_run_replays};
use sapphire_playtest::{DOCUMENT_FILE, Disk, FRAMES_FILE, Kept, RUNS_DIR, Session, journal};

/// A frame of 100 ms, so a session keeps every 100 frames.
const FRAME: Duration = Duration::from_millis(100);

/// Each run's length, in ticks: a run ends, and another begins.
const RUN: u32 = 700;

/// Which of its times after the first whole keep each place is aimed at, a round each.
const TIMES: [usize; 3] = [1, 2, 5];

/// Random kills, each up to a second after the helper's first whole keep.
const RANDOM: usize = 12;

/// How long a round waits for the helper to reach its place, in polls of 2 ms.
const POLLS: u32 = 30_000;

/// A place between a keep's writes. A batch grows `frames.csv`, then each run's journal, then replaces
/// `session.ron` (the helper keeps no settings), so these are all the places a kill can leave part of a keep
/// on the disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// The keep's rows of frames written, nothing after them.
    Rows,
    /// Part of the rows: an append cut short, inside its last row.
    TornRows,
    /// A run's record added to its journal, the document not yet.
    Record,
    /// Part of a run's record.
    TornRecord,
    /// A run's end written and the next run's first record not, in the batch that carries both.
    BetweenRecords,
    /// The document's temp written, not yet renamed over it.
    Document,
}

impl Place {
    const ALL: [Self; 6] =
        [Self::Rows, Self::TornRows, Self::Record, Self::TornRecord, Self::BetweenRecords, Self::Document];

    fn named(name: &str) -> Self {
        *Self::ALL.iter().find(|place| format!("{place:?}") == name).expect("a place")
    }

    /// Whether a kill here leaves `past` behind it.
    fn leaves(self, past: &Past) -> bool {
        match self {
            Self::Rows => matches!(past, Past::Rows(_)),
            Self::TornRows => matches!(past, Past::PartOfARow),
            Self::Record => matches!(past, Past::Records { .. } | Past::Unnamed { .. }),
            Self::TornRecord => matches!(past, Past::PartOfARecord { .. }),
            // The run that ended has its end written; the next run has nothing yet.
            Self::BetweenRecords => matches!(past, Past::Records { ended: true, .. }),
            Self::Document => matches!(past, Past::NextDocument),
        }
    }
}

/// Part of a keep that a kill left past the session's document.
#[derive(Debug)]
#[allow(dead_code, reason = "the fields say what was left, in a failure's message")]
enum Past {
    /// Rows of frames it doesn't count.
    Rows(usize),
    PartOfARow,
    /// A run's records past the ticks it names, as far as they reach.
    Records {
        run: u32,
        ticks: u32,
        ended: bool,
    },
    PartOfARecord {
        run: u32,
    },
    /// Records of a run it doesn't name.
    Unnamed {
        run: u32,
    },
    /// The next document's temp.
    NextDocument,
}

/// What the disk has seen.
#[derive(Default)]
struct Seen {
    documents: usize,
    /// Since the last document: records grown, and whether anything grew.
    records: usize,
    grew: bool,
    /// Times the aimed place has come.
    met: usize,
}

/// The helper's disk: [`Plain`], which can be held as a slow disk is, and stops at its aim. With no aim it marks
/// the first whole keep and goes on.
struct Aimed {
    /// The place, and which of its times after the first whole keep.
    aim: Option<(Place, usize)>,
    marker: PathBuf,
    seen: Mutex<Seen>,
    held: Mutex<bool>,
    released: Condvar,
}

impl Aimed {
    fn hold(&self, held: bool) {
        *self.held.lock().unwrap() = held;
        self.released.notify_all();
    }

    /// Waits while held, and gives what's been seen.
    fn begin(&self) -> MutexGuard<'_, Seen> {
        let mut held = self.held.lock().unwrap();
        while *held {
            held = self.released.wait(held).unwrap();
        }
        self.seen.lock().unwrap()
    }

    /// Whether this is the aimed place's chosen time: counted only once a whole keep is on the disk.
    fn at(&self, seen: &mut Seen, place: Place) -> bool {
        match self.aim {
            Some((aimed, time)) if aimed == place && seen.documents > 0 => {
                seen.met += 1;
                seen.met == time
            }
            _ => false,
        }
    }

    /// Tells this test it's there, and waits for the kill.
    fn there(&self) -> ! {
        fs::write(&self.marker, b"").expect("marks the place");
        loop {
            thread::park();
        }
    }
}

impl Disk for Aimed {
    fn replace(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut seen = self.begin();
        if !path.ends_with(DOCUMENT_FILE) {
            return Plain.replace(path, bytes);
        }
        if seen.grew && self.at(&mut seen, Place::Document) {
            fs::write(path.with_added_extension("tmp"), bytes)?;
            self.there();
        }
        Plain.replace(path, bytes)?;
        seen.documents += 1;
        seen.records = 0;
        seen.grew = false;
        if self.aim.is_none() && seen.documents == 1 {
            fs::write(&self.marker, b"")?;
        }
        Ok(())
    }

    fn append(&self, path: &Path, at: u64, bytes: &[u8]) -> io::Result<()> {
        let mut seen = self.begin();
        if bytes.is_empty() {
            return Plain.append(path, at, bytes);
        }
        let record = !path.ends_with(FRAMES_FILE);
        let (whole, torn) =
            if record { (Place::Record, Place::TornRecord) } else { (Place::Rows, Place::TornRows) };
        if record && seen.records > 0 && self.at(&mut seen, Place::BetweenRecords) {
            self.there();
        }
        if self.at(&mut seen, torn) {
            // Rows are cut inside the last one, since half of rows alike in length ends on a row's end.
            let from = if record {
                0
            } else {
                bytes[..bytes.len() - 1].iter().rposition(|&byte| byte == b'\n').map_or(0, |end| end + 1)
            };
            Plain.append(path, at, &bytes[..from + (bytes.len() - from) / 2])?;
            self.there();
        }
        Plain.append(path, at, bytes)?;
        seen.grew = true;
        seen.records += usize::from(record);
        if self.at(&mut seen, whole) {
            self.there();
        }
        Ok(())
    }
}

/// The helper: plays runs into a session under `sessions` until it's killed.
fn player(sessions: &Path, marker: PathBuf, aim: Option<(Place, usize)>) -> ExitCode {
    let disk = Arc::new(Aimed {
        aim,
        marker,
        seen: Mutex::default(),
        held: Mutex::new(false),
        released: Condvar::new(),
    });
    let mut session = Session::begin(sessions, about(None), disk.clone()).expect("begins");
    loop {
        let mut played = Played::new();
        played.play_with(&mut session, RUN - 10, FRAME, false);
        // The run ends on a slow disk: a keep's batch is held in flight while the run keeps once more and ends,
        // so the end overtakes that record and goes with the next run's first.
        session.flush(Some(played.so_far()));
        disk.hold(true);
        for _ in 0..2 {
            played.play_with(&mut session, 5, FRAME, false);
            session.keep(Some(played.so_far()));
        }
        let (replay, trail) = played.so_far();
        session.run_ended(replay, trail);
        disk.hold(false);
    }
}

/// xorshift64: the random kills only need to spread out, and to be the same each run.
fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// What the kill left past the session's document.
fn past_the_document(dir: &Path, kept: &Kept) -> Vec<Past> {
    let mut past = Vec::new();
    let text = fs::read_to_string(dir.join(FRAMES_FILE)).unwrap_or_default();
    let rows = text.matches('\n').count().saturating_sub(1);
    let counted = kept.document.frames.unwrap_or(0) as usize;
    if rows > counted {
        past.push(Past::Rows(rows - counted));
    }
    if !text.is_empty() && !text.ends_with('\n') {
        past.push(Past::PartOfARow);
    }
    for entry in fs::read_dir(dir.join(RUNS_DIR)).into_iter().flatten() {
        let entry = entry.unwrap().path();
        let run: u32 = entry.file_name().unwrap().to_str().unwrap().parse().unwrap();
        // A run's directory is made just before its first record.
        let bytes = fs::read_dir(&entry)
            .unwrap()
            .map(|file| file.unwrap().path())
            .find(|file| file.extension().is_some_and(|extension| extension == "journal"))
            .map_or_else(Vec::new, |journal| fs::read(journal).unwrap());
        if bytes.is_empty() {
            continue;
        }
        let (reached, torn) = records(&bytes);
        match (kept.document.runs.iter().find(|named| named.number == run), reached) {
            (None, _) => past.push(Past::Unnamed { run }),
            (Some(named), Some((ticks, ended))) if (ticks, ended) > (named.ticks, named.ended) => {
                past.push(Past::Records { run, ticks, ended });
            }
            _ => {}
        }
        if torn {
            past.push(Past::PartOfARecord { run });
        }
    }
    if dir.join(Path::new(DOCUMENT_FILE).with_added_extension("tmp")).exists() {
        past.push(Past::NextDocument);
    }
    past
}

/// The ticks and end a journal's whole records reach, and whether part of another follows them.
fn records(bytes: &[u8]) -> (Option<(u32, bool)>, bool) {
    let mut at = journal::MAGIC.len() + 4;
    if bytes.len() < at {
        return (None, !bytes.is_empty());
    }
    let mut reached = None;
    // Each record: its payload's length, the payload (ticks, then whether the run had ended), a digest.
    while let Some(length) = bytes.get(at..at + 4) {
        let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
        let Some(payload) = bytes.get(at + 4..at + 4 + length).filter(|_| bytes.len() >= at + 12 + length)
        else {
            break;
        };
        reached = Some((u32::from_le_bytes(payload[..4].try_into().unwrap()), payload[4] == 1));
        at += 12 + length;
    }
    (reached, at < bytes.len())
}

/// Starts the helper aimed at `aim`, kills it `after` it marks its place, and checks the session it left. Gives
/// the runs replayed and what lay past the document.
fn kill(scratch: &Scratch, aim: Option<(Place, usize)>, after: Duration, what: &str) -> (usize, Vec<Past>) {
    let sessions = scratch.sessions();
    let marker = scratch.0.join("marker");
    let mut helper = Command::new(std::env::current_exe().expect("this test's path"));
    helper.arg("--player").arg(&sessions).arg(&marker);
    if let Some((place, time)) = aim {
        helper.arg(format!("{place:?}")).arg(time.to_string());
    }
    let mut helper = helper.spawn().expect("starts the helper");
    for _ in 0..POLLS {
        if marker.exists() {
            break;
        }
        if let Some(status) = helper.try_wait().unwrap() {
            panic!("{what}: the helper ended ({status}) before it got there");
        }
        thread::sleep(Duration::from_millis(2));
    }
    let there = marker.exists();
    thread::sleep(after);
    helper.kill().expect("kills the helper");
    helper.wait().expect("reaps the helper");
    assert!(there, "{what}: the helper never got there");
    fs::remove_file(&marker).unwrap();

    let dirs: Vec<_> = fs::read_dir(&sessions).unwrap().map(|entry| entry.unwrap().path()).collect();
    assert_eq!(dirs.len(), 1, "{what}");
    let dir = &dirs[0];
    let kept = std::panic::catch_unwind(|| every_run_replays(dir))
        .unwrap_or_else(|_| panic!("{what}: the session doesn't read whole as of its last keep"));
    let past = past_the_document(dir, &kept);
    fs::remove_dir_all(&sessions).unwrap();
    (kept.document.runs.len(), past)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--player") => {
            let aim = args.get(4).map(|place| (Place::named(place), args[5].parse().expect("a time")));
            return player(Path::new(&args[2]), PathBuf::from(&args[3]), aim);
        }
        // Libtest's own questions: this has no ignored tests to run or list.
        Some("--ignored" | "--list") => return ExitCode::SUCCESS,
        _ => {}
    }
    let scratch = Scratch::new("keep_kill");

    let (mut aimed, mut runs) = (0, 0);
    for place in Place::ALL {
        for time in TIMES {
            let what = format!("killed at {place:?}, time {time}");
            let (replayed, past) = kill(&scratch, Some((place, time)), Duration::ZERO, &what);
            assert!(
                past.iter().any(|past| place.leaves(past)),
                "{what}: the kill didn't leave what that place does past the document, but {past:?}"
            );
            aimed += 1;
            runs += replayed;
        }
    }
    println!(
        "{aimed} aimed kills at {} places: each left what its place does past its document, and all {runs} runs \
         replayed to the device's checksums",
        Place::ALL.len()
    );

    let mut random = 0x5361_7070_6869_7265;
    let (mut part_way, mut runs) = (0, 0);
    for round in 0..RANDOM {
        let after = Duration::from_millis(next_random(&mut random) % 1_000);
        let what = format!("random kill {round}, {after:?} after the first keep");
        let (replayed, past) = kill(&scratch, None, after, &what);
        part_way += usize::from(!past.is_empty());
        runs += replayed;
    }
    println!(
        "{RANDOM} random kills: all {runs} runs replayed to the device's checksums, {part_way} of them with a keep \
         part way written past their document"
    );
    ExitCode::SUCCESS
}
