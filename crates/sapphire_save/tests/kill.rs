//! A real kill: a helper process writes saves in a loop, this one kills it at a random moment,
//! a few hundred times, and each load after must be whole and must hold every write the helper
//! said was done.
//!
//! `Child::kill` is `SIGKILL` on Unix and `TerminateProcess` on Windows: nothing runs in the helper
//! after it. It's the process that dies here, not the OS, so the page cache survives every kill. A
//! power cut can't be tested on this machine or any CI runner: that's `durable_replace`'s flushes,
//! whose docs say where each OS's guarantee stops.
//!
//! Its own `main` (`harness = false`), because the test binary is also the helper.

#![allow(clippy::print_stdout, reason = "with no harness, this says how the kills landed")]

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use bevy_reflect::TypePath;
use sapphire_platform::durable_replace;
use sapphire_reflect::Versioned;
use sapphire_save::{OtherSlot, ProgressStore};
use serde::{Deserialize, Serialize};

const ROUNDS: u32 = 300;
/// Enough that writing the temp takes a share of the loop, and a kill can land inside it.
const FILLER_BYTES: usize = 64 * 1024;
/// The longest wait before a kill, past the helper's first finished write.
const MAX_DELAY_US: u64 = 15_000;

#[derive(TypePath, Serialize, Deserialize)]
#[type_path = "sapphire::save_test"]
struct Counted {
    sequence: u64,
    filler: Vec<u8>,
}

impl Versioned for Counted {
    const VERSION: u32 = 1;
}

impl Counted {
    fn new(sequence: u64) -> Self {
        Self { sequence, filler: (0..FILLER_BYTES).map(|at| filler_byte(sequence, at)).collect() }
    }

    fn is_whole(&self) -> bool {
        self.filler.len() == FILLER_BYTES
            && self.filler.iter().enumerate().all(|(at, byte)| *byte == filler_byte(self.sequence, at))
    }
}

fn filler_byte(sequence: u64, at: usize) -> u8 {
    (sequence as u8).wrapping_mul(31).wrapping_add(at as u8)
}

fn store(dir: &Path) -> ProgressStore<Counted> {
    ProgressStore::new(dir, "progress", durable_replace)
}

/// The helper: writes forever, saying each sequence number once its write has returned.
fn writer(dir: &Path) -> ! {
    let store = store(dir);
    let loaded = store.load().expect("the helper's load");
    let mut sequence = loaded.map_or(0, |save| save.value.sequence);
    let mut out = std::io::stdout().lock();
    loop {
        sequence += 1;
        let snapshot = store.snapshot(&Counted::new(sequence)).expect("snapshot");
        store.write(&snapshot).expect("the helper's write");
        writeln!(out, "{sequence}").and_then(|()| out.flush()).expect("say it's done");
    }
}

/// xorshift64: the delays only need to spread out, and to be the same each run.
fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn kill_loop(dir: &Path) {
    let store = store(dir);
    let exe = std::env::current_exe().expect("this test's path");
    let mut random = 0x5361_7070_6869_7265;
    let mut acknowledged = 0;
    let (mut kills_with_temp, mut kills_past_the_ack) = (0, 0);

    for round in 0..ROUNDS {
        let mut helper = Command::new(&exe)
            .arg("--writer")
            .arg(dir)
            .stdout(Stdio::piped())
            .spawn()
            .expect("start the helper");
        let mut out = BufReader::new(helper.stdout.take().expect("piped"));
        let mut said = String::new();
        // Every round's helper finishes a write before the kill, so every round makes progress.
        out.read_line(&mut said).expect("read the helper");
        assert!(said.ends_with('\n'), "round {round}: the helper stopped before a write finished");
        thread::sleep(Duration::from_micros(next_random(&mut random) % MAX_DELAY_US));
        helper.kill().expect("kill the helper");
        let status = helper.wait().expect("wait for the helper");
        // 101 is a Rust panic: the helper's own load or write failed.
        assert_ne!(status.code(), Some(101), "round {round}: the helper panicked");
        out.read_to_string(&mut said).expect("read the helper");

        for line in said.split_inclusive('\n').filter(|line| line.ends_with('\n')) {
            acknowledged = line.trim().parse().expect("a sequence number");
        }

        let leftovers = fs::read_dir(dir)
            .expect("list")
            .filter(|entry| entry.as_ref().expect("entry").file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        kills_with_temp += u32::from(leftovers > 0);

        let save = store.load().expect("a load after a kill").expect("a write had finished");
        assert!(save.value.is_whole(), "round {round}: a torn value");
        assert!(!save.fell_back(), "round {round}: a kill damaged a slot: {:?}", save.other);
        // Nothing the helper said was done is lost, and at most the write it died in is ahead.
        let sequence = save.value.sequence;
        assert!(
            sequence == acknowledged || sequence == acknowledged + 1,
            "round {round}: acknowledged {acknowledged}, loaded {sequence}"
        );
        kills_past_the_ack += u32::from(sequence > acknowledged);
        assert_eq!(save.generation, sequence, "round {round}: one generation for each write");
        if save.generation > 1 {
            assert!(
                matches!(save.other, OtherSlot::Older { generation } if generation + 1 == save.generation),
                "round {round}: {:?}",
                save.other
            );
        }
        acknowledged = sequence;
    }

    println!(
        "{ROUNDS} kills, {acknowledged} writes: {kills_with_temp} left a temp, \
         {kills_past_the_ack} landed after a rename and before its report; every load was whole"
    );
    // If no kill ever landed in a write, this test proved nothing.
    assert!(kills_with_temp > 0, "no kill landed inside a write");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--writer") => writer(Path::new(&args[2])),
        // Libtest's own questions: this has no ignored tests to run or list.
        Some("--ignored" | "--list") => return,
        _ => {}
    }
    let dir = std::env::temp_dir().join(format!("sapphire_save-kill-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the test's directory");
    kill_loop(&dir);
    let _ = fs::remove_dir_all(&dir);
}
