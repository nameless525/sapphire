use std::fs;

use bevy_reflect::TypePath;
use sapphire_platform::durable_replace;
use serde::Deserialize;

use super::*;
use crate::slot;
use crate::test_support::{Fault, Step, TestDir, died, faulty_replace};

/// A small example of what a zone start reads. The real one is the game's.
#[derive(Debug, Clone, PartialEq, TypePath, Serialize, Deserialize)]
#[type_path = "sapphire::save_test"]
struct ZoneStart {
    checkpoint: String,
    cleared: Vec<u32>,
}

impl Versioned for ZoneStart {
    const VERSION: u32 = 1;
}

/// The same save as a later build writes it.
#[derive(TypePath, Serialize, Deserialize)]
#[type_path = "sapphire::save_test"]
#[type_name = "ZoneStart"]
struct NextZoneStart {
    checkpoint: String,
}

impl Versioned for NextZoneStart {
    const VERSION: u32 = 2;
}

#[derive(TypePath, Serialize, Deserialize)]
#[type_path = "sapphire::save_test"]
struct SomethingElse {
    number: u32,
}

impl Versioned for SomethingElse {
    const VERSION: u32 = 1;
}

fn zone(checkpoint: u32) -> ZoneStart {
    ZoneStart { checkpoint: format!("checkpoint {checkpoint}"), cleared: vec![checkpoint, 7] }
}

fn store(dir: &TestDir) -> ProgressStore<ZoneStart> {
    ProgressStore::new(dir.path(), "progress", durable_replace)
}

/// Writes checkpoints 1 to `count`.
fn write_saves(store: &ProgressStore<ZoneStart>, count: u32) {
    for number in 1..=count {
        store.write(&store.snapshot(&zone(number)).expect("snapshot")).expect("write");
    }
}

fn load(store: &ProgressStore<ZoneStart>) -> LoadedSave<ZoneStart> {
    store.load().expect("load").expect("a save")
}

#[test]
fn nothing_written_is_a_new_game() {
    let dir = TestDir::new("empty");
    assert!(store(&dir).load().expect("load").is_none());
    assert!(dir.listing().is_empty(), "a load writes nothing");
}

#[test]
fn writes_take_turns_and_the_newest_loads() {
    let dir = TestDir::new("rotates");
    let store = store(&dir);
    for (number, slot) in [(1, Slot::A), (2, Slot::B), (3, Slot::A), (4, Slot::B), (5, Slot::A)] {
        let written = store.write(&store.snapshot(&zone(number)).expect("snapshot")).expect("write");
        assert_eq!((written.slot, written.generation), (slot, u64::from(number)));
        assert_eq!(written.bytes as u64, fs::metadata(store.slot_path(slot)).expect("stat").len());
        let save = load(&store);
        assert_eq!((save.value.clone(), save.slot, save.generation), (zone(number), slot, u64::from(number)));
        assert_eq!(save.upgraded_from, None);
        assert!(!save.fell_back());
        if number == 1 {
            assert!(matches!(save.other, OtherSlot::Missing));
        } else {
            assert!(
                matches!(save.other, OtherSlot::Older { generation } if generation + 1 == u64::from(number))
            );
        }
    }
    assert!(!dir.listing().iter().any(|name| name.contains("tmp") || name.contains("bad")));
}

#[test]
fn newer_means_the_counter_not_the_clock() {
    let dir = TestDir::new("clock");
    let store = store(&dir);
    write_saves(&store, 2);
    // Slot A is generation 1. Rewriting its bytes makes it the newest by any clock.
    let bytes = fs::read(store.slot_path(Slot::A)).expect("read");
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(store.slot_path(Slot::A), bytes).expect("rewrite");
    let save = load(&store);
    assert_eq!((save.value, save.generation), (zone(2), 2));
}

#[test]
fn a_tie_takes_slot_a_and_the_next_write_breaks_it() {
    let dir = TestDir::new("tie");
    let store = store(&dir);
    write_saves(&store, 1);
    fs::copy(store.slot_path(Slot::A), store.slot_path(Slot::B)).expect("copy");
    assert_eq!(load(&store).slot, Slot::A);
    let written = store.write(&store.snapshot(&zone(2)).expect("snapshot")).expect("write");
    assert_eq!(written.generation, 2);
    assert_eq!(load(&store).value, zone(2));
}

/// The heart of the crate: at every step of a write, with every number of saves before it, a
/// failure or a death leaves the old save or the new one, whole, and the next write works.
#[test]
fn a_write_stopped_at_any_step_leaves_a_whole_save() {
    for saves_before in 0..=3 {
        for step in Step::ALL {
            for fault in [Fault::Fail, Fault::Die] {
                let context = format!("{saves_before} saves, {step:?}, {fault:?}");
                let dir = TestDir::new("steps");
                let store = store(&dir);
                write_saves(&store, saves_before);
                let new = zone(100);

                let faulty =
                    ProgressStore::<ZoneStart>::new(dir.path(), "progress", faulty_replace(step, fault));
                let snapshot = faulty.snapshot(&new).expect("snapshot");
                let mut result = None;
                let dead = died(|| result = Some(faulty.write(&snapshot)));
                assert_eq!(dead, fault == Fault::Die, "{context}");
                if let Some(result) = result {
                    assert!(matches!(result, Err(WriteError::Io { .. })), "{context}: {result:?}");
                }

                let temps = dir.listing().iter().filter(|name| name.ends_with(".tmp")).count();
                let left_temp = fault == Fault::Die && !step.after_rename();
                assert_eq!(temps, usize::from(left_temp), "{context}");

                let loaded = store.load().expect(&context);
                if step.after_rename() {
                    let save = loaded.expect(&context);
                    assert_eq!(save.value, new, "{context}");
                    assert_eq!(save.generation, u64::from(saves_before) + 1, "{context}");
                } else if saves_before == 0 {
                    assert!(loaded.is_none(), "{context}");
                } else {
                    let save = loaded.expect(&context);
                    assert_eq!(save.value, zone(saves_before), "{context}");
                    assert_eq!(save.generation, u64::from(saves_before), "{context}");
                }

                // Nothing's stuck: the stale temp is overwritten, and the save after it wins.
                let after = zone(200);
                store.write(&store.snapshot(&after).expect("snapshot")).expect(&context);
                let save = load(&store);
                assert_eq!(save.value, after, "{context}");
                assert!(!save.fell_back(), "{context}");
                assert!(!dir.listing().iter().any(|name| name.ends_with(".tmp")), "{context}");
            }
        }
    }
}

#[test]
fn a_flipped_byte_anywhere_in_the_newest_slot_falls_back() {
    let dir = TestDir::new("flips");
    let store = store(&dir);
    write_saves(&store, 2);
    let path = store.slot_path(Slot::B);
    let good = fs::read(&path).expect("read");
    for at in 0..good.len() {
        for flip in [0x01, 0x80, 0xff] {
            let mut bad = good.clone();
            bad[at] ^= flip;
            fs::write(&path, &bad).expect("damage");
            let save = load(&store);
            assert!(save.fell_back(), "byte {at}");
            assert_eq!(save.value, zone(1), "byte {at}");
            assert_eq!((save.slot, save.generation), (Slot::A, 1), "byte {at}");
        }
    }
}

#[test]
fn a_flipped_byte_in_the_older_slot_costs_nothing_but_is_said() {
    let dir = TestDir::new("flips_older");
    let store = store(&dir);
    write_saves(&store, 2);
    let path = store.slot_path(Slot::A);
    let good = fs::read(&path).expect("read");
    for at in 0..good.len() {
        let mut bad = good.clone();
        bad[at] ^= 0x10;
        fs::write(&path, &bad).expect("damage");
        let save = load(&store);
        assert!(save.fell_back(), "byte {at}");
        assert_eq!(save.value, zone(2), "byte {at}");
    }
}

#[test]
fn a_flipped_byte_with_no_other_slot_is_an_error_never_a_wrong_save() {
    let dir = TestDir::new("flips_alone");
    let store = store(&dir);
    write_saves(&store, 1);
    let path = store.slot_path(Slot::A);
    let good = fs::read(&path).expect("read");
    for at in 0..good.len() {
        let mut bad = good.clone();
        bad[at] ^= 0x04;
        fs::write(&path, &bad).expect("damage");
        let result = store.load();
        assert!(
            matches!(result, Err(LoadError::AllSlotsDamaged { a: Some(_), b: None })),
            "byte {at}: {result:?}"
        );
    }
}

#[test]
fn a_slot_cut_at_any_length_falls_back() {
    let dir = TestDir::new("cuts");
    let store = store(&dir);
    write_saves(&store, 2);
    let path = store.slot_path(Slot::B);
    let good = fs::read(&path).expect("read");
    for len in 0..good.len() {
        fs::write(&path, &good[..len]).expect("cut");
        let save = load(&store);
        assert!(save.fell_back(), "{len} bytes");
        assert_eq!(save.value, zone(1), "{len} bytes");
    }
}

#[test]
fn an_empty_slot_and_extra_bytes_are_damage() {
    let dir = TestDir::new("empty_slot");
    let store = store(&dir);
    write_saves(&store, 2);
    let path = store.slot_path(Slot::B);
    let good = fs::read(&path).expect("read");
    fs::write(&path, b"").expect("empty");
    assert!(matches!(load(&store).other, OtherSlot::Damaged(SlotFault::Truncated { len: 0 })));
    let mut longer = good;
    longer.push(0);
    fs::write(&path, &longer).expect("lengthen");
    assert!(matches!(load(&store).other, OtherSlot::Damaged(SlotFault::WrongLength { .. })));
}

#[test]
fn both_slots_bad_is_an_error_and_a_write_starts_again() {
    let dir = TestDir::new("both");
    let store = store(&dir);
    write_saves(&store, 2);
    fs::write(store.slot_path(Slot::A), b"ruined a").expect("damage");
    fs::write(store.slot_path(Slot::B), b"ruined b").expect("damage");
    let result = store.load();
    assert!(matches!(result, Err(LoadError::AllSlotsDamaged { a: Some(_), b: Some(_) })), "{result:?}");

    // The player starts again: each damaged file is kept as it's replaced.
    write_saves(&store, 2);
    let save = load(&store);
    assert_eq!((save.value, save.generation), (zone(2), 2));
    assert_eq!(fs::read(dir.path().join("progress.a.sav.bad.1")).expect("kept"), b"ruined a");
    assert_eq!(fs::read(dir.path().join("progress.b.sav.bad.1")).expect("kept"), b"ruined b");
}

#[test]
fn the_write_after_a_fall_back_replaces_the_damaged_slot_and_keeps_the_good_one() {
    let dir = TestDir::new("heals");
    let store = store(&dir);
    write_saves(&store, 3);
    // Slot A is the newest, generation 3.
    let path = store.slot_path(Slot::A);
    let mut bad = fs::read(&path).expect("read");
    bad[50] ^= 1;
    fs::write(&path, &bad).expect("damage");
    assert_eq!(load(&store).value, zone(2));

    let written = store.write(&store.snapshot(&zone(9)).expect("snapshot")).expect("write");
    assert_eq!((written.slot, written.generation), (Slot::A, 3));
    let save = load(&store);
    assert_eq!(save.value, zone(9));
    assert!(matches!(save.other, OtherSlot::Older { generation: 2 }));
    assert_eq!(fs::read(dir.path().join("progress.a.sav.bad.1")).expect("kept"), bad);
}

#[test]
fn a_stale_temp_is_never_read() {
    let dir = TestDir::new("temp");
    let store = store(&dir);
    write_saves(&store, 1);
    let good = fs::read(store.slot_path(Slot::A)).expect("read");
    // A perfectly good file, under the temp's name.
    fs::write(dir.path().join("progress.b.sav.tmp"), &good).expect("write");
    let save = load(&store);
    assert!(matches!(save.other, OtherSlot::Missing), "{:?}", save.other);
}

/// A newer build's file: `slot` holds it, the other slot holds a save of this build's.
fn with_newer_slot(dir: &TestDir, slot: Slot) -> (ProgressStore<ZoneStart>, Vec<u8>) {
    let store = store(dir);
    write_saves(&store, 2);
    let next = ProgressStore::<NextZoneStart>::new(dir.path(), "progress", durable_replace);
    let snapshot = next.snapshot(&NextZoneStart { checkpoint: "later".to_owned() }).expect("snapshot");
    fs::write(store.slot_path(slot), slot::encode(9, NextZoneStart::VERSION, snapshot.as_bytes()))
        .expect("write");
    let bytes = fs::read(store.slot_path(slot)).expect("read");
    (store, bytes)
}

#[test]
fn a_newer_schema_is_refused_and_never_overwritten() {
    for slot in Slot::BOTH {
        let dir = TestDir::new("newer");
        let (store, bytes) = with_newer_slot(&dir, slot);
        let listing = dir.listing();

        let error = store.load().expect_err("a load refuses");
        let LoadError::Newer(newer) = &error else { panic!("{error}") };
        assert_eq!((newer.slot, newer.schema, newer.current), (slot, 2, 1));
        assert!(error.to_string().contains("newer build"), "{error}");

        for _ in 0..3 {
            let snapshot = store.snapshot(&zone(5)).expect("snapshot");
            assert!(matches!(store.write(&snapshot), Err(WriteError::Newer(_))));
        }
        assert_eq!(fs::read(store.slot_path(slot)).expect("read"), bytes);
        assert_eq!(dir.listing(), listing, "nothing kept aside, nothing added");
    }
}

#[test]
fn a_newer_format_is_refused_even_where_this_build_cant_read_the_rest() {
    let dir = TestDir::new("format");
    let store = store(&dir);
    write_saves(&store, 2);
    let path = store.slot_path(Slot::B);
    fs::write(&path, slot::encode_as_format(FORMAT + 1, 9, 1, b"who knows")).expect("write");
    let bytes = fs::read(&path).expect("read");
    assert!(matches!(store.load(), Err(LoadError::Newer(NewerSave { format: 2, .. }))));
    let snapshot = store.snapshot(&zone(5)).expect("snapshot");
    assert!(matches!(store.write(&snapshot), Err(WriteError::Newer(_))));
    assert_eq!(fs::read(&path).expect("read"), bytes);
}

#[test]
fn a_whole_file_that_wont_decode_falls_back_and_rotates_out() {
    let dir = TestDir::new("undecodable");
    let store = store(&dir);
    write_saves(&store, 2);
    // The checksum holds, so this is a bug in some build, not damage.
    fs::write(store.slot_path(Slot::B), slot::encode(3, 1, b"not postcard")).expect("write");
    let save = load(&store);
    assert_eq!(save.value, zone(1));
    assert!(matches!(save.other, OtherSlot::Damaged(SlotFault::Undecodable(_))), "{:?}", save.other);

    // The write goes over the lower generation, which here is the one that loads: the new save
    // takes over at once, and the next write clears the bad file.
    write_saves(&store, 1);
    let save = load(&store);
    assert_eq!((save.slot, save.generation), (Slot::A, 4));
    write_saves(&store, 1);
    assert!(!load(&store).fell_back());
}

#[test]
fn a_save_of_another_type_isnt_read_as_this_one() {
    let dir = TestDir::new("other_type");
    let store = store(&dir);
    let other = ProgressStore::<SomethingElse>::new(dir.path(), "progress", durable_replace);
    other.write(&other.snapshot(&SomethingElse { number: 3 }).expect("snapshot")).expect("write");
    let result = store.load();
    assert!(
        matches!(result, Err(LoadError::AllSlotsDamaged { a: Some(SlotFault::Undecodable(_)), b: None })),
        "{result:?}"
    );
}

#[test]
fn a_version_older_than_the_chain_is_damage_not_a_refusal() {
    let dir = TestDir::new("too_old");
    let store = store(&dir);
    write_saves(&store, 2);
    // Schema 0 in the body's own header too, which is where decoding reads it.
    let mut body = store.snapshot(&zone(3)).expect("snapshot").as_bytes().to_vec();
    body[16..20].copy_from_slice(&0u32.to_le_bytes());
    fs::write(store.slot_path(Slot::B), slot::encode(3, 0, &body)).expect("write");
    let save = load(&store);
    assert_eq!(save.value, zone(1));
    let OtherSlot::Damaged(SlotFault::Undecodable(error)) = &save.other else { panic!("{:?}", save.other) };
    assert!(matches!(error, SaveError::TooOld { found: 0, .. }), "{error}");
}

#[test]
fn writers_in_turn_never_tear_or_repeat_a_generation() {
    let dir = TestDir::new("writers");
    let store = store(&dir);
    let threads: Vec<_> = (0..4)
        .map(|thread| {
            let store = store.clone();
            std::thread::spawn(move || {
                let snapshot = store.snapshot(&zone(thread)).expect("snapshot");
                (0..8).map(|_| store.write(&snapshot).expect("write").generation).collect::<Vec<_>>()
            })
        })
        .collect();
    let mut generations: Vec<u64> =
        threads.into_iter().flat_map(|thread| thread.join().expect("a writer")).collect();
    generations.sort_unstable();
    assert_eq!(generations, (1..=32).collect::<Vec<u64>>());
    let save = load(&store);
    assert_eq!(save.generation, 32);
    assert!(!save.fell_back());
}

#[test]
fn a_job_writes_a_snapshot_while_the_game_changes_its_state() {
    let dir = TestDir::new("job");
    let store = store(&dir);
    let mut state = zone(1);
    let snapshot = store.snapshot(&state).expect("snapshot");
    let (report, written) = std::sync::mpsc::channel();
    let job_store = store.clone();
    sapphire_jobs::spawn_async(move || {
        let _ = report.send(job_store.write(&snapshot));
    });
    // The snapshot holds plain bytes, so what the game does next isn't in the save.
    state.checkpoint = "further on".to_owned();
    written.recv().expect("the job reports").expect("write");
    assert_eq!(load(&store).value, zone(1));
    assert_ne!(state, zone(1));
}

#[test]
#[should_panic(expected = "plain file stem")]
fn a_name_cant_climb_out_of_the_directory() {
    let _ = ProgressStore::<ZoneStart>::new("user", "../progress", durable_replace);
}

#[test]
fn the_largest_generation_is_readable_but_refuses_another_write() {
    let dir = TestDir::new("generation_exhausted");
    let store = store(&dir);
    let old = store.snapshot(&zone(1)).expect("snapshot");
    fs::write(store.slot_path(Slot::A), slot::encode(u64::MAX - 1, ZoneStart::VERSION, &old.bytes))
        .expect("seed");
    let new = store.snapshot(&zone(2)).expect("snapshot");
    let written = store.write(&new).expect("the last generation fits");
    assert_eq!((written.slot, written.generation), (Slot::B, u64::MAX));
    assert_eq!((load(&store).value, load(&store).generation), (zone(2), u64::MAX));
    let before = Slot::BOTH.map(|slot| fs::read(store.slot_path(slot)).expect("read"));
    assert!(matches!(store.write(&old), Err(WriteError::GenerationExhausted)));
    assert_eq!(before, Slot::BOTH.map(|slot| fs::read(store.slot_path(slot)).expect("unchanged")));
}
