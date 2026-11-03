//! The simulation's copy of the arena's navmesh (`crates/sapphire_gameplay/baked/arena.snav`): what a
//! bake of that arena's colliders makes, on every platform CI has, which makes this gate 2 for a bake as
//! well. A replay's tables hash takes those bytes.

use std::io::Write as _;
use std::path::Path;

use sapphire_gameplay::arena;
use sapphire_nav::{NavGraph, PathSearch, Reach, RouteEnd};
use sapphire_navmesh::NavMesh;
use sapphire_pipeline::nav::{BakeSettings, Seed, bake_colliders};

/// Where a character can be: the whole of the colliders' 128 m.
const AREA: ([f32; 2], [f32; 2]) = ([-64.0, -64.0], [64.0, 64.0]);

#[test]
fn the_simulations_copy_of_the_arenas_navmesh_is_what_a_cook_bakes() {
    let colliders = arena::cooked_colliders().expect("it cooks");
    let spawns = arena::spawns();
    let seeds = [
        Seed { field: "player".to_owned(), position: spawns.player.position().to_array() },
        Seed { field: "enemy".to_owned(), position: spawns.enemy.position().to_array() },
    ];
    let (baked, islands) =
        bake_colliders(&colliders, AREA, &seeds, &BakeSettings::CHARACTER, None).expect("it bakes");
    let cooked = baked.mesh.to_cooked().expect("it writes");
    writeln!(
        std::io::stdout(),
        "NAV-DIGEST {} arena copy: baked {}, {} bytes, {} polygons, {islands:?}",
        std::env::consts::ARCH,
        blake3::hash(&cooked),
        cooked.len(),
        baked.mesh.polygon_count(),
    )
    .expect("it prints");

    let copy = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/sapphire_gameplay/baked/arena.snav");
    if std::env::var_os("SAPPHIRE_BAKE_NAV").is_some() {
        std::fs::write(&copy, &cooked).expect("writes");
    }
    let held = std::fs::read(&copy).expect("the simulation's copy");
    assert!(
        held == cooked.as_slice(),
        "the colliders, the spawn points or the bake moved: SAPPHIRE_BAKE_NAV=1 cargo test -p sapphire_pipeline --test \
         nav_baked, then cargo xtask replay --update"
    );

    // The two ends of the arena are one piece of ground, or what's kept isn't where a fight goes.
    let graph = NavGraph::new(NavMesh::read(&cooked).expect("it reads"));
    let start = graph.locate(spawns.player.position(), Reach::CHARACTER).expect("on the mesh");
    let end = graph.locate(spawns.enemy.position(), Reach::CHARACTER).expect("on the mesh");
    let mut corridor = vec![0; graph.polygon_count()];
    let route = PathSearch::new(&graph, u32::MAX).find(&graph, start, end, &mut corridor);
    assert_eq!(route.end, RouteEnd::Reached);
}
