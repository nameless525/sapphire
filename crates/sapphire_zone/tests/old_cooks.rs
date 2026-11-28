//! A zone an older cook wrote, read for one release after its layout changed (cooker rule 2): the proving hub
//! as M5's cook wrote it at version 8, the last before water (M6 W2), its bytes as they were committed.

use sapphire_serialize::AssetId;
use sapphire_serialize::cooked::{AlignedVec, schema_version};
use sapphire_zone::Zone;

const VERSION_8: &[u8] = include_bytes!("../testdata/proving_hub.v8.szone");

#[test]
fn a_version_8_cook_reads_as_a_zone_with_no_water() {
    let mut old: AlignedVec = AlignedVec::new();
    old.extend_from_slice(VERSION_8);
    assert_eq!(schema_version::<Zone>(&old).expect("a zone's header"), 8);
    let message = Zone::read(&old).expect_err("stale in place").to_string();
    assert!(message.contains("recook"), "{message}");

    let upgraded = Zone::upgrade(&old).expect("reads").expect("rewritten at this version");
    let zone = Zone::read(&upgraded).expect("reads in place");
    assert!(zone.simulation.water.is_empty() && zone.presentation.water.is_empty());
    assert!(
        !zone.simulation.spawners.is_empty() && !zone.presentation.cells.is_empty(),
        "the rest came across"
    );

    // Its digest, of the values the simulation reads and of what it references (each read here as its ID's
    // bytes), as pinned when water came (M6 W2): this holds the upgrade's digest steady from then on, through
    // every later version's upgrade. That water moved no zone without it is `shipped_digests.rs`'s to show,
    // whose lines were pinned before water existed.
    let ids: Vec<AssetId> = zone.references();
    let stand_ins: Vec<[u8; 16]> = ids.iter().map(|id| id.to_bytes()).collect();
    let read = |id: AssetId| ids.iter().position(|&held| held == id).map(|at| &stand_ins[at][..]);
    assert_eq!(zone.simulation_digest(read).expect("digests"), 0x8133_3b7b_214b_4579);
}
