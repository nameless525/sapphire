//! What the session tests share: a scratch directory, a disk with nothing to fail, and a run played into a
//! session.

#![allow(dead_code, reason = "each test file takes what it needs")]

use std::fs::{self, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use bevy_reflect::TypePath;
use sapphire_core::thermal::{ThermalReading, ThermalState};
use sapphire_gameplay::replay::{ARCHITECTURE, Player, Recording, Replay, SCENARIOS, Trail, find_script};
use sapphire_playtest::{About, Build, Device, Disk, Kept, SESSIONS_DIR, Session, SessionDocument};
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

pub const FRAME: Duration = Duration::from_micros(16_667);

/// A directory of its own, gone with the test.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sapphire-playtest-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn sessions(&self) -> PathBuf {
        self.0.join(SESSIONS_DIR)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The disk as `durable_replace` and `durable_append` write it, without their flushes, which only a power cut
/// needs.
pub struct Plain;

impl Disk for Plain {
    fn replace(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        // Whole or not at all: a temp beside it, then a rename.
        let temp = path.with_added_extension("tmp");
        fs::write(&temp, bytes)?;
        fs::rename(&temp, path)
    }

    fn append(&self, path: &Path, at: u64, bytes: &[u8]) -> io::Result<()> {
        let mut file = OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
        if file.metadata()?.len() < at {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "shorter than its writer thinks"));
        }
        file.set_len(at)?;
        file.seek(SeekFrom::Start(at))?;
        file.write_all(bytes)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::playtest::tests"]
pub struct Settings {
    pub volume: u32,
}

impl Versioned for Settings {
    const VERSION: u32 = 1;
}

pub fn about(began: Option<u64>) -> About {
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

pub fn heat() -> ThermalReading {
    ThermalReading { state: ThermalState::Light, headroom: Some(0.5), ..ThermalReading::default() }
}

/// A run someone's playing, a frame a step, with the session told each frame.
pub struct Played {
    pub recording: Recording,
}

impl Played {
    pub fn new() -> Self {
        let scenario = &SCENARIOS[0];
        Self { recording: Recording::new(scenario, scenario.config).unwrap() }
    }

    /// `frames` frames of `frame` each, keeping as the session asks; [`Session::flush`] instead of
    /// [`Session::keep`] with `wait`, so each keep is on the disk before the next.
    pub fn play_with(&mut self, session: &mut Session, frames: u32, frame: Duration, wait: bool) {
        let mut source = find_script("melee-play").unwrap();
        for _ in 0..frames {
            self.recording.step(&mut source).unwrap();
            session.frame(frame, FRAME, heat(), self.recording.simulation().tick());
            if session.wants_keep() {
                if wait {
                    session.flush(Some(self.so_far()));
                } else {
                    session.keep(Some(self.so_far()));
                }
            }
        }
    }

    pub fn play(&mut self, session: &mut Session, frames: u32) {
        self.play_with(session, frames, FRAME, false);
    }

    pub fn so_far(&self) -> (Replay, Trail) {
        (self.recording.so_far(), self.recording.trail().clone())
    }
}

pub fn read_document(dir: &Path) -> SessionDocument {
    SessionDocument::from_ron(&fs::read_to_string(dir.join(sapphire_playtest::DOCUMENT_FILE)).unwrap())
        .unwrap()
}

/// Reads the session as a host does a pulled one, and plays each run's file to the checksums the device kept
/// beside it, as of the document: every run must. Gives the session.
pub fn every_run_replays(dir: &Path) -> Kept {
    let kept = Kept::read(dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
    for (run, files) in kept.document.runs.iter().zip(&kept.runs) {
        let files = files.as_ref().unwrap_or_else(|error| panic!("run {}: {error}", run.number));
        let name = run.file.rsplit('/').next().unwrap().strip_suffix(".sreplay").unwrap();
        let recorded = files.checksums.get(name).expect("the run's checksums are under its name");
        let mut player = Player::new(files.replay.clone(), &SCENARIOS).unwrap();
        player.run_to_end();
        let played = player.finish();
        assert_eq!(
            recorded.first_difference(&played.trail),
            None,
            "run {} differs from the device's",
            run.number
        );
        assert_eq!((recorded.ticks(), played.trail.ticks()), (run.ticks, run.ticks));
        assert_eq!(format!("{:016x}", played.trail.end()), run.checksum);
    }
    if let Some(rows) = kept.document.frames {
        assert_eq!(kept.frames.len(), rows as usize);
    }
    kept
}
