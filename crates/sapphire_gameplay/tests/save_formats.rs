//! What a player's save holds of the simulation, frozen at each version (M3 exit test 8: an old save
//! migrates). A save is `postcard` after `sapphire_serialize`'s header: positional, with no field names, so
//! a field added, removed or reordered without a new `Versioned::VERSION`, and an upgrade from the old one,
//! reads a player's file as garbage or not at all. `sapphire_save`'s fixtures prove the chain on a type of
//! its own; these hold the game's types to it.
//!
//! Each version's bytes are written out here once, when the version is frozen, and never regenerated: a
//! change that fails here wants a new version whose upgrade reads these, and new bytes beside them.

use sapphire_core::Name;
use sapphire_gameplay::{FlagValues, ZoneStart};
use sapphire_serialize::{AssetId, from_save, to_save};

/// `ZoneStart` version 1 (M3 B4): a checkpoint by name and two cleared encounters.
const ZONE_START_V1: &[u8] = &[
    83, 80, 83, 86, 1, 0, 0, 0, 55, 210, 90, 65, 150, 154, 254, 255, 1, 0, 0, 0, 0, 0, 0, 0, 41, 0, 0, 0, 0,
    0, 0, 0, 1, 6, 109, 97, 114, 107, 101, 116, 2, 90, 90, 90, 90, 90, 90, 90, 90, 90, 90, 90, 90, 90, 90,
    90, 90, 195, 195, 195, 195, 195, 195, 195, 195, 195, 195, 195, 195, 195, 195, 195, 195,
];

/// `FlagValues` version 1 (M3 Q2): two flags, one negative.
const FLAG_VALUES_V1: &[u8] = &[
    83, 80, 83, 86, 1, 0, 0, 0, 167, 85, 176, 35, 121, 182, 11, 222, 1, 0, 0, 0, 0, 0, 0, 0, 22, 0, 0, 0, 0,
    0, 0, 0, 2, 194, 193, 175, 216, 142, 131, 194, 198, 201, 1, 2, 210, 176, 243, 159, 193, 234, 144, 184,
    87, 5,
];

fn zone_start_v1() -> ZoneStart {
    ZoneStart {
        checkpoint: Some("market".to_owned()),
        cleared: vec![AssetId::from_bytes([0x5a; 16]), AssetId::from_bytes([0xc3; 16])],
    }
}

fn flag_values_v1() -> FlagValues {
    FlagValues { flags: vec![(Name::new("met_guide").as_u64(), 1), (Name::new("ward_lit").as_u64(), -3)] }
}

#[test]
fn a_version_1_zone_start_still_reads() {
    let read: ZoneStart = from_save(ZONE_START_V1).expect("a player's version 1 file reads");
    assert_eq!(read, zone_start_v1());
}

#[test]
fn a_version_1_set_of_flags_still_reads() {
    let read: FlagValues = from_save(FLAG_VALUES_V1).expect("a player's version 1 file reads");
    assert_eq!(read, flag_values_v1());
}

/// What this build writes, while it's version 1: a type that changes shape without a new version fails here
/// even when the old bytes happen to read. A new version moves this to its own bytes and keeps the reads
/// above.
#[test]
fn this_build_writes_version_1() {
    assert_eq!(to_save(&zone_start_v1()).expect("writes"), ZONE_START_V1);
    assert_eq!(to_save(&flag_values_v1()).expect("writes"), FLAG_VALUES_V1);
}
