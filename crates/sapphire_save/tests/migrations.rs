//! Old saves still load: fixture files written by earlier versions of a small example type, each
//! run through the chain of migrations to today's.
//!
//! The fixtures are what a player's disk holds, so they don't change. A later format change that
//! can't read them fails here, and the hashes below stop anyone regenerating their way past that:
//! add a reader and a new fixture, keeping these.

use std::fs;
use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use sapphire_platform::durable_replace;
use sapphire_reflect::{Decoder, VersionError, Versioned, decode_versioned};
use sapphire_save::{OtherSlot, ProgressStore, Slot};
use serde::{Deserialize, Serialize};

/// Today's: what a zone start might read. The real one is the game's.
#[derive(Debug, PartialEq, TypePath, Serialize, Deserialize)]
#[type_path = "sapphire::save_test"]
struct ZoneStart {
    checkpoint: String,
    cleared: Vec<u32>,
    flags: Vec<(String, bool)>,
    party: Vec<String>,
}

impl Versioned for ZoneStart {
    const VERSION: u32 = 3;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        decode_versioned::<v2::ZoneStart, D>(version, data).map(|old| ZoneStart {
            checkpoint: old.checkpoint,
            cleared: old.cleared,
            flags: old.flags,
            party: vec!["lead".to_owned()],
        })
    }
}

mod v2 {
    use super::*;

    #[derive(TypePath, Serialize, Deserialize)]
    #[type_path = "sapphire::save_test"]
    #[type_name = "ZoneStart"]
    pub struct ZoneStart {
        pub checkpoint: String,
        pub cleared: Vec<u32>,
        pub flags: Vec<(String, bool)>,
    }

    impl Versioned for ZoneStart {
        const VERSION: u32 = 2;

        fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
            decode_versioned::<v1::ZoneStart, D>(version, data).map(|old| ZoneStart {
                checkpoint: old.checkpoint,
                cleared: old.cleared.into_iter().map(u32::from).collect(),
                flags: Vec::new(),
            })
        }
    }

    pub fn fixture() -> ZoneStart {
        ZoneStart {
            checkpoint: "gate".to_owned(),
            cleared: vec![1, 70_000],
            flags: vec![("met_the_guide".to_owned(), true)],
        }
    }
}

mod v1 {
    use super::*;

    #[derive(TypePath, Serialize, Deserialize)]
    #[type_path = "sapphire::save_test"]
    #[type_name = "ZoneStart"]
    pub struct ZoneStart {
        pub checkpoint: String,
        pub cleared: Vec<u16>,
    }

    impl Versioned for ZoneStart {
        const VERSION: u32 = 1;
    }

    pub fn fixture() -> ZoneStart {
        ZoneStart { checkpoint: "bridge".to_owned(), cleared: vec![3, 300] }
    }
}

/// xxh3-64 of each committed fixture.
const V1_HASH: u64 = 0x85ef_ac2e_2ec4_8ae9;
const V2_HASH: u64 = 0x64fa_0b0d_5336_3017;

fn fixture_path(version: u32) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/zone_start_v{version}.sav"))
}

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sapphire_save-migrations-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the test's directory");
    dir
}

/// A store whose slot `slot` holds the fixture of `version`.
fn store_with_fixture(dir: &Path, version: u32, slot: Slot) -> ProgressStore<ZoneStart> {
    let store = ProgressStore::<ZoneStart>::new(dir, "progress", durable_replace);
    fs::copy(fixture_path(version), store.slot_path(slot)).expect("copy the fixture");
    store
}

/// Writes the fixtures, once, for a version that has just been frozen: run with `--ignored` and put
/// the printed hash above. Never to get past a failing test: see the file's docs.
#[test]
#[ignore = "writes into the source tree"]
#[allow(clippy::print_stdout, reason = "the hash is what it's run for")]
fn write_fixtures() {
    fn write<T: Versioned + Serialize + serde::de::DeserializeOwned + TypePath>(value: &T) {
        let dir = test_dir("write");
        let store = ProgressStore::<T>::new(&dir, "fixture", durable_replace);
        store.write(&store.snapshot(value).expect("snapshot")).expect("write");
        let path = fixture_path(T::VERSION);
        fs::copy(store.slot_path(Slot::A), &path).expect("copy into the source tree");
        let hash = xxhash_rust::xxh3::xxh3_64(&fs::read(&path).expect("read"));
        println!("{}: {hash:#018x}", path.display());
        fs::remove_dir_all(&dir).expect("clean up");
    }
    write(&v1::fixture());
    write(&v2::fixture());
}

#[test]
fn the_fixtures_are_the_ones_players_have() {
    for (version, expected) in [(1, V1_HASH), (2, V2_HASH)] {
        let bytes = fs::read(fixture_path(version)).expect("read the fixture");
        assert_eq!(
            xxhash_rust::xxh3::xxh3_64(&bytes),
            expected,
            "the v{version} fixture changed. It stands for files already on players' disks: add a \
             reader and a new fixture rather than regenerating it"
        );
    }
}

#[test]
fn version_1_runs_the_whole_chain() {
    let dir = test_dir("v1");
    let save = store_with_fixture(&dir, 1, Slot::A).load().expect("load").expect("a save");
    assert_eq!(
        save.value,
        ZoneStart {
            checkpoint: "bridge".to_owned(),
            cleared: vec![3, 300],
            flags: Vec::new(),
            party: vec!["lead".to_owned()],
        }
    );
    assert_eq!(save.upgraded_from, Some(1));
    assert_eq!((save.slot, save.generation), (Slot::A, 1));
    assert!(matches!(save.other, OtherSlot::Missing));
    fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn version_2_joins_the_chain_halfway() {
    let dir = test_dir("v2");
    let save = store_with_fixture(&dir, 2, Slot::B).load().expect("load").expect("a save");
    assert_eq!(
        save.value,
        ZoneStart {
            checkpoint: "gate".to_owned(),
            cleared: vec![1, 70_000],
            flags: vec![("met_the_guide".to_owned(), true)],
            party: vec!["lead".to_owned()],
        }
    );
    assert_eq!(save.upgraded_from, Some(2));
    assert_eq!(save.slot, Slot::B);
    fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn an_old_save_is_rotated_out_and_stays_the_fall_back_until_then() {
    let dir = test_dir("rotate");
    let store = store_with_fixture(&dir, 1, Slot::A);
    let old = store.load().expect("load").expect("a save").value;

    // The first write at today's schema goes beside the old save, not over it.
    let new = ZoneStart { checkpoint: "tower".to_owned(), ..old };
    let written = store.write(&store.snapshot(&new).expect("snapshot")).expect("write");
    assert_eq!((written.slot, written.generation), (Slot::B, 2));
    let save = store.load().expect("load").expect("a save");
    assert_eq!((save.value.checkpoint.as_str(), save.upgraded_from), ("tower", None));
    assert!(matches!(save.other, OtherSlot::Older { generation: 1 }));

    // If the new one is lost, the old one still migrates.
    fs::write(store.slot_path(Slot::B), b"lost").expect("damage the slot");
    let save = store.load().expect("load").expect("a save");
    assert_eq!((save.value.checkpoint.as_str(), save.upgraded_from), ("bridge", Some(1)));
    assert!(save.fell_back());
    fs::remove_dir_all(&dir).expect("clean up");
}
