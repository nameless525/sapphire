//! A session kept as a playtest build keeps one, read back as a host reads a pulled one: its document, its
//! frames, and each run's file replayed here to the checksums the run reached as it was played.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bevy_reflect::TypePath;
use sapphire_core::thermal::{ThermalReading, ThermalState};
use sapphire_gameplay::replay::{
    ARCHITECTURE, ChecksumFile, Player, Recording, Replay, SCENARIOS, Trail, checksums_file_name, find_script,
};
use sapphire_playtest::{
    About, Build, DOCUMENT_FILE, Device, Ending, FRAMES_FILE, FrameRow, MAX_MARKS, MAX_SECONDS, MAX_SESSIONS,
    Replace, SESSIONS_DIR, Session, SessionDocument,
};
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

const FRAME: Duration = Duration::from_micros(16_667);

/// A directory of its own, gone with the test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sapphire-playtest-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn sessions(&self) -> PathBuf {
        self.0.join(SESSIONS_DIR)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::playtest::tests"]
struct Settings {
    volume: u32,
}

impl Versioned for Settings {
    const VERSION: u32 = 1;
}

fn replace() -> Replace {
    Arc::new(|path: &Path, bytes: &[u8]| {
        // Whole or not at all, as `durable_replace` writes: a temp beside it, then a rename.
        let temp = path.with_added_extension("tmp");
        fs::write(&temp, bytes)?;
        fs::rename(&temp, path)
    })
}

fn about(began: Option<u64>) -> About {
    About {
        build: Build { app: "Sapphire".into(), version: "0.1.0".into(), id: "0.1.0+local".into() },
        device: Device {
            os: "MacOs".into(),
            class: "Desktop".into(),
            emulator: false,
            architecture: ARCHITECTURE.into(),
            ram_mib: 24_576,
            cores: 10,
        },
        began,
    }
}

fn heat() -> ThermalReading {
    ThermalReading { state: ThermalState::Light, headroom: Some(0.5), ..ThermalReading::default() }
}

/// A run someone's playing, a frame a step, with the session told each frame.
struct Played {
    recording: Recording,
}

impl Played {
    fn new() -> Self {
        let scenario = &SCENARIOS[0];
        Self { recording: Recording::new(scenario, scenario.config).unwrap() }
    }

    fn play(&mut self, session: &mut Session, frames: u32) {
        let mut source = find_script("melee-play").unwrap();
        for _ in 0..frames {
            self.recording.step(&mut source).unwrap();
            session.frame(FRAME, FRAME, heat(), self.recording.simulation().tick());
            if session.wants_keep() {
                session.keep(Some(self.so_far()));
            }
        }
    }

    fn so_far(&self) -> (Replay, Trail) {
        (self.recording.so_far(), self.recording.trail().clone())
    }
}

fn read_document(dir: &Path) -> SessionDocument {
    SessionDocument::from_ron(&fs::read_to_string(dir.join(DOCUMENT_FILE)).unwrap()).unwrap()
}

/// Plays the run's file as a host does a pulled one, held to the checksums the device kept beside it.
fn replays_to_its_checksums(session: &Path, document: &SessionDocument, number: u32) -> u32 {
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
    let mut session = Session::begin(&scratch.sessions(), about(Some(1_794_344_673)), replace()).unwrap();
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

    let frames = FrameRow::read_all(&fs::read_to_string(dir.join(FRAMES_FILE)).unwrap()).unwrap();
    assert!((8..=9).contains(&frames.len()), "a row a second of 500 frames: {}", frames.len());
    assert!(frames.iter().all(|row| row.run == 1 && row.heat.state == ThermalState::Light && row.late == 0));
    assert!(frames.windows(2).all(|pair| pair[0].tick < pair[1].tick));
}

/// A session that ends with the app keeps every run as it ended, numbered in order, and says how it ended. A
/// run with no ticks, the title's, is kept nowhere.
#[test]
fn a_session_that_quits_keeps_each_run_as_it_ended() {
    let scratch = Scratch::new("quit");
    let mut session = Session::begin(&scratch.sessions(), about(None), replace()).unwrap();
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
    let mut session = Session::begin(&scratch.sessions(), about(Some(2_000_000_000)), replace()).unwrap();
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
