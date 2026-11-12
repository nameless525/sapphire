//! What a keep costs the disk near the four-hour cap (M4 F3): a run played to 3 h 59 m 40 s, kept, then played
//! 10 s more (a keep's interval at 60 frames a second) and kept again. The second keep is what every keep that
//! late costs; beside it, what writing the session's files whole costs then, as M4 F1's keeps did.
//!
//! Ignored, since it steps 863,000 ticks twice: `cargo test --release -p sapphire_playtest --test keep_bytes --
//! --ignored --nocapture`.

#![allow(clippy::print_stdout, reason = "the measurement is the output")]

mod common;

use std::fs;
use std::sync::Arc;

use common::{FRAME, Plain, Scratch, about, heat};
use sapphire_gameplay::replay::{
    ARCHITECTURE, ActionSource, ChecksumFile, RecordedTrail, Recording, SCENARIOS, find_script,
};
use sapphire_gameplay::{ActorActions, Buttons, Stick, TickActions};
use sapphire_playtest::{FRAMES_FILE, KEEP_EVERY, Session};

/// The tick of the measured keep: 3 h 59 m 40 s at 60 a second.
const LATE: u32 = 863_000;

/// Measures a keep at [`LATE`] of a run whose actions `source` gives, printing it under `name`.
fn measure(name: &str, source: &mut dyn ActionSource) -> (u64, u64) {
    let scratch = Scratch::new(&format!("bytes-{name}"));
    let mut session = Session::begin(&scratch.sessions(), about(None), Arc::new(Plain)).unwrap();
    let scenario = &SCENARIOS[0];
    let mut recording = Recording::new(scenario, scenario.config).unwrap();
    let interval = (KEEP_EVERY.as_secs() * 60) as u32;
    for _ in 0..LATE - interval {
        recording.step(source).unwrap();
        session.frame(FRAME, FRAME, heat(), recording.simulation().tick());
    }
    session.flush(Some((recording.so_far(), recording.trail().clone())));
    for _ in 0..interval {
        recording.step(source).unwrap();
        session.frame(FRAME, FRAME, heat(), recording.simulation().tick());
    }
    let before = session.stats().bytes;
    let (replay, trail) = (recording.so_far(), recording.trail().clone());
    session.flush(Some((replay.clone(), trail.clone())));
    let keep = session.stats().bytes - before;

    // The same files written whole, as M4 F1's keep wrote them every time.
    let mut checksums = ChecksumFile::new(ARCHITECTURE);
    checksums.set("melee-1", RecordedTrail::from_trail(&trail));
    let document = sapphire_serialize::to_ron(session.document()).unwrap();
    let frames = fs::metadata(session.dir().join(FRAMES_FILE)).unwrap().len();
    let (replay_bytes, checksum_bytes) = (replay.encode().len() as u64, checksums.to_text().len() as u64);
    let whole = replay_bytes + checksum_bytes + document.len() as u64 + frames;
    println!(
        "{name}: a keep at tick {LATE} writes {keep} bytes; whole, its files are {whole}: the replay {replay_bytes}, \
         its checksums {checksum_bytes}, the document {}, the frames {frames}",
        document.len()
    );
    assert_eq!(session.stats().failed, 0);
    (keep, whole)
}

#[test]
#[ignore = "steps 863,000 ticks twice; run it in release"]
fn a_keep_near_the_cap_writes_only_the_new_ticks() {
    let script = find_script("melee-play").unwrap();
    let mut scripted = |tick: u64, cast: &[sapphire_gameplay::ActorId]| script.actions(tick, cast);
    let (keep, whole) = measure("scripted", &mut scripted);
    assert!(keep * 100 < whole, "{keep} against {whole}");

    // The worst a person can do: the stick somewhere new every tick, and a button now and then.
    let mut restless = |tick: u64, cast: &[sapphire_gameplay::ActorId]| {
        let mut actions = TickActions::NEUTRAL;
        let x = (tick % 255) as i16 - 127;
        let buttons = if tick.is_multiple_of(90) { Buttons::LIGHT } else { Buttons::NONE };
        let stick = Stick::new(x as i8, (127 - x.abs()) as i8);
        actions.set(cast[0], ActorActions { buttons, stick }).unwrap();
        actions
    };
    let (keep, whole) = measure("restless", &mut restless);
    assert!(keep * 100 < whole, "{keep} against {whole}");
}
