//! The navmesh bake on real geometry: does it repeat, and what its digest is on this machine.
//!
//! **CI's Test jobs are the comparison.** Each job prints `NAV-DIGEST` lines, written past the harness's
//! capture, and the ones for colliders are pinned, so a platform that disagrees fails its job with the
//! number: a bake's bytes go in `tables_hash`. `input` is the triangles' bits and `baked` the cooked
//! file's bytes, so a difference can be placed.
//!
//! **The town is pinned too,** though no zone bakes from drawn triangles: they're the largest real
//! geometry there is, and what `assembly` draws must be the same on every host. It once wasn't: its
//! trigonometry was `std`'s, and glibc rounded some of `#ground`'s ring directions otherwise than
//! Apple's and Microsoft's libraries, so Linux's `input` differed in its low bits. It goes through
//! `sapphire_math::det` now. Its `NAV-TOWN` lines give each of its top-level placements a digest, so
//! if the pin fails on one platform, a diff of two platforms' logs names the pieces that differ.

mod nav_support;

use std::fs;
use std::io::Write as _;

use nav_support::{arena_triangles, proving_ground, town_part_digests, town_triangles, triangles_digest};
use sapphire_navmesh::NavMesh;
use sapphire_pipeline::nav::{
    BakeSettings, TileBounds, Triangle, bake, colliders_to_triangles, solid_groups,
};

/// The arena's ground reaches 60 m each way.
const ARENA: TileBounds = ([-4, -4], [3, 3]);
/// The town and its streets, 96 m each way: its ground goes on for kilometres past that.
const TOWN: TileBounds = ([-6, -6], [5, 5]);

/// `NAV-DIGEST`'s pins: what's read and what's written, the same on every platform.
const ARENA_INPUT: &str = "9fc719ca74cedde58fcd99e2177c8c8661ab60306f898775e395706b503f432f";
const ARENA_BAKED: &str = "c36d92bce9879696de17791376f20bc16c0dc8c0696008d8dbabc63c7be581d0";
const PROVING_GROUND_INPUT: &str = "09d0ceb2b37e115b05107d35c68f7a25ca19d50f419ad993077f5c8448676cd6";
const PROVING_GROUND_BAKED: &str = "935ba1185821fc192ceadf5c95016c998f6783b2c4d584a25de98bf0c3620d17";
const TOWN_INPUT: &str = "fd6761e46979818171a0513dca190b1db834542112b6087f50ebd815551a5f3a";
const TOWN_BAKED: &str = "5b35e20ab91fa860b8a7b497e5fe1cf790faa46da9369c9509fe18d0c81e5599";

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
fn the_towns_bake_repeats_and_is_pinned() {
    let (input, baked) = holds("town", &town_triangles(), TOWN);
    assert_eq!(input, TOWN_INPUT, "the town's drawn triangles moved: diff this platform's `NAV-TOWN` lines");
    assert_eq!(baked, TOWN_BAKED, "the town's bake moved");
}

/// A real zone's whole way through a cook's steps, bar the cook: its document's area and spawners, from
/// the bytes of its cooked colliders, and what the cook writes.
#[test]
fn the_proving_grounds_bake_is_pinned() {
    let (colliders, baked, islands) = proving_ground();
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
