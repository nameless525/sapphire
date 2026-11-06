//! What a path costs on a real navmesh, against spec §3.2's fixed step (1.0 ms on Desktop-Low, 1.5 on
//! Mobile-Low, for all of combat, AI and the controller). Bake one first:
//!
//! ```text
//! cargo run --profile tools -p sapphire_pipeline --example nav_bake -- <directory>
//! cargo run --release -p sapphire_nav --example nav_bench -- <directory>/town.snav
//! ```
//!
//! A query here is the whole of what a repath does: locate both ends, search, straighten. Each query
//! runs in several rounds. A query's least across rounds is what its work costs and its worst is mostly
//! the scheduler on a busy machine, so both are read. Polygons expanded don't depend on load at all.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

#[path = "support/chasers.rs"]
mod chasers;

use std::time::Duration;

use chasers::{Chasers, Lcg, target_at};
use glam::Vec3;
use sapphire_nav::{NODES_PER_TICK, NavGraph, NavStats, PathSearch, Reach, RouteEnd, straighten};
use sapphire_navmesh::{ArchivedNavMesh, NO_NEIGHBOUR, NavMesh};
use sapphire_serialize::cooked::AlignedVec;
use sapphire_time::Stopwatch;

const ROUNDS: usize = 9;
const QUERIES: usize = 400;

fn percentile(sorted: &[Duration], share: f64) -> f64 {
    let at = ((sorted.len() - 1) as f64 * share).round() as usize;
    sorted[at].as_secs_f64() * 1e6
}

fn report(label: &str, mut times: Vec<Duration>) {
    times.sort_unstable();
    println!(
        "  {label}: p50 {:.1} µs, p99 {:.1}, worst {:.1} ({} samples)",
        percentile(&times, 0.5),
        percentile(&times, 0.99),
        percentile(&times, 1.0),
        times.len(),
    );
}

struct Pair {
    from: Vec3,
    to: Vec3,
}

/// Pairs of polygon centres a search connects whose straight-line distance is in `range`.
fn pairs(graph: &NavGraph, range: (f32, f32), seed: u64) -> Vec<Pair> {
    let mut random = Lcg(seed);
    let mut search = PathSearch::new(graph, u32::MAX);
    let mut corridor = vec![0; 4096];
    let mut found = Vec::with_capacity(QUERIES);
    let mut tries = 0;
    while found.len() < QUERIES && tries < 4_000_000 {
        tries += 1;
        let from = graph.centre(random.next(graph.polygon_count() as u32));
        let to = graph.centre(random.next(graph.polygon_count() as u32));
        let gap = from.distance(to);
        if gap < range.0 || gap >= range.1 {
            continue;
        }
        let (Some(start), Some(end)) =
            (graph.locate(from, Reach::CHARACTER), graph.locate(to, Reach::CHARACTER))
        else {
            continue;
        };
        if search.find(graph, start, end, &mut corridor).end == RouteEnd::Reached {
            found.push(Pair { from, to });
        }
    }
    found
}

fn queries(graph: &NavGraph, label: &str, set: &[Pair]) {
    if set.is_empty() {
        println!("  {label}: no such pairs");
        return;
    }
    let mut search = PathSearch::new(graph, u32::MAX);
    let mut corridor = [0; 1024];
    let mut corners = [Vec3::ZERO; 64];
    let mut least = vec![Duration::MAX; set.len()];
    let mut every = Vec::with_capacity(set.len() * ROUNDS);
    let mut expanded = vec![0; set.len()];
    let mut most_corners = 0;
    for _ in 0..ROUNDS {
        for (index, pair) in set.iter().enumerate() {
            let watch = Stopwatch::start();
            let start = graph.locate(pair.from, Reach::CHARACTER).expect("located before");
            let end = graph.locate(pair.to, Reach::CHARACTER).expect("located before");
            let route = search.find(graph, start, end, &mut corridor);
            let (count, _) = straighten(
                graph,
                &corridor[..route.polygons as usize],
                start.position,
                end.position,
                &mut corners,
            );
            let time = watch.elapsed();
            std::hint::black_box(count);
            least[index] = least[index].min(time);
            every.push(time);
            expanded[index] = route.expanded;
            most_corners = most_corners.max(count);
        }
    }
    expanded.sort_unstable();
    println!(
        "  {label}: {} pairs; expanded p50 {}, worst {}; up to {most_corners} corners",
        set.len(),
        expanded[expanded.len() / 2],
        expanded[expanded.len() - 1],
    );
    report("each query's least across rounds", least);
    report("every run", every);
}

fn chasers_step(mesh: &ArchivedNavMesh, walk: &[Vec3], count: usize, budget: u32) {
    const TICKS: usize = 1800;
    const CHASER_ROUNDS: usize = 5;
    let mut least = vec![Duration::MAX; TICKS];
    let mut worst_seen = Duration::ZERO;
    let mut stats = NavStats::default();
    for _ in 0..CHASER_ROUNDS {
        let mut chasers = Chasers::new(mesh, count, budget, walk[0], 7);
        for (tick, least) in least.iter_mut().enumerate() {
            let target = target_at(walk, tick as u32);
            let watch = Stopwatch::start();
            chasers.step(target);
            let time = watch.elapsed();
            *least = (*least).min(time);
            worst_seen = worst_seen.max(time);
        }
        stats = chasers.stats;
    }
    least.sort_unstable();
    println!(
        "  {count} chasers on {budget} nodes a tick ({} searches, {} nodes over {TICKS} ticks, worst tick {}, {} deferred): each tick's least p50 {:.1} µs, p99 {:.1}, worst {:.1}; worst seen {:.1}",
        stats.searches,
        stats.expanded,
        stats.worst_tick,
        stats.deferred,
        percentile(&least, 0.5),
        percentile(&least, 0.99),
        percentile(&least, 1.0),
        worst_seen.as_secs_f64() * 1e6,
    );
}

/// Pairs on different islands: the most a search spends, since it has to use up all it can reach.
fn no_way_pairs(graph: &NavGraph) -> Vec<Pair> {
    let mut random = Lcg(4);
    let mut search = PathSearch::new(graph, u32::MAX);
    let mut corridor = vec![0; 4096];
    let mut found = Vec::with_capacity(QUERIES);
    let mut tries = 0;
    while found.len() < QUERIES && tries < 400_000 {
        tries += 1;
        let from = graph.centre(random.next(graph.polygon_count() as u32));
        let to = graph.centre(random.next(graph.polygon_count() as u32));
        let (Some(start), Some(end)) =
            (graph.locate(from, Reach::CHARACTER), graph.locate(to, Reach::CHARACTER))
        else {
            continue;
        };
        let route = search.find(graph, start, end, &mut corridor);
        // Only those that spent a lot: from a roof's island there's little to use up.
        if route.end == RouteEnd::Unreachable && route.expanded > 100 {
            found.push(Pair { from, to });
        }
    }
    found
}

/// A zone far larger than anything here: squares 2 m on a side, in a grid, as polygons. What a node
/// costs once the pool outgrows the caches, and what a limit on them holds a search to.
fn large_zone(side: i32) -> AlignedVec {
    let vertex = |x: i32, z: i32| (x + z * (side + 1)) as u32;
    let mut mesh =
        NavMesh { cell_size: 2.0, cell_height: 1.0, polygon_starts: vec![0], ..NavMesh::default() };
    for z in 0..=side {
        for x in 0..=side {
            mesh.vertices.push([x, 0, z]);
        }
    }
    let across = |x: i32, z: i32| {
        if x >= 0 && z >= 0 && x < side && z < side { (x + z * side) as u32 } else { NO_NEIGHBOUR }
    };
    for z in 0..side {
        for x in 0..side {
            mesh.corners.extend([vertex(x, z), vertex(x, z + 1), vertex(x + 1, z + 1), vertex(x + 1, z)]);
            mesh.neighbours.extend([across(x - 1, z), across(x, z + 1), across(x + 1, z), across(x, z - 1)]);
            mesh.polygon_starts.push(mesh.corners.len() as u32);
        }
    }
    mesh.to_cooked().expect("it writes")
}

fn node_limits(side: i32) {
    let bytes = large_zone(side);
    let graph = NavGraph::new(NavMesh::read(&bytes).expect("it reads"));
    println!("a grid of {} polygons, {} bytes cooked:", graph.polygon_count(), bytes.len());
    let far = (side * 2) as f32 - 1.0;
    let mut corridor = vec![0; 8192];
    for limit in [256, 1024, 4096, u32::MAX] {
        let mut search = PathSearch::new(&graph, limit);
        let mut random = Lcg(9);
        let mut least = vec![Duration::MAX; 40];
        let mut expanded = [0; 40];
        let pairs: Vec<(Vec3, Vec3)> = (0..40)
            .map(|_| {
                let along = random.next(side as u32) as f32 * 2.0 + 1.0;
                (Vec3::new(1.0, 0.0, along), Vec3::new(far, 0.0, far - along + 1.0))
            })
            .collect();
        for _ in 0..7 {
            for (index, (from, to)) in pairs.iter().enumerate() {
                let start = graph.locate(*from, Reach::CHARACTER).expect("on the grid");
                let end = graph.locate(*to, Reach::CHARACTER).expect("on the grid");
                let watch = Stopwatch::start();
                let route = search.find(&graph, start, end, &mut corridor);
                let time = watch.elapsed();
                least[index] = least[index].min(time);
                expanded[index] = route.expanded;
            }
        }
        let nodes: u64 = expanded.iter().map(|&n| u64::from(n)).sum();
        let time: f64 = least.iter().map(Duration::as_secs_f64).sum();
        least.sort_unstable();
        println!(
            "  limit {limit}: search p50 {:.1} µs, worst {:.1}, {:.0} ns a node over {} nodes",
            percentile(&least, 0.5),
            percentile(&least, 1.0),
            time * 1e9 / nodes.max(1) as f64,
            nodes,
        );
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).expect("the path of a cooked .snav");
    let bytes = std::fs::read(&path).expect("it reads");
    let mut aligned: AlignedVec = AlignedVec::with_capacity(bytes.len());
    aligned.extend_from_slice(&bytes);
    let watch = Stopwatch::start();
    let mesh = NavMesh::read(&aligned).expect("a navmesh");
    if mesh.polygon_count() == 0 {
        return Err("the navigation benchmark needs a nonempty mesh".into());
    }
    let checked = watch.elapsed();
    let watch = Stopwatch::start();
    let graph = NavGraph::new(mesh);
    let built = watch.elapsed();
    println!(
        "{path}: {} polygons, {} bytes; checked in {:.0} µs, graph built in {:.0} µs",
        graph.polygon_count(),
        bytes.len(),
        checked.as_secs_f64() * 1e6,
        built.as_secs_f64() * 1e6,
    );

    let short = pairs(&graph, (1.0, 16.0), 1);
    let middle = pairs(&graph, (16.0, 64.0), 2);
    let long = pairs(&graph, (64.0, 1000.0), 3);
    if short.is_empty() {
        return Err("the navigation benchmark needs a connected pair 1 to 16 metres apart".into());
    }
    queries(&graph, "short, under 16 m", &short);
    queries(&graph, "middling, 16 to 64 m", &middle);
    queries(&graph, "crossing, over 64 m", &long);
    queries(&graph, "no way there, across islands", &no_way_pairs(&graph));

    let mut located = Vec::with_capacity(QUERIES);
    for pair in &short {
        let watch = Stopwatch::start();
        std::hint::black_box(graph.locate(pair.from, Reach::CHARACTER));
        located.push(watch.elapsed());
    }
    report("locate alone", located);

    // The longest pair there is, whose ends a search connects, so the chasers run a real distance.
    let walk: Vec<Vec3> = match (long.first(), middle.first()) {
        (Some(pair), _) | (None, Some(pair)) => vec![pair.from, pair.to],
        (None, None) => vec![short[0].from, short[0].to],
    };
    for count in [4, 16, 64] {
        chasers_step(mesh, &walk, count, NODES_PER_TICK);
    }
    chasers_step(mesh, &walk, 64, NODES_PER_TICK / 4);

    node_limits(100);
    node_limits(400);
    Ok(())
}
