//! A bounding volume hierarchy over triangles, for the probe bake's rays and its brick choice.
//!
//! Built by splitting each node's triangles at the middle of their centroids' bounds along its
//! longest axis (at the median when that splits nothing, or past [`MIDDLE_DEPTH`]), down to a
//! few triangles a leaf. A middle split of clustered triangles can peel a few off a level, so
//! deep nodes split at the median, which halves them: no tree is deeper than [`MAX_DEPTH`], and
//! the walk's fixed stack always holds it.
//! Everything is scalar `f32` in a fixed order, so the same triangles give the same tree and the
//! same hits on every target.

use glam::Vec3;

/// Triangles a leaf holds at most.
const LEAF_SIZE: usize = 4;
/// Levels split at the middle of the centroids' bounds; deeper ones split at the median.
const MIDDLE_DEPTH: usize = 24;
/// The deepest a node lies: the middle splits, then halvings of at most 2³² triangles.
const MAX_DEPTH: usize = MIDDLE_DEPTH + 32;

/// Where a ray hit a triangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hit {
    /// Distance along the ray, in units of its direction's length.
    pub t: f32,
    pub triangle: u32,
    /// Barycentric weights of the triangle's second and third corners.
    pub u: f32,
    pub v: f32,
    /// The ray hit the side the triangle's winding faces (counter-clockwise seen from it).
    pub front: bool,
}

#[derive(Clone, Copy, Debug)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// A leaf's first entry in `order`, or an interior node's first child (the second follows).
    first: u32,
    /// Triangles in a leaf; 0 for an interior node.
    count: u32,
}

/// The hierarchy, with its triangles' corners.
#[derive(Debug)]
pub(crate) struct Bvh {
    nodes: Vec<Node>,
    /// Triangle indices, leaf by leaf.
    order: Vec<u32>,
    triangles: Vec<[Vec3; 3]>,
}

impl Bvh {
    pub(crate) fn new(triangles: Vec<[Vec3; 3]>) -> Self {
        let mut order: Vec<u32> = (0..triangles.len() as u32).collect();
        let centroids: Vec<Vec3> = triangles.iter().map(|[a, b, c]| (*a + *b + *c) / 3.0).collect();
        let mut nodes = vec![Node { min: Vec3::ZERO, max: Vec3::ZERO, first: 0, count: 0 }];
        // (node, range of `order`, depth)
        let mut work = vec![(0usize, 0usize, order.len(), 0usize)];
        while let Some((node, start, end, depth)) = work.pop() {
            let (min, max) = bounds(order[start..end].iter().flat_map(|&t| triangles[t as usize]));
            nodes[node].min = min;
            nodes[node].max = max;
            if end - start <= LEAF_SIZE {
                nodes[node].first = start as u32;
                nodes[node].count = (end - start) as u32;
                continue;
            }
            let (cmin, cmax) = bounds(order[start..end].iter().map(|&t| centroids[t as usize]));
            let extent = cmax - cmin;
            let axis = if extent.x >= extent.y && extent.x >= extent.z {
                0
            } else if extent.y >= extent.z {
                1
            } else {
                2
            };
            let middle = 0.5 * (cmin[axis] + cmax[axis]);
            let slice = &mut order[start..end];
            let mut split = slice.len();
            if depth < MIDDLE_DEPTH {
                split = partition(slice, |&t| centroids[t as usize][axis] < middle);
            }
            if split == 0 || split == slice.len() {
                // Too deep, or every centroid on one side (all equal along the axis): split at
                // the median, ordered by index so the tree doesn't depend on the sort's
                // stability.
                slice.sort_by(|&a, &b| {
                    centroids[a as usize][axis].total_cmp(&centroids[b as usize][axis]).then(a.cmp(&b))
                });
                split = slice.len() / 2;
            }
            let first = nodes.len();
            nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, first: 0, count: 0 });
            nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, first: 0, count: 0 });
            nodes[node].first = first as u32;
            work.push((first + 1, start + split, end, depth + 1));
            work.push((first, start, start + split, depth + 1));
        }
        Self { nodes, order, triangles }
    }

    /// The nearest hit `accept` takes of the ray from `origin` along `dir` within `t_max`, either
    /// side: those it refuses (where a card is cut out) the ray passes.
    pub(crate) fn closest(
        &self,
        origin: Vec3,
        dir: Vec3,
        t_max: f32,
        accept: &dyn Fn(&Hit) -> bool,
    ) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        let mut limit = t_max;
        self.walk(origin, dir, &mut limit, &mut |triangle, limit| {
            if let Some(hit) = intersect(&self.triangles[triangle as usize], origin, dir, *limit) {
                let hit = Hit { triangle, ..hit };
                // Ties go to the lower index, so the walk's order never decides a hit.
                let better =
                    best.is_none_or(|best| hit.t < best.t || (hit.t == best.t && triangle < best.triangle));
                if better && accept(&hit) {
                    *limit = hit.t;
                    best = Some(hit);
                }
            }
            false
        });
        best
    }

    /// Whether anything `accept` takes lies on the ray from `origin` along `dir` within `t_max`.
    pub(crate) fn occluded(
        &self,
        origin: Vec3,
        dir: Vec3,
        t_max: f32,
        accept: &dyn Fn(&Hit) -> bool,
    ) -> bool {
        let mut limit = t_max;
        let mut found = false;
        self.walk(origin, dir, &mut limit, &mut |triangle, limit| {
            found = intersect(&self.triangles[triangle as usize], origin, dir, *limit)
                .is_some_and(|hit| accept(&Hit { triangle, ..hit }));
            found
        });
        found
    }

    /// Whether any triangle passes through the box `min`..`max`.
    pub(crate) fn overlaps_box(&self, min: Vec3, max: Vec3) -> bool {
        let center = 0.5 * (min + max);
        let half = 0.5 * (max - min);
        let mut stack = vec![0u32];
        while let Some(index) = stack.pop() {
            let node = self.nodes[index as usize];
            if node.min.cmpgt(max).any() || node.max.cmplt(min).any() {
                continue;
            }
            if node.count == 0 {
                stack.extend([node.first, node.first + 1]);
                continue;
            }
            let leaf = &self.order[node.first as usize..(node.first + node.count) as usize];
            if leaf.iter().any(|&t| triangle_box_overlap(&self.triangles[t as usize], center, half)) {
                return true;
            }
        }
        false
    }

    /// Visits the triangles of every leaf the ray's box test passes, nearest node first;
    /// `visit` may shorten `limit`, and stops the walk by returning true.
    fn walk(&self, origin: Vec3, dir: Vec3, limit: &mut f32, visit: &mut dyn FnMut(u32, &mut f32) -> bool) {
        if self.triangles.is_empty() {
            return;
        }
        let inverse = Vec3::ONE / dir;
        // Each level holds at most the sibling it put off: never more than the tree's depth
        // and the two children of the node being visited.
        let mut stack = [0u32; MAX_DEPTH + 2];
        let mut depth = 1;
        while depth > 0 {
            depth -= 1;
            let node = self.nodes[stack[depth] as usize];
            if slab(node.min, node.max, origin, inverse, *limit).is_none() {
                continue;
            }
            if node.count > 0 {
                for &triangle in &self.order[node.first as usize..(node.first + node.count) as usize] {
                    if visit(triangle, limit) {
                        return;
                    }
                }
                continue;
            }
            let (a, b) = (node.first, node.first + 1);
            let near_a =
                slab(self.nodes[a as usize].min, self.nodes[a as usize].max, origin, inverse, *limit);
            let near_b =
                slab(self.nodes[b as usize].min, self.nodes[b as usize].max, origin, inverse, *limit);
            // The farther child goes on the stack first, so the nearer is visited first.
            let (first, second) = match (near_a, near_b) {
                (Some(ta), Some(tb)) if tb < ta => (a, b),
                (Some(_), Some(_)) => (b, a),
                (Some(_), None) => (a, a),
                (None, Some(_)) => (b, b),
                (None, None) => continue,
            };
            stack[depth] = first;
            depth += 1;
            if second != first {
                stack[depth] = second;
                depth += 1;
            }
            // `first` was pushed first, so the nearer (`second`) pops first.
        }
    }
}

/// The bounds of `points`.
fn bounds(points: impl Iterator<Item = Vec3>) -> (Vec3, Vec3) {
    points.fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(min, max), p| {
        (min.min(p), max.max(p))
    })
}

/// Moves the entries `keep` accepts to the front, in order, and returns how many there are.
fn partition(slice: &mut [u32], keep: impl Fn(&u32) -> bool) -> usize {
    let mut front: Vec<u32> = slice.iter().copied().filter(|t| keep(t)).collect();
    let back: Vec<u32> = slice.iter().copied().filter(|t| !keep(t)).collect();
    let split = front.len();
    front.extend(back);
    slice.copy_from_slice(&front);
    split
}

/// Where the ray enters the box, if it does within `limit`.
fn slab(min: Vec3, max: Vec3, origin: Vec3, inverse: Vec3, limit: f32) -> Option<f32> {
    let t0 = (min - origin) * inverse;
    let t1 = (max - origin) * inverse;
    let near = t0.min(t1);
    let far = t0.max(t1);
    // NaN (a zero direction component on the slab's plane) counts as inside.
    let enter = near.x.max(near.y).max(near.z).max(0.0);
    let exit = far.x.min(far.y).min(far.z).min(limit);
    (enter <= exit).then_some(enter)
}

/// Möller–Trumbore: the hit of the ray with `triangle` within `limit`, either side.
fn intersect(triangle: &[Vec3; 3], origin: Vec3, dir: Vec3, limit: f32) -> Option<Hit> {
    const EPSILON: f32 = 1e-9;
    let [a, b, c] = *triangle;
    let (e1, e2) = (b - a, c - a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < EPSILON {
        return None;
    }
    let inverse = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inverse;
    // det > 0 is a ray against the winding's normal: the front.
    (t > 0.0 && t < limit).then_some(Hit { t, triangle: 0, u, v, front: det > 0.0 })
}

/// Whether `triangle` passes through the box at `center` with half extents `half`: the
/// separating axis test of Akenine-Möller (2001).
fn triangle_box_overlap(triangle: &[Vec3; 3], center: Vec3, half: Vec3) -> bool {
    let v = triangle.map(|p| p - center);
    let edges = [v[1] - v[0], v[2] - v[1], v[0] - v[2]];
    // The box's face normals.
    for axis in 0..3 {
        let (min, max) =
            (v[0][axis].min(v[1][axis]).min(v[2][axis]), v[0][axis].max(v[1][axis]).max(v[2][axis]));
        if min > half[axis] || max < -half[axis] {
            return false;
        }
    }
    // The triangle's normal.
    let normal = edges[0].cross(edges[1]);
    let radius = half.dot(normal.abs());
    if normal.dot(v[0]).abs() > radius {
        return false;
    }
    // The nine cross products of box axes and triangle edges.
    for edge in edges {
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            let separating = axis.cross(edge);
            let projected = v.map(|p| p.dot(separating));
            let radius = half.dot(separating.abs());
            let (min, max) = (
                projected[0].min(projected[1]).min(projected[2]),
                projected[0].max(projected[1]).max(projected[2]),
            );
            if min > radius || max < -radius {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit square in the XZ plane at height `y`, facing up, as two triangles.
    fn square(y: f32, x: f32, z: f32) -> [[Vec3; 3]; 2] {
        let (a, b, c, d) = (
            Vec3::new(x, y, z),
            Vec3::new(x + 1.0, y, z),
            Vec3::new(x, y, z + 1.0),
            Vec3::new(x + 1.0, y, z + 1.0),
        );
        // Counter-clockwise seen from above.
        [[a, c, b], [b, c, d]]
    }

    fn floor_of_squares() -> Bvh {
        let mut triangles = Vec::new();
        for x in -5..5 {
            for z in -5..5 {
                triangles.extend(square(0.0, x as f32, z as f32));
            }
        }
        triangles.extend(square(3.0, 0.0, 0.0));
        Bvh::new(triangles)
    }

    #[test]
    fn rays_find_the_nearest_triangle_and_its_side() {
        let bvh = floor_of_squares();
        let down = bvh.closest(Vec3::new(0.5, 5.0, 0.5), Vec3::NEG_Y, 100.0, &|_| true).expect("a hit");
        assert!((down.t - 2.0).abs() < 1e-5, "the square at 3 m first: {down:?}");
        assert!(down.front, "seen from above");
        let up = bvh.closest(Vec3::new(0.5, 1.0, 0.5), Vec3::Y, 100.0, &|_| true).expect("a hit");
        assert!((up.t - 2.0).abs() < 1e-5 && !up.front, "its underside: {up:?}");
        assert!(bvh.closest(Vec3::new(0.5, 1.0, 0.5), Vec3::Y, 1.5, &|_| true).is_none(), "out of reach");
        assert!(bvh.occluded(Vec3::new(-3.5, 1.0, 2.5), Vec3::NEG_Y, 2.0, &|_| true));
        assert!(!bvh.occluded(Vec3::new(-3.5, 1.0, 2.5), Vec3::Y, 100.0, &|_| true), "open sky");
        // Every brute-force hit agrees.
        let dir = Vec3::new(0.3, -1.0, 0.2).normalize();
        let origin = Vec3::new(0.2, 4.0, -0.3);
        let brute = bvh
            .triangles
            .iter()
            .filter_map(|triangle| intersect(triangle, origin, dir, f32::INFINITY))
            .map(|hit| hit.t)
            .fold(f32::INFINITY, f32::min);
        assert_eq!(bvh.closest(origin, dir, f32::INFINITY, &|_| true).map(|hit| hit.t), Some(brute));
    }

    #[test]
    fn clustered_triangles_keep_the_tree_shallow() {
        // Squares at powers of two along x: every middle split peels the farthest off alone, a
        // hundred levels, until the median takes over.
        let bvh = Bvh::new(
            (0..100)
                .map(|i| {
                    let x = 2.0f32.powi(i);
                    [Vec3::new(x, -1.0, -1.0), Vec3::new(x, 1.0, 0.0), Vec3::new(x, -1.0, 1.0)]
                })
                .collect(),
        );
        let mut deepest = 0;
        let mut nodes = vec![(0u32, 0usize)];
        while let Some((index, depth)) = nodes.pop() {
            deepest = deepest.max(depth);
            let node = bvh.nodes[index as usize];
            if node.count == 0 {
                nodes.extend([(node.first, depth + 1), (node.first + 1, depth + 1)]);
            }
        }
        assert!(deepest <= MAX_DEPTH, "{deepest} levels");
        // Every square is still found, the deepest ones included.
        for i in 1..100 {
            let origin = Vec3::new(0.75 * 2.0f32.powi(i), 0.0, 0.0);
            let hit = bvh.closest(origin, Vec3::X, f32::INFINITY, &|_| true).expect("a hit");
            assert_eq!(hit.triangle, i as u32);
        }
    }

    #[test]
    fn boxes_overlap_the_triangles_they_cut() {
        let bvh = floor_of_squares();
        assert!(bvh.overlaps_box(Vec3::new(-0.5, -0.5, -0.5), Vec3::new(0.5, 0.5, 0.5)), "the floor");
        assert!(!bvh.overlaps_box(Vec3::new(-0.5, 0.5, -0.5), Vec3::new(0.5, 2.5, 0.5)), "between");
        assert!(bvh.overlaps_box(Vec3::new(0.4, 2.9, 0.4), Vec3::new(0.6, 3.1, 0.6)), "the raised square");
        assert!(!bvh.overlaps_box(Vec3::new(20.0, -1.0, 20.0), Vec3::new(21.0, 1.0, 21.0)), "past the floor");
        // A box beside a tilted triangle's bounds' corner but off the triangle itself.
        let tilted = Bvh::new(vec![[Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 2.0, 0.0)]]);
        assert!(!tilted.overlaps_box(Vec3::new(1.5, 1.5, -0.1), Vec3::new(1.9, 1.9, 0.1)));
        assert!(tilted.overlaps_box(Vec3::new(0.5, 0.5, -0.1), Vec3::new(0.7, 0.7, 0.1)));
    }
}
