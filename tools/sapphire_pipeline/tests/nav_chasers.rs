//! Rule R6 and gate 2 for paths: N chasers repathing over a real bake, with no heap allocation in any
//! step and one checksum across runs. Installs `CountingAllocator` as this test binary's global
//! allocator. Each run prints its checksum, so CI's jobs can be compared.

mod nav_support;

#[path = "../../../crates/sapphire_nav/examples/support/chasers.rs"]
mod chasers;

use std::alloc::System;
use std::io::Write as _;

use chasers::{Chasers, target_at};
use glam::Vec3;
use nav_support::arena_triangles;
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_nav::NavGraph;
use sapphire_navmesh::NavMesh;
use sapphire_pipeline::nav::{BakeSettings, bake};
use sapphire_serialize::cooked::AlignedVec;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// Ten seconds of a fight. Long enough to have every chaser reach and overrun its target.
const TICKS: u32 = 600;

fn arena() -> AlignedVec {
    let baked = bake(&arena_triangles(), ([-4, -4], [3, 3]), &BakeSettings::CHARACTER).expect("it bakes");
    baked.mesh.to_cooked().expect("it writes")
}

/// Round the square's stalls and its pavilion, at the paving's height.
const WALK: [Vec3; 4] = [
    Vec3::new(-14.0, 0.0, 12.0),
    Vec3::new(14.0, 0.0, 12.0),
    Vec3::new(14.0, 0.0, -13.0),
    Vec3::new(-10.0, 0.0, -14.0),
];

/// Runs `count` chasers with `budget` searches a tick. Returns the checksum, having held every step
/// to no allocation at all.
fn run(graph: &NavGraph, count: usize, budget: usize) -> (u64, u64, u64) {
    let mut chasers = Chasers::new(graph, count, 4096, WALK[0], 11);
    for tick in 0..TICKS {
        let target = target_at(&WALK, tick);
        let ((), counts) = alloc::count_in_scope(|| chasers.step(graph, target, budget));
        assert_eq!(counts.allocations, 0, "{count} chasers allocated at tick {tick}");
    }
    assert!(chasers.searches >= u64::from(TICKS - 1) * budget as u64, "they searched: {}", chasers.searches);
    // The target walks straight, over planters' tops as well, and those are islands, so some
    // searches find no way and walk as near as there is.
    assert!(
        chasers.unreached * 4 < chasers.searches,
        "{} of {} searches found no way",
        chasers.unreached,
        chasers.searches,
    );
    (chasers.checksum, chasers.searches, chasers.unreached)
}

#[test]
fn chasers_repath_with_no_allocation_and_one_checksum() {
    if !PER_THREAD_COUNTS {
        return;
    }
    assert!(alloc::is_installed());
    let bytes = arena();
    let graph = NavGraph::new(NavMesh::read(&bytes).expect("it reads"));
    let mut stdout = std::io::stdout();
    let mut seen = Vec::new();
    for count in [4, 16, 64] {
        // Every chaser every tick is the worst a tick can be; a budget of 1 is the likeliest.
        for budget in [1, count] {
            let (first, searches, unreached) = run(&graph, count, budget);
            let (again, ..) = run(&graph, count, budget);
            assert_eq!(first, again, "{count} chasers, {budget} a tick: a run must repeat");
            // Direct, since the harness holds back what `println!` writes while a test passes.
            writeln!(
                stdout,
                "NAV-CHASERS {}: {count} chasers, {budget} a tick, {searches} searches, {unreached} found no way, checksum {first:#018x}",
                std::env::consts::ARCH,
            )
            .expect("it prints");
            seen.push(first);
        }
    }
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 6, "each run is its own run, or the checksum isn't reading it");
}
