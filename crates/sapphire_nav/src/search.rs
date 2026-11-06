//! A* over polygons, on a pool sized once at load.

use glam::Vec3;

use crate::graph::{Location, NavGraph, add_distances, distance};

/// In the parent list: where a search began.
const NO_PARENT: u32 = u32::MAX;
/// In the heap's positions: not in the open set.
const NOT_OPEN: u32 = u32::MAX;

/// How a search ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteEnd {
    /// The corridor reaches the goal's polygon.
    Reached,
    /// Nothing walkable connects them. The corridor goes as near the goal as there's a way to.
    Unreachable,
    /// The search spent its nodes first. The corridor goes as near the goal as it had looked, and a
    /// later search from where the character is carries on.
    OutOfNodes,
}

/// What a search found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub end: RouteEnd,
    /// How many polygons it wrote to the corridor. Starts at the first polygon, and stops at the
    /// corridor's length if a route is longer: a character walks that far and searches again.
    pub polygons: u32,
    /// How many polygons it expanded, which is what it cost.
    pub expanded: u32,
}

/// What searches have cost since the last [`PathSearch::take_stats`], for the stat counters (R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchStats {
    pub searches: u32,
    pub expanded: u64,
    pub worst_expanded: u32,
    pub out_of_nodes: u32,
}

/// One search's working memory, reused for every search in a zone. A node is a polygon, so the pool
/// holds one per polygon, and a generation stamp clears it in no time at all.
#[derive(Clone, Debug)]
pub struct PathSearch {
    /// The most polygons one search may expand.
    node_limit: u32,
    generation: u32,
    /// Which search last touched each polygon.
    stamps: Vec<u32>,
    /// Nodes whose current entry can be the fallback destination, in discovery order. Bounded by
    /// the pool and reused, so choosing the nearest current entry costs only work this search did.
    touched: Vec<u32>,
    /// Cost from the start to where the walk enters each polygon.
    costs: Vec<f64>,
    /// Cost plus the distance left: what the open set orders by.
    totals: Vec<f64>,
    parents: Vec<u32>,
    /// Where the walk enters each polygon: the start, or the middle of the edge it crossed.
    entries: Vec<Vec3>,
    /// The open set, a binary heap of polygon IDs by total then ID, which makes every tie one way.
    heap: Vec<u32>,
    heap_len: usize,
    /// Where each polygon is in the heap, so a changed entry can restore its order.
    heap_positions: Vec<u32>,
    stats: SearchStats,
}

impl PathSearch {
    /// A pool for `graph`, where one search may expand `node_limit` polygons. The only allocation
    /// a path ever makes.
    pub fn new(graph: &NavGraph, node_limit: u32) -> Self {
        let polygons = graph.polygon_count();
        Self {
            node_limit,
            generation: 0,
            stamps: vec![0; polygons],
            touched: Vec::with_capacity(polygons),
            costs: vec![0.0; polygons],
            totals: vec![0.0; polygons],
            parents: vec![NO_PARENT; polygons],
            entries: vec![Vec3::ZERO; polygons],
            heap: vec![0; polygons],
            heap_len: 0,
            heap_positions: vec![NOT_OPEN; polygons],
            stats: SearchStats::default(),
        }
    }

    /// What searches have cost since the last call.
    pub fn take_stats(&mut self) -> SearchStats {
        std::mem::take(&mut self.stats)
    }

    /// Finds a corridor of polygons from `from` to `to`, writing it to `corridor`.
    pub fn find(&mut self, graph: &NavGraph, from: Location, to: Location, corridor: &mut [u32]) -> Route {
        self.find_within(graph, from, to, corridor, self.node_limit)
    }

    /// [`Self::find`], expanding at most `node_limit` polygons: what's left of a tick's budget, say.
    pub fn find_within(
        &mut self,
        graph: &NavGraph,
        from: Location,
        to: Location,
        corridor: &mut [u32],
        node_limit: u32,
    ) -> Route {
        self.begin();
        let start = from.polygon as usize;
        self.touch(start);
        self.costs[start] = 0.0;
        self.entries[start] = from.position;
        self.parents[start] = NO_PARENT;
        let start_left = distance(from.position, to.position);
        self.totals[start] = start_left;
        self.push(from.polygon);

        let mut reached = false;
        let mut expanded = 0;
        let mut end = RouteEnd::Unreachable;
        while self.heap_len > 0 {
            let polygon = self.pop();
            if polygon == to.polygon {
                reached = true;
                end = RouteEnd::Reached;
                break;
            }
            if expanded == node_limit {
                end = RouteEnd::OutOfNodes;
                break;
            }
            expanded += 1;

            let cost = self.costs[polygon as usize];
            let entry = self.entries[polygon as usize];
            for corner in graph.corner_range(polygon) {
                let Some(neighbour) = graph.neighbour(corner) else { continue };
                let index = neighbour as usize;
                let middle = graph.middle(corner);
                let walked = add_distances(cost, distance(entry, middle));
                let seen = self.stamps[index] == self.generation;
                if seen && walked >= self.costs[index] {
                    continue;
                }
                let left = distance(middle, to.position);
                let total = add_distances(walked, left);
                if !seen {
                    self.touch(index);
                }
                self.costs[index] = walked;
                self.totals[index] = total;
                self.parents[index] = polygon;
                self.entries[index] = middle;
                if self.heap_positions[index] == NOT_OPEN {
                    self.push(neighbour);
                } else {
                    self.reorder(self.heap_positions[index] as usize);
                }
            }
        }

        self.stats.searches += 1;
        self.stats.expanded += u64::from(expanded);
        self.stats.worst_expanded = self.stats.worst_expanded.max(expanded);
        if end == RouteEnd::OutOfNodes {
            self.stats.out_of_nodes += 1;
        }
        // An improved cost can enter a polygon through a different portal, farther from the goal.
        // Judge its current entry rather than retaining the obsolete portal's heuristic.
        let last = if reached {
            to.polygon
        } else {
            self.touched
                .iter()
                .copied()
                .min_by(|&a, &b| {
                    distance(self.entries[a as usize], to.position)
                        .total_cmp(&distance(self.entries[b as usize], to.position))
                        .then(a.cmp(&b))
                })
                .expect("the start was touched")
        };
        Route { end, polygons: self.write_corridor(last, corridor), expanded }
    }

    /// Walks the parents back from `last` and writes the route's first polygons, in walking order.
    fn write_corridor(&self, last: u32, corridor: &mut [u32]) -> u32 {
        let mut length = 0usize;
        let mut at = last;
        loop {
            length += 1;
            let parent = self.parents[at as usize];
            if parent == NO_PARENT {
                break;
            }
            at = parent;
        }
        let kept = length.min(corridor.len());
        let mut index = length;
        let mut at = last;
        loop {
            index -= 1;
            if index < kept {
                corridor[index] = at;
            }
            let parent = self.parents[at as usize];
            if parent == NO_PARENT {
                break;
            }
            at = parent;
        }
        kept as u32
    }

    fn begin(&mut self) {
        // Everything still open from a search that stopped early leaves the heap's positions.
        for slot in 0..self.heap_len {
            self.heap_positions[self.heap[slot] as usize] = NOT_OPEN;
        }
        self.heap_len = 0;
        self.touched.clear();
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamps.fill(0);
            self.generation = 1;
        }
    }

    fn touch(&mut self, polygon: usize) {
        self.stamps[polygon] = self.generation;
        self.touched.push(polygon as u32);
    }

    /// Whether polygon `a` comes out of the heap before polygon `b`.
    fn before(&self, a: u32, b: u32) -> bool {
        let (left, right) = (self.totals[a as usize], self.totals[b as usize]);
        left < right || (left == right && a < b)
    }

    fn push(&mut self, polygon: u32) {
        let slot = self.heap_len;
        self.heap[slot] = polygon;
        self.heap_positions[polygon as usize] = slot as u32;
        self.heap_len += 1;
        self.raise(slot);
    }

    fn raise(&mut self, mut slot: usize) {
        let polygon = self.heap[slot];
        while slot > 0 {
            let parent = (slot - 1) / 2;
            let above = self.heap[parent];
            if !self.before(polygon, above) {
                break;
            }
            self.heap[slot] = above;
            self.heap_positions[above as usize] = slot as u32;
            slot = parent;
        }
        self.heap[slot] = polygon;
        self.heap_positions[polygon as usize] = slot as u32;
    }

    fn reorder(&mut self, slot: usize) {
        if slot > 0 && self.before(self.heap[slot], self.heap[(slot - 1) / 2]) {
            self.raise(slot);
        } else {
            self.lower(slot);
        }
    }

    fn lower(&mut self, mut slot: usize) {
        let polygon = self.heap[slot];
        loop {
            let (left, right) = (slot * 2 + 1, slot * 2 + 2);
            if left >= self.heap_len {
                break;
            }
            let mut child = left;
            if right < self.heap_len && self.before(self.heap[right], self.heap[left]) {
                child = right;
            }
            if !self.before(self.heap[child], polygon) {
                break;
            }
            self.heap[slot] = self.heap[child];
            self.heap_positions[self.heap[slot] as usize] = slot as u32;
            slot = child;
        }
        self.heap[slot] = polygon;
        self.heap_positions[polygon as usize] = slot as u32;
    }

    fn pop(&mut self) -> u32 {
        let top = self.heap[0];
        self.heap_positions[top as usize] = NOT_OPEN;
        self.heap_len -= 1;
        if self.heap_len > 0 {
            self.heap[0] = self.heap[self.heap_len];
            self.lower(0);
        }
        top
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Reach;
    use crate::graph::tests::{graph, scaled_squares, squares};

    fn at(graph: &NavGraph, x: f32, z: f32) -> Location {
        graph.locate(Vec3::new(x, 0.0, z), Reach::CHARACTER).expect("on the mesh")
    }

    fn irregular_graph() -> NavGraph {
        use sapphire_navmesh::{NO_NEIGHBOUR, NavMesh};
        let xs = [0, 9, 20, 41, 70];
        let zs = [0, 16, 27, 48, 67];
        let mut mesh =
            NavMesh { cell_size: 1.0, cell_height: 1.0, polygon_starts: vec![0], ..NavMesh::default() };
        for z in zs {
            for x in xs {
                mesh.vertices.push([x, 0, z]);
            }
        }
        let mut ids = [NO_NEIGHBOUR; 16];
        let mut count = 0;
        for (index, id) in ids.iter_mut().enumerate() {
            if ![6, 12].contains(&index) {
                *id = count;
                count += 1;
            }
        }
        for z in 0..4i32 {
            for x in 0..4i32 {
                if ids[(x + z * 4) as usize] == NO_NEIGHBOUR {
                    continue;
                }
                let vertex = |x, z| (x + z * 5) as u32;
                let neighbour = |x, z| {
                    if (0..4).contains(&x) && (0..4).contains(&z) {
                        ids[(x + z * 4) as usize]
                    } else {
                        NO_NEIGHBOUR
                    }
                };
                mesh.corners.extend([vertex(x, z), vertex(x, z + 1), vertex(x + 1, z + 1), vertex(x + 1, z)]);
                mesh.neighbours.extend([
                    neighbour(x - 1, z),
                    neighbour(x, z + 1),
                    neighbour(x + 1, z),
                    neighbour(x, z - 1),
                ]);
                mesh.polygon_starts.push(mesh.corners.len() as u32);
            }
        }
        graph(&mesh.to_cooked().expect("write"))
    }

    #[test]
    fn a_multi_portal_route_keeps_costs_beyond_f32_max() {
        let scale = 1e38_f32;
        let graph = graph(&scaled_squares(3, 3, &[(0, 1), (1, 1)], scale));
        let mut search = PathSearch::new(&graph, 16);
        let mut corridor = [0; 8];
        let route = search.find(
            &graph,
            Location { polygon: 0, position: graph.centre(0) },
            Location { polygon: 4, position: graph.centre(4) },
            &mut corridor,
        );
        assert_eq!(route.end, RouteEnd::Reached);
        assert_eq!(&corridor[..route.polygons as usize], &[0, 1, 2, 3, 6, 5, 4]);
        let cost = search.costs[4];
        assert!(cost.is_finite() && cost > f64::from(f32::MAX));
        // Portal-centre walk: 0.5 + 1 + sqrt(0.5) + 1 + sqrt(0.5) + 1 cells.
        // The tolerance covers the represented f32 scale/portal coordinates, not f32 cost overflow.
        let expected = 4.914_213_562_373_095 * f64::from(scale);
        assert!((cost / expected - 1.0).abs() < 1e-6, "{cost}, expected {expected}");
    }

    #[test]
    fn a_wide_strip_walks_toward_the_nearest_discovered_polygon() {
        use sapphire_navmesh::{NO_NEIGHBOUR, NavMesh};
        let mut mesh =
            NavMesh { cell_size: 5e19, cell_height: 1.0, polygon_starts: vec![0], ..NavMesh::default() };
        for x in 0..4 {
            let first = mesh.vertices.len() as u32;
            mesh.vertices.extend([[x, 0, 0], [x, 0, 1], [x + 1, 0, 1], [x + 1, 0, 0]]);
            mesh.corners.extend(first..first + 4);
            mesh.neighbours.extend([
                if x > 0 { x as u32 - 1 } else { NO_NEIGHBOUR },
                NO_NEIGHBOUR,
                if x < 3 { x as u32 + 1 } else { NO_NEIGHBOUR },
                NO_NEIGHBOUR,
            ]);
            mesh.polygon_starts.push(mesh.corners.len() as u32);
        }
        let graph = graph(&mesh.to_cooked().expect("write"));
        let mut search = PathSearch::new(&graph, 16);
        let mut corridor = [0; 8];
        let route = search.find_within(
            &graph,
            Location { polygon: 0, position: Vec3::new(2.5e19, 0.0, 2.5e19) },
            Location { polygon: 3, position: Vec3::new(1.75e20, 0.0, 2.5e19) },
            &mut corridor,
            1,
        );
        assert_eq!(route.end, RouteEnd::OutOfNodes);
        assert_eq!(&corridor[..route.polygons as usize], &[0, 1]);
        assert!(search.costs.iter().chain(&search.totals).all(|cost| cost.is_finite()));
    }

    #[test]
    fn irregular_mesh_searches_keep_current_heap_and_fallback_invariants() {
        let graph = irregular_graph();
        let mut search = PathSearch::new(&graph, 64);
        let mut corridor = [0; 32];
        for from in 0..graph.polygon_count() as u32 {
            for to in 0..graph.polygon_count() as u32 {
                for budget in 0..graph.polygon_count() as u32 {
                    let target = Location { polygon: to, position: graph.centre(to) };
                    let route = search.find_within(
                        &graph,
                        Location { polygon: from, position: graph.centre(from) },
                        target,
                        &mut corridor,
                        budget,
                    );
                    for slot in 1..search.heap_len {
                        assert!(
                            !search.before(search.heap[slot], search.heap[(slot - 1) / 2]),
                            "{from} to {to}, budget {budget}"
                        );
                    }
                    if route.end == RouteEnd::Reached {
                        continue;
                    }
                    let last = corridor[route.polygons as usize - 1];
                    let distance = search.entries[last as usize].distance(target.position);
                    for (id, &stamp) in search.stamps.iter().enumerate() {
                        if stamp != search.generation {
                            continue;
                        }
                        let other = search.entries[id].distance(target.position);
                        assert!(
                            distance < other || (distance == other && last <= id as u32),
                            "{from} to {to}, budget {budget}, fallback {last} farther than {id}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_changed_portal_can_raise_the_key_while_lowering_the_walked_cost() {
        let graph = graph(&squares(7, 1, &[]));
        let mut search = PathSearch::new(&graph, 64);
        search.begin();
        for polygon in 0..7 {
            search.totals[polygon] = polygon as f64;
            search.push(polygon as u32);
        }
        // A cheaper path through a farther portal may increase g + h. Heap updates must work
        // both ways, including when the changed key has children and when it becomes the root.
        search.totals[1] = 10.0;
        search.reorder(search.heap_positions[1] as usize);
        search.totals[6] = -1.0;
        search.reorder(search.heap_positions[6] as usize);
        let order: Vec<_> = (0..7).map(|_| search.pop()).collect();
        assert_eq!(order, [6, 0, 2, 3, 4, 5, 1]);
    }

    #[test]
    fn a_route_goes_round_a_wall() {
        // A wall across the middle row, but for its last square.
        let bytes = squares(5, 3, &[(0, 1), (1, 1), (2, 1), (3, 1)]);
        let graph = graph(&bytes);
        let mut search = PathSearch::new(&graph, 64);
        let mut corridor = [0; 32];
        let route = search.find(&graph, at(&graph, 0.5, 0.5), at(&graph, 0.5, 2.5), &mut corridor);
        assert_eq!(route.end, RouteEnd::Reached);
        assert_eq!(route.polygons, 11, "five across, one through the gap, five back: {corridor:?}");
    }

    #[test]
    fn a_goal_with_no_way_to_it_walks_as_near_as_there_is() {
        let bytes = squares(3, 3, &[(0, 1), (1, 1), (2, 1)]);
        let graph = graph(&bytes);
        let mut search = PathSearch::new(&graph, 64);
        let mut corridor = [0; 32];
        let route = search.find(&graph, at(&graph, 0.5, 0.5), at(&graph, 0.5, 2.5), &mut corridor);
        assert_eq!(route.end, RouteEnd::Unreachable);
        let last = corridor[route.polygons as usize - 1];
        assert_eq!(last, at(&graph, 0.5, 0.5).polygon, "the nearest it can get is straight below");
    }

    #[test]
    fn a_search_out_of_nodes_says_so_and_the_next_starts_clean() {
        let bytes = squares(40, 1, &[]);
        let graph = graph(&bytes);
        let mut search = PathSearch::new(&graph, 8);
        let mut corridor = [0; 64];
        let (from, to) = (at(&graph, 0.5, 0.5), at(&graph, 39.5, 0.5));
        let cut = search.find(&graph, from, to, &mut corridor);
        assert_eq!(cut.end, RouteEnd::OutOfNodes);
        assert_eq!(cut.expanded, 8);
        assert_eq!(corridor[cut.polygons as usize - 1], 8, "as far as it looked");

        let near = at(&graph, 4.5, 0.5);
        let whole = search.find(&graph, from, near, &mut corridor);
        assert_eq!(whole.end, RouteEnd::Reached, "nothing left over from the search before");
        assert_eq!(&corridor[..whole.polygons as usize], &[0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_short_corridor_keeps_the_start_of_the_route() {
        let bytes = squares(10, 1, &[]);
        let graph = graph(&bytes);
        let mut search = PathSearch::new(&graph, 64);
        let mut corridor = [0; 4];
        let route = search.find(&graph, at(&graph, 0.5, 0.5), at(&graph, 9.5, 0.5), &mut corridor);
        assert_eq!(route.end, RouteEnd::Reached);
        assert_eq!(route.polygons, 4);
        assert_eq!(corridor, [0, 1, 2, 3]);
    }

    /// Every way of winning a tie must be the same one: an open field is nothing but ties.
    #[test]
    fn ties_break_one_way_however_often_it_runs() {
        let bytes = squares(12, 12, &[(5, 5), (6, 6)]);
        let graph = graph(&bytes);
        let mut search = PathSearch::new(&graph, 1024);
        let (mut first, mut again) = ([0; 64], [0; 64]);
        let route = search.find(&graph, at(&graph, 0.5, 0.5), at(&graph, 11.5, 11.5), &mut first);
        for _ in 0..3 {
            let other = search.find(&graph, at(&graph, 0.5, 0.5), at(&graph, 11.5, 11.5), &mut again);
            assert_eq!(route, other);
            assert_eq!(first, again);
        }
    }

    #[test]
    fn the_counters_read_what_searches_cost() {
        let bytes = squares(6, 1, &[]);
        let graph = graph(&bytes);
        let mut search = PathSearch::new(&graph, 64);
        let mut corridor = [0; 8];
        let route = search.find(&graph, at(&graph, 0.5, 0.5), at(&graph, 5.5, 0.5), &mut corridor);
        let stats = search.take_stats();
        assert_eq!(stats.searches, 1);
        assert_eq!(stats.expanded, u64::from(route.expanded));
        assert_eq!(stats.worst_expanded, route.expanded);
        assert_eq!(search.take_stats(), SearchStats::default(), "taking them clears them");
    }
}
