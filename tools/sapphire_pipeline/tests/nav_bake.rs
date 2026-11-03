//! The navmesh bake on real geometry: does it repeat, and what its digest is on this machine.
//!
//! **CI's Test jobs are the comparison.** Each job prints `NAV-DIGEST` lines, written past the harness's
//! capture, and the ones for colliders are pinned, so a platform that disagrees fails its job with the
//! number: a bake's bytes go in `tables_hash`. `input` is the triangles' bits and `baked` the cooked
//! file's bytes, so a difference can be placed.
//!
//! **The town isn't pinned,** as no zone bakes from drawn triangles: `assembly` makes them with `std`'s
//! transcendentals, so the low bits of its `input` differ on Linux, whose C library isn't the others'.
//! Its `NAV-TOWN` lines give each of its top-level placements a digest, so a diff of two platforms' logs
//! names the pieces that differ.

mod nav_support;

use std::fs;
use std::io::Write as _;

use nav_support::{arena_triangles, content, town_part_digests, town_triangles, triangles_digest};
use sapphire_colliders::ColliderDocument;
use sapphire_navmesh::NavMesh;
use sapphire_pipeline::nav::{
    BakeSettings, Seed, TileBounds, Triangle, bake, bake_colliders, colliders_to_triangles, solid_groups,
};
use sapphire_zone::ZoneDocument;

/// The arena's ground reaches 60 m each way.
const ARENA: TileBounds = ([-4, -4], [3, 3]);
/// The town and its streets, 96 m each way: its ground goes on for kilometres past that.
const TOWN: TileBounds = ([-6, -6], [5, 5]);

/// `NAV-DIGEST`'s pins: what's read and what's written, the same on every platform.
const ARENA_INPUT: &str = "9fc719ca74cedde58fcd99e2177c8c8661ab60306f898775e395706b503f432f";
const ARENA_BAKED: &str = "c36d92bce9879696de17791376f20bc16c0dc8c0696008d8dbabc63c7be581d0";
const PROVING_GROUND_INPUT: &str = "09d0ceb2b37e115b05107d35c68f7a25ca19d50f419ad993077f5c8448676cd6";
const PROVING_GROUND_BAKED: &str = "935ba1185821fc192ceadf5c95016c998f6783b2c4d584a25de98bf0c3620d17";

/// Says what a bake came to, and returns its digests.
fn holds(name: &str, triangles: &[Triangle], bounds: TileBounds) -> (String, String) {
    let settings = BakeSettings::CHARACTER;
    let first = bake(triangles, bounds, &settings).expect("it bakes");
    let again = bake(triangles, bounds, &settings).expect("it bakes");
    let bytes = first.mesh.to_cooked().expect("it writes");
    assert_eq!(bytes.as_slice(), again.mesh.to_cooked().expect("it writes").as_slice(), "{name} must repeat");
    NavMesh::read(&bytes).expect("what a bake writes, the runtime reads");
    let digests = (triangles_digest(triangles).to_string(), blake3::hash(&bytes).to_string());
    writeln!(
        std::io::stdout(),
        "NAV-DIGEST {} {name}: input {}, baked {}, {} bytes, {} polygons",
        std::env::consts::ARCH,
        digests.0,
        digests.1,
        bytes.len(),
        first.mesh.polygon_count(),
    )
    .expect("it prints");
    digests
}

#[test]
fn the_arenas_bake_repeats_and_is_pinned() {
    let (input, baked) = holds("arena", &arena_triangles(), ARENA);
    assert_eq!(input, ARENA_INPUT, "the triangles of the arena's colliders moved");
    assert_eq!(baked, ARENA_BAKED, "the arena's bake moved");
}

#[test]
fn the_towns_bake_repeats() {
    holds("town", &town_triangles(), TOWN);
}

/// A real zone's whole way through a cook's steps, bar the cook: its document's area and spawners, from
/// the bytes of its cooked colliders, and what the cook writes.
#[test]
fn the_proving_grounds_bake_is_pinned() {
    let dir = content().join("zones/proving_ground");
    let read = |name: &str| fs::read_to_string(dir.join(name)).expect("the zone's file");
    let zone = ZoneDocument::from_ron(&read("proving_ground.zone.ron")).expect("the zone reads");
    let navigation = zone.navigation.as_ref().expect("it asks for a navmesh");
    let colliders = ColliderDocument::from_ron(&read("proving_ground.colliders.ron"))
        .expect("it reads")
        .cook()
        .expect("it cooks");
    let cooked = colliders.to_cooked().expect("it writes");
    let seeds: Vec<Seed> = zone
        .spawners
        .iter()
        .map(|spawner| Seed { field: spawner.name.clone(), position: spawner.position })
        .collect();
    let settings = BakeSettings::CHARACTER;
    let (baked, islands) =
        bake_colliders(&cooked, (navigation.min, navigation.max), &seeds, &settings, None).expect("it bakes");
    let input = triangles_digest(&colliders_to_triangles(&colliders, solid_groups())).to_string();
    let bytes = baked.mesh.to_cooked().expect("it writes");
    let output = blake3::hash(&bytes).to_string();
    writeln!(
        std::io::stdout(),
        "NAV-DIGEST {} proving_ground: input {input}, baked {output}, {} bytes, {} polygons, {islands:?}",
        std::env::consts::ARCH,
        bytes.len(),
        baked.mesh.polygon_count(),
    )
    .expect("it prints");
    assert_eq!(input, PROVING_GROUND_INPUT, "the triangles of its colliders moved");
    assert_eq!(output, PROVING_GROUND_BAKED, "its bake moved");
}

/// Not a check: lines for CI's logs. Search for `NAV-TOWN` and diff Linux's against Windows'.
#[test]
fn each_piece_of_the_town_says_its_digest() {
    let mut stdout = std::io::stdout();
    for (index, (name, triangles, digest)) in town_part_digests().iter().enumerate() {
        let arch = std::env::consts::ARCH;
        let os = std::env::consts::OS;
        writeln!(stdout, "NAV-TOWN {os} {arch} {index} {name}: {triangles} triangles, {digest}")
            .expect("it prints");
    }
}

/// **A bump of `rerecast` stops here.** Everything in `sapphire_pipeline::nav` was read against this
/// version: what it lacks (hole merging), what it gets wrong (a crossed contour, a 16-bit diagonal past
/// 180 cells, a vertex removal that can hang), and where its floats go (`+ - * /` only, and one `cos`
/// it's kept away from). So before changing this, read those again in the new version, and run:
///
/// - `SAPPHIRE_NAV_SCENES=1500 cargo test --profile tools -p sapphire_pipeline --test nav_attack --
///   --ignored --nocapture`, which must find no failure and no hang;
/// - this file on every platform CI has, whose pins will move, and which must all move alike;
/// - and bump `nav::TILE_BAKER`, as a tile's bake isn't what the cache holds any more.
///
/// Its `libm` and `nostd-libm` features stay off: each names glam's own, on the workspace's one glam.
#[test]
fn rerecast_is_the_version_this_module_was_read_against() {
    let lock =
        fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock")).expect("the lock file");
    let mut lines = lock.lines().skip_while(|line| *line != r#"name = "rerecast""#);
    lines.next();
    assert_eq!(
        lines.next(),
        Some(r#"version = "0.5.0""#),
        "see this test's comment before bumping `rerecast`"
    );
}
