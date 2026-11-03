//! The replay file through the public API: round trips, its pinned bytes, what it costs, and a table
//! of crafted files. Installs `CountingAllocator`, so the table can hold reading to allocating only
//! what a file's own length pays for.

use std::alloc::System;
use std::panic::{self, AssertUnwindSafe};
use std::time::Duration;

use sapphire_core::alloc::{self, CountingAllocator};
use sapphire_gameplay::replay::{
    FORMAT_VERSION, MAX_ACTORS, MAX_NAME_LEN, MAX_TICKS, PLAIN_FORMAT_VERSION, Recorder, Recording, Replay,
    ReplayError, SCENARIOS,
    Script, find_script,
};
use sapphire_gameplay::{ActionsError, ActorActions, ActorId, Buttons, SimConfig, Stick, TickActions};
use xxhash_rust::xxh3::xxh3_64;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

fn actor(raw: u32) -> ActorId {
    ActorId::from_raw(raw).expect("not zero")
}

fn press(raw: u32, buttons: Buttons, x: i8) -> TickActions {
    let mut tick = TickActions::NEUTRAL;
    tick.set(actor(raw), ActorActions { buttons, stick: Stick::new(x, 0) }).expect("room");
    tick
}

fn config() -> SimConfig {
    SimConfig {
        seed: 0xfeed,
        max_actors: 32,
        step_budget: Duration::from_micros(1500),
        ..SimConfig::default()
    }
}

/// A replay of these ticks, built the way an app does.
fn replay_of(ticks: &[TickActions]) -> Replay {
    let mut recorder = Recorder::new("some-scenario", config(), 0xabcd).expect("good");
    for tick in ticks {
        recorder.push(tick).expect("short");
    }
    recorder.finish()
}

#[test]
fn a_replay_round_trips_byte_for_byte() {
    let quiet = TickActions::NEUTRAL;
    let light = press(1, Buttons::LIGHT, 0);
    let stick = press(2, Buttons::NONE, -127);
    let mut two = light;
    two.set(actor(9), ActorActions { buttons: Buttons::ALL, stick: Stick::new(127, -127) }).expect("room");
    let cases: Vec<Vec<TickActions>> = vec![
        vec![],
        vec![quiet],
        vec![quiet; 1000],
        vec![light; 300],
        vec![quiet, light, quiet, stick, stick, two, two, two, quiet],
        (0..5000).map(|tick| if tick % 7 == 0 { light } else { quiet }).collect(),
    ];
    for ticks in cases {
        let replay = replay_of(&ticks);
        assert_eq!(replay.ticks() as usize, ticks.len());
        let bytes = replay.encode();
        let decoded = Replay::decode(&bytes).expect("what's written reads");
        assert_eq!(decoded, replay);
        assert_eq!(decoded.encode(), bytes, "one set of inputs has one encoding");
        assert_eq!(decoded.actions().collect::<Vec<_>>(), ticks);
        assert_eq!(decoded.scenario(), "some-scenario");
        assert_eq!(decoded.config(), config());
        assert_eq!(decoded.tables_hash(), 0xabcd);
    }
}

#[test]
fn stamping_a_new_move_hash_keeps_the_actions() {
    let ticks = [press(1, Buttons::LIGHT, 3); 5];
    let mut replay = replay_of(&ticks);
    replay.set_tables_hash(0x1234);
    let decoded = Replay::decode(&replay.encode()).expect("reads");
    assert_eq!(decoded.tables_hash(), 0x1234);
    assert_eq!(decoded.actions().collect::<Vec<_>>(), ticks);
}

/// These bytes are in committed files, so a change here needs a new [`FORMAT_VERSION`], and that
/// version's files recorded again.
#[test]
fn the_format_is_pinned() {
    assert_eq!(PLAIN_FORMAT_VERSION, 1);
    assert_eq!(TickActions::LAYOUT_VERSION, 1);
    let ticks = [TickActions::NEUTRAL, TickActions::NEUTRAL, press(1, Buttons::LIGHT, 2)];
    let mut recorder = Recorder::new("ab", config(), 0x0102_0304_0506_0708).expect("good");
    for tick in &ticks {
        recorder.push(tick).expect("short");
    }
    let bytes = recorder.finish().encode();
    #[rustfmt::skip]
    let mut expected: Vec<u8> = vec![
        b'S', b'R', b'E', b'P', b'L', b'A', b'Y', 0,
        1, 0, 0, 0,
        1, 0, 0, 0,
        0xed, 0xfe, 0, 0, 0, 0, 0, 0,
        32, 0, 0, 0,
        0x60, 0xe3, 0x16, 0, 0, 0, 0, 0,
        8, 7, 6, 5, 4, 3, 2, 1,
        3, 0, 0, 0,
        2, b'a', b'b',
        2, 0,
        1, 1, 1, 0, 0, 0, 1, 0, 2, 0,
    ];
    let digest = xxh3_64(&expected);
    expected.extend_from_slice(&digest.to_le_bytes());
    assert_eq!(bytes, expected);
}

/// What a minute of play costs, for each script's kind of input. `--nocapture` prints them.
#[test]
fn a_minute_of_play_costs_little() {
    fn bytes_per_minute(script: &Script) -> usize {
        let scenario = &SCENARIOS[0];
        let mut recording = Recording::new(scenario, scenario.config).expect("sets up");
        let mut source = script.actions;
        for _ in 0..3600 {
            recording.step(&mut source).expect("short");
        }
        recording.finish().0.encode().len()
    }
    let sweep = bytes_per_minute(find_script("melee-sweep").expect("listed"));
    let play = bytes_per_minute(find_script("melee-play").expect("listed"));
    let idle = bytes_per_minute(find_script("idle").expect("listed"));
    eprintln!("bytes per minute of play: idle {idle}, melee-play {play}, melee-sweep {sweep}");
    assert!(idle < 100, "{idle}: a run of ticks is a few bytes, whatever its length");
    assert!(play < 16 * 1024, "{play}");
    // No tick repeats: three actors at 8 bytes each, a count and a run length. The worst a duel gets.
    assert!(sweep < 26 * 3600 + 100, "{sweep}");
}

/// A file's fields, to change one at a time. Written with a correct digest unless a row says not.
#[derive(Clone)]
struct Crafted {
    version: u32,
    layout: u32,
    max_actors: u32,
    ticks: u32,
    name: Vec<u8>,
    /// What [`Self::fields`] writes after the name.
    body: Vec<u8>,
}

impl Crafted {
    /// Two neutral ticks and a press: decodes cleanly, so a row's edit is what breaks it.
    fn good() -> Self {
        Self {
            version: PLAIN_FORMAT_VERSION,
            layout: TickActions::LAYOUT_VERSION,
            max_actors: 8,
            ticks: 3,
            name: b"melee".to_vec(),
            body: vec![2, 0, 1, 1, 1, 0, 0, 0, 1, 0, 0, 0],
        }
    }

    /// Every field the digest covers, past the versions.
    fn fields(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&5_u64.to_le_bytes());
        out.extend_from_slice(&self.max_actors.to_le_bytes());
        out.extend_from_slice(&1_000_000_u64.to_le_bytes());
        out.extend_from_slice(&0x77_u64.to_le_bytes());
        out.extend_from_slice(&self.ticks.to_le_bytes());
        out.push(self.name.len() as u8);
        out.extend_from_slice(&self.name);
        out.extend_from_slice(&self.body);
        out
    }

    fn undigested(&self) -> Vec<u8> {
        let mut out = b"SREPLAY\0".to_vec();
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.layout.to_le_bytes());
        out.extend_from_slice(&self.fields());
        out
    }

    fn bytes(&self) -> Vec<u8> {
        digested(self.undigested())
    }
}

fn digested(mut bytes: Vec<u8>) -> Vec<u8> {
    let digest = xxh3_64(&bytes);
    bytes.extend_from_slice(&digest.to_le_bytes());
    bytes
}

fn with(edit: impl FnOnce(&mut Crafted)) -> Vec<u8> {
    let mut crafted = Crafted::good();
    edit(&mut crafted);
    crafted.bytes()
}

/// A tick's bytes for one actor: its raw ID, buttons and stick.
fn tick_bytes(raw: u32, buttons: u16, x: u8) -> Vec<u8> {
    let mut out = vec![1];
    out.extend_from_slice(&raw.to_le_bytes());
    out.extend_from_slice(&buttons.to_le_bytes());
    out.extend_from_slice(&[x, 0]);
    out
}

/// The good file with its body swapped, and its tick count to match.
fn with_body(ticks: u32, body: Vec<u8>) -> Vec<u8> {
    with(|crafted| {
        crafted.ticks = ticks;
        crafted.body = body;
    })
}

fn joined(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// Every row is a crafted file that must come back as its error: never a panic, and never an
/// allocation that its own bytes didn't pay for. Rows that fail are collected, so one bad row
/// doesn't hide the rest.
#[test]
fn crafted_files_fail_cleanly() {
    let good = Crafted::good().bytes();
    assert!(Replay::decode(&good).is_ok());
    let flipped = |at: usize| {
        let mut bytes = good.clone();
        bytes[at] ^= 0x40;
        bytes
    };
    let header = Crafted::good().undigested();
    let at_fields = 16;
    let cut_fields = |keep: usize| digested(header[..at_fields + keep].to_vec());
    let press = tick_bytes(7, 1, 0);
    let other = tick_bytes(8, 1, 0);

    type Check = fn(&ReplayError) -> bool;
    let rows: Vec<(&str, Vec<u8>, Check)> = vec![
        ("an empty file", vec![], |e| matches!(e, ReplayError::NotAReplay)),
        ("shorter than the magic", b"SREP".to_vec(), |e| matches!(e, ReplayError::NotAReplay)),
        ("another file's start", b"\x89PNG\r\n\x1a\nmore".to_vec(), |e| matches!(e, ReplayError::NotAReplay)),
        ("only the magic", b"SREPLAY\0".to_vec(), |e| {
            matches!(e, ReplayError::Truncated { what: "the format version", .. })
        }),
        ("a format from before", with(|c| c.version = 0), |e| {
            matches!(e, ReplayError::FormatVersion { found: 0 })
        }),
        (
            "a format from later",
            with(|c| c.version = FORMAT_VERSION + 1),
            |e| matches!(e, ReplayError::FormatVersion { found } if *found == FORMAT_VERSION + 1),
        ),
        ("a format from far later", with(|c| c.version = u32::MAX), |e| {
            matches!(e, ReplayError::FormatVersion { found: u32::MAX })
        }),
        ("only a version", joined(&[b"SREPLAY\0", &PLAIN_FORMAT_VERSION.to_le_bytes()]), |e| {
            matches!(e, ReplayError::Truncated { what: "the actions' layout version", .. })
        }),
        ("another actions layout", with(|c| c.layout = 2), |e| {
            matches!(e, ReplayError::LayoutVersion { found: 2 })
        }),
        ("no room for a digest", header[..at_fields + 3].to_vec(), |e| {
            matches!(e, ReplayError::Truncated { what: "the digest", needed: 8, found: 3 })
        }),
        ("a flipped bit in the seed", flipped(at_fields), |e| matches!(e, ReplayError::Damaged { .. })),
        ("a flipped bit in the digest", flipped(good.len() - 1), |e| {
            matches!(e, ReplayError::Damaged { .. })
        }),
        ("a lost last byte", good[..good.len() - 1].to_vec(), |e| matches!(e, ReplayError::Damaged { .. })),
        ("a byte too many", joined(&[&good, &[0]]), |e| matches!(e, ReplayError::Damaged { .. })),
        ("stops at the seed", cut_fields(0), |e| {
            matches!(e, ReplayError::Truncated { what: "the seed", .. })
        }),
        ("stops in the pool", cut_fields(10), |e| {
            matches!(e, ReplayError::Truncated { what: "the actor pool's size", .. })
        }),
        ("stops at the budget", cut_fields(12), |e| {
            matches!(e, ReplayError::Truncated { what: "the step budget", .. })
        }),
        ("stops at the hash", cut_fields(20), |e| {
            matches!(e, ReplayError::Truncated { what: "the move data's hash", .. })
        }),
        ("stops at the tick count", cut_fields(28), |e| {
            matches!(e, ReplayError::Truncated { what: "the tick count", .. })
        }),
        ("stops at the name's length", cut_fields(32), |e| {
            matches!(e, ReplayError::Truncated { what: "the scenario name's length", .. })
        }),
        ("a name longer than the file", cut_fields(33 + 3), |e| {
            matches!(e, ReplayError::Truncated { what: "the scenario's name", .. })
        }),
        ("an empty name", with(|c| c.name = vec![]), |e| matches!(e, ReplayError::BadName { .. })),
        ("a name too long", with(|c| c.name = vec![b'a'; MAX_NAME_LEN + 1]), |e| {
            matches!(e, ReplayError::BadName { .. })
        }),
        ("the longest name and more", with(|c| c.name = vec![b'a'; 255]), |e| {
            matches!(e, ReplayError::BadName { .. })
        }),
        ("a name with a path in it", with(|c| c.name = b"../melee".to_vec()), |e| {
            matches!(e, ReplayError::BadName { .. })
        }),
        ("a name with a terminal escape", with(|c| c.name = b"\x1b[31m".to_vec()), |e| {
            matches!(e, ReplayError::BadName { .. })
        }),
        ("a name that isn't UTF-8", with(|c| c.name = vec![0xff; 255]), |e| {
            matches!(e, ReplayError::BadName { .. })
        }),
        ("a pool past the limit", with(|c| c.max_actors = MAX_ACTORS + 1), |e| {
            matches!(e, ReplayError::TooManyActors { .. })
        }),
        ("a pool of everything", with(|c| c.max_actors = u32::MAX), |e| {
            matches!(e, ReplayError::TooManyActors { .. })
        }),
        ("more ticks than the limit", with_body(MAX_TICKS + 1, vec![]), |e| {
            matches!(e, ReplayError::TooManyTicks { .. })
        }),
        ("every tick there could be", with_body(u32::MAX, vec![]), |e| {
            matches!(e, ReplayError::TooManyTicks { .. })
        }),
        ("no runs, and ticks promised", with_body(3, vec![]), |e| {
            matches!(e, ReplayError::RunsEndShort { found: 0, ticks: 3 })
        }),
        ("runs that stop short", with_body(3, vec![2, 0]), |e| {
            matches!(e, ReplayError::RunsEndShort { found: 2, ticks: 3 })
        }),
        ("a run past the end", with_body(3, vec![4, 0]), |e| {
            matches!(e, ReplayError::RunPastEnd { tick: 0, ticks: 3 })
        }),
        ("a run as long as there can be", with_body(3, vec![0xff, 0xff, 0xff, 0xff, 0x0f, 0]), |e| {
            matches!(e, ReplayError::RunPastEnd { tick: 0, ticks: 3 })
        }),
        ("a second run past the end", with_body(3, joined(&[&[2, 0, 2], &press])), |e| {
            matches!(e, ReplayError::RunPastEnd { tick: 2, ticks: 3 })
        }),
        ("an empty run", with_body(3, vec![0, 0, 3, 0]), |e| matches!(e, ReplayError::EmptyRun { tick: 0 })),
        ("a run split in two", with_body(3, vec![1, 0, 2, 0]), |e| {
            matches!(e, ReplayError::SplitRun { tick: 1 })
        }),
        ("a press split in two", with_body(3, joined(&[&[1], &press, &[2], &press])), |e| {
            matches!(e, ReplayError::SplitRun { tick: 1 })
        }),
        ("a length with a byte to spare", with_body(3, vec![0x83, 0, 0]), |e| {
            matches!(e, ReplayError::BadRunLength { tick: 0 })
        }),
        ("a length of 33 bits", with_body(3, vec![0xff, 0xff, 0xff, 0xff, 0x10, 0]), |e| {
            matches!(e, ReplayError::BadRunLength { tick: 0 })
        }),
        ("a length that never ends", with_body(3, vec![0xff; 32]), |e| {
            matches!(e, ReplayError::BadRunLength { tick: 0 })
        }),
        ("a length with nothing after it", with_body(3, vec![3]), |e| {
            matches!(e, ReplayError::Actions { tick: 0, source: ActionsError::Truncated { .. } })
        }),
        ("actions that stop short", with_body(3, joined(&[&[3], &press[..4]])), |e| {
            matches!(e, ReplayError::Actions { tick: 0, source: ActionsError::Truncated { .. } })
        }),
        ("more actors than a tick holds", with_body(3, vec![3, 9, 1, 0, 0, 0]), |e| {
            matches!(e, ReplayError::Actions { tick: 0, source: ActionsError::TooManyActors { count: 9 } })
        }),
        ("every actor at once", with_body(3, vec![3, 255]), |e| {
            matches!(e, ReplayError::Actions { tick: 0, source: ActionsError::TooManyActors { count: 255 } })
        }),
        ("actor 0", with_body(3, joined(&[&[3], &tick_bytes(0, 1, 0)])), |e| {
            matches!(e, ReplayError::Actions { tick: 0, source: ActionsError::NoSuchActor { raw: 0 } })
        }),
        (
            "actors out of order",
            with_body(3, vec![3, 2, 8, 0, 0, 0, 1, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0]),
            |e| matches!(e, ReplayError::Actions { tick: 0, source: ActionsError::OutOfOrder { raw: 7 } }),
        ),
        ("an actor listed neutral", with_body(3, joined(&[&[3], &tick_bytes(7, 0, 0)])), |e| {
            matches!(e, ReplayError::Actions { tick: 0, source: ActionsError::NeutralListed { raw: 7 } })
        }),
        ("a button no build has", with_body(3, joined(&[&[3], &tick_bytes(7, 0x100, 0)])), |e| {
            matches!(e, ReplayError::Actions { source: ActionsError::UnknownButtons { .. }, .. })
        }),
        ("a stick below its range", with_body(3, joined(&[&[3], &tick_bytes(7, 1, 0x80)])), |e| {
            matches!(e, ReplayError::Actions { source: ActionsError::StickOutOfRange { raw: 7 }, .. })
        }),
        (
            "a bad tick after good ones",
            with_body(3, joined(&[&[1], &press, &[1], &other, &[1], &[9]])),
            |e| {
                matches!(
                    e,
                    ReplayError::Actions { tick: 2, source: ActionsError::TooManyActors { count: 9 } }
                )
            },
        ),
    ];

    let mut failures = Vec::new();
    for (name, bytes, check) in &rows {
        let decoded = panic::catch_unwind(AssertUnwindSafe(|| {
            alloc::count_in_scope(|| Replay::decode(bytes).map(|_| ()))
        }));
        match decoded {
            Err(_) => failures.push(format!("{name}: panicked")),
            Ok((Ok(()), _)) => failures.push(format!("{name}: decoded")),
            Ok((Err(error), counts)) => {
                if !check(&error) {
                    failures.push(format!("{name}: {error:?}"));
                }
                // Room for a refused name, in the error.
                let paid_for = bytes.len() as u64 + 1024;
                if counts.bytes > paid_for {
                    failures.push(format!(
                        "{name}: allocated {} bytes for a {} byte file",
                        counts.bytes,
                        bytes.len()
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} of {} rows:\n{}", failures.len(), rows.len(), failures.join("\n"));
}

/// A short file with the longest run there can be is a good file. Reading it costs what it weighs.
#[test]
fn the_longest_run_reads_for_what_its_bytes_cost() {
    let bytes = with_body(MAX_TICKS, vec![0x80, 0xde, 0x34, 0]);
    let (replay, counts) = alloc::count_in_scope(|| Replay::decode(&bytes));
    assert_eq!(replay.expect("good").ticks(), MAX_TICKS);
    assert!(counts.bytes < 256, "{} bytes", counts.bytes);
}

#[test]
fn a_new_format_says_to_record_again() {
    let error = Replay::decode(&with(|c| c.version = FORMAT_VERSION + 1)).expect_err("refused");
    let message = error.to_string();
    assert!(message.contains(&format!("version {}", FORMAT_VERSION + 1)), "{message}");
    assert!(message.contains("record it again"), "{message}");
}

#[test]
fn a_recorder_refuses_what_a_reader_would() {
    assert!(matches!(Recorder::new("Not A Name", config(), 0), Err(ReplayError::BadName { .. })));
    let config = SimConfig { max_actors: MAX_ACTORS + 1, ..config() };
    assert!(matches!(Recorder::new("good", config, 0), Err(ReplayError::TooManyActors { .. })));
}
