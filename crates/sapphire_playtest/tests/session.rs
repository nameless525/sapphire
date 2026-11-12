//! A session kept as a playtest build keeps one, read back as a host reads a pulled one: its document, its
//! frames, and each run's file replayed here to the checksums the run reached as it was played.

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::{FRAME, Plain, Played, Scratch, Settings, about, every_run_replays, read_document};
use sapphire_core::thermal::{ThermalReading, ThermalState};
use sapphire_gameplay::replay::{ARCHITECTURE, ChecksumFile, Player, Replay, SCENARIOS, checksums_file_name};
use sapphire_playtest::{
    DOCUMENT_FILE, Ending, FRAMES_FILE, FrameRow, Kept, MAX_MARKS, MAX_SECONDS, MAX_SESSIONS, Session,
    SessionDocument,
};

fn plain() -> Arc<Plain> {
    Arc::new(Plain)
}

/// Settles the session as a host does a pulled one, then plays the run's file as `check --dir` does, held to
/// the checksums the device kept, written beside it.
fn replays_to_its_checksums(session: &Path, document: &SessionDocument, number: u32) -> u32 {
    Kept::read(session).unwrap().settle(session).unwrap();
    let run = document.run(number).expect("the run's kept");
    let file = session.join(&run.file);
    let replay = Replay::decode(&fs::read(&file).unwrap()).unwrap();
    let text = fs::read_to_string(file.with_file_name(checksums_file_name(ARCHITECTURE))).unwrap();
    let recorded = ChecksumFile::parse(&text).unwrap();
    let name = file.file_stem().unwrap().to_str().unwrap();
    let recorded = recorded.get(name).expect("the run's checksums are under its name").clone();
    let mut player = Player::new(replay, &SCENARIOS).unwrap();
    player.run_to_end();
    let played = player.finish();
    assert_eq!(recorded.first_difference(&played.trail), None, "run {number} differs from the device's");
    assert_eq!((recorded.ticks(), played.trail.ticks()), (run.ticks, run.ticks));
    assert_eq!(format!("{:016x}", played.trail.end()), run.checksum);
    run.ticks
}

/// A process killed after it last kept leaves a whole session as that keep had it: the document says the session
/// didn't end and the run hadn't, and the run's file replays to the checksums the device had then.
#[test]
fn a_killed_session_keeps_what_it_last_kept_and_replays_to_its_checksums() {
    let scratch = Scratch::new("killed");
    let mut session = Session::begin(&scratch.sessions(), about(Some(1_794_344_673)), plain()).unwrap();
    let dir = session.dir().to_owned();
    assert_eq!(dir.file_name().unwrap(), "2026-11-10-210433");
    session.settings(&Settings { volume: 80 });

    let mut played = Played::new();
    played.play(&mut session, 300);
    session.mark(Some("  the dodge\nfelt late  "));
    session.settings(&Settings { volume: 40 });
    played.play(&mut session, 200);
    // Gone to the background, and never back: what a phone's app gets before the OS ends it.
    session.left_foreground();
    session.flush(Some(played.so_far()));
    played.play(&mut session, 100);
    drop(session);

    let document = read_document(&dir);
    assert_eq!(document.ended, None, "nothing said it ended");
    assert_eq!(document.marks.len(), 1);
    let mark = &document.marks[0];
    assert_eq!((mark.run, mark.tick, mark.label.as_deref()), (1, 300, Some("the dodge felt late")));
    assert_eq!(document.settings.len(), 2);
    assert_eq!((document.settings[1].run, document.settings[1].tick), (1, 300));
    let second: Settings =
        sapphire_serialize::from_ron(&fs::read_to_string(dir.join(&document.settings[1].file)).unwrap())
            .unwrap();
    assert_eq!(second, Settings { volume: 40 });
    assert_eq!(document.trips.len(), 1);
    let run = document.run(1).unwrap();
    assert!(!run.ended, "the run was going on when it was last kept");
    assert_eq!(run.ticks, 500, "as far as the flush before the kill");
    assert_eq!(replays_to_its_checksums(&dir, &document, 1), 500);

    let frames = every_run_replays(&dir).frames;
    assert!((8..=9).contains(&frames.len()), "a row a second of 500 frames: {}", frames.len());
    assert!(frames.iter().all(|row| row.run == 1 && row.heat.state == ThermalState::Light && row.late == 0));
    assert!(frames.windows(2).all(|pair| pair[0].tick < pair[1].tick));
}

/// A session that ends with the app keeps every run as it ended, numbered in order, and says how it ended. A
/// run with no ticks, the title's, is kept nowhere.
#[test]
fn a_session_that_quits_keeps_each_run_as_it_ended() {
    let scratch = Scratch::new("quit");
    let mut session = Session::begin(&scratch.sessions(), about(None), plain()).unwrap();
    let dir = session.dir().to_owned();
    assert_eq!(dir.file_name().unwrap(), "session-1", "no calendar, so a number");

    let title = Played::new();
    let (replay, trail) = title.so_far();
    session.run_ended(replay, trail);
    assert_eq!(session.run(), 1, "the title's run took no number");

    for frames in [400, 250] {
        let mut played = Played::new();
        played.play(&mut session, frames);
        let (replay, trail) = played.so_far();
        session.run_ended(replay, trail);
    }
    let stats = session.end(None);
    assert_eq!(stats.failed, 0);

    let document = read_document(&dir);
    assert!(matches!(document.ended, Some(Ending::Quit { .. })));
    assert_eq!(document.runs.len(), 2);
    assert!(document.runs.iter().all(|run| run.ended));
    assert_eq!(document.runs[1].file, "runs/2/melee-2.sreplay");
    assert!(document.runs[0].began < document.runs[1].began);
    assert_eq!(replays_to_its_checksums(&dir, &document, 1), 400);
    assert_eq!(replays_to_its_checksums(&dir, &document, 2), 250);
    assert!((document.seconds - 650.0 * FRAME.as_secs_f64()).abs() < 0.01);
}

/// What a session keeps is bounded: past its limits it counts what it leaves out, and past
/// [`MAX_SECONDS`] it ends and keeps nothing more. A device keeps at most [`MAX_SESSIONS`].
#[test]
fn a_session_and_the_sessions_are_bounded() {
    let scratch = Scratch::new("bounded");
    for second in 0..MAX_SESSIONS as u64 + 3 {
        fs::create_dir_all(scratch.sessions().join(sapphire_playtest::session_name(1_000_000 + second)))
            .unwrap();
    }
    let mut session = Session::begin(&scratch.sessions(), about(Some(2_000_000_000)), plain()).unwrap();
    let left: Vec<_> =
        fs::read_dir(scratch.sessions()).unwrap().map(|entry| entry.unwrap().file_name()).collect();
    assert_eq!(left.len(), MAX_SESSIONS);
    assert!(!scratch.sessions().join(sapphire_playtest::session_name(1_000_000)).exists(), "the oldest went");

    for _ in 0..MAX_MARKS + 5 {
        session.mark(None);
    }
    for _ in 0..u64::from(MAX_SECONDS) + 30 {
        session.frame(Duration::from_secs(1), FRAME, ThermalReading::default(), 0);
    }
    session.mark(Some("too late"));
    let dir = session.dir().to_owned();
    session.end(None);

    let document = read_document(&dir);
    assert_eq!(document.marks.len(), MAX_MARKS);
    assert_eq!(document.dropped.marks, 5, "the mark after the end isn't one the limit left out");
    assert_eq!(document.ended, Some(Ending::Full { at: f64::from(MAX_SECONDS) }));
    let frames = FrameRow::read_all(&fs::read_to_string(dir.join(FRAMES_FILE)).unwrap()).unwrap();
    assert_eq!(frames.len(), MAX_SECONDS as usize);
}

/// A session kept by M4 F1's build (version 1), whose runs' files and frames were each written whole, reads
/// as it is: its document upgrades with no journals and no row count, and its runs replay from their files.
#[test]
fn a_version_1_session_reads() {
    let scratch = Scratch::new("version-1");
    let mut session = Session::begin(&scratch.sessions(), about(Some(1_794_344_673)), plain()).unwrap();
    let dir = session.dir().to_owned();
    let mut played = Played::new();
    played.play(&mut session, 300);
    let (replay, trail) = played.so_far();
    session.run_ended(replay.clone(), trail.clone());
    session.end(None);

    // What F1's build left: the run's file and checksums beside each other, and the document of version 1.
    let mut document = read_document(&dir);
    let journal = dir.join(document.runs[0].journal.take().unwrap());
    fs::remove_file(journal).unwrap();
    document.frames = None;
    fs::write(dir.join(&document.runs[0].file), replay.encode()).unwrap();
    let mut checksums = ChecksumFile::new(ARCHITECTURE);
    checksums.set("melee-1", sapphire_gameplay::replay::RecordedTrail::from_trail(&trail));
    fs::write(dir.join("runs/1").join(checksums_file_name(ARCHITECTURE)), checksums.to_text()).unwrap();
    let text = sapphire_serialize::to_ron(&document).unwrap();
    assert!(text.contains("version: 2,"));
    let old: String = text
        .replace("version: 2,", "version: 1,")
        .lines()
        .filter(|line| {
            !line.trim_start().starts_with("journal:") && !line.trim_start().starts_with("frames:")
        })
        .map(|line| format!("{line}\n"))
        .collect();
    fs::write(dir.join(DOCUMENT_FILE), &old).unwrap();

    let upgraded = SessionDocument::from_ron(&old).unwrap();
    assert_eq!(upgraded, document);
    let kept = every_run_replays(&dir);
    assert_eq!(kept.document, document);
    assert_eq!(kept.runs[0].as_ref().unwrap().replay, replay);
    let rows = FrameRow::read_all(&fs::read_to_string(dir.join(FRAMES_FILE)).unwrap()).unwrap();
    assert_eq!(kept.frames, rows, "every row, with no count to cut them to");
    assert_eq!(replays_to_its_checksums(&dir, &kept.document, 1), 300);
}
