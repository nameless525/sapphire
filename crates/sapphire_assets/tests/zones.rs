//! A zone's bytes on the road every asset takes: the packer's and a decode job's check.

use sapphire_assets::{AssetError, AssetId, AssetKind};
use sapphire_zone::{Bounds, Place, Zone, ZoneDocument, ZoneError, ZoneGeometry};

const COLLIDERS: AssetId = AssetId::from_bytes([1; 16]);
const MESH: AssetId = AssetId::from_bytes([2; 16]);
const PROBES: AssetId = AssetId::from_bytes([3; 16]);

/// A number nothing else in the test zone's cooked bytes holds, as a cell's light count.
const COUNT: u32 = 0x5a5a_3331;

fn zone() -> Zone {
    let document = ZoneDocument::from_ron(&format!(
        r#"(type: "sapphire::zone::ZoneDocument", version: 1, data: (
            grid: (cell_size: 32.0),
            probes: Some("{PROBES}"),
            colliders: Some("{COLLIDERS}"),
            spawners: [(name: "start", position: (1.0, 0.0, 1.0), spawns: "player")],
        ))"#
    ))
    .expect("parses");
    let bounds = Bounds { min: [0.0; 3], max: [8.0; 3] };
    let meshes = [(Place::Backdrop, (MESH, bounds))].into();
    document.cook(ZoneGeometry { meshes, ..ZoneGeometry::default() }).expect("cooks")
}

#[test]
fn a_zone_depends_on_what_the_simulation_reads_and_reaches_the_rest() {
    let cooked = zone().to_cooked().expect("writes");
    assert_eq!(AssetKind::Zone.references(&cooked).expect("good"), [COLLIDERS], "what loading it loads");
    let mut reach = vec![COLLIDERS, MESH, PROBES];
    reach.sort_unstable();
    assert_eq!(AssetKind::Zone.reach(&cooked).expect("good"), reach, "what must ship with it");
    assert!(AssetKind::Zone.may_depend_on(AssetKind::Colliders));
    assert!(AssetKind::Zone.may_depend_on(AssetKind::NavMesh));
    assert!(!AssetKind::NavMesh.uploads() && AssetKind::NavMesh.keeps_bytes());
    assert!(!AssetKind::Zone.may_depend_on(AssetKind::Mesh), "a decode job would refuse a cell's mesh");
}

/// Other kinds reach what they depend on, and nothing else.
#[test]
fn only_a_zone_reaches_further_than_it_depends() {
    let refused = AssetKind::Mesh.reach(&[]);
    assert!(matches!(refused, Err(AssetError::Mesh(_))), "{refused:?}");
    let refused = AssetKind::Zone.reach(&[]);
    assert!(matches!(refused, Err(AssetError::Zone(ZoneError::Cooked(_)))), "{refused:?}");
}

/// One changed number in a good archive: every pointer and length still right, so a bytecheck can't
/// tell. A cell's light range is what a world would index with.
#[test]
fn a_range_no_cook_can_write_is_refused_and_doesnt_panic() {
    let mut zone = zone();
    let backdrop = zone.presentation.backdrop.as_mut().expect("there");
    backdrop.light_count = COUNT;
    backdrop.first_light = 0;
    let cooked = {
        let bytes = sapphire_serialize::to_cooked(&zone).expect("writes");
        assert!(Zone::read(&bytes).is_err(), "too many lights for an empty list");
        bytes
    };
    let places: Vec<usize> = cooked
        .windows(4)
        .enumerate()
        .filter(|(_, bytes)| *bytes == COUNT.to_le_bytes())
        .map(|at| at.0)
        .collect();
    let [place] = places[..] else { panic!("{COUNT} should be there once, not at {places:?}") };

    let mut healed = cooked.clone();
    healed[place..place + 4].copy_from_slice(&0u32.to_le_bytes());
    assert!(AssetKind::Zone.references(&healed).is_ok(), "only that number was wrong");

    for count in [u32::MAX, u32::MAX - 1, 1, 1 << 20] {
        let mut damaged = cooked.clone();
        damaged[place..place + 4].copy_from_slice(&count.to_le_bytes());
        let refused = AssetKind::Zone.references(&damaged);
        assert!(matches!(&refused, Err(AssetError::Zone(ZoneError::Invalid { .. }))), "{count}: {refused:?}");
    }
}
