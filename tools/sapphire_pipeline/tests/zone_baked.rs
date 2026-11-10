//! The simulation's copies of the zones a replay goes through (`crates/sapphire_gameplay/baked/zones/`), one for
//! each zone a scenario shelves (M4 R3): what their documents say and what a cook makes of them, on every
//! platform CI has. A replay's tables hash takes those bytes. The cells' half of the copies isn't looked at,
//! since the simulation reads none of it.
//!
//! `SAPPHIRE_BAKE_ZONES=1 cargo test -p sapphire_pipeline --test zone_baked`, after `cargo xtask content`, takes
//! new copies from what that cooked, a newly shelved zone's first among them. Then `cargo xtask replay --update`.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use sapphire_colliders::ColliderDocument;
use sapphire_gameplay::replay::{BAKED_ZONES, SHELVED_ZONES, baked_zone};
use sapphire_pipeline::nav::{BakeSettings, Seed, bake_colliders};
use sapphire_serialize::cooked::AlignedVec;
use sapphire_zone::{Zone, ZoneDocument, pack_name};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn take_new_copies(stem: &str) {
    for extension in ["szone", "scol", "snav"] {
        let cooked = root().join(format!("target/content/loose/desktop/zones/{stem}/{stem}.{extension}"));
        let copy = root().join(format!("crates/sapphire_gameplay/baked/zones/{stem}.{extension}"));
        fs::copy(&cooked, &copy).unwrap_or_else(|error| panic!("{}: {error}", cooked.display()));
    }
}

fn aligned(bytes: &[u8]) -> AlignedVec {
    let mut aligned = AlignedVec::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    aligned
}

#[test]
fn the_simulations_copies_of_the_zones_are_what_their_documents_cook_to() {
    let advice = "a zone's document, its colliders or the bake moved: see this test's module docs";
    for &stem in SHELVED_ZONES {
        if std::env::var_os("SAPPHIRE_BAKE_ZONES").is_some() {
            // The run after this one is what reads them: this one's `include_bytes!` were built already.
            take_new_copies(stem);
            continue;
        }
        let baked =
            baked_zone(stem).unwrap_or_else(|| panic!("a scenario shelves {stem}, with no copy: {advice}"));
        let dir = root().join("content/zones").join(stem);
        let read = |name: String| fs::read_to_string(dir.join(name)).expect("the zone's file");
        let document = ZoneDocument::from_ron(&read(format!("{stem}.zone.ron"))).expect("it reads");
        let expected = document.simulation().expect("it's good");
        let held = aligned(baked.zone);
        let zone = Zone::read(&held).unwrap_or_else(|error| panic!("{stem}: {error}: {advice}"));
        let simulation = &zone.simulation;

        assert_eq!(simulation.colliders.as_ref().copied(), expected.colliders, "{stem}: {advice}");
        assert_eq!(simulation.spawners.len(), expected.spawners.len(), "{stem}: {advice}");
        for (held, document) in simulation.spawners.iter().zip(&expected.spawners) {
            assert_eq!(held.id.to_native(), document.id, "{stem}: {advice}");
            assert_eq!(held.position(), document.position, "{stem}: {advice}");
            assert_eq!(held.yaw_degrees.to_native(), document.yaw_degrees, "{stem}: {advice}");
        }
        assert_eq!(simulation.triggers.len(), expected.triggers.len(), "{stem}: {advice}");
        for (held, document) in simulation.triggers.iter().zip(&expected.triggers) {
            assert_eq!(held.id.to_native(), document.id, "{stem}: {advice}");
            assert_eq!(held.event.as_str(), document.event, "{stem}: {advice}");
        }
        assert_eq!(simulation.exits.len(), expected.exits.len(), "{stem}: {advice}");
        for (held, document) in simulation.exits.iter().zip(&expected.exits) {
            assert_eq!(held.id.to_native(), document.id, "{stem}: {advice}");
            assert_eq!(held.to, document.to, "{stem}: {advice}");
            assert_eq!(held.arrive.to_native(), document.arrive, "{stem}: {advice}");
            let other = BAKED_ZONES.iter().find(|baked| baked.asset == document.to).map(|baked| baked.stem);
            if let Some(other) = other {
                assert_eq!(held.pack.as_str(), pack_name(other), "{stem}: {advice}");
            }
        }

        let colliders = ColliderDocument::from_ron(&read(format!("{stem}.colliders.ron")))
            .expect("it reads")
            .cook()
            .expect("it cooks")
            .to_cooked()
            .expect("it writes");
        assert!(colliders.as_slice() == baked.colliders, "{stem}'s colliders: {advice}");

        let navigation = document.navigation.as_ref().expect("each asks for a navmesh");
        let seeds: Vec<Seed> = document
            .spawners
            .iter()
            .map(|spawner| Seed { field: spawner.name.clone(), position: spawner.position })
            .collect();
        let area = (navigation.min, navigation.max);
        let (bake, _) =
            bake_colliders(&colliders, area, &seeds, &BakeSettings::CHARACTER, None).expect("it bakes");
        let navmesh = bake.mesh.to_cooked().expect("it writes");
        assert!(navmesh.as_slice() == baked.navmesh, "{stem}'s navmesh: {advice}");

        // What the replay's tables hold, for a look across CI's logs.
        let digest = baked.load().expect("it loads").digest();
        writeln!(std::io::stdout(), "ZONE-DIGEST {} {stem} {digest:#018x}", std::env::consts::ARCH)
            .expect("it prints");
    }
}
