//! Every shipped zone's simulation digest, held: a change to the zone format that moves what the simulation
//! reads of a zone that doesn't use it fails here, by the zone's name.
//!
//! The digest is taken over the document's own simulation half, cooked with an empty presentation and a
//! stand-in for each exit's pack, and each asset it references (colliders, navmesh, encounters) read as its
//! ID's bytes: those cook in their own
//! crates and their replays pin them. What this holds is the zone's own part, the part a change to this
//! crate can move. A zone whose document changes on purpose sets its line to what this prints.

use std::path::Path;

use sapphire_serialize::AssetId;
use sapphire_zone::{Zone, ZoneDocument, ZonePresentation};

/// Each shipped zone's stem and its digest.
const SHIPPED: [(&str, u64); 6] = [
    ("lantern_ward", 0xefea_6520_9028_3267),
    ("proving_ground", 0xe0b8_b6b1_e8a0_5dcc),
    ("proving_hub", 0xdeb2_2d8a_6645_871a),
    ("square", 0x6aa4_6f3d_757e_3880),
    ("square_street", 0xa7dd_8f25_a39b_ec08),
    ("town", 0x1d90_7ced_8e28_2c90),
];

fn digest(stem: &str) -> u64 {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../content/zones/{stem}/{stem}.zone.ron"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let document = ZoneDocument::from_ron(&text).unwrap_or_else(|error| panic!("{stem}: {error}"));
    let mut simulation = document.simulation().unwrap_or_else(|error| panic!("{stem}: {error}"));
    // The pack an exit leads into is the cook's to name, from the zone it leads to.
    for exit in &mut simulation.exits {
        exit.pack = "a_pack".to_owned();
    }
    let zone = Zone {
        simulation,
        presentation: ZonePresentation {
            cell_size: document.grid.cell_size,
            backdrop: None,
            cells: Vec::new(),
            instances: Vec::new(),
            lights: Vec::new(),
            probes: None,
            reflections: None,
            day: None,
            rigs: None,
            splines: Vec::new(),
            sky: None,
            wind: None,
            weather: None,
            fog: None,
            water: Vec::new(),
            rooms: Vec::new(),
            weather_plan: None,
        },
    };
    let bytes = zone.to_cooked().expect("writes");
    let ids: Vec<AssetId> = Zone::read(&bytes).expect("reads").references();
    let stand_ins: Vec<[u8; 16]> = ids.iter().map(|id| id.to_bytes()).collect();
    let read = |id: AssetId| ids.iter().position(|&held| held == id).map(|at| &stand_ins[at][..]);
    Zone::read(&bytes).expect("reads").simulation_digest(read).expect("digests")
}

#[test]
fn every_shipped_zones_simulation_digest_holds() {
    let mut moved = Vec::new();
    for (stem, pinned) in SHIPPED {
        let found = digest(stem);
        if found != pinned {
            moved.push(format!("(\"{stem}\", {found:#018x}),"));
        }
    }
    assert!(moved.is_empty(), "these zones' digests moved; if on purpose, pin them:\n{}", moved.join("\n"));
}

#[test]
fn the_list_is_every_shipped_zone() {
    let zones = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/zones");
    let mut stems: Vec<String> = std::fs::read_dir(&zones)
        .expect("the shipped zones")
        .map(|entry| entry.expect("an entry").file_name().to_string_lossy().into_owned())
        .collect();
    stems.sort_unstable();
    let listed: Vec<&str> = SHIPPED.iter().map(|(stem, _)| *stem).collect();
    assert_eq!(stems, listed);
}
