//! Runs saves and settings on a real filesystem, with no window and no SDL running: a write, a load,
//! a corrupted slot's fall-back, rotation, then what a write costs. A plain program, so that the iOS
//! Simulator (`xcrun simctl spawn`) and an Android emulator (`adb shell`) can run it from a command
//! line. `SDL_GetPrefPath`'s real paths wait for an app to run this crate.
//!
//! `save_smoke <writable directory>`. It works in `<directory>/save_smoke`, removed at the end, and
//! any failure is a panic and a non-zero exit.

#![allow(clippy::print_stdout, reason = "its output is what it's for")]

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use bevy_reflect::TypePath;
use sapphire_platform::durable_replace;
use sapphire_reflect::Versioned;
use sapphire_save::{OtherSlot, ProgressStore, SettingsFile, SettingsOutcome, Slot};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, TypePath, Serialize, Deserialize)]
#[type_path = "sapphire::save_smoke"]
struct Progress {
    number: u32,
    filler: Vec<u8>,
}

impl Versioned for Progress {
    const VERSION: u32 = 1;
}

#[derive(Debug, Default, PartialEq, TypePath, Serialize, Deserialize)]
#[type_path = "sapphire::save_smoke"]
#[serde(default)]
struct Settings {
    volume: f32,
    subtitles: bool,
}

impl Versioned for Settings {
    const VERSION: u32 = 1;
}

fn progress(number: u32, filler: usize) -> Progress {
    Progress { number, filler: vec![number as u8; filler] }
}

fn progress_round(dir: &Path) {
    let store = ProgressStore::<Progress>::new(dir, "progress", durable_replace);
    assert!(store.load().expect("load an empty directory").is_none());

    for (number, slot) in [(1, Slot::A), (2, Slot::B), (3, Slot::A)] {
        let value = progress(number, 256);
        let written = store.write(&store.snapshot(&value).expect("snapshot")).expect("write");
        assert_eq!((written.slot, written.generation), (slot, u64::from(number)));
        let save = store.load().expect("load").expect("a save");
        assert_eq!((save.value, save.slot), (value, slot));
    }
    println!("write, load and rotation: ok");

    // Damage the newest slot: slot A, generation 3.
    let path = store.slot_path(Slot::A);
    let mut bytes = fs::read(&path).expect("read the slot");
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x40;
    fs::write(&path, &bytes).expect("damage the slot");
    let save = store.load().expect("load").expect("a save");
    assert_eq!((save.value.number, save.slot), (2, Slot::B));
    assert!(save.fell_back());
    println!("corrupted slot's fall-back: ok ({:?})", save.other);

    // The next write takes the damaged slot's place, and keeps it aside.
    let written = store.write(&store.snapshot(&progress(4, 256)).expect("snapshot")).expect("write");
    assert_eq!((written.slot, written.generation), (Slot::A, 3));
    assert_eq!(fs::read(dir.join("progress.a.sav.bad.1")).expect("kept aside"), bytes);
    let save = store.load().expect("load").expect("a save");
    assert_eq!(save.value.number, 4);
    assert!(matches!(save.other, OtherSlot::Older { generation: 2 }));
    println!("the write after it: ok");
}

fn settings_round(dir: &Path) {
    let file = SettingsFile::<Settings>::new(dir, "settings", durable_replace);
    assert!(matches!(file.load().outcome, SettingsOutcome::Missing));
    let value = Settings { volume: 0.5, subtitles: true };
    file.write(&file.snapshot(&value).expect("snapshot")).expect("write");
    let loaded = file.load();
    assert!(matches!(loaded.outcome, SettingsOutcome::Read));
    assert_eq!(loaded.value, value);

    let text = fs::read_to_string(file.path()).expect("read");
    fs::write(file.path(), text.replace("0.5", "\"loud\"")).expect("damage the file");
    let loaded = file.load();
    assert!(matches!(loaded.outcome, SettingsOutcome::Repaired { .. }), "{:?}", loaded.outcome);
    assert_eq!(loaded.value, Settings { volume: 0.0, subtitles: true });
    assert!(matches!(file.load().outcome, SettingsOutcome::Read));
    println!("settings' write, load and repair: ok");
}

fn cost_round(dir: &Path) {
    const WRITES: usize = 40;
    for filler in [1024, 64 * 1024] {
        let store = ProgressStore::<Progress>::new(dir, "cost", durable_replace);
        let (mut snapshots, mut writes) = (Vec::new(), Vec::new());
        for number in 0..WRITES {
            let value = progress(number as u32, filler);
            let start = now();
            let snapshot = store.snapshot(&value).expect("snapshot");
            snapshots.push(start.elapsed());
            let start = now();
            store.write(&snapshot).expect("write");
            writes.push(start.elapsed());
        }
        let (snapshot_median, write_median) = (median(&mut snapshots), median(&mut writes));
        println!(
            "{filler} bytes, {WRITES} writes: snapshot {snapshot_median:?} median; write {:?} min, \
             {write_median:?} median, {:?} max",
            writes[0],
            writes[WRITES - 1],
        );
    }
}

/// A bench's wall time, with no simulation reading it.
#[allow(clippy::disallowed_methods)]
fn now() -> Instant {
    Instant::now()
}

/// Sorts `times`, and gives the middle.
fn median(times: &mut [Duration]) -> Duration {
    times.sort_unstable();
    times[times.len() / 2]
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: save_smoke <writable directory>");
    let dir = Path::new(&dir).join("save_smoke");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the working directory");
    println!("working in {}", dir.display());
    // `durable_replace` takes a filesystem's refusal to flush a directory as done, so say it here.
    let flushed = fs::File::open(&dir).and_then(|handle| handle.sync_all());
    println!("flushing the directory directly: {flushed:?}");
    progress_round(&dir);
    settings_round(&dir);
    cost_round(&dir);
    fs::remove_dir_all(&dir).expect("clean up");
    println!("save_smoke ok");
}
