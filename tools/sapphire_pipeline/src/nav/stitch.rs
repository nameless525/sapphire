//! Joins tiles into one graph across their seams.
//!
//! `rerecast` has no stitching, and it simplifies each tile's contours on its own, so two tiles'
//! edges along a seam lie on the same line but needn't end at the same places, and a seam can hold
//! two floors. So each pair of edges that overlap, at heights within a climb, claims their overlap,
//! and each edge is cut where its claims end. A cut adds a corner in line with its neighbours, which
//! a convex polygon can take, and after that every shared edge has exactly one polygon on each side:
//! the runtime has no links and no partial edges to know about.

use std::collections::{BTreeMap, BTreeSet};

use sapphire_navmesh::{MAX_CORNERS, NO_NEIGHBOUR, NavMesh};

use super::NavBakeError;
use super::bake::{Across, BakeSettings, ROWS, Seam, Tile};

/// What a stitch found along its seams, in edges and in cells of length.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeamReport {
    /// Polygon edges on a tile's side.
    pub seam_edges: usize,
    /// Claims whose two edges had the same ends, at the same heights.
    pub exact: usize,
    /// Claims whose two edges had the same ends on the XZ plane, at heights within a climb.
    pub same_ends_other_height: usize,
    /// Claims of only part of one edge or the other, which had to cut.
    pub partial: usize,
    /// Corners added.
    pub cuts: usize,
    /// Length of all seam edges, on both sides, and how much of it found the other side.
    pub seam_cells: u64,
    pub joined_cells: u64,
}

/// A polygon edge on a seam, along the seam's own axis.
#[derive(Clone, Copy, Debug)]
struct SeamEdge {
    polygon: u32,
    corner: u32,
    low: i32,
    high: i32,
    low_y: i32,
    high_y: i32,
}

impl SeamEdge {
    /// Height at a place along the edge, in sixteenths of a cell, so no rounding hides a difference.
    fn height_at(&self, along: i32) -> i64 {
        let span = i64::from(self.high - self.low);
        let run = i64::from(along - self.low);
        i64::from(self.low_y) * 16 + i64::from(self.high_y - self.low_y) * 16 * run / span
    }
}

/// Part of an edge joined to a polygon on the other side.
#[derive(Clone, Copy, Debug)]
struct Claim {
    low: i32,
    high: i32,
    other: u32,
}

/// One flat graph from `tiles`, with cell heights counted from the rows' start.
pub fn stitch(tiles: &[Tile], settings: &BakeSettings) -> Result<(NavMesh, SeamReport), NavBakeError> {
    let mut report = SeamReport::default();
    let mut firsts = Vec::with_capacity(tiles.len());
    let mut total = 0u32;
    for tile in tiles {
        firsts.push(total);
        total += tile.polygons.len() as u32;
    }

    // Every seam edge, by the line it lies on: (axis, where), then the tile below the line and the
    // tile above it. BTreeMaps, so the order is the geometry's.
    let mut lines: BTreeMap<(u8, i32), [Vec<SeamEdge>; 2]> = BTreeMap::new();
    for (tile, &first) in tiles.iter().zip(&firsts) {
        for (index, polygon) in tile.polygons.iter().enumerate() {
            let count = polygon.corners.len();
            for corner in 0..count {
                let Across::Seam(seam) = polygon.across[corner] else { continue };
                let (a, b) = (polygon.corners[corner], polygon.corners[(corner + 1) % count]);
                let (axis, where_, side, along) = match seam {
                    Seam::LowX => (0, a[0], 1, 2),
                    Seam::HighX => (0, a[0], 0, 2),
                    Seam::LowZ => (1, a[2], 1, 0),
                    Seam::HighZ => (1, a[2], 0, 0),
                };
                let (low, high, low_y, high_y) = if a[along] < b[along] {
                    (a[along], b[along], a[1], b[1])
                } else {
                    (b[along], a[along], b[1], a[1])
                };
                report.seam_edges += 1;
                report.seam_cells += (high - low) as u64;
                lines.entry((axis, where_)).or_default()[side].push(SeamEdge {
                    polygon: first + index as u32,
                    corner: corner as u32,
                    low,
                    high,
                    low_y,
                    high_y,
                });
            }
        }
    }

    // Claims, closest in height first, so a seam with two floors joins each to its own.
    let climb = i64::from(settings.walkable_climb()) * 16;
    let mut claims: BTreeMap<(u32, u32), Vec<Claim>> = BTreeMap::new();
    for [below, above] in lines.values() {
        let mut candidates = Vec::new();
        for (b, low_edge) in below.iter().enumerate() {
            for (a, high_edge) in above.iter().enumerate() {
                let (low, high) = (low_edge.low.max(high_edge.low), low_edge.high.min(high_edge.high));
                if low >= high {
                    continue;
                }
                let gap = |at: i32| (low_edge.height_at(at) - high_edge.height_at(at)).abs();
                let worst = gap(low).max(gap(high));
                if worst <= climb {
                    candidates.push((worst, b, a, low, high));
                }
            }
        }
        candidates.sort_unstable();
        for (_, b, a, low, high) in candidates {
            let (low_edge, high_edge) = (below[b], above[a]);
            let free = |key: (u32, u32), claims: &BTreeMap<(u32, u32), Vec<Claim>>| {
                claims.get(&key).is_none_or(|held| held.iter().all(|c| c.high <= low || c.low >= high))
            };
            let (b_key, a_key) = ((low_edge.polygon, low_edge.corner), (high_edge.polygon, high_edge.corner));
            if !free(b_key, &claims) || !free(a_key, &claims) {
                continue;
            }
            claims.entry(b_key).or_default().push(Claim { low, high, other: high_edge.polygon });
            claims.entry(a_key).or_default().push(Claim { low, high, other: low_edge.polygon });
            report.joined_cells += 2 * (high - low) as u64;
            let same_ends = low_edge.low == high_edge.low && low_edge.high == high_edge.high;
            if !same_ends {
                report.partial += 1;
            } else if low_edge.low_y == high_edge.low_y && low_edge.high_y == high_edge.high_y {
                report.exact += 1;
            } else {
                report.same_ends_other_height += 1;
            }
        }
    }

    // The flat lists: polygons in tile order, each edge cut where its claims end.
    let mut vertex_ids: BTreeMap<[i32; 3], u32> = BTreeMap::new();
    let mut mesh = NavMesh {
        origin: [0.0, ROWS.0, 0.0],
        cell_size: settings.cell_size,
        cell_height: settings.cell_height,
        polygon_starts: vec![0],
        ..NavMesh::default()
    };
    let mut add = |corner: [i32; 3], neighbour: u32, mesh: &mut NavMesh| {
        let next = vertex_ids.len() as u32;
        let id = *vertex_ids.entry(corner).or_insert(next);
        if id == next {
            mesh.vertices.push(corner);
        }
        mesh.corners.push(id);
        mesh.neighbours.push(neighbour);
    };
    for (tile, &first) in tiles.iter().zip(&firsts) {
        for (index, polygon) in tile.polygons.iter().enumerate() {
            let id = first + index as u32;
            let count = polygon.corners.len();
            let begun = mesh.corners.len();
            for corner in 0..count {
                let (a, b) = (polygon.corners[corner], polygon.corners[(corner + 1) % count]);
                match polygon.across[corner] {
                    Across::Wall => add(a, NO_NEIGHBOUR, &mut mesh),
                    Across::Polygon(other) => add(a, first + other, &mut mesh),
                    Across::Seam(seam) => {
                        let held = claims.get(&(id, corner as u32)).map_or(&[][..], Vec::as_slice);
                        report.cuts += add_cut_edge(a, b, seam, held, &mut |at, neighbour| {
                            add(at, neighbour, &mut mesh);
                        });
                    }
                }
            }
            let corners = mesh.corners.len() - begun;
            if corners > MAX_CORNERS {
                return Err(NavBakeError::TooManyCorners { corners });
            }
            mesh.polygon_starts.push(mesh.corners.len() as u32);
        }
    }
    Ok((mesh, report))
}

/// Writes the edge from `a` towards `b` as its pieces: each piece's first corner and what's across
/// it. Returns how many corners that added.
fn add_cut_edge(
    a: [i32; 3],
    b: [i32; 3],
    seam: Seam,
    claims: &[Claim],
    add: &mut dyn FnMut([i32; 3], u32),
) -> usize {
    let along = match seam {
        Seam::LowX | Seam::HighX => 2,
        Seam::LowZ | Seam::HighZ => 0,
    };
    let forward = a[along] < b[along];
    let (low, high) = if forward { (a[along], b[along]) } else { (b[along], a[along]) };
    let mut cuts: BTreeSet<i32> = BTreeSet::new();
    for claim in claims {
        cuts.insert(claim.low);
        cuts.insert(claim.high);
    }
    cuts.retain(|&cut| cut > low && cut < high);
    let added = cuts.len();

    let mut stops: Vec<i32> = Vec::with_capacity(added + 2);
    stops.push(low);
    stops.extend(cuts);
    stops.push(high);
    let walk: Box<dyn Iterator<Item = &[i32]>> =
        if forward { Box::new(stops.windows(2)) } else { Box::new(stops.windows(2).rev()) };
    for piece in walk {
        let (piece_low, piece_high) = (piece[0], piece[1]);
        let first_along = if forward { piece_low } else { piece_high };
        let mut corner = a;
        corner[along] = first_along;
        // In line with the edge, and at its height there to the nearest cell.
        let run = i64::from(first_along - a[along]);
        let span = i64::from(b[along] - a[along]);
        corner[1] = a[1] + (i64::from(b[1] - a[1]) * run / span) as i32;
        let across = claims
            .iter()
            .find(|claim| claim.low <= piece_low && piece_high <= claim.high)
            .map_or(NO_NEIGHBOUR, |claim| claim.other);
        add(corner, across);
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::bake::{Tile, TilePolygon};

    const SETTINGS: BakeSettings = BakeSettings { tile_cells: 8, ..BakeSettings::CHARACTER };

    /// A polygon on the tile at `coord`, from `low` to `high` along z, against its high x side (or
    /// its low x side), wound as `rerecast` winds them, with the seam its only open edge.
    fn square(x: [i32; 2], z: [i32; 2], y: i32, seam: Seam) -> TilePolygon {
        let corners = vec![[x[0], y, z[0]], [x[0], y, z[1]], [x[1], y, z[1]], [x[1], y, z[0]]];
        let mut across = vec![Across::Wall; 4];
        match seam {
            Seam::LowX => across[0] = Across::Seam(Seam::LowX),
            Seam::HighX => across[2] = Across::Seam(Seam::HighX),
            _ => unreachable!("these tests' seams run along z"),
        }
        TilePolygon { corners, across }
    }

    fn sides(left: Vec<TilePolygon>, right: Vec<TilePolygon>) -> (NavMesh, SeamReport) {
        let tile = |coord, polygons| Tile { coord, polygons, cell_exact: false, holes_lost: 0 };
        let tiles = [tile([0, 0], left), tile([1, 0], right)];
        let (mesh, report) = stitch(&tiles, &SETTINGS).expect("it stitches");
        let bytes = mesh.to_cooked().expect("it writes");
        NavMesh::read(&bytes).expect("every shared edge is shared both ways");
        (mesh, report)
    }

    #[test]
    fn edges_that_end_alike_join_with_no_new_corner() {
        let (mesh, report) =
            sides(vec![square([4, 8], [0, 8], 0, Seam::HighX)], vec![square([8, 12], [0, 8], 0, Seam::LowX)]);
        assert_eq!(report.exact, 1);
        assert_eq!(report.cuts, 0);
        assert_eq!(report.joined_cells, 16);
        assert_eq!(mesh.corners.len(), 8);
        assert_eq!(mesh.vertices.len(), 6, "the seam's two corners are one vertex each");
    }

    /// One edge against two: the long one is cut where the other two meet, so each piece has one
    /// polygon on each side, and the runtime never sees a partial edge.
    #[test]
    fn a_t_junction_cuts_the_long_edge() {
        let (mesh, report) = sides(
            vec![square([4, 8], [0, 8], 0, Seam::HighX)],
            vec![square([8, 12], [0, 3], 0, Seam::LowX), square([8, 12], [3, 8], 0, Seam::LowX)],
        );
        assert_eq!(report.partial, 2);
        assert_eq!(report.cuts, 1);
        assert_eq!(report.joined_cells, 16);
        assert_eq!(mesh.polygon_starts[1], 5, "the long edge's polygon has a corner more");
        let across: Vec<u32> = mesh.neighbours[..5].iter().copied().filter(|&n| n != NO_NEIGHBOUR).collect();
        assert_eq!(across.len(), 2, "{:?}", mesh.neighbours);
    }

    #[test]
    fn edges_that_only_partly_meet_join_where_they_do() {
        let (_, report) =
            sides(vec![square([4, 8], [0, 5], 0, Seam::HighX)], vec![square([8, 12], [3, 8], 0, Seam::LowX)]);
        assert_eq!(report.partial, 1);
        assert_eq!(report.cuts, 2);
        assert_eq!(report.joined_cells, 4);
        assert_eq!(report.seam_cells, 10);
    }

    #[test]
    fn a_floor_past_the_climb_doesnt_join() {
        let climb = i32::from(SETTINGS.walkable_climb());
        let (mesh, report) = sides(
            vec![square([4, 8], [0, 8], 0, Seam::HighX)],
            vec![square([8, 12], [0, 8], climb + 1, Seam::LowX)],
        );
        assert_eq!(report.joined_cells, 0);
        assert!(mesh.neighbours.iter().all(|&n| n == NO_NEIGHBOUR));

        let (_, within) = sides(
            vec![square([4, 8], [0, 8], 0, Seam::HighX)],
            vec![square([8, 12], [0, 8], climb, Seam::LowX)],
        );
        assert_eq!(within.same_ends_other_height, 1, "a step the character can take");
    }

    /// Two floors over one seam: each joins its own, whatever order the tiles list them.
    #[test]
    fn two_floors_on_a_seam_each_join_their_own() {
        let upper = 100;
        let (mesh, report) = sides(
            vec![square([4, 8], [0, 8], 0, Seam::HighX), square([4, 8], [0, 8], upper, Seam::HighX)],
            vec![square([8, 12], [0, 8], upper, Seam::LowX), square([8, 12], [0, 8], 0, Seam::LowX)],
        );
        assert_eq!(report.exact, 2);
        let across = |polygon: usize| {
            let (from, to) = (mesh.polygon_starts[polygon], mesh.polygon_starts[polygon + 1]);
            mesh.neighbours[from as usize..to as usize].iter().copied().find(|&n| n != NO_NEIGHBOUR)
        };
        assert_eq!(across(0), Some(3), "ground to ground");
        assert_eq!(across(1), Some(2), "upper to upper");
    }
}
