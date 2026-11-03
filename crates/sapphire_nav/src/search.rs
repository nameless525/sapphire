//! A* over polygons, on a pool sized once at load.

use glam::Vec3;

use crate::graph::{Location, NavGraph};

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
    /// Cost from the start to where the walk enters each polygon.
    costs: Vec<f32>,
    /// Cost plus the distance left: what the open set orders by.
    totals: Vec<f32>,
    parents: Vec<u32>,
    /// Where the walk enters each polygon: the start, or the middle of the edge it crossed.
    entries: Vec<Vec3>,
    /// The open set, a binary heap of polygon IDs by total then ID, which makes every tie one way.
    heap: Vec<u32>,
    heap_len: usize,
    /// Where each polygon is in the heap, so a shorter way in can raise it.
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
        let start_left = from.position.distance(to.position);
        self.totals[start] = start_left;
        self.push(from.polygon);

        // What a search that doesn't arrive walks towards: the polygon it reached nearest the goal.
        let mut nearest = (start_left, from.polygon);
        let mut expanded = 0;
        let mut end = RouteEnd::Unreachable;
        while self.heap_len > 0 {
            let polygon = self.pop();
            if polygon == to.polygon {
                nearest.1 = polygon;
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
                let walked = cost + entry.distance(middle);
                let seen = self.stamps[index] == self.generation;
                if seen && walked >= self.costs[index] {
                    continue;
                }
                let left = middle.distance(to.position);
                let total = walked + left;
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
                    self.raise(self.heap_positions[index] as usize);
                }
                if left < nearest.0 || (left == nearest.0 && neighbour < nearest.1) {
                    nearest = (left, neighbour);
                }
            }
        }

        self.stats.searches += 1;
        self.stats.expanded += u64::from(expanded);
        self.stats.worst_expanded = self.stats.worst_expanded.max(expanded);
        if end == RouteEnd::OutOfNodes {
            self.stats.out_of_nodes += 1;
        }
        Route { end, polygons: self.write_corridor(nearest.1, corridor), expanded }
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
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamps.fill(0);
            self.generation = 1;
        }
    }

    fn touch(&mut self, polygon: usize) {
        self.stamps[polygon] = self.generation;
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

    fn pop(&mut self) -> u32 {
        let top = self.heap[0];
        self.heap_positions[top as usize] = NOT_OPEN;
        self.heap_len -= 1;
        if self.heap_len > 0 {
            let moved = self.heap[self.heap_len];
            let mut slot = 0;
            loop {
                let (left, right) = (slot * 2 + 1, slot * 2 + 2);
                if left >= self.heap_len {
                    break;
                }
                let mut child = left;
                if right < self.heap_len && self.before(self.heap[right], self.heap[left]) {
                    child = right;
                }
                if !self.before(self.heap[child], moved) {
                    break;
                }
                self.heap[slot] = self.heap[child];
                self.heap_positions[self.heap[slot] as usize] = slot as u32;
                slot = child;
            }
            self.heap[slot] = moved;
            self.heap_positions[moved as usize] = slot as u32;
        }
        top
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Reach;
    use crate::graph::tests::{graph, squares};

    fn at(graph: &NavGraph, x: f32, z: f32) -> Location {
        graph.locate(Vec3::new(x, 0.0, z), Reach::CHARACTER).expect("on the mesh")
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
