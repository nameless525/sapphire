//! A corridor of polygons to the corners a character walks: the funnel over its edges.
//!
//! On the XZ plane, with heights carried from the corners it turns at. Characters' clearance is the
//! bake's erosion, so the funnel can run along a polygon's edges and hold the corners exactly.

use glam::Vec3;

use crate::graph::{NavGraph, cross_xz};

/// How a corner list ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunnelEnd {
    /// The last corner is the goal.
    Whole,
    /// The list filled first, so walk it and straighten again.
    Filled,
}

/// Writes the corners from `from` to `to` through `corridor` into `corners`, returning how many and
/// how it ended. The first is `from`, and `to` is last if it fits. Nothing here allocates.
pub fn straighten(
    graph: &NavGraph,
    corridor: &[u32],
    from: Vec3,
    to: Vec3,
    corners: &mut [Vec3],
) -> (u32, FunnelEnd) {
    let mut out = Corners { list: corners, written: 0 };
    if !out.add(from) {
        return (out.written, FunnelEnd::Filled);
    }
    let mut apex = from;
    let (mut left, mut right) = (from, from);
    let (mut left_at, mut right_at) = (0, 0);

    let mut index = 0;
    while index <= corridor.len().saturating_sub(1) {
        let (new_left, new_right) = if index + 1 < corridor.len() {
            portal(graph, corridor[index], corridor[index + 1])
        } else {
            (to, to)
        };

        // Does the right side tighten?
        if cross_xz(apex, right, new_right) <= 0.0 {
            if apex == right || cross_xz(apex, left, new_right) > 0.0 {
                right = new_right;
                right_at = index;
            } else {
                // Right crossed left, so left is a corner: start again from it.
                if !out.add(left) {
                    return (out.written, FunnelEnd::Filled);
                }
                apex = left;
                right = apex;
                right_at = left_at;
                index = left_at + 1;
                continue;
            }
        }
        // Does the left side tighten?
        if cross_xz(apex, left, new_left) >= 0.0 {
            if apex == left || cross_xz(apex, right, new_left) < 0.0 {
                left = new_left;
                left_at = index;
            } else {
                if !out.add(right) {
                    return (out.written, FunnelEnd::Filled);
                }
                apex = right;
                left = apex;
                left_at = right_at;
                index = right_at + 1;
                continue;
            }
        }
        index += 1;
    }
    if out.add(to) { (out.written, FunnelEnd::Whole) } else { (out.written, FunnelEnd::Filled) }
}

/// The edge from `from` into `into`, as (left, right) walking across it.
fn portal(graph: &NavGraph, from: u32, into: u32) -> (Vec3, Vec3) {
    for corner in graph.corner_range(from) {
        if graph.neighbour(corner) == Some(into) {
            return graph.edge(from, corner);
        }
    }
    // A corridor is a search's, and a search only crosses edges that exist.
    unreachable!("polygons {from} and {into} share no edge")
}

struct Corners<'a> {
    list: &'a mut [Vec3],
    written: u32,
}

impl Corners<'_> {
    /// Adds a corner unless it's the one before it. False if the list is full.
    fn add(&mut self, corner: Vec3) -> bool {
        if self.written > 0 && self.list[self.written as usize - 1] == corner {
            return true;
        }
        let Some(slot) = self.list.get_mut(self.written as usize) else { return false };
        *slot = corner;
        self.written += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{graph, squares};
    use crate::graph::{Location, Reach};
    use crate::search::{PathSearch, RouteEnd};

    fn route(bytes: &sapphire_serialize::cooked::AlignedVec, from: (f32, f32), to: (f32, f32)) -> Vec<Vec3> {
        let graph = graph(bytes);
        let mut search = PathSearch::new(&graph, 1024);
        let mut corridor = [0; 128];
        let at = |(x, z): (f32, f32)| -> Location {
            graph.locate(Vec3::new(x, 0.0, z), Reach::CHARACTER).expect("on the mesh")
        };
        let (from, to) = (at(from), at(to));
        let found = search.find(&graph, from, to, &mut corridor);
        assert_eq!(found.end, RouteEnd::Reached);
        let mut corners = [Vec3::ZERO; 32];
        let (count, end) = straighten(
            &graph,
            &corridor[..found.polygons as usize],
            from.position,
            to.position,
            &mut corners,
        );
        assert_eq!(end, FunnelEnd::Whole);
        corners[..count as usize].to_vec()
    }

    #[test]
    fn nothing_in_the_way_is_a_straight_line() {
        let bytes = squares(8, 8, &[]);
        let corners = route(&bytes, (0.5, 0.5), (7.5, 6.5));
        assert_eq!(corners, [Vec3::new(0.5, 0.0, 0.5), Vec3::new(7.5, 0.0, 6.5)]);
    }

    #[test]
    fn a_wall_turns_the_path_at_its_end_and_nowhere_else() {
        // A wall along z = 1..2 from x = 0 to x = 4, whichever side of it the funnel turns.
        let bytes = squares(5, 3, &[(0, 1), (1, 1), (2, 1), (3, 1)]);
        let corners = route(&bytes, (0.5, 0.5), (0.5, 2.5));
        assert_eq!(
            corners,
            [
                Vec3::new(0.5, 0.0, 0.5),
                Vec3::new(4.0, 0.0, 1.0),
                Vec3::new(4.0, 0.0, 2.0),
                Vec3::new(0.5, 0.0, 2.5)
            ],
        );
    }

    #[test]
    fn a_turn_the_other_way_is_the_same_turn() {
        let bytes = squares(5, 3, &[(0, 1), (1, 1), (2, 1), (3, 1)]);
        let mut corners = route(&bytes, (0.5, 2.5), (0.5, 0.5));
        corners.reverse();
        assert_eq!(corners, route(&bytes, (0.5, 0.5), (0.5, 2.5)));
    }

    #[test]
    fn one_polygon_is_its_two_ends() {
        let bytes = squares(2, 2, &[]);
        let corners = route(&bytes, (0.2, 0.2), (0.8, 0.7));
        assert_eq!(corners, [Vec3::new(0.2, 0.0, 0.2), Vec3::new(0.8, 0.0, 0.7)]);
    }

    #[test]
    fn a_short_list_says_it_filled_and_starts_at_the_start() {
        let bytes = squares(5, 3, &[(0, 1), (1, 1), (2, 1), (3, 1)]);
        let graph = graph(&bytes);
        let mut search = PathSearch::new(&graph, 1024);
        let mut corridor = [0; 32];
        let at = |x, z| graph.locate(Vec3::new(x, 0.0, z), Reach::CHARACTER).expect("on the mesh");
        let (from, to) = (at(0.5, 0.5), at(0.5, 2.5));
        let found = search.find(&graph, from, to, &mut corridor);
        let mut corners = [Vec3::ZERO; 2];
        let (count, end) = straighten(
            &graph,
            &corridor[..found.polygons as usize],
            from.position,
            to.position,
            &mut corners,
        );
        assert_eq!(end, FunnelEnd::Filled);
        assert_eq!(count, 2);
        assert_eq!(corners, [Vec3::new(0.5, 0.0, 0.5), Vec3::new(4.0, 0.0, 1.0)]);
    }
}
