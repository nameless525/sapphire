//! A cooked mesh in world positions, and where a point is on it.

use glam::Vec3;
use sapphire_navmesh::{ArchivedNavMesh, NO_NEIGHBOUR};

/// Polygons are filed in cells of this size on the XZ plane, in metres. About a building's side: a
/// cell holds a handful in the open and a few dozen around clutter.
const GRID_CELL: f32 = 4.0;

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
                middles[corner] = (a + b) * 0.5;
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
        sum / count
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
            let (d1, d2) = (cross_xz(first, b, point), cross_xz(b, c, point));
            let d3 = cross_xz(c, first, point);
            if d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0 {
                let area = d1 + d2 + d3;
                if area > 0.0 {
                    return (c.y * d1 + first.y * d2 + b.y * d3) / area;
                }
            }
        }
        first.y
    }

    /// The closest point of a polygon's outline to `point`, on the XZ plane, and its squared
    /// distance there.
    fn closest_on_outline(&self, polygon: u32, point: Vec3) -> (Vec3, f32) {
        let range = self.corner_range(polygon);
        let mut best = (Vec3::ZERO, f32::MAX);
        let mut a = self.positions[self.corners[range.end - 1] as usize];
        for corner in range {
            let b = self.positions[self.corners[corner] as usize];
            let (dx, dz) = (b.x - a.x, b.z - a.z);
            let length = dx * dx + dz * dz;
            let t = if length > 0.0 {
                (((point.x - a.x) * dx + (point.z - a.z) * dz) / length).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let on = a + (b - a) * t;
            let (ox, oz) = (point.x - on.x, point.z - on.z);
            let distance = ox * ox + oz * oz;
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

        let limit = reach.across * reach.across;
        let mut nearest: Option<(f32, Location)> = None;
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
}

/// How far `c` is to one side of the line from `a` to `b` on the XZ plane, times the line's length:
/// zero or more for a point inside a polygon wound as `sapphire_navmesh` documents.
pub(crate) fn cross_xz(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    (b.z - a.z) * (c.x - a.x) - (b.x - a.x) * (c.z - a.z)
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
        let cells =
            [((high[0] - low[0]) / GRID_CELL) as i32 + 1, ((high[1] - low[1]) / GRID_CELL) as i32 + 1];
        let mut grid = Self {
            min: low,
            cells,
            starts: vec![0; (cells[0] * cells[1]) as usize + 1],
            polygons: Vec::new(),
        };

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
            (((x - self.min[0]) / GRID_CELL) as i32).clamp(0, self.cells[0] - 1),
            (((z - self.min[1]) / GRID_CELL) as i32).clamp(0, self.cells[1] - 1),
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
        let vertex = |x: i32, z: i32| (x + z * (columns + 1)) as u32;
        let square = |x: i32, z: i32| z * columns + x;
        let mut mesh =
            NavMesh { cell_size: 1.0, cell_height: 1.0, polygon_starts: vec![0], ..NavMesh::default() };
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
