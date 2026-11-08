//! A cooked mesh in world positions, and where a point is on it.

use glam::Vec3;
use sapphire_navmesh::{ArchivedNavMesh, NO_NEIGHBOUR};

/// Polygons are filed in cells of this size on the XZ plane, in metres. About a building's side: a
/// cell holds a handful in the open and a few dozen around clutter.
const GRID_CELL: f64 = 4.0;
/// Bound empty-space storage independently of a zone's world extent. Coarsening preserves every
/// candidate and its ID order; it only trades lookup work for bounded load memory.
const MAX_GRID_CELLS: usize = 65_536;
const MIN_GRID_REFERENCES: usize = 1024;
const REFERENCES_PER_POLYGON: usize = 16;

/// The most polygons [`NavGraph::walks_straight`] crosses before it says no: a line longer than this is
/// a route's business. A town's polygons are about 4 m across, so this is further than anyone looks.
const LINE_POLYGONS: usize = 64;

/// Which polygon a point is on, and the point on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Location {
    pub polygon: u32,
    pub position: Vec3,
}

/// How far from the mesh a point may be and still count as on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    /// On the XZ plane, in metres, when the point isn't over any polygon. A capsule pressed against a
    /// wall stands outside the mesh, which is eroded by about its radius.
    pub across: f32,
    /// Up or down, in metres. What tells a floor from the one above it.
    pub vertical: f32,
}

impl Reach {
    /// For a character on its feet: a capsule's width across, and up or down by what a polygon's
    /// height can be off. With no detail mesh, one polygon can run from a floor over a stair to its
    /// landing: on a flight of 1.8 m that read 0.79 m out at the worst tread. Floors stand a
    /// character's height apart (1.6 m), so this still tells them apart.
    pub const CHARACTER: Self = Self { across: 0.75, vertical: 1.0 };
}

/// A zone's polygons, built once at load and only read after that.
#[derive(Clone, Debug)]
pub struct NavGraph {
    positions: Vec<Vec3>,
    starts: Vec<u32>,
    corners: Vec<u32>,
    neighbours: Vec<u32>,
    /// Beside `corners`: the middle of the edge from this corner to the next, which a search walks.
    middles: Vec<Vec3>,
    /// Each polygon's bounds, as (min x, min z, max x, max z).
    bounds: Vec<[f32; 4]>,
    grid: Grid,
}

#[derive(Clone, Debug)]
struct Grid {
    min: [f32; 2],
    cells: [i32; 2],
    cell_size: f64,
    /// Where each cell's polygons start in `polygons`, with one more entry at the end.
    starts: Vec<u32>,
    /// By cell, then ascending polygon ID.
    polygons: Vec<u32>,
}

impl NavGraph {
    /// Builds the runtime's lists from a checked mesh ([`NavMesh::read`](sapphire_navmesh::NavMesh::read)).
    pub fn new(mesh: &ArchivedNavMesh) -> Self {
        let origin = mesh.origin();
        let (size, height) = (mesh.cell_size.to_native(), mesh.cell_height.to_native());
        let positions: Vec<Vec3> = (0..mesh.vertices.len())
            .map(|index| {
                let cell = mesh.vertex(index);
                Vec3::new(
                    origin[0] + cell[0] as f32 * size,
                    origin[1] + cell[1] as f32 * height,
                    origin[2] + cell[2] as f32 * size,
                )
            })
            .collect();
        let starts: Vec<u32> = mesh.polygon_starts.iter().map(|v| v.to_native()).collect();
        let corners: Vec<u32> = mesh.corners.iter().map(|v| v.to_native()).collect();
        let neighbours: Vec<u32> = mesh.neighbours.iter().map(|v| v.to_native()).collect();

        let mut middles = vec![Vec3::ZERO; corners.len()];
        let mut bounds = Vec::with_capacity(starts.len().saturating_sub(1));
        for pair in starts.windows(2) {
            let (start, end) = (pair[0] as usize, pair[1] as usize);
            let mut bound = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for corner in start..end {
                let next = if corner + 1 == end { start } else { corner + 1 };
                let (a, b) = (positions[corners[corner] as usize], positions[corners[next] as usize]);
                let sum = a + b;
                middles[corner] = if sum.is_finite() { sum * 0.5 } else { a * 0.5 + b * 0.5 };
                bound = [bound[0].min(a.x), bound[1].min(a.z), bound[2].max(a.x), bound[3].max(a.z)];
            }
            bounds.push(bound);
        }
        let grid = Grid::new(&bounds);
        Self { positions, starts, corners, neighbours, middles, bounds, grid }
    }

    /// How many polygons there are: what a [`PathSearch`](crate::PathSearch)'s pool holds.
    pub fn polygon_count(&self) -> usize {
        self.bounds.len()
    }

    pub(crate) fn corner_range(&self, polygon: u32) -> std::ops::Range<usize> {
        self.starts[polygon as usize] as usize..self.starts[polygon as usize + 1] as usize
    }

    pub(crate) fn neighbour(&self, corner: usize) -> Option<u32> {
        let neighbour = self.neighbours[corner];
        (neighbour != NO_NEIGHBOUR).then_some(neighbour)
    }

    pub(crate) fn middle(&self, corner: usize) -> Vec3 {
        self.middles[corner]
    }

    /// The edge from `corner` to the next corner of `polygon`, as seen walking out across it: the
    /// first point is on the left.
    pub(crate) fn edge(&self, polygon: u32, corner: usize) -> (Vec3, Vec3) {
        let range = self.corner_range(polygon);
        let next = if corner + 1 == range.end { range.start } else { corner + 1 };
        (self.positions[self.corners[corner] as usize], self.positions[self.corners[next] as usize])
    }

    /// A polygon's corner positions, in order.
    pub fn polygon(&self, polygon: u32) -> impl Iterator<Item = Vec3> + '_ {
        self.corner_range(polygon).map(|corner| self.positions[self.corners[corner] as usize])
    }

    /// A polygon's corners' mean, which a convex polygon holds.
    pub fn centre(&self, polygon: u32) -> Vec3 {
        let (mut sum, mut count) = (Vec3::ZERO, 0.0);
        for corner in self.polygon(polygon) {
            sum += corner;
            count += 1.0;
        }
        if sum.is_finite() {
            return sum / count;
        }
        let mut wide = [0.0f64; 3];
        for corner in self.polygon(polygon) {
            for (sum, value) in wide.iter_mut().zip(corner.to_array()) {
                *sum += f64::from(value);
            }
        }
        Vec3::from_array(std::array::from_fn(|axis| {
            if sum[axis].is_finite() { sum[axis] / count } else { (wide[axis] / f64::from(count)) as f32 }
        }))
    }

    /// The polygons across a polygon's edges, for tools and tests.
    pub fn neighbours_of(&self, polygon: u32) -> impl Iterator<Item = u32> + '_ {
        self.corner_range(polygon).filter_map(|corner| self.neighbour(corner))
    }

    /// Whether `point` is over `polygon` on the XZ plane, edges included.
    fn over(&self, polygon: u32, point: Vec3) -> bool {
        let range = self.corner_range(polygon);
        let mut a = self.positions[self.corners[range.end - 1] as usize];
        for corner in range {
            let b = self.positions[self.corners[corner] as usize];
            if cross_xz(a, b, point) < 0.0 {
                return false;
            }
            a = b;
        }
        true
    }

    /// The polygon's height at `point`, which must be over it: the fan of triangles from its first
    /// corner, since a polygon needn't be flat.
    fn height_over(&self, polygon: u32, point: Vec3) -> f32 {
        let range = self.corner_range(polygon);
        let first = self.positions[self.corners[range.start] as usize];
        for corner in range.start + 1..range.end - 1 {
            let b = self.positions[self.corners[corner] as usize];
            let c = self.positions[self.corners[corner + 1] as usize];
            let (d1, d2) = (cross_xz_f32(first, b, point), cross_xz_f32(b, c, point));
            let d3 = cross_xz_f32(c, first, point);
            if !(d1.is_finite() && d2.is_finite() && d3.is_finite()) {
                return self.height_over_wide(polygon, point);
            }
            if d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0 {
                let area = d1 + d2 + d3;
                if area > 0.0 {
                    let numerator = c.y * d1 + first.y * d2 + b.y * d3;
                    let height = numerator / area;
                    if area.is_finite() && numerator.is_finite() && height.is_finite() {
                        return height;
                    }
                    return self.height_over_wide(polygon, point);
                }
            }
        }
        first.y
    }

    /// Recompute the whole fan in the wider domain; mixing a wide area with narrow products
    /// would still overflow and would change ordinary rounding even when no overflow occurs.
    fn height_over_wide(&self, polygon: u32, point: Vec3) -> f32 {
        let range = self.corner_range(polygon);
        let first = self.positions[self.corners[range.start] as usize];
        for corner in range.start + 1..range.end - 1 {
            let b = self.positions[self.corners[corner] as usize];
            let c = self.positions[self.corners[corner + 1] as usize];
            let d1 = cross_xz_wide(first, b, point);
            let d2 = cross_xz_wide(b, c, point);
            let d3 = cross_xz_wide(c, first, point);
            if d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0 {
                let area = d1 + d2 + d3;
                if area > 0.0 {
                    return ((f64::from(c.y) * d1 + f64::from(first.y) * d2 + f64::from(b.y) * d3) / area)
                        as f32;
                }
            }
        }
        first.y
    }

    /// The closest point of a polygon's outline to `point`, on the XZ plane, and its squared
    /// distance there.
    fn closest_on_outline(&self, polygon: u32, point: Vec3) -> (Vec3, f64) {
        let range = self.corner_range(polygon);
        let mut best = (Vec3::ZERO, f64::INFINITY);
        let mut a = self.positions[self.corners[range.end - 1] as usize];
        for corner in range {
            let b = self.positions[self.corners[corner] as usize];
            let (dx, dz) = (b.x - a.x, b.z - a.z);
            let length = dx * dx + dz * dz;
            let numerator = (point.x - a.x) * dx + (point.z - a.z) * dz;
            let parameter = if length > 0.0 { numerator / length } else { 0.0 };
            let t = parameter.clamp(0.0, 1.0);
            let on = a + (b - a) * t;
            let (ox, oz) = (point.x - on.x, point.z - on.z);
            let distance = ox * ox + oz * oz;
            // Check before clamping: an overflowed ratio can clamp to a plausible endpoint.
            let (on, distance) = if length.is_finite()
                && numerator.is_finite()
                && parameter.is_finite()
                && on.is_finite()
                && distance.is_finite()
            {
                (on, f64::from(distance))
            } else {
                closest_on_edge_wide(a, b, point)
            };
            if distance < best.1 {
                best = (on, distance);
            }
            a = b;
        }
        best
    }

    /// Where `point` is on the mesh: the polygon under or over it with the nearest height, or failing
    /// that the nearest outline within `reach`. Among equals, the lowest polygon ID.
    pub fn locate(&self, point: Vec3, reach: Reach) -> Option<Location> {
        let mut best: Option<(f32, u32)> = None;
        for &polygon in self.grid.cell_at(point) {
            let polygon_bound = self.bounds[polygon as usize];
            if point.x < polygon_bound[0]
                || point.z < polygon_bound[1]
                || point.x > polygon_bound[2]
                || point.z > polygon_bound[3]
                || !self.over(polygon, point)
            {
                continue;
            }
            let gap = (self.height_over(polygon, point) - point.y).abs();
            if gap <= reach.vertical && best.is_none_or(|(least, _)| gap < least) {
                best = Some((gap, polygon));
            }
        }
        if let Some((_, polygon)) = best {
            let position = Vec3::new(point.x, self.height_over(polygon, point), point.z);
            return Some(Location { polygon, position });
        }

        let narrow_limit = reach.across * reach.across;
        let limit = if narrow_limit.is_finite() {
            f64::from(narrow_limit)
        } else {
            f64::from(reach.across) * f64::from(reach.across)
        };
        let mut nearest: Option<(f64, Location)> = None;
        let (low, high) = self.grid.span(point, reach.across);
        for z in low[1]..=high[1] {
            for x in low[0]..=high[0] {
                for &polygon in self.grid.cell(x, z) {
                    let bound = self.bounds[polygon as usize];
                    if point.x < bound[0] - reach.across
                        || point.z < bound[1] - reach.across
                        || point.x > bound[2] + reach.across
                        || point.z > bound[3] + reach.across
                    {
                        continue;
                    }
                    let (on, distance) = self.closest_on_outline(polygon, point);
                    if distance > limit || (on.y - point.y).abs() > reach.vertical {
                        continue;
                    }
                    // A polygon large enough to span cells comes up once per cell, and with the same
                    // reading each time, so it can't beat itself.
                    let better = nearest.as_ref().is_none_or(|(least, held)| {
                        distance < *least || (distance == *least && polygon < held.polygon)
                    });
                    if better {
                        nearest = Some((distance, Location { polygon, position: on }));
                    }
                }
            }
        }
        nearest.map(|(_, location)| location)
    }

    /// Whether a character can walk the straight line from `from` to `to` and stay on the mesh, from
    /// polygon to polygon across the edges they share. A mesh is cut back by a capsule's radius, so a line
    /// on it has a capsule's room all along, and "in one room" needs no room: this is what it comes to.
    ///
    /// It answers for walking, not seeing: a pit stops it as a wall does. A line through a corner may
    /// come out at no. So may one of over [`LINE_POLYGONS`], and either costs only a route.
    #[must_use]
    pub fn walks_straight(&self, from: Location, to: Location) -> bool {
        let mut polygon = from.polygon;
        for _ in 0..LINE_POLYGONS {
            if polygon == to.polygon {
                return true;
            }
            // Where the line leaves this polygon: the first of the edges with its end beyond them.
            let mut leaving: Option<(f32, Option<u32>)> = None;
            for corner in self.corner_range(polygon) {
                let (a, b) = self.edge(polygon, corner);
                // In `f32`, as P3 first walked them: `cross_xz` widens only what `f32` can't hold.
                let at_end = cross_xz(a, b, to.position) as f32;
                if at_end >= 0.0 {
                    continue;
                }
                let at_start = cross_xz(a, b, from.position) as f32;
                let along = at_start / (at_start - at_end);
                let across = self.neighbour(corner);
                let first = leaving.is_none_or(|(least, way)| {
                    along < least || (along == least && way.is_none() && across.is_some())
                });
                if first {
                    leaving = Some((along, across));
                }
            }
            match leaving {
                // It ends over this polygon: on the edge of its own, or on another floor than this.
                None => {
                    let gap = (self.height_over(polygon, to.position) - to.position.y).abs();
                    return gap <= Reach::CHARACTER.vertical;
                }
                Some((_, Some(across))) => polygon = across,
                Some((_, None)) => return false,
            }
        }
        false
    }
}

/// How far `c` is to one side of the line from `a` to `b` on the XZ plane, times the line's length:
/// zero or more for a point inside a polygon wound as `sapphire_navmesh` documents.
pub(crate) fn cross_xz(a: Vec3, b: Vec3, c: Vec3) -> f64 {
    let narrow = cross_xz_f32(a, b, c);
    if narrow.is_finite() { f64::from(narrow) } else { cross_xz_wide(a, b, c) }
}

fn cross_xz_f32(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    (b.z - a.z) * (c.x - a.x) - (b.x - a.x) * (c.z - a.z)
}

fn cross_xz_wide(a: Vec3, b: Vec3, c: Vec3) -> f64 {
    (f64::from(b.z) - f64::from(a.z)) * (f64::from(c.x) - f64::from(a.x))
        - (f64::from(b.x) - f64::from(a.x)) * (f64::from(c.z) - f64::from(a.z))
}

fn closest_on_edge_wide(a: Vec3, b: Vec3, point: Vec3) -> (Vec3, f64) {
    let (dx, dz) = (f64::from(b.x) - f64::from(a.x), f64::from(b.z) - f64::from(a.z));
    let length = dx * dx + dz * dz;
    let t = if length > 0.0 {
        (((f64::from(point.x) - f64::from(a.x)) * dx + (f64::from(point.z) - f64::from(a.z)) * dz) / length)
            .clamp(0.0, 1.0)
    } else {
        0.0
    };
    let on = Vec3::from_array(std::array::from_fn(|axis| {
        (f64::from(a[axis]) + (f64::from(b[axis]) - f64::from(a[axis])) * t) as f32
    }));
    // The closest point may need wide projection while its final distance does not. Match
    // locate's reach-squared policy so finite f32 rounding keeps exact-limit inclusivity.
    let (ox, oz) = (point.x - on.x, point.z - on.z);
    let narrow_distance = ox * ox + oz * oz;
    let distance = if narrow_distance.is_finite() {
        f64::from(narrow_distance)
    } else {
        let (ox, oz) = (f64::from(point.x) - f64::from(on.x), f64::from(point.z) - f64::from(on.z));
        ox * ox + oz * oz
    };
    (on, distance)
}

/// Keep every legacy f32 result when it is finite, widening only the exceptional norm.
/// A finite f32 endpoint pair has a finite f64 distance, even across opposite extrema.
pub(crate) fn distance(a: Vec3, b: Vec3) -> f64 {
    let narrow = a.distance(b);
    if narrow.is_finite() {
        return f64::from(narrow);
    }
    let dx = f64::from(a.x) - f64::from(b.x);
    let dy = f64::from(a.y) - f64::from(b.y);
    let dz = f64::from(a.z) - f64::from(b.z);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// An operation-level hybrid rule: exact f32 operands and a finite f32 sum keep legacy rounding.
/// Otherwise add in f64 without discarding an inexact wider operand. No provenance state is needed.
pub(crate) fn add_distances(a: f64, b: f64) -> f64 {
    let (narrow_a, narrow_b) = (a as f32, b as f32);
    let sum = narrow_a + narrow_b;
    if narrow_a.is_finite()
        && narrow_b.is_finite()
        && sum.is_finite()
        && f64::from(narrow_a) == a
        && f64::from(narrow_b) == b
    {
        f64::from(sum)
    } else {
        a + b
    }
}

impl Grid {
    fn new(bounds: &[[f32; 4]]) -> Self {
        let mut low = [f32::MAX; 2];
        let mut high = [f32::MIN; 2];
        for bound in bounds {
            low = [low[0].min(bound[0]), low[1].min(bound[1])];
            high = [high[0].max(bound[2]), high[1].max(bound[3])];
        }
        if bounds.is_empty() {
            low = [0.0; 2];
            high = [0.0; 2];
        }
        let reference_budget = (bounds.len() as u64)
            .saturating_mul(REFERENCES_PER_POLYGON as u64)
            .max(MIN_GRID_REFERENCES as u64)
            .min(u64::from(u32::MAX));
        let mut cell_size = GRID_CELL;
        let mut grid;
        loop {
            // Widen before subtracting: finite f32 endpoints can have an infinite f32 span.
            let dimensions = [
                (f64::from(high[0]) - f64::from(low[0])) / cell_size,
                (f64::from(high[1]) - f64::from(low[1])) / cell_size,
            ];
            if dimensions.iter().any(|&d| d >= MAX_GRID_CELLS as f64) {
                cell_size *= 2.0;
                continue;
            }
            let cells = [dimensions[0] as i32 + 1, dimensions[1] as i32 + 1];
            let count = cells[0] as u64 * cells[1] as u64;
            if count > MAX_GRID_CELLS as u64 {
                cell_size *= 2.0;
                continue;
            }
            grid = Self { min: low, cells, cell_size, starts: Vec::new(), polygons: Vec::new() };
            let mut references = 0u64;
            for bound in bounds {
                let (from, to) = (grid.clamped(bound[0], bound[1]), grid.clamped(bound[2], bound[3]));
                let covered = (to[0] - from[0] + 1) as usize * (to[1] - from[1] + 1) as usize;
                references = references.saturating_add(covered as u64);
                if references > reference_budget {
                    break;
                }
            }
            if references <= reference_budget {
                grid.starts = vec![0; count as usize + 1];
                break;
            }
            cell_size *= 2.0;
        }

        // Count, then fill: polygons go in ascending ID within each cell.
        for bound in bounds {
            let (from, to) = (grid.clamped(bound[0], bound[1]), grid.clamped(bound[2], bound[3]));
            for z in from[1]..=to[1] {
                for x in from[0]..=to[0] {
                    let index = grid.index(x, z);
                    grid.starts[index + 1] += 1;
                }
            }
        }
        for cell in 1..grid.starts.len() {
            grid.starts[cell] += grid.starts[cell - 1];
        }
        grid.polygons = vec![0; *grid.starts.last().unwrap_or(&0) as usize];
        let mut next = grid.starts.clone();
        for (polygon, bound) in bounds.iter().enumerate() {
            let (from, to) = (grid.clamped(bound[0], bound[1]), grid.clamped(bound[2], bound[3]));
            for z in from[1]..=to[1] {
                for x in from[0]..=to[0] {
                    let index = grid.index(x, z);
                    grid.polygons[next[index] as usize] = polygon as u32;
                    next[index] += 1;
                }
            }
        }
        grid
    }

    fn index(&self, x: i32, z: i32) -> usize {
        (x + z * self.cells[0]) as usize
    }

    fn clamped(&self, x: f32, z: f32) -> [i32; 2] {
        [
            (((f64::from(x) - f64::from(self.min[0])) / self.cell_size) as i32).clamp(0, self.cells[0] - 1),
            (((f64::from(z) - f64::from(self.min[1])) / self.cell_size) as i32).clamp(0, self.cells[1] - 1),
        ]
    }

    fn cell(&self, x: i32, z: i32) -> &[u32] {
        let index = self.index(x, z);
        &self.polygons[self.starts[index] as usize..self.starts[index + 1] as usize]
    }

    fn cell_at(&self, point: Vec3) -> &[u32] {
        let [x, z] = self.clamped(point.x, point.z);
        self.cell(x, z)
    }

    /// The cells within `reach` of `point`.
    fn span(&self, point: Vec3, reach: f32) -> ([i32; 2], [i32; 2]) {
        (self.clamped(point.x - reach, point.z - reach), self.clamped(point.x + reach, point.z + reach))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sapphire_navmesh::NavMesh;
    use sapphire_serialize::cooked::AlignedVec;

    /// A hand-made cooked mesh: `rows` by `columns` unit squares, on the ground.
    pub(crate) fn squares(columns: i32, rows: i32, walled: &[(i32, i32)]) -> AlignedVec {
        scaled_squares(columns, rows, walled, 1.0)
    }

    pub(crate) fn scaled_squares(columns: i32, rows: i32, walled: &[(i32, i32)], size: f32) -> AlignedVec {
        let vertex = |x: i32, z: i32| (x + z * (columns + 1)) as u32;
        let square = |x: i32, z: i32| z * columns + x;
        let mut mesh =
            NavMesh { cell_size: size, cell_height: 1.0, polygon_starts: vec![0], ..NavMesh::default() };
        for z in 0..=rows {
            for x in 0..=columns {
                mesh.vertices.push([x, 0, z]);
            }
        }
        let open = |x: i32, z: i32| x >= 0 && z >= 0 && x < columns && z < rows && !walled.contains(&(x, z));
        let mut ids = vec![u32::MAX; (columns * rows) as usize];
        let mut count = 0;
        for z in 0..rows {
            for x in 0..columns {
                if open(x, z) {
                    ids[square(x, z) as usize] = count;
                    count += 1;
                }
            }
        }
        for z in 0..rows {
            for x in 0..columns {
                if !open(x, z) {
                    continue;
                }
                let across = |nx: i32, nz: i32| {
                    if open(nx, nz) { ids[square(nx, nz) as usize] } else { NO_NEIGHBOUR }
                };
                mesh.corners.extend([vertex(x, z), vertex(x, z + 1), vertex(x + 1, z + 1), vertex(x + 1, z)]);
                mesh.neighbours.extend([
                    across(x - 1, z),
                    across(x, z + 1),
                    across(x + 1, z),
                    across(x, z - 1),
                ]);
                mesh.polygon_starts.push(mesh.corners.len() as u32);
            }
        }
        mesh.to_cooked().expect("it writes")
    }

    pub(crate) fn graph(bytes: &AlignedVec) -> NavGraph {
        NavGraph::new(NavMesh::read(bytes).expect("it reads"))
    }

    fn exceptional_quad(size: f32, height: f32, origin: [f32; 3], vertices: Vec<[i32; 3]>) -> NavMesh {
        NavMesh {
            cell_size: size,
            cell_height: height,
            origin,
            vertices,
            polygon_starts: vec![0, 4],
            corners: vec![0, 1, 2, 3],
            neighbours: vec![NO_NEIGHBOUR; 4],
        }
    }

    #[test]
    fn representable_centres_and_heights_survive_intermediate_overflow() {
        let unit = vec![[0, 0, 0], [0, 0, 1], [1, 0, 1], [1, 0, 0]];
        let mesh = exceptional_quad(2.0, 1.0, [0.0, 2e38, 0.0], unit);
        let graph = graph(&mesh.to_cooked().expect("write"));
        assert_eq!(graph.centre(0), Vec3::new(1.0, 2e38, 1.0));
        let point = Vec3::new(1.0, 2e38, 1.0);
        assert_eq!(graph.locate(point, Reach::CHARACTER).expect("over the floor").position, point);

        let mesh =
            exceptional_quad(2e38, 1e38, [0.0; 3], vec![[-1, 0, -1], [-1, 0, 1], [1, 1, 1], [1, 1, -1]]);
        let graph = self::graph(&mesh.to_cooked().expect("write"));
        let found = graph
            .locate(Vec3::ZERO, Reach { across: 0.0, vertical: f32::MAX })
            .expect("over the sloped floor");
        assert_eq!(found.position, Vec3::new(0.0, 5e37, 0.0));
        assert_eq!(graph.centre(0), Vec3::new(0.0, 5e37, 0.0));
    }

    #[test]
    fn wide_outline_projection_keeps_inclusive_limits_and_id_ties() {
        let size = 1e20_f32;
        let mut mesh =
            exceptional_quad(size, 1.0, [0.0; 3], vec![[0, 0, 0], [0, 0, 1], [1, 0, 1], [1, 0, 0]]);
        // Equal outlines remain a lowest-ID tie even when projection needs a wider domain.
        mesh.corners.extend([0, 1, 2, 3]);
        mesh.neighbours.extend([NO_NEIGHBOUR; 4]);
        mesh.polygon_starts.push(8);
        let graph = graph(&mesh.to_cooked().expect("write"));
        // The non-power-of-two gap detects a wide-distance versus rounded-f32-limit mismatch.
        for ulps in [1, 5001] {
            let point = Vec3::new(f32::from_bits(size.to_bits() + ulps), 0.0, size * 0.5);
            let gap = point.x - size;
            let found =
                graph.locate(point, Reach { across: gap, vertical: 1.0 }).expect("right on the reach limit");
            assert_eq!(found.polygon, 0);
            assert_eq!(found.position, Vec3::new(size, 0.0, size * 0.5));
            assert!(graph.locate(point, Reach { across: gap.next_down(), vertical: 1.0 }).is_none());
        }
    }

    #[test]
    fn distance_and_cost_helpers_keep_legacy_bits_or_preserve_wider_operands() {
        for (a, b) in [
            (Vec3::ZERO, Vec3::new(1.0, 2.0, 3.0)),
            (Vec3::new(-10.0, 2.0, 4.0), Vec3::new(300.0, 4.0, -12.0)),
        ] {
            assert_eq!(distance(a, b), f64::from(a.distance(b)));
        }
        assert_eq!(distance(Vec3::ZERO, Vec3::new(1e20, 0.0, 0.0)), f64::from(1e20_f32));
        assert!(distance(Vec3::splat(f32::MIN), Vec3::splat(f32::MAX)).is_finite());
        for (a, b) in [(1.0_f32, 0.1_f32), (2.0, f32::EPSILON * 0.5), (1e10, 3.0)] {
            assert_eq!(add_distances(f64::from(a), f64::from(b)), f64::from(a + b));
        }
        assert_eq!(add_distances(16_777_216.0, 1.0), 16_777_216.0, "legacy rounded tie stays a tie");
        let inexact = 1.0 + 1.0 / (1u64 << 30) as f64;
        assert_ne!(f64::from(inexact as f32), inexact);
        assert_eq!(add_distances(inexact, 1.0), inexact + 1.0);
        let max = f64::from(f32::MAX);
        assert_eq!(add_distances(max, max), max + max);
        assert!(add_distances(max + max, max).is_finite());
    }

    #[test]
    fn sparse_extents_and_large_polygons_have_bounded_index_storage() {
        for bounds in [
            vec![[0.0, 0.0, 1.0, 1.0], [1e20, 1e20, 1e20, 1e20]],
            vec![[-f32::MAX, -f32::MAX, f32::MAX, f32::MAX]],
            vec![[0.0, 0.0, 100_000.0, 100_000.0]; 8],
        ] {
            let grid = Grid::new(&bounds);
            assert!(grid.starts.len() <= MAX_GRID_CELLS + 1);
            assert!(grid.polygons.len() <= (bounds.len() * REFERENCES_PER_POLYGON).max(MIN_GRID_REFERENCES));
            for (id, bound) in bounds.iter().enumerate() {
                for point in [Vec3::new(bound[0], 0.0, bound[1]), Vec3::new(bound[2], 0.0, bound[3])] {
                    assert!(grid.cell_at(point).contains(&(id as u32)));
                }
            }
        }
    }

    /// Whether a line walks between two points on a floor.
    fn walks(graph: &NavGraph, from: (f32, f32), to: (f32, f32)) -> bool {
        let at = |(x, z): (f32, f32)| {
            graph.locate(Vec3::new(x, 0.0, z), Reach::CHARACTER).expect("it's on the floor")
        };
        graph.walks_straight(at(from), at(to))
    }

    #[test]
    fn a_line_over_open_floor_walks_and_one_through_a_wall_doesnt() {
        // A wall along x = 4 from z = 2 to z = 8 of a floor of ten by ten, with a way round each end.
        let walled: Vec<(i32, i32)> = (2..8).map(|z| (4, z)).collect();
        let bytes = squares(10, 10, &walled);
        let graph = graph(&bytes);
        assert!(walks(&graph, (0.5, 0.5), (0.7, 0.9)), "one polygon");
        assert!(walks(&graph, (0.5, 0.5), (9.5, 1.5)), "many, below the wall");
        assert!(walks(&graph, (9.5, 1.5), (0.5, 0.5)), "and back");
        assert!(!walks(&graph, (2.5, 5.5), (6.5, 5.5)), "through the wall");
        assert!(!walks(&graph, (6.5, 5.5), (2.5, 5.5)), "through it the other way");
        assert!(!walks(&graph, (3.5, 1.5), (5.5, 2.5)), "clipping its corner");
        assert!(walks(&graph, (3.5, 1.5), (5.5, 1.5)), "past its end");
    }

    #[test]
    fn a_line_through_a_corner_of_the_grid_takes_the_way_that_goes_on() {
        let bytes = squares(4, 4, &[]);
        let graph = graph(&bytes);
        // Exactly through (1, 1), (2, 2) and (3, 3), where four squares meet.
        assert!(walks(&graph, (0.5, 0.5), (3.5, 3.5)));
        assert!(walks(&graph, (3.5, 3.5), (0.5, 0.5)));
    }

    #[test]
    fn a_line_that_ends_on_a_floor_it_isnt_on_doesnt_walk() {
        // Two floors of a square each, one over the other, joined by nothing. Beside the lower is a
        // neighbour for the line to come from, so it gets as far as standing under its end.
        let mut mesh =
            NavMesh { cell_size: 1.0, cell_height: 1.0, polygon_starts: vec![0], ..NavMesh::default() };
        mesh.vertices.extend([[0, 0, 0], [0, 0, 2], [2, 0, 2], [2, 0, 0], [4, 0, 2], [4, 0, 0]]);
        mesh.vertices.extend([[2, 3, 0], [2, 3, 2], [4, 3, 2], [4, 3, 0]]);
        mesh.corners.extend([0, 1, 2, 3]);
        mesh.neighbours.extend([NO_NEIGHBOUR, NO_NEIGHBOUR, 1, NO_NEIGHBOUR]);
        mesh.polygon_starts.push(4);
        mesh.corners.extend([3, 2, 4, 5]);
        mesh.neighbours.extend([0, NO_NEIGHBOUR, NO_NEIGHBOUR, NO_NEIGHBOUR]);
        mesh.polygon_starts.push(8);
        mesh.corners.extend([6, 7, 8, 9]);
        mesh.neighbours.extend([NO_NEIGHBOUR; 4]);
        mesh.polygon_starts.push(12);
        let bytes = mesh.to_cooked().expect("it writes");
        let graph = graph(&bytes);
        let low = graph.locate(Vec3::new(1.0, 0.0, 1.0), Reach::CHARACTER).expect("the lower floor");
        let beside = graph.locate(Vec3::new(3.0, 0.0, 1.0), Reach::CHARACTER).expect("its neighbour");
        let high = graph.locate(Vec3::new(3.0, 3.0, 1.0), Reach::CHARACTER).expect("the upper floor");
        assert_eq!((low.polygon, beside.polygon, high.polygon), (0, 1, 2));
        assert!(graph.walks_straight(low, beside));
        assert!(!graph.walks_straight(low, high));
    }

    #[test]
    fn a_line_past_what_it_will_cross_says_no() {
        let bytes = squares(i32::try_from(LINE_POLYGONS).unwrap() + 4, 1, &[]);
        let graph = graph(&bytes);
        assert!(walks(&graph, (0.5, 0.5), (LINE_POLYGONS as f32 - 0.5, 0.5)));
        assert!(!walks(&graph, (0.5, 0.5), (LINE_POLYGONS as f32 + 2.5, 0.5)));
    }

    #[test]
    fn a_point_over_a_polygon_finds_it() {
        let graph = graph(&squares(3, 3, &[]));
        let found = graph.locate(Vec3::new(1.5, 0.2, 2.5), Reach::CHARACTER).expect("it's on the mesh");
        assert_eq!(found.polygon, 7);
        assert_eq!(found.position, Vec3::new(1.5, 0.0, 2.5));
    }

    #[test]
    fn a_point_beside_the_mesh_comes_back_to_its_edge_within_reach() {
        let graph = graph(&squares(3, 3, &[]));
        let found = graph.locate(Vec3::new(3.5, 0.0, 0.5), Reach::CHARACTER).expect("within reach");
        assert_eq!(found.position, Vec3::new(3.0, 0.0, 0.5));
        assert_eq!(found.polygon, 2);
        assert_eq!(graph.locate(Vec3::new(5.0, 0.0, 0.5), Reach::CHARACTER), None);
    }

    #[test]
    fn a_floor_too_far_above_isnt_this_floor() {
        let graph = graph(&squares(2, 2, &[]));
        assert_eq!(graph.locate(Vec3::new(0.5, 1.6, 0.5), Reach::CHARACTER), None);
    }

    #[test]
    fn a_point_on_a_shared_edge_takes_the_lowest_id_whichever_way_it_comes() {
        let graph = graph(&squares(2, 1, &[]));
        let found = graph.locate(Vec3::new(1.0, 0.0, 0.5), Reach::CHARACTER).expect("on the mesh");
        assert_eq!(found.polygon, 0);
    }
}
