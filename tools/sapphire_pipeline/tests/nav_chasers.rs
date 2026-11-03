//! Rule R6 and gate 2 for paths at every size a fight might be: N chasers following paths over the arena's
//! real navmesh, with no heap allocation in any step and one checksum across runs. Installs
//! `CountingAllocator` as this test binary's global allocator. Each run prints its checksum and what it
//! cost in nodes, so CI's jobs can be compared by `NAV-CHASERS`. The recorded gate is the `nav-chase`
//! replay, with 64 of them in the real controller: this one has no physics, so all it runs is paths.

#[path = "../../../crates/sapphire_nav/examples/support/chasers.rs"]
mod chasers;

use std::alloc::System;
use std::io::Write as _;

use chasers::{Chasers, target_at};
use glam::Vec3;
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_gameplay::arena;
use sapphire_nav::{NODES_PER_TICK, NavStats};
use sapphire_navmesh::{ArchivedNavMesh, NavMesh};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// Ten seconds of a fight. Long enough to have every chaser reach and overrun its target.
const TICKS: u32 = 600;

/// Round the square's stalls and its pavilion, at the paving's height.
const WALK: [Vec3; 4] = [
    Vec3::new(-14.0, 0.0, 12.0),
    Vec3::new(14.0, 0.0, 12.0),
    Vec3::new(14.0, 0.0, -13.0),
    Vec3::new(-10.0, 0.0, -14.0),
];

/// Runs `count` chasers on `budget` nodes a tick. Returns the checksum and what it cost, having held every
/// step to no allocation at all and every tick to its budget.
fn run(mesh: &ArchivedNavMesh, count: usize, budget: u32) -> (u64, NavStats) {
    let mut chasers = Chasers::new(mesh, count, budget, WALK[0], 11);
    for tick in 0..TICKS {
        let target = target_at(&WALK, tick);
        let ((), counts) = alloc::count_in_scope(|| chasers.step(target));
        assert_eq!(counts.allocations, 0, "{count} chasers allocated at tick {tick}");
    }
    assert!(chasers.stats.worst_tick <= budget, "{count} chasers on {budget} nodes: {:?}", chasers.stats);
    assert!(chasers.stats.searches >= count as u32, "everyone searched: {:?}", chasers.stats);
    // The target walks straight, over planters' tops as well. They aren't on the mesh, since the cook
    // pruned them, so a target up there keeps the route it has, and nobody's left with no way.
    assert_eq!(chasers.no_way, 0, "{count} chasers on {budget} nodes");
    (chasers.checksum, chasers.stats)
}

#[test]
fn chasers_follow_paths_with_no_allocation_and_one_checksum() {
    if !PER_THREAD_COUNTS {
        return;
    }
    assert!(alloc::is_installed());
    let bytes = arena::cooked_navmesh();
    let mesh = NavMesh::read(&bytes).expect("it reads");
    let mut stdout = std::io::stdout();
    for count in [4, 16, 64] {
        // The plan's budget, and one that pinches: a quarter of it, so the deferring runs too.
        for budget in [NODES_PER_TICK, NODES_PER_TICK / 4] {
            let (first, stats) = run(mesh, count, budget);
            let (again, _) = run(mesh, count, budget);
            assert_eq!(first, again, "{count} chasers on {budget} nodes: a run must repeat");
            // Direct, since the harness holds back what `println!` writes while a test passes.
            writeln!(
                stdout,
                "NAV-CHASERS {}: {count} chasers, {budget} nodes a tick, {} searches, {} nodes, worst tick {}, {} deferred, checksum {first:#018x}",
                std::env::consts::ARCH,
                stats.searches,
                stats.expanded,
                stats.worst_tick,
                stats.deferred,
            )
            .expect("it prints");
        }
    }
}
