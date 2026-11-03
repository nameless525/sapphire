//! What `rerecast`'s contours need before they can triangulate, from outside, as its `ContourSet` is
//! public.
//!
//! **Holes.** Joins a region's holes to its outline, which `rerecast` doesn't do.
//!
//! A region with a hole in it, such as a planter's top around a tree or open ground around a box too
//! small to split it, gets one contour for its outline and one, wound the other way, for the hole.
//! Recast's `rcBuildContours` merges those before it triangulates. `rerecast` 0.5's `build_contours`
//! has no such step, so its triangulator meets the backward contour and fails the whole tile with
//! `InvalidContour`. This does it by the same method as Recast: each hole, leftmost first, joins the
//! outline by the shortest diagonal that crosses nothing, and the two become one contour that runs out
//! along the diagonal and back.
//!
//! **Crossings.** Simplifying keeps an edge between two regions straight, however far its cells bend
//! away from it, as Recast does. Now and then such an edge crosses another of the same contour, and
//! the triangulator fails the tile or writes a polygon wound backward. Recast warns and carries on;
//! [`any_crossed`] sees it, and [`follow_cells`] gives a tile contours that can't.

use glam::U16Vec3;
use rerecast::{Contour, ContourSet, RegionId, RegionVertexId};

type Corner = (U16Vec3, u32);

/// Merges every hole in `set` into the outline round it. Returns how many it merged and how many it
/// had to leave out: a hole with no outline round it, or that no diagonal reaches. One left out goes,
/// and whatever it kept out counts as walkable.
pub fn merge_holes(set: &mut ContourSet) -> (usize, usize) {
    let (mut merged, mut dropped) = (0, 0);
    let mut holes: Vec<(RegionId, usize)> = Vec::new();
    for (index, contour) in set.contours.iter().enumerate() {
        if signed_area(&contour.vertices) < 0 {
            holes.push((contour.region, index));
        }
    }
    if holes.is_empty() {
        return (0, 0);
    }
    let mut regions: Vec<RegionId> = holes.iter().map(|(region, _)| *region).collect();
    regions.sort_unstable_by_key(|r| r.bits());
    regions.dedup();
    for region in regions {
        // Leftmost first, so that a hole further in sees past those already joined.
        let mut ordered: Vec<usize> =
            holes.iter().filter(|(of, _)| *of == region).map(|(_, index)| *index).collect();
        ordered.sort_by_key(|&index| {
            let at = leftmost(&set.contours[index].vertices);
            let corner = set.contours[index].vertices[at].0;
            (corner.x, corner.z, index)
        });
        for position in 0..ordered.len() {
            let hole = std::mem::take(&mut set.contours[ordered[position]].vertices);
            let later: Vec<&[Corner]> =
                ordered[position + 1..].iter().map(|&i| set.contours[i].vertices.as_slice()).collect();
            // A region can have more than one outline, and only one of them is round this hole.
            let around = set.contours.iter().position(|c| {
                c.region == region
                    && signed_area(&c.vertices) > 0
                    && hole.iter().any(|(corner, _)| contains(&c.vertices, *corner))
            });
            let Some(outline) = around else {
                dropped += 1;
                continue;
            };
            match find_diagonal(&set.contours[outline].vertices, &hole, &later) {
                Some((at_outline, at_hole)) => {
                    let joined = join(&set.contours[outline].vertices, at_outline, &hole, at_hole);
                    set.contours[outline].vertices = joined;
                    merged += 1;
                }
                None => dropped += 1,
            }
        }
    }
    set.contours.retain(|contour: &Contour| contour.vertices.len() >= 3);
    (merged, dropped)
}

/// Whether any two edges of any one contour cross. Read before holes join, since a join's diagonal
/// runs along itself.
pub fn any_crossed(set: &ContourSet) -> bool {
    set.contours.iter().any(|contour| {
        let corners = &contour.vertices;
        let n = corners.len();
        (0..n).any(|i| {
            let (a, b) = (corners[i].0, corners[(i + 1) % n].0);
            (i + 2..n).any(|j| {
                let (c, d) = (corners[j].0, corners[(j + 1) % n].0);
                let (abc, abd, cda, cdb) = (area(a, b, c), area(a, b, d), area(c, d, a), area(c, d, b));
                (abc > 0 && abd < 0 || abc < 0 && abd > 0) && (cda > 0 && cdb < 0 || cda < 0 && cdb > 0)
            })
        })
    })
}

/// Replaces each contour with the cells' own outline, less the corners in a straight line. Corners
/// where the region across changes stay, so that two contours sharing an edge still share its ends and
/// every corner on it.
pub fn follow_cells(set: &mut ContourSet) {
    for contour in &mut set.contours {
        let raw = &contour.raw_vertices;
        let n = raw.len();
        let mut corners: Vec<Corner> = Vec::with_capacity(n);
        for i in 0..n {
            let (before, (at, flags), after) = (raw[(i + n - 1) % n], raw[i], raw[(i + 1) % n]);
            let region = RegionVertexId::REGION_MASK;
            let changes = (before.1 & region) != (flags & region) || (flags & region) != (after.1 & region);
            if changes || area(before.0, at, after.0) != 0 {
                corners.push((at, flags.bits()));
            }
        }
        contour.vertices = corners;
    }
    set.contours.retain(|contour: &Contour| contour.vertices.len() >= 3);
}

/// Whether `point` is inside the contour or on its edge, on the XZ plane, by counting crossings.
fn contains(corners: &[Corner], point: U16Vec3) -> bool {
    let n = corners.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (corners[i].0, corners[(i + 1) % n].0);
        if area(a, b, point) == 0 && between(a, b, point) {
            return true;
        }
        if (a.z > point.z) != (b.z > point.z) {
            // The edge's x where it meets the point's row, against the point's, in whole numbers.
            let (dz, dx) = (i64::from(b.z) - i64::from(a.z), i64::from(b.x) - i64::from(a.x));
            let reach = dx * (i64::from(point.z) - i64::from(a.z));
            let offset = (i64::from(point.x) - i64::from(a.x)) * dz;
            if (dz > 0 && offset < reach) || (dz < 0 && offset > reach) {
                inside = !inside;
            }
        }
    }
    inside
}

/// Twice the contour's area on the XZ plane: positive for an outline, negative for a hole.
fn signed_area(corners: &[Corner]) -> i64 {
    let mut area = 0;
    let mut previous = corners.len().wrapping_sub(1);
    for (index, (corner, _)) in corners.iter().enumerate() {
        let before = corners[previous].0;
        area += i64::from(corner.x) * i64::from(before.z) - i64::from(before.x) * i64::from(corner.z);
        previous = index;
    }
    area
}

fn leftmost(corners: &[Corner]) -> usize {
    (0..corners.len()).min_by_key(|&i| (corners[i].0.x, corners[i].0.z, i)).unwrap_or(0)
}

/// Twice the area of `a`, `b`, `c` on the XZ plane, in Recast's sense of left: below zero is left.
fn area(a: U16Vec3, b: U16Vec3, c: U16Vec3) -> i64 {
    let (ax, az) = (i64::from(a.x), i64::from(a.z));
    (i64::from(b.x) - ax) * (i64::from(c.z) - az) - (i64::from(c.x) - ax) * (i64::from(b.z) - az)
}

/// Whether the line from corner `i` of `corners` to `to` starts inside the contour.
fn in_cone(corners: &[Corner], i: usize, to: U16Vec3) -> bool {
    let n = corners.len();
    let (at, next, before) = (corners[i].0, corners[(i + 1) % n].0, corners[(i + n - 1) % n].0);
    if area(before, at, next) <= 0 {
        area(at, to, before) < 0 && area(to, at, next) < 0
    } else {
        !(area(at, to, next) <= 0 && area(to, at, before) <= 0)
    }
}

/// Whether the segment from `a` to `b` touches any edge of `corners` that doesn't end at `a` or `b`.
fn crosses(corners: &[Corner], a: U16Vec3, b: U16Vec3) -> bool {
    let n = corners.len();
    (0..n).any(|i| {
        let (c, d) = (corners[i].0, corners[(i + 1) % n].0);
        let shares = |p: U16Vec3| p.x == a.x && p.z == a.z || p.x == b.x && p.z == b.z;
        !shares(c) && !shares(d) && intersects(a, b, c, d)
    })
}

fn intersects(a: U16Vec3, b: U16Vec3, c: U16Vec3, d: U16Vec3) -> bool {
    let (abc, abd, cda, cdb) = (area(a, b, c), area(a, b, d), area(c, d, a), area(c, d, b));
    if ((abc > 0) != (abd > 0)) && ((cda > 0) != (cdb > 0)) && abc != 0 && abd != 0 && cda != 0 && cdb != 0 {
        return true;
    }
    (abc == 0 && between(a, b, c))
        || (abd == 0 && between(a, b, d))
        || (cda == 0 && between(c, d, a))
        || (cdb == 0 && between(c, d, b))
}

/// Whether `c`, in line with `a` and `b`, is between them.
fn between(a: U16Vec3, b: U16Vec3, c: U16Vec3) -> bool {
    if a.x != b.x {
        (a.x <= c.x && c.x <= b.x) || (b.x <= c.x && c.x <= a.x)
    } else {
        (a.z <= c.z && c.z <= b.z) || (b.z <= c.z && c.z <= a.z)
    }
}

/// The shortest clear diagonal from a corner of `hole` to a corner of `outline`, trying the hole's
/// leftmost corner first, as indices into each.
fn find_diagonal(outline: &[Corner], hole: &[Corner], later: &[&[Corner]]) -> Option<(usize, usize)> {
    let first = leftmost(hole);
    for step in 0..hole.len() {
        let at_hole = (first + step) % hole.len();
        let corner = hole[at_hole].0;
        let mut candidates: Vec<(i64, usize)> = (0..outline.len())
            .filter(|&i| in_cone(outline, i, corner))
            .map(|i| {
                let (dx, dz) = (
                    i64::from(outline[i].0.x) - i64::from(corner.x),
                    i64::from(outline[i].0.z) - i64::from(corner.z),
                );
                (dx * dx + dz * dz, i)
            })
            .collect();
        candidates.sort_unstable();
        for (_, at_outline) in candidates {
            let to = outline[at_outline].0;
            if !crosses(outline, to, corner)
                && !crosses(hole, to, corner)
                && later.iter().all(|other| !crosses(other, to, corner))
            {
                return Some((at_outline, at_hole));
            }
        }
    }
    None
}

/// The outline from `at_outline` round to itself, then the hole from `at_hole` round to itself.
fn join(outline: &[Corner], at_outline: usize, hole: &[Corner], at_hole: usize) -> Vec<Corner> {
    let mut joined = Vec::with_capacity(outline.len() + hole.len() + 2);
    joined.extend((0..=outline.len()).map(|i| outline[(at_outline + i) % outline.len()]));
    joined.extend((0..=hole.len()).map(|i| hole[(at_hole + i) % hole.len()]));
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contour(region: u16, points: &[(u16, u16)]) -> Contour {
        Contour {
            vertices: points.iter().map(|&(x, z)| (U16Vec3::new(x, 0, z), 0)).collect(),
            region: RegionId::from(region),
            ..Contour::default()
        }
    }

    fn set(contours: Vec<Contour>) -> ContourSet {
        ContourSet {
            contours,
            aabb: rerecast::Aabb3d::default(),
            cell_size: 1.0,
            cell_height: 1.0,
            width: 64,
            height: 64,
            border_size: 0,
            max_error: 1.3,
        }
    }

    /// As `rerecast` writes them: an outline round one way, its hole round the other.
    const OUTLINE: [(u16, u16); 4] = [(0, 0), (0, 40), (40, 40), (40, 0)];
    const HOLE: [(u16, u16); 4] = [(10, 10), (20, 10), (20, 20), (10, 20)];

    #[test]
    fn what_rerecast_fails_on_triangulates_once_its_hole_is_joined() {
        let mut bare = set(vec![contour(1, &OUTLINE), contour(1, &HOLE)]);
        assert!(bare.clone().into_polygon_mesh(6).is_err(), "still what `merge_holes` exists for");

        assert_eq!(merge_holes(&mut bare), (1, 0));
        assert_eq!(bare.contours.len(), 1);
        assert_eq!(bare.contours[0].vertices.len(), 4 + 4 + 2);
        let mesh = bare.into_polygon_mesh(6).expect("it triangulates");
        assert!(mesh.polygon_count() >= 4, "round all four sides of the hole");
    }

    #[test]
    fn two_holes_in_one_region_both_join() {
        const OTHER: [(u16, u16); 4] = [(25, 25), (35, 25), (35, 35), (25, 35)];
        let mut holed = set(vec![contour(1, &OUTLINE), contour(1, &HOLE), contour(1, &OTHER)]);
        assert_eq!(merge_holes(&mut holed), (2, 0));
        assert_eq!(holed.contours.len(), 1);
        holed.into_polygon_mesh(6).expect("it triangulates");
    }

    #[test]
    fn a_hole_joins_the_outline_round_it_not_the_first_of_its_region() {
        const FAR: [(u16, u16); 4] = [(100, 0), (100, 40), (140, 40), (140, 0)];
        const HOLE_IN_FAR: [(u16, u16); 4] = [(110, 10), (120, 10), (120, 20), (110, 20)];
        let mut two = set(vec![contour(1, &OUTLINE), contour(1, &FAR), contour(1, &HOLE_IN_FAR)]);
        assert_eq!(merge_holes(&mut two), (1, 0));
        assert_eq!(two.contours[0].vertices.len(), 4, "not the first outline");
        assert_eq!(two.contours[1].vertices.len(), 4 + 4 + 2);
    }

    #[test]
    fn a_hole_with_nothing_round_it_goes() {
        const ELSEWHERE: [(u16, u16); 4] = [(110, 10), (120, 10), (120, 20), (110, 20)];
        let mut lone = set(vec![contour(1, &OUTLINE), contour(1, &ELSEWHERE)]);
        assert_eq!(merge_holes(&mut lone), (0, 1));
        assert_eq!(lone.contours.len(), 1);
    }

    /// The contour a real scene's turned wall left, whose long edge clips a corner.
    #[test]
    fn a_contour_that_crosses_itself_is_seen() {
        const CROSSED: [(u16, u16); 14] = [
            (7, 18),
            (20, 76),
            (20, 68),
            (22, 66),
            (36, 66),
            (50, 66),
            (61, 23),
            (61, 0),
            (17, 0),
            (17, 12),
            (17, 25),
            (15, 27),
            (9, 27),
            (7, 25),
        ];
        let crossed = set(vec![contour(1, &CROSSED)]);
        assert!(any_crossed(&crossed));
        assert!(crossed.into_polygon_mesh(6).is_err(), "what the check is there for");
        assert!(!any_crossed(&set(vec![contour(1, &OUTLINE), contour(1, &HOLE)])));
    }

    #[test]
    fn following_the_cells_keeps_bends_and_where_the_region_changes() {
        let free = RegionVertexId::NONE;
        let other = RegionVertexId::from_bits_retain(2);
        let at = |x, z| U16Vec3::new(x, 0, z);
        let raw = vec![
            (at(0, 0), free),
            (at(0, 5), free),
            (at(0, 10), free),
            (at(5, 10), other),
            (at(10, 10), other),
            (at(10, 0), free),
        ];
        let mut cells = set(vec![Contour { raw_vertices: raw, ..contour(1, &[]) }]);
        follow_cells(&mut cells);
        let kept: Vec<(u16, u16)> = cells.contours[0].vertices.iter().map(|(p, _)| (p.x, p.z)).collect();
        // (0, 5) is in line with the same neighbour each way; (5, 10) is in line, but ends the wall.
        assert_eq!(kept, [(0, 0), (0, 10), (5, 10), (10, 10), (10, 0)]);
    }

    #[test]
    fn a_contour_with_no_hole_is_left_alone() {
        let mut plain = set(vec![contour(1, &OUTLINE)]);
        let before = plain.contours.clone();
        assert_eq!(merge_holes(&mut plain), (0, 0));
        assert_eq!(plain.contours, before);
    }

    #[test]
    fn another_regions_outline_is_no_home_for_a_hole() {
        let mut orphan = set(vec![contour(2, &OUTLINE), contour(1, &HOLE)]);
        assert_eq!(merge_holes(&mut orphan), (0, 1));
        assert_eq!(orphan.contours.len(), 1);
        assert_eq!(orphan.contours[0].region, RegionId::from(2));
    }
}
