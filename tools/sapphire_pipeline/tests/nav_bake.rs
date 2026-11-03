//! The navmesh bake on real geometry: does it repeat, and what its digest is on this machine.
//!
//! **CI's Test jobs are the comparison.** Each job prints `NAV-DIGEST` lines, written past the harness's
//! capture. Compare Linux's with Windows' (x86-64, what `tables_hash` asks of them) and both with macOS'
//! (arm64). `input` is the triangles' bits and `baked` the cooked file's bytes, so a difference can be
//! placed: an `input` that differs on the town is the assembly's floats, not the bake.

mod nav_support;

use std::io::Write as _;

use nav_support::{arena_triangles, town_triangles, triangles_digest};
use sapphire_navmesh::NavMesh;
use sapphire_pipeline::nav::{BakeSettings, TileBounds, Triangle, bake};

/// The arena's ground reaches 60 m each way.
const ARENA: TileBounds = ([-4, -4], [3, 3]);
/// The town and its streets, 96 m each way: its ground goes on for kilometres past that.
const TOWN: TileBounds = ([-6, -6], [5, 5]);

fn holds(name: &str, triangles: &[Triangle], bounds: TileBounds) {
    let settings = BakeSettings::CHARACTER;
    let first = bake(triangles, bounds, &settings).expect("it bakes");
    let again = bake(triangles, bounds, &settings).expect("it bakes");
    let bytes = first.mesh.to_cooked().expect("it writes");
    assert_eq!(bytes.as_slice(), again.mesh.to_cooked().expect("it writes").as_slice(), "{name} must repeat");
    NavMesh::read(&bytes).expect("what a bake writes, the runtime reads");
    writeln!(
        std::io::stdout(),
        "NAV-DIGEST {} {name}: input {}, baked {}, {} bytes, {} polygons",
        std::env::consts::ARCH,
        triangles_digest(triangles),
        blake3::hash(&bytes),
        bytes.len(),
        first.mesh.polygon_count(),
    )
    .expect("it prints");
}

#[test]
fn the_arenas_bake_repeats() {
    holds("arena", &arena_triangles(), ARENA);
}

#[test]
fn the_towns_bake_repeats() {
    holds("town", &town_triangles(), TOWN);
}
