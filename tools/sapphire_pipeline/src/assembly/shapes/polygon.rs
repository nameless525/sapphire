//! Flat polygons for the builtin shapes: a face less its openings, triangulated with only its
//! own corners (no added points, so faces that share an edge share its vertices), and convex
//! polygons clipped by lines.
//!
//! [`triangulate`] is ear clipping, the holes first bridged into the outline (Eberly's
//! "Triangulation by Ear Clipping"): a few hundred corners at most, so the quadratic search is
//! nothing beside the rest of a cook. It works in `f64`, as the faces' coordinates are meters
//! from the shape's origin.

use glam::{DVec2, Vec2};

/// Triangulates the simple polygon `outer` less `holes`, each strictly inside it and apart from
/// the others, in either winding. Returns counter-clockwise triangles as indices into the
/// corners in order: `outer`'s, then each hole's. Fails if the polygon isn't simple.
pub fn triangulate(outer: &[Vec2], holes: &[Vec<Vec2>]) -> Result<Vec<[usize; 3]>, String> {
    let mut points: Vec<DVec2> = outer.iter().map(|p| p.as_dvec2()).collect();
    let mut ring = distinct(&points, (0..outer.len()).collect());
    if area(&points, &ring) < 0.0 {
        ring.reverse();
    }
    // Holes clockwise, the one reaching furthest along +X bridged first.
    let mut hole_rings = Vec::new();
    for hole in holes {
        let start = points.len();
        points.extend(hole.iter().map(|p| p.as_dvec2()));
        let mut hole_ring = distinct(&points, (start..points.len()).collect());
        if area(&points, &hole_ring) > 0.0 {
            hole_ring.reverse();
        }
        if hole_ring.len() >= 3 {
            hole_rings.push(hole_ring);
        }
    }
    let rightmost = |ring: &[usize]| ring.iter().map(|&i| points[i].x).fold(f64::MIN, f64::max);
    hole_rings.sort_by(|a, b| rightmost(b).total_cmp(&rightmost(a)));
    for hole in hole_rings {
        ring = bridge(&points, ring, &hole)?;
    }
    clip_ears(&points, ring)
}

/// `ring` without corners that repeat the one before them (round from its last to its first).
fn distinct(points: &[DVec2], mut ring: Vec<usize>) -> Vec<usize> {
    ring.dedup_by(|a, b| points[*a] == points[*b]);
    while ring.len() > 1 && points[ring[0]] == points[ring[ring.len() - 1]] {
        ring.pop();
    }
    ring
}

/// Twice the signed area of `ring`: positive counter-clockwise.
fn area(points: &[DVec2], ring: &[usize]) -> f64 {
    (0..ring.len()).map(|k| points[ring[k]].perp_dot(points[ring[(k + 1) % ring.len()]])).sum()
}

/// `ring` with `hole` (clockwise) spliced in by a bridge from the hole's rightmost corner to a
/// corner of the ring it sees.
fn bridge(points: &[DVec2], ring: Vec<usize>, hole: &[usize]) -> Result<Vec<usize>, String> {
    let (m_at, &m) = hole
        .iter()
        .enumerate()
        .max_by(|a, b| {
            points[*a.1].x.total_cmp(&points[*b.1].x).then(points[*b.1].y.total_cmp(&points[*a.1].y))
        })
        .expect("a hole has corners");
    let mp = points[m];
    // The nearest crossing of the ray from M along +X with the ring's edges.
    let mut best: Option<(f64, usize)> = None;
    for k in 0..ring.len() {
        let next = (k + 1) % ring.len();
        let (a, b) = (points[ring[k]], points[ring[next]]);
        // The ray leaves the inside (on every edge's left) through an edge going up.
        if !(a.y <= mp.y && b.y >= mp.y) || a.y == b.y {
            continue;
        }
        let t = (mp.y - a.y) / (b.y - a.y);
        let x = a.x + t * (b.x - a.x);
        if x < mp.x {
            continue;
        }
        // A corner the ray meets is the candidate itself; otherwise the edge's corner
        // furthest along +X.
        let candidate = if t == 0.0 {
            k
        } else if t == 1.0 || b.x > a.x {
            next
        } else {
            k
        };
        if best.is_none_or(|(best_x, _)| x < best_x) {
            best = Some((x, candidate));
        }
    }
    let (ix, mut p_at) = best.ok_or("a hole isn't inside its face")?;
    let i = DVec2::new(ix, mp.y);
    let p = points[ring[p_at]];
    if p != i {
        // A reflex corner inside triangle (M, I, P) would block the bridge: take the one
        // nearest the ray's direction instead.
        let candidate = p_at;
        let mut best_angle = f64::MAX;
        for k in 0..ring.len() {
            let r = points[ring[k]];
            if k == candidate || !reflex(points, &ring, k) || !inside_triangle(r, mp, i, p) || r == mp {
                continue;
            }
            let d = r - mp;
            let angle = d.y.abs().atan2(d.x);
            if angle < best_angle {
                best_angle = angle;
                p_at = k;
            }
        }
    }
    let mut spliced = Vec::with_capacity(ring.len() + hole.len() + 2);
    spliced.extend_from_slice(&ring[..=p_at]);
    spliced.extend((0..=hole.len()).map(|k| hole[(m_at + k) % hole.len()]));
    spliced.push(ring[p_at]);
    spliced.extend_from_slice(&ring[p_at + 1..]);
    Ok(spliced)
}

/// Whether corner `k` of the counter-clockwise `ring` turns clockwise.
fn reflex(points: &[DVec2], ring: &[usize], k: usize) -> bool {
    let n = ring.len();
    let (a, b, c) = (points[ring[(k + n - 1) % n]], points[ring[k]], points[ring[(k + 1) % n]]);
    (b - a).perp_dot(c - b) < 0.0
}

/// Whether `p` lies in triangle `a b c` (either winding), its edges included.
fn inside_triangle(p: DVec2, a: DVec2, b: DVec2, c: DVec2) -> bool {
    let d1 = (b - a).perp_dot(p - a);
    let d2 = (c - b).perp_dot(p - b);
    let d3 = (a - c).perp_dot(p - c);
    let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(negative && positive)
}

/// Ear clipping of a counter-clockwise ring that may visit a corner twice (a bridge's ends).
fn clip_ears(points: &[DVec2], mut ring: Vec<usize>) -> Result<Vec<[usize; 3]>, String> {
    let mut triangles = Vec::with_capacity(ring.len().saturating_sub(2));
    let scale = ring.iter().map(|&i| points[i].abs().max_element()).fold(1.0, f64::max);
    let epsilon = 1e-12 * scale * scale;
    while ring.len() > 3 {
        let n = ring.len();
        let mut clipped = false;
        for k in 0..n {
            let (ia, ib, ic) = (ring[(k + n - 1) % n], ring[k], ring[(k + 1) % n]);
            let (a, b, c) = (points[ia], points[ib], points[ic]);
            let turn = (b - a).perp_dot(c - b);
            if turn <= epsilon {
                // A corner on a straight run between its neighbors adds nothing: drop it once
                // nothing else can be clipped (below), so faces sharing it keep it if they can.
                continue;
            }
            let blocked = ring.iter().enumerate().any(|(j, &other)| {
                let q = points[other];
                j != k
                    && j != (k + n - 1) % n
                    && j != (k + 1) % n
                    && q != a
                    && q != b
                    && q != c
                    && inside_triangle(q, a, b, c)
            });
            if blocked {
                continue;
            }
            triangles.push([ia, ib, ic]);
            ring.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            // Only straight runs (or a polygon that isn't simple) are left: drop a corner that
            // lies on the segment between its neighbors.
            let straight = (0..n).find(|&k| {
                let (a, b, c) = (points[ring[(k + n - 1) % n]], points[ring[k]], points[ring[(k + 1) % n]]);
                (b - a).perp_dot(c - b).abs() <= epsilon && (b - a).dot(c - b) >= 0.0
            });
            match straight {
                Some(k) => {
                    ring.remove(k);
                }
                None => return Err("a face's outline crosses itself".to_owned()),
            }
        }
    }
    if ring.len() == 3 {
        let (a, b, c) = (points[ring[0]], points[ring[1]], points[ring[2]]);
        if (b - a).perp_dot(c - b) > epsilon {
            triangles.push([ring[0], ring[1], ring[2]]);
        }
    }
    Ok(triangles)
}

/// A half-plane: the points `p` with `normal · p <= offset`.
#[derive(Clone, Copy, Debug)]
pub struct HalfPlane {
    pub normal: Vec2,
    pub offset: f32,
}

impl HalfPlane {
    pub fn new(normal: Vec2, offset: f32) -> Self {
        Self { normal, offset }
    }

    fn distance(self, p: Vec2) -> f32 {
        self.normal.dot(p) - self.offset
    }
}

/// The convex polygon `polygon` clipped to every one of `planes` (Sutherland–Hodgman). Corners
/// within a hair of a line count as on it, so cells that meet a line along an edge keep that
/// edge's corners exactly.
pub fn clip(polygon: &[Vec2], planes: &[HalfPlane]) -> Vec<Vec2> {
    const HAIR: f32 = 1e-6;
    let mut current = polygon.to_vec();
    for &plane in planes {
        if current.is_empty() {
            break;
        }
        let mut next = Vec::with_capacity(current.len() + 2);
        for k in 0..current.len() {
            let (a, b) = (current[k], current[(k + 1) % current.len()]);
            let (da, db) = (plane.distance(a), plane.distance(b));
            if da <= HAIR {
                next.push(a);
            }
            if (da < -HAIR && db > HAIR) || (da > HAIR && db < -HAIR) {
                let t = da / (da - db);
                next.push(a + (b - a) * t);
            }
        }
        // Repeats where a corner lay on the line.
        next.dedup_by(|a, b| a.distance_squared(*b) < HAIR * HAIR);
        if next.len() > 1 && next[0].distance_squared(next[next.len() - 1]) < HAIR * HAIR {
            next.pop();
        }
        current = next;
    }
    if current.len() < 3 { Vec::new() } else { current }
}

/// Twice the signed area of a polygon: positive counter-clockwise.
pub fn signed_area(polygon: &[Vec2]) -> f32 {
    (0..polygon.len()).map(|k| polygon[k].perp_dot(polygon[(k + 1) % polygon.len()])).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total_area(points: &[Vec2], triangles: &[[usize; 3]]) -> f32 {
        triangles
            .iter()
            .map(|&[a, b, c]| {
                let area = (points[b] - points[a]).perp_dot(points[c] - points[a]);
                assert!(area > 0.0, "counter-clockwise and not degenerate: {:?}", [a, b, c]);
                area
            })
            .sum::<f32>()
            * 0.5
    }

    fn circle(center: Vec2, radius: f32, count: usize) -> Vec<Vec2> {
        (0..count)
            .map(|k| {
                let angle = std::f32::consts::TAU * k as f32 / count as f32;
                center + Vec2::new(angle.cos(), angle.sin()) * radius
            })
            .collect()
    }

    #[test]
    fn faces_less_their_openings_cover_their_area_with_their_own_corners() {
        let outer =
            vec![Vec2::new(-4.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(4.0, 3.0), Vec2::new(-4.0, 3.0)];
        let moon = circle(Vec2::new(-2.0, 1.5), 0.8, 24);
        let window = vec![Vec2::new(1.0, 1.0), Vec2::new(1.0, 2.0), Vec2::new(2.5, 2.0), Vec2::new(2.5, 1.0)];
        let triangles = triangulate(&outer, &[moon.clone(), window.clone()]).unwrap();
        let points: Vec<Vec2> = outer.iter().chain(&moon).chain(&window).copied().collect();
        let expected = 24.0 - 0.5 * signed_area(&moon).abs() - 1.5;
        assert!((total_area(&points, &triangles) - expected).abs() < 1e-3);
        // Every corner is used, so neighbors sharing an edge share its corners.
        for index in 0..points.len() {
            assert!(triangles.iter().any(|t| t.contains(&index)), "corner {index} is used");
        }
    }

    #[test]
    fn notched_and_concave_outlines_triangulate() {
        // A wall with a door notched into its foot, and a gable end's pentagon with a sagging top.
        let notched = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 2.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(4.0, 3.0),
            Vec2::new(0.0, 3.0),
        ];
        let triangles = triangulate(&notched, &[]).unwrap();
        assert!((total_area(&notched, &triangles) - 10.0).abs() < 1e-4);
        let mut gable = vec![Vec2::new(-3.0, 0.0), Vec2::new(3.0, 0.0)];
        for k in 0..=10 {
            let x = 3.0 - 6.0 * k as f32 / 10.0;
            let s = 1.0 - x.abs() / 3.0;
            gable.push(Vec2::new(x, 2.0 * (0.5 * s + 0.5 * s * s)));
        }
        gable.dedup();
        let triangles = triangulate(&gable, &[]).unwrap();
        assert!((total_area(&gable, &triangles) - 0.5 * signed_area(&gable)).abs() < 1e-4);
        // Clockwise outlines are turned round.
        let reversed: Vec<Vec2> = notched.iter().rev().copied().collect();
        assert!((total_area(&reversed, &triangulate(&reversed, &[]).unwrap()) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn clipping_keeps_the_part_inside_every_line() {
        let square = [Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0), Vec2::new(2.0, 2.0), Vec2::new(0.0, 2.0)];
        // x + y <= 2: the lower left triangle.
        let half = clip(&square, &[HalfPlane::new(Vec2::new(1.0, 1.0), 2.0)]);
        assert_eq!(half.len(), 3);
        assert!((signed_area(&half) - 4.0).abs() < 1e-5);
        // A line along an edge keeps the square whole, and one outside it leaves nothing.
        assert_eq!(clip(&square, &[HalfPlane::new(Vec2::X, 2.0)]).len(), 4);
        assert!(clip(&square, &[HalfPlane::new(Vec2::X, -1.0)]).is_empty());
    }
}
