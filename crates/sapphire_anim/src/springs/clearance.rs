//! Presentation contact for the heroine's long hair and deforming shirt.

mod residual;
mod underarm;

use residual::{Residual, short_face::LOCKS, short_face::ShortFace};
use sapphire_assets::mesh::Mesh;
use sapphire_assets::rig::{ArchivedCharacterRig, ArchivedColliderShape, HumanBone};
use sapphire_assets::skeleton::ArchivedSkeleton;
use sapphire_math::{Affine3A, Quat, Vec3};
use underarm::Underarm;

use super::{CharacterPose, Frame, Placed};
use crate::Bind;

const CELL: f32 = 0.01;
const GRID: usize = 128;
const HALF: f32 = GRID as f32 * CELL * 0.5;
const GARMENT_MARGIN: f32 = 0.012;
const RELEASE_HALF_LIFE: f32 = 4.0; // Presentation ticks.
const CLEAR: f32 = 0.0008;
const MAX_TURN_PER_TICK: f32 = 4.5;
const LOOKAHEAD_TICKS: f32 = 2.0;
/// A solve's parts, in the order a frame takes them ([`Clearance::parts`]): the garment posed, its
/// envelope's three axes, the forecast envelope's three, then each strand, the residual and the face's
/// locks.
const POSE: usize = 0;
const AXES: usize = 1;
const AHEAD_AXES: usize = 4;
const STRANDS: usize = 7;
/// What each part of a solve costs, in µs (H5's probe of the walk on Desktop-Low, release, on the M5):
/// what [`Spread`] balances, by their ratios. Posing the garment is mostly the underarm's relief; a face
/// lock's includes its share of the face's field. M4 H7 made most parts cheaper (posing about 74, an
/// axis 62 to 100, a strand 20, the residual 174, a face lock 84) but left these as they are: which
/// tick a part falls on decides what it reads, so weighing them again would move the poses of every
/// tier with an interval.
const POSE_COST: u32 = 212;
const AXIS_COST: u32 = 111;
const AHEAD_AXIS_COST: u32 = 105;
const STRAND_COST: u32 = 6;
const RESIDUAL_COST: u32 = 303;
const FACE_LOCK_COST: u32 = 131;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Influence {
    joint: usize,
    local: Vec3,
    weight: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Point {
    parts: [Influence; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MorphPoint {
    shirt_point: usize,
    target: usize,
    joint_offsets: [Vec3; 4],
}

impl Point {
    fn model(self, pose: &[Affine3A]) -> Vec3 {
        self.parts
            .iter()
            .filter(|p| p.weight > 0.0)
            .fold(Vec3::ZERO, |sum, p| sum + pose[p.joint].transform_point3(p.local) * p.weight)
    }

    /// Split skinning into the part rotated by the root and the part staying put.
    fn split(self, pose: &[Affine3A], chest_from_model: Affine3A, root: Vec3, joints: &[usize]) -> Probe {
        let mut probe = Probe::default();
        for p in self.parts.iter().filter(|p| p.weight > 0.0) {
            let point = chest_from_model.transform_point3(pose[p.joint].transform_point3(p.local));
            if joints.contains(&p.joint) {
                probe.rotating += (point - root) * p.weight;
                probe.fixed += root * p.weight;
            } else {
                probe.fixed += point * p.weight;
            }
        }
        probe
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Probe {
    fixed: Vec3,
    rotating: Vec3,
    radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Limb {
    joint: usize,
    offset: Vec3,
    tail: Vec3,
    radius: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Relief {
    direction: Vec3,
    rotation: Quat,
}

impl Relief {
    fn rotation(along: Vec3, direction: Vec3, degrees: f32) -> Quat {
        along
            .cross(direction)
            .try_normalize()
            .map_or(Quat::IDENTITY, |axis| Quat::from_axis_angle(axis, degrees.to_radians()))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Strand {
    joints: Vec<usize>,
    samples: Vec<Point>,
    side: f32,
    front: f32,
    shoulder_route: Option<usize>,
    relief: Relief,
    previous_probes: Vec<Probe>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Interval {
    lo: f32,
    hi: f32,
}

impl Interval {
    const EMPTY: Self = Self { lo: f32::INFINITY, hi: f32::NEG_INFINITY };

    fn add(&mut self, v: f32) {
        self.lo = self.lo.min(v);
        self.hi = self.hi.max(v);
    }
}

/// Orthographic depth ranges along the chest's local axes. A point must lie inside all
/// three projections to belong to the garment envelope.
///
/// An axis's cells are kept by tiles, each rastered for the refit it was last rastered for. Where the
/// strands read an axis in the same update it refits in, it rasters as it refits the tiles read since
/// it last did, and any other when one of its cells is first read, from the triangles whose bounds
/// reach it, so only the tiles read are rastered. Where they read it on a later tick of the interval,
/// it rasters every triangle at once as it refits, so the work stays on the axis's own tick. A cell's
/// range is the least and greatest depth its triangles give, which no order changes, so either way it
/// comes out as a raster of the whole grid would leave it.
#[derive(Clone, Debug, PartialEq)]
struct Garment {
    /// The garment's triangles, in the order of `nodes`' leaves.
    triangles: Vec<[usize; 3]>,
    /// A tree over `triangles`, grouped as they lie at rest: each node's left child follows it.
    nodes: Vec<RasterNode>,
    /// Per node, the bounds of its triangles where the garment was last posed.
    bounds: Vec<(Vec3, Vec3)>,
    /// Where the garment was last posed, which its axes raster.
    vertices: Vec<Vec3>,
    /// Per axis, each cell's depth range.
    grids: [Vec<Interval>; 3],
    /// Per axis, the refit each tile of [`TILE`] by [`TILE`] cells was last rastered for, and read for.
    tiles: [Vec<u32>; 3],
    read: [Vec<u32>; 3],
    /// Per axis, the tiles read since it last refit, which it rasters as it next does.
    reading: [Vec<usize>; 3],
    /// Per axis, how many times it has refit: a cell rastered for an earlier refit is stale.
    refits: [u32; 3],
    min: Vec3,
    max: Vec3,
}

/// A node of [`Garment::nodes`]: its triangles, and its right child if it has children.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RasterNode {
    start: usize,
    end: usize,
    right: Option<usize>,
}

/// The side of a tile of a garment's grid, in cells, which rasters as one.
const TILE: usize = 8;
const TILES: usize = GRID / TILE;

/// The cell along a grid's axis that `x` falls in, clamped to the grid.
fn cell_index(x: f32) -> usize {
    (((x + HALF) / CELL).floor() as isize).clamp(0, GRID as isize - 1) as usize
}

/// Adds to `grid`, across `axis`, the depth of the triangle `a`, `b`, `c` at the centre of each cell
/// of `cells` (its columns, then its rows, inclusive) that it covers.
fn cover(
    grid: &mut [Interval],
    axis: usize,
    [a, b, c]: [Vec3; 3],
    (xs, ys): ((usize, usize), (usize, usize)),
) {
    let u = (axis + 1) % 3;
    let v = (axis + 2) % 3;
    let coordinate = |i: usize| (i as f32 + 0.5) * CELL - HALF;
    let ab = b - a;
    let ac = c - a;
    let area = ab[u] * ac[v] - ab[v] * ac[u];
    if area.abs() < 1.0e-9 {
        return;
    }
    let inverse = area.recip();
    for y in ys.0..=ys.1 {
        let dy = coordinate(y) - a[v];
        for x in xs.0..=xs.1 {
            let dx = coordinate(x) - a[u];
            let s = (dx * ac[v] - dy * ac[u]) * inverse;
            let t = (ab[u] * dy - ab[v] * dx) * inverse;
            if s >= 0.0 && t >= 0.0 && s + t <= 1.0 {
                grid[y * GRID + x].add(a[axis] + s * ab[axis] + t * ac[axis]);
            }
        }
    }
}

impl Garment {
    /// An envelope of `triangles`, grouped by where they lie on `rest`.
    fn new(rest: &[Vec3], triangles: &[[usize; 3]]) -> Self {
        let mut ordered: Vec<_> =
            triangles.iter().map(|&t| ((rest[t[0]] + rest[t[1]] + rest[t[2]]) / 3.0, t)).collect();
        let mut nodes = Vec::new();
        Self::layout(&mut ordered, 0, &mut nodes);
        Self {
            triangles: ordered.into_iter().map(|(_, t)| t).collect(),
            bounds: vec![(Vec3::ZERO, Vec3::ZERO); nodes.len()],
            nodes,
            vertices: vec![Vec3::ZERO; rest.len()],
            grids: std::array::from_fn(|_| vec![Interval::EMPTY; GRID * GRID]),
            tiles: std::array::from_fn(|_| vec![0; TILES * TILES]),
            read: std::array::from_fn(|_| vec![0; TILES * TILES]),
            reading: std::array::from_fn(|_| Vec::with_capacity(TILES * TILES)),
            refits: [0; 3],
            min: Vec3::ZERO,
            max: Vec3::ZERO,
        }
    }

    /// Splits `triangles` at the middle of their widest axis until a leaf holds 8 or fewer.
    fn layout(triangles: &mut [(Vec3, [usize; 3])], start: usize, nodes: &mut Vec<RasterNode>) {
        let at = nodes.len();
        nodes.push(RasterNode { start, end: start + triangles.len(), right: None });
        if triangles.len() <= 8 {
            return;
        }
        let (lo, hi) = triangles
            .iter()
            .fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(lo, hi), (c, _)| {
                (lo.min(*c), hi.max(*c))
            });
        let extent = hi - lo;
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        triangles.sort_unstable_by(|a, b| a.0[axis].total_cmp(&b.0[axis]).then(a.1.cmp(&b.1)));
        let middle = triangles.len() / 2;
        let (left, right) = triangles.split_at_mut(middle);
        Self::layout(left, start, nodes);
        nodes[at].right = Some(nodes.len());
        Self::layout(right, start + middle, nodes);
    }

    #[cfg(test)]
    fn refit(&mut self, vertices: &[Vec3]) {
        self.bound(vertices);
        for axis in 0..3 {
            self.refit_axis(axis, false);
        }
    }

    /// Poses the envelope at `vertices`, which its axes are then refit from, and bounds it.
    fn bound(&mut self, vertices: &[Vec3]) {
        self.vertices.copy_from_slice(vertices);
        self.min = vertices.iter().copied().fold(Vec3::splat(f32::INFINITY), Vec3::min);
        self.max = vertices.iter().copied().fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
        for (i, node) in self.nodes.iter().enumerate().rev() {
            self.bounds[i] = if let Some(right) = node.right {
                let (a, b) = (self.bounds[i + 1], self.bounds[right]);
                (a.0.min(b.0), a.1.max(b.1))
            } else {
                self.triangles[node.start..node.end].iter().fold(
                    (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
                    |(lo, hi), triangle| {
                        let [a, b, c] = triangle.map(|i| vertices[i]);
                        (lo.min(a).min(b).min(c), hi.max(a).max(b).max(c))
                    },
                )
            };
        }
    }

    /// Refits `axis` to where the garment was last posed, rastering every tile the garment reaches if
    /// `whole`, or else the tiles read since it last refit: the others raster as they're read.
    fn refit_axis(&mut self, axis: usize, whole: bool) {
        self.refits[axis] = self.refits[axis].wrapping_add(1);
        let mut reading = std::mem::take(&mut self.reading[axis]);
        if whole {
            self.raster_whole(axis);
        } else {
            for &tile in &reading {
                self.raster(axis, tile % TILES * TILE, tile / TILES * TILE);
                self.tiles[axis][tile] = self.refits[axis];
            }
        }
        reading.clear();
        self.reading[axis] = reading;
    }

    /// Rasters across `axis` every tile the garment reaches, from every triangle at once: the tiles
    /// past it are rastered if they're read.
    fn raster_whole(&mut self, axis: usize) {
        let Self { triangles, vertices, grids, tiles, refits, min, max, .. } = self;
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        let xs = (cell_index(min[u]) / TILE, cell_index(max[u]) / TILE);
        let ys = (cell_index(min[v]) / TILE, cell_index(max[v]) / TILE);
        let grid = &mut grids[axis];
        for y in ys.0 * TILE..(ys.1 + 1) * TILE {
            grid[y * GRID + xs.0 * TILE..y * GRID + (xs.1 + 1) * TILE].fill(Interval::EMPTY);
        }
        for triangle in triangles.iter() {
            let [a, b, c] = triangle.map(|i| vertices[i]);
            let (low, high) = (a.min(b).min(c), a.max(b).max(c));
            let cells =
                ((cell_index(low[u]), cell_index(high[u])), (cell_index(low[v]), cell_index(high[v])));
            cover(grid, axis, [a, b, c], cells);
        }
        for y in ys.0..=ys.1 {
            tiles[axis][y * TILES + xs.0..=y * TILES + xs.1].fill(refits[axis]);
        }
    }

    /// The depth range of cell `x`, `y` across `axis`, its tile rastered if it hasn't been since the
    /// axis refit.
    fn cell(&mut self, axis: usize, x: usize, y: usize) -> Interval {
        let tile = (y / TILE) * TILES + x / TILE;
        if self.read[axis][tile] != self.refits[axis] {
            self.read[axis][tile] = self.refits[axis];
            self.reading[axis].push(tile);
        }
        if self.tiles[axis][tile] != self.refits[axis] {
            self.raster(axis, x / TILE * TILE, y / TILE * TILE);
            self.tiles[axis][tile] = self.refits[axis];
        }
        self.grids[axis][y * GRID + x]
    }

    /// Rasters across `axis` the tile whose first column is `x0` and first row `y0`: the depths along
    /// it of the triangles covering each of its cells' centres, as a raster of the whole grid would.
    fn raster(&mut self, axis: usize, x0: usize, y0: usize) {
        let Self { triangles, nodes, bounds, vertices, grids, .. } = self;
        let grid = &mut grids[axis];
        let (x1, y1) = (x0 + TILE - 1, y0 + TILE - 1);
        for y in y0..=y1 {
            grid[y * GRID + x0..=y * GRID + x1].fill(Interval::EMPTY);
        }
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        // The columns and rows of the tile that `min` to `max` reach, if any.
        let reach = |min: Vec3, max: Vec3| {
            let x = (cell_index(min[u]).max(x0), cell_index(max[u]).min(x1));
            let y = (cell_index(min[v]).max(y0), cell_index(max[v]).min(y1));
            (x.0 <= x.1 && y.0 <= y.1).then_some((x, y))
        };
        let mut stack = [0; 64];
        let mut depth = 1;
        while depth > 0 {
            depth -= 1;
            let i = stack[depth];
            let (min, max) = bounds[i];
            if reach(min, max).is_none() {
                continue;
            }
            let node = nodes[i];
            if let Some(right) = node.right {
                stack[depth] = right;
                stack[depth + 1] = i + 1;
                depth += 2;
                continue;
            }
            for triangle in &triangles[node.start..node.end] {
                let [a, b, c] = triangle.map(|i| vertices[i]);
                if let Some(cells) = reach(a.min(b).min(c), a.max(b).max(c)) {
                    cover(grid, axis, [a, b, c], cells);
                }
            }
        }
    }

    fn within(&self, point: Vec3, margin: f32) -> bool {
        let margin = Vec3::splat(margin);
        (self.min - point).cmple(margin).all() && (point - self.max).cmple(margin).all()
    }

    fn depth(&mut self, point: Vec3, radius: f32) -> f32 {
        if !self.within(point, GARMENT_MARGIN + radius) || point.abs().max_element() >= HALF - CELL {
            return 0.0;
        }
        let sample = (point + Vec3::splat(HALF)) / CELL - Vec3::splat(0.5);
        let mut depth = f32::INFINITY;
        for axis in 0..3 {
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            let x = sample[u] as usize;
            let y = sample[v] as usize;
            let fx = sample[u] - x as f32;
            let fy = sample[v] - y as f32;
            let at = |a: Interval| (point[axis] - a.lo).min(a.hi - point[axis]);
            // Fade the edge of the sampled silhouette over a cell so entry stays continuous.
            let interpolate = |a: f32, b: f32, weight: f32| a + (b - a) * weight;
            let mut at = |x: usize, y: usize| at(self.cell(axis, x, y)).clamp(-CELL * 2.0, 1.0);
            let top = interpolate(at(x, y), at(x + 1, y), fx);
            let bottom = interpolate(at(x, y + 1), at(x + 1, y + 1), fx);
            let distance = interpolate(top, bottom, fy);
            depth = depth.min(distance + GARMENT_MARGIN + radius);
            if depth <= 0.0 {
                return 0.0;
            }
        }
        depth
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Clearance {
    chest: usize,
    shirt: Vec<Point>,
    shirt_morphs: Vec<MorphPoint>,
    triangles: Vec<[usize; 3]>,
    limbs: Vec<Limb>,
    strands: Vec<Strand>,
    garment: Garment,
    ahead_garment: Garment,
    posed_shirt: Vec<Vec3>,
    previous_shirt: Vec<Vec3>,
    ahead_shirt: Vec<Vec3>,
    posed_limbs: Vec<Placed>,
    previous_limbs: Vec<Placed>,
    ahead_limbs: Vec<Placed>,
    probes: Vec<Probe>,
    ahead_probes: Vec<Probe>,
    has_history: bool,
    waking: bool,
    residual: Option<Residual>,
    short_face: Option<ShortFace>,
    underarm: Option<Underarm>,
    /// How far ahead, in multiples of its motion since the one before, the garment was last posed
    /// forecast: the strands' solves forecast their own probes as far.
    forecast: f32,
    /// Per part of a solve ([`Self::parts`]), the presented ticks drawn since it last solved, which it
    /// takes as its elapsed time when it next does.
    pending: Vec<f64>,
    /// The tick of the last update at an interval: `None` solves every part at the next.
    last: Option<i64>,
    /// The tick of its interval the next tick drawn is: the parts from here on haven't solved in it.
    phase: u32,
    /// The tick of its interval each part solves on.
    spread: Spread,
}

/// Where a solve's parts fall in its interval, each on one tick, so that no tick carries much more than
/// its share of the solve. The parts that read what others solve keep their order over the interval, so
/// each sees what those before it solved in the same one; the free parts, which read only the pose, go
/// wherever is lightest. Deterministic: the same interval always spreads the same way, so a replay
/// solves each part on the same frames every run.
#[derive(Clone, Debug, PartialEq)]
struct Spread {
    interval: u32,
    /// Per part, the tick of the interval it solves on, from 0.
    ticks: Vec<u32>,
    /// Per tick of the interval, what its parts cost.
    loads: Vec<u32>,
}

impl Spread {
    fn new(parts: usize) -> Self {
        Self { interval: 1, ticks: vec![0; parts], loads: vec![0] }
    }

    /// Spreads the parts over `interval` ticks by `cost`, those not `free` in order: the least load a
    /// tick must take for them all to fit, filling the ticks in turn with the ordered parts, then each
    /// free part on the lightest tick.
    fn fit(&mut self, interval: u32, cost: impl Fn(usize) -> u32, free: impl Fn(usize) -> bool) {
        let interval = interval.max(1);
        self.interval = interval;
        self.loads.clear();
        self.loads.resize(interval as usize, 0);
        let fits = |capacity: u32, ticks: &mut [u32], loads: &mut [u32]| {
            loads.fill(0);
            let mut tick = 0;
            for part in (0..ticks.len()).filter(|&part| !free(part)) {
                let cost = cost(part);
                if loads[tick] > 0 && loads[tick] + cost > capacity {
                    tick += 1;
                    if tick == loads.len() {
                        return false;
                    }
                }
                loads[tick] += cost;
                ticks[part] = tick as u32;
            }
            for part in (0..ticks.len()).filter(|&part| free(part)) {
                let (tick, &load) =
                    loads.iter().enumerate().min_by_key(|&(_, &load)| load).expect("an interval has a tick");
                if load + cost(part) > capacity {
                    return false;
                }
                loads[tick] += cost(part);
                ticks[part] = tick as u32;
            }
            true
        };
        let (most, total) = (0..self.ticks.len())
            .map(&cost)
            .fold((0, 0), |(most, total), cost| (most.max(cost), total + cost));
        // Everything on the first tick always fits.
        let (mut least, mut enough) = (most.max(total.div_ceil(interval)).min(total), total);
        while least < enough {
            let middle = least + (enough - least) / 2;
            if fits(middle, &mut self.ticks, &mut self.loads) {
                enough = middle;
            } else {
                least = middle + 1;
            }
        }
        let fitted = fits(least, &mut self.ticks, &mut self.loads);
        debug_assert!(fitted, "a solve's parts fit {least} a tick");
    }
}

impl Clearance {
    pub(super) fn new(mesh: &Mesh, skeleton: &ArchivedSkeleton, rig: &ArchivedCharacterRig) -> Option<Self> {
        let skin = mesh.skin.as_ref()?;
        if skin.joint_count as usize != skeleton.names.len()
            || skin.joint_count != rig.joint_count.to_native()
        {
            return None;
        }
        let binds = Bind::from_skin(&skin.binds);
        let point = |index: usize| {
            let position = Vec3::from_array(mesh.streams.vertex(index)?.position);
            let influence = skin.influences.get(index)?;
            Some((
                position,
                Point {
                    parts: std::array::from_fn(|i| {
                        let bind = binds[usize::from(influence.binds[i])];
                        Influence {
                            joint: usize::from(bind.joint),
                            local: bind.inverse_bind.transform_point3(position),
                            weight: f32::from(influence.weights[i]) / 255.0,
                        }
                    }),
                },
            ))
        };
        let indices = |material: &str| {
            mesh.lods.first()?.submeshes.iter().find_map(|part| {
                let slot = mesh.materials.get(part.material as usize)?;
                let start = part.first_index as usize;
                (slot.name == material).then(|| &mesh.indices[start..start + part.index_count as usize])
            })
        };
        let joint = |bone| {
            rig.humanoid
                .iter()
                .find(|entry| entry.bone == bone)
                .map(|entry| usize::from(entry.joint.to_native()))
        };
        let chest = joint(HumanBone::UpperChest)?;
        let mut shirt = Vec::new();
        let mut rest = Vec::new();
        let mut triangles = Vec::new();
        let mut mapped = vec![None; mesh.streams.len()];
        for face in indices("Tops_01_CLOTH")?.as_chunks::<3>().0 {
            let mut triangle = [0; 3];
            for (corner, &index) in triangle.iter_mut().zip(face) {
                let index = index as usize;
                *corner = if let Some(existing) = mapped[index] {
                    existing
                } else {
                    let next = shirt.len();
                    let (position, point) = point(index)?;
                    shirt.push(point);
                    rest.push(position);
                    mapped[index] = Some(next);
                    next
                };
            }
            triangles.push(triangle);
        }
        let mut shirt_morphs = Vec::new();
        for (target, shape) in mesh.morph_targets.iter().enumerate() {
            let start = shape.first_delta as usize;
            for delta in &mesh.morph_deltas[start..start + shape.delta_count as usize] {
                let index = delta.vertex() as usize;
                let offset = Vec3::from_array(delta.position());
                if let Some(shirt_point) = mapped[index].filter(|_| offset != Vec3::ZERO) {
                    let influence = skin.influences.get(index)?;
                    let joint_offsets = std::array::from_fn(|i| {
                        binds[usize::from(influence.binds[i])].inverse_bind.transform_vector3(offset)
                    });
                    shirt_morphs.push(MorphPoint { shirt_point, target, joint_offsets });
                }
            }
        }
        let underarm = Underarm::new(mesh, skeleton, &binds, &triangles, &mapped, point, &shirt_morphs);
        let mut hair = indices("Hair_00_HAIR")?.to_vec();
        hair.sort_unstable();
        hair.dedup();
        let points: Vec<_> = hair.into_iter().filter_map(|index| point(index as usize)).collect();
        let mut strands = Vec::new();
        for spring in rig.springs.iter().filter(|spring| spring.name.as_str() == "Hair") {
            let joints: Vec<_> = spring.joints.iter().map(|j| usize::from(j.joint.to_native())).collect();
            if joints.len() < 2 {
                return None;
            }
            let name = skeleton.names[joints[0]].as_str();
            let id: u8 = name.strip_prefix("J_Sec_Hair1_")?.parse().ok()?;
            let candidates: Vec<_> = points
                .iter()
                .copied()
                .filter(|(_, p)| {
                    p.parts.iter().filter(|p| joints.contains(&p.joint)).map(|p| p.weight).sum::<f32>() > 0.60
                })
                .collect();
            let side = if matches!(id, 3..=7 | 12) { 1.0 } else { -1.0 };
            let front = if matches!(id, 1..=4 | 11 | 12) { 1.0 } else { -1.0 };
            let shoulder_route = match id {
                5 => joint(HumanBone::LeftUpperArm),
                10 => joint(HumanBone::RightUpperArm),
                _ => None,
            };
            let samples = spread(&candidates);
            let probe_count = samples.len() + (joints.len() - 1) * 6;
            strands.push(Strand {
                joints,
                samples,
                side,
                front,
                shoulder_route,
                relief: Relief::default(),
                previous_probes: Vec::with_capacity(probe_count),
            });
        }
        if strands.len() != 12 || shirt.is_empty() || strands.iter().any(|strand| strand.samples.is_empty()) {
            return None;
        }
        let arm_joints = [
            HumanBone::LeftUpperArm,
            HumanBone::LeftLowerArm,
            HumanBone::LeftHand,
            HumanBone::RightUpperArm,
            HumanBone::RightLowerArm,
            HumanBone::RightHand,
        ]
        .map(joint);
        let limbs: Vec<_> = rig
            .colliders
            .iter()
            .filter_map(|collider| {
                let joint = usize::from(collider.joint.to_native());
                if !arm_joints.contains(&Some(joint)) {
                    return None;
                }
                let (offset, tail, radius) = match &collider.shape {
                    ArchivedColliderShape::Sphere { offset, radius } => {
                        let center = Vec3::from_array(offset.map(|v| v.to_native()));
                        (center, center, radius.to_native())
                    }
                    ArchivedColliderShape::Capsule { offset, tail, radius } => (
                        Vec3::from_array(offset.map(|v| v.to_native())),
                        Vec3::from_array(tail.map(|v| v.to_native())),
                        radius.to_native(),
                    ),
                };
                Some(Limb { joint, offset, tail, radius })
            })
            .collect();
        let residual = Residual::new(mesh, skeleton, &strands, &mapped, point);

        let short_face = {
            ShortFace::new(
                mesh,
                skeleton,
                &strands,
                point,
                |joint| {
                    binds.iter().find(|bind| usize::from(bind.joint) == joint).map(|bind| bind.inverse_bind)
                },
                |index, offset| {
                    let influence = skin.influences.get(index).expect("validated face skin point");
                    std::array::from_fn(|i| {
                        binds[usize::from(influence.binds[i])].inverse_bind.transform_vector3(offset)
                    })
                },
            )
        };
        let shirt_points = vec![Vec3::ZERO; shirt.len()];
        let parts = Self::parts(strands.len());
        debug_assert!(parts <= 32, "a solve's parts are bits of a u32");
        let placed = vec![Placed::default(); limbs.len()];
        let probe_count =
            strands.iter().map(|strand| strand.previous_probes.capacity()).max().unwrap_or_default();
        let garment = Garment::new(&rest, &triangles);
        Some(Self {
            chest,
            shirt,
            shirt_morphs,
            triangles,
            limbs,
            strands,
            ahead_garment: garment.clone(),
            garment,
            posed_shirt: shirt_points.clone(),
            previous_shirt: shirt_points.clone(),
            ahead_shirt: shirt_points,
            posed_limbs: placed.clone(),
            previous_limbs: placed.clone(),
            ahead_limbs: placed,
            probes: Vec::with_capacity(probe_count),
            ahead_probes: Vec::with_capacity(probe_count),
            has_history: false,
            waking: false,
            residual,
            short_face,
            underarm,
            forecast: 0.0,
            pending: vec![0.0; parts],
            last: None,
            phase: 0,
            spread: Spread::new(parts),
        })
    }

    /// How many parts a solve has with `strands`: the garment posed, its envelopes' axes, each strand,
    /// the residual and each of the face's locks.
    fn parts(strands: usize) -> usize {
        STRANDS + strands + 1 + LOCKS
    }

    fn strand_part(strand: usize) -> usize {
        STRANDS + strand
    }

    fn residual_part(&self) -> usize {
        STRANDS + self.strands.len()
    }

    fn face_part(&self, lock: usize) -> usize {
        self.residual_part() + 1 + lock
    }

    /// Every part, a bit each.
    fn every_part(&self) -> u32 {
        u32::MAX >> (32 - self.pending.len())
    }

    /// What a part of the solve costs at `interval`, for [`Spread`] to balance. The forecast envelope's
    /// axes cost nothing where the interval is too long for a forecast, and a part absent nothing.
    fn cost(&self, part: usize, interval: u32) -> u32 {
        match part {
            POSE => POSE_COST,
            AXES..AHEAD_AXES => AXIS_COST,
            AHEAD_AXES..STRANDS => {
                if interval < 4 {
                    AHEAD_AXIS_COST
                } else {
                    0
                }
            }
            _ if part < self.residual_part() => STRAND_COST,
            _ if part == self.residual_part() => self.residual.as_ref().map_or(0, |_| RESIDUAL_COST),
            _ => self.short_face.as_ref().map_or(0, |_| FACE_LOCK_COST),
        }
    }

    pub(super) fn reset(&mut self) {
        if let Some(residual) = &mut self.residual {
            residual.reset();
        }
        if let Some(face) = &mut self.short_face {
            face.reset();
        }
        self.has_history = false;
        self.waking = false;
        self.pending.fill(0.0);
        self.last = None;
        for strand in &mut self.strands {
            strand.relief = Relief::default();
        }
    }

    pub(super) fn sleep(&mut self) {
        // The character may return before its owner clock has advanced. Keep the drawn
        // orientation and discard only the motion estimate, which has a gap now.
        self.has_history = false;
        self.waking = true;
        self.pending.fill(0.0);
        self.last = None;
        if let Some(residual) = &mut self.residual {
            residual.sleep();
        }
        if let Some(face) = &mut self.short_face {
            face.sleep();
        }
    }

    pub(super) fn settle(&mut self, pose: &mut CharacterPose, wake_elapsed: f32) {
        let elapsed = self.waking.then_some(wake_elapsed);
        self.waking = false;
        self.solve_all(pose, elapsed);
    }

    /// Solves every part over `elapsed`, `None` across a cut.
    fn solve_all(&mut self, pose: &mut CharacterPose, elapsed: Option<f32>) {
        self.pending.fill(0.0);
        self.run(pose, self.every_part(), |_| elapsed);
    }

    /// Solves the parts whose ticks in `interval` have come round since the update before, each taking
    /// every tick drawn since it last solved as its elapsed time, and writes what the others last
    /// solved. An interval of 1 solves every part on every update with `span`, the ticks since the
    /// last, and `None` holds.
    pub(super) fn update(&mut self, pose: &mut CharacterPose, ticks: f64, span: f64, interval: Option<u32>) {
        for pending in &mut self.pending {
            *pending += span;
        }
        let due = match interval {
            None => 0,
            Some(0 | 1) => {
                self.last = None;
                self.every_part()
            }
            Some(interval) => self.due(ticks.floor() as i64, interval),
        };
        self.run(pose, due, |pending| Some(pending as f32));
    }

    /// The parts due at `tick`, a bit each: those on the ticks of `interval` this update reaches from
    /// where the last left off. An interval runs to its end before the next begins, so its parts solve
    /// in their order even where a frame draws more than one tick, and its last frame leaves any ticks
    /// over. The first update at an interval, or one a whole interval late, solves every part and
    /// begins the next.
    fn due(&mut self, tick: i64, interval: u32) -> u32 {
        let Some(last) = self.last else {
            self.last = Some(tick);
            self.phase = 0;
            return self.every_part();
        };
        let ticks = tick - last;
        if ticks <= 0 {
            return 0;
        }
        self.last = Some(tick);
        if self.spread.interval != interval {
            let mut costs = [0; 32];
            for (part, cost) in costs.iter_mut().enumerate().take(self.pending.len()) {
                *cost = self.cost(part, interval);
            }
            let residual = self.residual_part();
            self.spread.fit(interval, |part| costs[part], |part| part > residual);
            self.phase = 0;
        }
        if ticks >= i64::from(interval) {
            self.phase = 0;
            return self.every_part();
        }
        let from = self.phase;
        let to = (from + ticks as u32).min(interval);
        self.phase = if to == interval { 0 } else { to };
        let mut due = 0;
        for (part, &at) in self.spread.ticks.iter().enumerate() {
            if (from..to).contains(&at) {
                due |= 1 << part;
            }
        }
        due
    }

    pub(super) fn continue_from(&mut self, old: &Self) {
        let same = self.chest == old.chest
            && self.shirt == old.shirt
            && self.shirt_morphs == old.shirt_morphs
            && self.triangles == old.triangles
            && self.limbs == old.limbs
            && self.strands.len() == old.strands.len()
            && self.strands.iter().zip(&old.strands).all(|(a, b)| {
                a.joints == b.joints && a.samples == b.samples && a.shoulder_route == b.shoulder_route
            });
        if !same {
            self.reset();
            self.waking = true;
            return;
        }
        if let (Some(residual), Some(old)) = (&mut self.residual, &old.residual) {
            residual.continue_from(old);
        }
        if let (Some(face), Some(old)) = (&mut self.short_face, &old.short_face) {
            face.continue_from(old);
        }
        self.has_history = old.has_history;
        self.waking = old.waking;
        self.pending.copy_from_slice(&old.pending);
        // This one's garment has no envelope yet, so its next update solves every part, the garment's
        // motion measured from where the old one last posed it.
        self.last = None;
        self.posed_shirt.copy_from_slice(&old.posed_shirt);
        self.posed_limbs.copy_from_slice(&old.posed_limbs);
        for (new, old) in self.strands.iter_mut().zip(&old.strands) {
            new.relief = old.relief;
            new.previous_probes.clone_from(&old.previous_probes);
        }
    }

    pub(super) fn underarm_morph(&self) -> Option<(&'static str, usize)> {
        self.underarm.as_ref().map(Underarm::morph_slot)
    }

    pub(super) fn protected_write(&self, conflicts: impl Fn(usize) -> bool) -> Option<(&'static str, usize)> {
        self.strands
            .iter()
            .map(|strand| strand.joints[0])
            .find(|&joint| conflicts(joint))
            .map(|joint| ("garment relief", joint))
            .or_else(|| {
                self.residual
                    .as_ref()
                    .and_then(|residual| residual.protected_write(&conflicts))
                    .map(|joint| ("garment residual", joint))
            })
            .or_else(|| {
                self.short_face
                    .as_ref()?
                    .protected_write(conflicts)
                    .map(|joint| ("short face clearance", joint))
            })
    }

    /// Solves the parts in `due`, a bit each, on the pose as it is now, each over `elapsed` of the ticks
    /// drawn since it last solved, and writes what the others last solved, as offsets on the pose as it
    /// is now. Parts go in order, so a strand solved here sees this frame's envelopes if they solved,
    /// and the residual and the face this frame's strands.
    fn run(&mut self, pose: &mut CharacterPose, due: u32, elapsed: impl Fn(f64) -> Option<f32>) {
        // Posing the garment moves no joint, so the chest's frame stands through it.
        let chest_from_model = pose.model()[self.chest].inverse();
        match take(&mut self.pending, due, POSE, &elapsed) {
            Some(elapsed) => self.pose_garment(pose, chest_from_model, elapsed),
            None => {
                if let Some(underarm) = &self.underarm {
                    underarm.hold(pose);
                }
            }
        }
        // An envelope the strands read in this update rasters only what they read; one they read on a
        // later tick rasters whole now, on its axis's own tick.
        let strands = (STRANDS..self.residual_part()).fold(0, |bits, part| bits | 1 << part);
        let whole = due & strands == 0;
        for axis in 0..3 {
            if take(&mut self.pending, due, AXES + axis, &elapsed).is_some() {
                self.garment.refit_axis(axis, whole);
            }
            if take(&mut self.pending, due, AHEAD_AXES + axis, &elapsed).is_some() && self.forecast > 0.0 {
                self.ahead_garment.refit_axis(axis, whole);
            }
        }
        let chest = Frame::of(&pose.model()[self.chest]);
        let forecast = self.forecast;
        let predict = |now: Vec3, before: Vec3| now + ((now - before) * forecast).clamp_length_max(0.07);
        let mut wrote = false;
        for (index, strand) in self.strands.iter_mut().enumerate() {
            let joint = strand.joints[0];
            if let Some(elapsed) = take(&mut self.pending, due, Self::strand_part(index), &elapsed) {
                let pivot =
                    |joint: usize| chest_from_model.transform_point3(pose.model()[joint].translation.into());
                let root = pivot(joint);
                let end = pivot(*strand.joints.last().expect("has a tail joint"));
                let Some(along) = (end - root).try_normalize() else { continue };
                self.probes.clear();
                self.probes.extend(
                    strand
                        .samples
                        .iter()
                        .map(|p| p.split(pose.model(), chest_from_model, root, &strand.joints)),
                );
                for link in strand.joints.windows(2) {
                    let a = pivot(link[0]);
                    let b = pivot(link[1]);
                    for sample in 1..=6 {
                        let at = a.lerp(b, sample as f32 / 6.0);
                        self.probes.push(Probe { fixed: root, rotating: at - root, radius: 0.002 });
                    }
                }
                self.ahead_probes.clear();
                if forecast > 0.0 {
                    self.ahead_probes.extend(self.probes.iter().zip(&strand.previous_probes).map(
                        |(now, before)| Probe {
                            fixed: predict(now.fixed, before.fixed),
                            rotating: predict(now.rotating, before.rotating),
                            radius: now.radius,
                        },
                    ));
                }
                strand.previous_probes.clear();
                strand.previous_probes.extend_from_slice(&self.probes);
                let excursion = elapsed.map_or(0.55, |dt| (dt.min(2.0) * MAX_TURN_PER_TICK).to_radians());
                let filter = |probe: &Probe, garment: &Garment, limbs: &[Placed]| {
                    let point = probe.fixed + strand.relief.rotation * probe.rotating;
                    let margin = excursion * probe.rotating.length() + probe.radius + GARMENT_MARGIN;
                    garment.within(point, margin)
                        || limbs.iter().any(|limb| {
                            point.distance_squared(limb.nearest(point)) <= (limb.radius + margin).powi(2)
                        })
                };
                self.probes.retain(|probe| filter(probe, &self.garment, &self.posed_limbs));
                self.ahead_probes.retain(|probe| filter(probe, &self.ahead_garment, &self.ahead_limbs));
                // Take the outer locks around the sleeve as it moves from back to front.
                // The forward edge of the upper arm is about 4 cm ahead of its pivot.
                let route = strand.shoulder_route.map(|arm| {
                    let middle = pivot(strand.joints[strand.joints.len() / 2 - 1]);
                    let fore = ((middle.z - pivot(arm).z - 0.04) / 0.06).clamp(-0.8, 0.8);
                    Vec3::new(strand.side, 0.3, fore)
                });
                strand.relief =
                    choose(strand.relief, elapsed, along, strand.side, strand.front, route, |rotation| {
                        let current =
                            penetration(rotation, &mut self.garment, &self.posed_limbs, &self.probes);
                        let ahead = penetration(
                            rotation,
                            &mut self.ahead_garment,
                            &self.ahead_limbs,
                            &self.ahead_probes,
                        );
                        (current, ahead)
                    });
            }
            wrote |= write_relief(pose, chest, joint, strand.relief);
        }
        if wrote {
            pose.finish();
        }
        let residual_part = self.residual_part();
        if let Some(residual) = &mut self.residual {
            match take(&mut self.pending, due, residual_part, &elapsed) {
                Some(elapsed) => residual.apply(
                    pose,
                    chest_from_model,
                    &self.strands,
                    &self.posed_shirt,
                    &self.previous_shirt,
                    elapsed,
                ),
                None => residual.hold(pose),
            }
        }
        let face_part = self.face_part(0);
        if let Some(face) = &mut self.short_face {
            let elapsed = std::array::from_fn(|lock| {
                take(&mut self.pending, due, face_part + lock, &elapsed).unwrap_or(Some(0.0))
            });
            face.apply(pose, &self.strands, elapsed);
        }
    }

    /// Poses the garment over `elapsed` ticks, in the chest's frame: the underarm's relief, the shirt
    /// and the arms where they stand and as forecast ahead, and the envelopes' bounds, whose axes
    /// refit as parts of their own. Where it posed them before becomes what the forecast and the
    /// residual measure motion from.
    fn pose_garment(&mut self, pose: &mut CharacterPose, chest_from_model: Affine3A, elapsed: Option<f32>) {
        if let Some(underarm) = &self.underarm {
            underarm.clear(pose);
        }
        let forecast = elapsed
            .filter(|&dt| self.has_history && dt > 0.0 && dt < 4.0)
            .map_or(0.0, |dt| LOOKAHEAD_TICKS / dt.max(0.125) * ((4.0 - dt) * 0.5).min(1.0));
        self.forecast = forecast;
        let predict = |now: Vec3, before: Vec3| now + ((now - before) * forecast).clamp_length_max(0.07);
        std::mem::swap(&mut self.previous_shirt, &mut self.posed_shirt);
        std::mem::swap(&mut self.previous_limbs, &mut self.posed_limbs);
        for (out, &point) in self.posed_shirt.iter_mut().zip(&self.shirt) {
            *out = chest_from_model.transform_point3(point.model(pose.model()));
        }
        for morph in &self.shirt_morphs {
            let weight = pose.weights()[morph.target];
            if weight == 0.0 {
                continue;
            }
            let mut offset = Vec3::ZERO;
            for (part, local) in self.shirt[morph.shirt_point].parts.iter().zip(morph.joint_offsets) {
                if part.weight > 0.0 {
                    offset += pose.model()[part.joint].transform_vector3(local) * part.weight;
                }
            }
            self.posed_shirt[morph.shirt_point] += chest_from_model.transform_vector3(offset) * weight;
        }
        if let Some(underarm) = &mut self.underarm {
            underarm.solve(pose, chest_from_model, &self.shirt, &mut self.posed_shirt, elapsed);
        }
        self.garment.bound(&self.posed_shirt);
        if forecast > 0.0 {
            for ((ahead, &now), &before) in
                self.ahead_shirt.iter_mut().zip(&self.posed_shirt).zip(&self.previous_shirt)
            {
                *ahead = predict(now, before);
            }
            self.ahead_garment.bound(&self.ahead_shirt);
        }
        for (out, limb) in self.posed_limbs.iter_mut().zip(&self.limbs) {
            let transform = chest_from_model * pose.model()[limb.joint];
            *out = Placed {
                start: transform.transform_point3(limb.offset),
                axis: transform.transform_vector3(limb.tail - limb.offset),
                radius: limb.radius * transform.matrix3.x_axis.length(),
            };
        }
        if forecast > 0.0 {
            for ((ahead, now), before) in
                self.ahead_limbs.iter_mut().zip(&self.posed_limbs).zip(&self.previous_limbs)
            {
                let start = predict(now.start, before.start);
                let end = predict(now.start + now.axis, before.start + before.axis);
                let axis =
                    (end - start).try_normalize().map_or(now.axis, |direction| direction * now.axis.length());
                *ahead = Placed { start, axis, radius: now.radius };
            }
        }
        self.has_history = true;
    }
}

/// A part's elapsed time if it's in `due`, taking its ticks pending: `None` if it holds.
fn take(
    pending: &mut [f64],
    due: u32,
    part: usize,
    elapsed: impl Fn(f64) -> Option<f32>,
) -> Option<Option<f32>> {
    (due & 1 << part != 0).then(|| elapsed(std::mem::take(&mut pending[part])))
}

/// Turns a strand's root `joint` by `relief`, which is in the chest's frame, and says whether it did.
fn write_relief(pose: &mut CharacterPose, chest: Frame, joint: usize, relief: Relief) -> bool {
    if relief.rotation == Quat::IDENTITY {
        return false;
    }
    let parent = pose.parent(joint).map_or(Quat::IDENTITY, |p| Frame::of(&pose.model()[p]).rotation);
    let chest_to_parent = parent.inverse() * chest.rotation;
    let offset = chest_to_parent * relief.rotation * chest_to_parent.inverse();
    let local = &mut pose.locals_mut()[joint].rotation;
    *local = (offset * *local).normalize();
    true
}

/// Keep samples along the full strand, including its edges, without carrying thousands of
/// seam-duplicated vertices into the frame loop.
fn spread(candidates: &[(Vec3, Point)]) -> Vec<Point> {
    let mut result = Vec::new();
    let mut gaps = vec![f32::INFINITY; candidates.len()];
    let Some((mut next, _)) = candidates.iter().enumerate().min_by(|(_, a), (_, b)| a.0.y.total_cmp(&b.0.y))
    else {
        return result;
    };
    while result.len() < 100 {
        let (at, point) = candidates[next];
        result.push(point);
        for ((other, _), gap) in candidates.iter().zip(&mut gaps) {
            *gap = (*gap).min(at.distance_squared(*other));
        }
        let (farthest, &gap) =
            gaps.iter().enumerate().max_by(|(_, a), (_, b)| a.total_cmp(b)).expect("one point");
        if gap < 0.012_f32.powi(2) {
            break;
        }
        next = farthest;
    }
    result
}

fn penetration(rotation: Quat, garment: &mut Garment, limbs: &[Placed], probes: &[Probe]) -> f32 {
    if probes.is_empty() {
        return 0.0;
    }
    let mut worst = 0.0_f32;
    let mut sum = 0.0;
    for probe in probes {
        let point = probe.fixed + rotation * probe.rotating;
        let mut deep = garment.depth(point, probe.radius);
        for limb in limbs {
            let reach = limb.radius + probe.radius + 0.003;
            let delta = point - limb.nearest(point);
            if delta.length_squared() < reach * reach {
                deep = deep.max(reach - delta.length());
            }
        }
        worst = worst.max(deep);
        sum += deep * deep;
    }
    worst + (sum / probes.len() as f32).sqrt()
}

fn degrees_between(a: Quat, b: Quat) -> f32 {
    let relative = a.conjugate() * b;
    let sine = Vec3::new(relative.x, relative.y, relative.z).length();
    2.0 * sine.atan2(relative.w.abs()).to_degrees()
}

fn limited_turn(from: Quat, to: Quat, degrees: f32) -> Quat {
    let delta = degrees_between(from, to);
    if delta <= degrees { to } else { from.slerp(to, degrees / delta).normalize() }
}

fn choose(
    previous: Relief,
    elapsed: Option<f32>,
    along: Vec3,
    side: f32,
    front: f32,
    route: Option<Vec3>,
    mut depth: impl FnMut(Quat) -> (f32, f32),
) -> Relief {
    if elapsed == Some(0.0) {
        return previous;
    }
    let limit = elapsed.map_or(28.0, |dt| dt.min(2.0) * MAX_TURN_PER_TICK);
    let fade = elapsed.map_or(0.0, |dt| (-dt / RELEASE_HALF_LIFE).exp2());
    let decayed = Quat::IDENTITY.slerp(previous.rotation, fade).normalize();
    let release = if degrees_between(decayed, Quat::IDENTITY) < 0.02 { Quat::IDENTITY } else { decayed };
    let decayed = limited_turn(previous.rotation, release, limit);
    let (present, ahead) = depth(decayed);
    let release_angle = degrees_between(decayed, Quat::IDENTITY);
    if present < CLEAR && ahead < 0.002 {
        if let Some(direction) = route
            && release_angle > 3.0
        {
            // Start following the free side while there is room. Waiting for another collision
            // to choose a direction can leave the sleeve between a strand and its escape path.
            let target = Relief::rotation(along, direction, release_angle);
            let rotation = limited_turn(previous.rotation, target, limit);
            let (now, ahead) = depth(rotation);
            if now < 0.003 && ahead < 0.003 {
                return Relief { direction, rotation };
            }
        }
        return Relief { rotation: decayed, ..previous };
    }
    let preferred = route.unwrap_or(Vec3::new(side * 0.65, 0.0, front));
    let tangent = (preferred - along * preferred.dot(along)).try_normalize();
    let lateral = 0.68 * (1.0 - along.x * along.x).max(0.0).sqrt();
    let cost = |q: Quat, present, ahead| {
        let route_cost = if route.is_some() {
            tangent.zip((q * along - along).try_normalize()).map_or(0.0, |(preferred, actual)| {
                14.0 * (1.0 - preferred.dot(actual)) + 82.0 * (lateral - side * actual.x).max(0.0)
            })
        } else {
            0.0
        };
        present * 2500.0
            + ahead * 700.0
            + degrees_between(q, Quat::IDENTITY)
            + degrees_between(q, previous.rotation) * 0.6
            + route_cost
    };
    let mut best = Relief { rotation: decayed, ..previous };
    let mut best_cost = cost(decayed, present, ahead);
    let (held_present, held_ahead) = depth(previous.rotation);
    let hold_cost = cost(previous.rotation, held_present, held_ahead);
    if hold_cost < best_cost {
        best = previous;
        best_cost = hold_cost;
    }
    let previous_angle = degrees_between(previous.rotation, Quat::IDENTITY);
    let wanted = (previous_angle + (present + ahead * 0.6) * 600.0 + 7.0).clamp(8.0, 28.0);
    let heading = if previous.direction == Vec3::ZERO { preferred } else { previous.direction };
    let opposite = Vec3::new(side, 0.0, -front);
    let directions = [heading, preferred, Vec3::Z * front, Vec3::X * side, opposite, Vec3::NEG_Z * front];
    let mut try_turn = |direction: Vec3, target| {
        let q = limited_turn(previous.rotation, target, limit);
        let (present, ahead) = depth(q);
        let bias = 1.6 * (1.0 - preferred.normalize().dot(direction.normalize()));
        let score = cost(q, present, ahead) + bias;
        if score < best_cost {
            best_cost = score;
            best = Relief { direction, rotation: q };
        }
    };
    for direction in directions {
        if elapsed.is_none() {
            // A cut can start with the strand already on the wrong side. Check intermediate
            // turns too, since the furthest turn may hit the other side of the sleeve.
            for degrees in [4.0, 8.0, 12.0, 16.0, 20.0, 24.0, 28.0] {
                try_turn(direction, Relief::rotation(along, direction, degrees));
            }
        } else {
            try_turn(direction, Relief::rotation(along, direction, wanted));
        }
    }
    if let Some(direction) = route
        && previous_angle > 3.0
    {
        try_turn(direction, Relief::rotation(along, direction, previous_angle));
    }
    // A change of contact side can follow the edge of the garment. Turning the old
    // correction's axis keeps its outward extent while changing the escape direction.
    if previous_angle > 2.0 {
        for sign in [-1.0, 1.0] {
            let around = Quat::from_axis_angle(along, (25.0_f32 * sign).to_radians());
            let direction = around * heading;
            try_turn(direction, around * previous.rotation * around.inverse());
        }
    }
    let strong = best;
    let release = Relief { rotation: decayed, ..previous };
    let mut fraction = 0.5;
    for _ in 0..3 {
        let candidate = release.rotation.slerp(strong.rotation, fraction).normalize();
        let (present, ahead) = depth(candidate);
        let score = cost(candidate, present, ahead);
        if score < best_cost {
            best_cost = score;
            best = Relief { rotation: candidate, ..strong };
            fraction *= 0.5;
        } else {
            fraction = (fraction + 1.0) * 0.5;
        }
    }
    best.rotation = limited_turn(previous.rotation, best.rotation, limit);
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Joint, JointTransform, Skeleton, SpringBones, SpringFrame};
    use sapphire_assets::rig::{CharacterRig, Spring, SpringJoint, SpringTip};

    #[test]
    fn garment_probe_sees_a_link_crossing_between_clear_ends() {
        let vertices = [
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [-1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        ]
        .map(|p| Vec3::from_array(p) * 0.06);
        let triangles = [
            [0, 2, 1],
            [1, 2, 3],
            [4, 5, 6],
            [5, 7, 6],
            [0, 4, 2],
            [2, 4, 6],
            [1, 3, 5],
            [3, 7, 5],
            [0, 1, 4],
            [1, 5, 4],
            [2, 6, 3],
            [3, 6, 7],
        ];
        let mut garment = Garment::new(&vertices, &triangles);
        garment.refit(&vertices);
        let a = Vec3::NEG_X * 0.12;
        let b = Vec3::X * 0.12;
        assert_eq!(garment.depth(a, 0.0), 0.0);
        assert_eq!(garment.depth(b, 0.0), 0.0);
        assert!(garment.depth(a.lerp(b, 0.5), 0.0) > 0.05);
        assert_eq!(garment.depth(Vec3::Y * 0.12, 0.0), 0.0);
    }

    /// An axis rasters by tiles as they're read, or whole as it refits, and either way must leave every
    /// cell as a raster of every triangle over the whole grid does, through refits that move it.
    #[test]
    fn a_garment_rastered_by_tiles_as_read_matches_one_rastered_whole() {
        let mut seed = 0x9e37_79b9_u32;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        let random_point = |random: &mut dyn FnMut() -> f32| {
            Vec3::new(random(), random(), random()) * 0.6 - Vec3::splat(0.3)
        };
        let rest: Vec<_> = (0..240).map(|_| random_point(&mut random)).collect();
        let triangles: Vec<_> = (0..300).map(|_| [0; 3].map(|_| (random() * 239.99) as usize)).collect();
        let whole = |vertices: &[Vec3], axis: usize| {
            let mut grid = vec![Interval::EMPTY; GRID * GRID];
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let coordinate = |i: usize| (i as f32 + 0.5) * CELL - HALF;
            for triangle in &triangles {
                let [a, b, c] = triangle.map(|i| vertices[i]);
                let (min, max) = (a.min(b).min(c), a.max(b).max(c));
                let (ab, ac) = (b - a, c - a);
                let area = ab[u] * ac[v] - ab[v] * ac[u];
                if area.abs() < 1.0e-9 {
                    continue;
                }
                let inverse = area.recip();
                for y in cell_index(min[v])..=cell_index(max[v]) {
                    let dy = coordinate(y) - a[v];
                    for x in cell_index(min[u])..=cell_index(max[u]) {
                        let dx = coordinate(x) - a[u];
                        let s = (dx * ac[v] - dy * ac[u]) * inverse;
                        let t = (ab[u] * dy - ab[v] * dx) * inverse;
                        if s >= 0.0 && t >= 0.0 && s + t <= 1.0 {
                            grid[y * GRID + x].add(a[axis] + s * ab[axis] + t * ac[axis]);
                        }
                    }
                }
            }
            grid
        };
        let mut garment = Garment::new(&rest, &triangles);
        let mut vertices = rest.clone();
        for pose in 0..4 {
            if pose > 0 {
                vertices.iter_mut().for_each(|p| *p += random_point(&mut random) * 0.1);
            }
            garment.bound(&vertices);
            for axis in 0..3 {
                garment.refit_axis(axis, pose >= 2);
                let expected = whole(&vertices, axis);
                // Read a scattered few first, as the strands do, then every cell.
                for _ in 0..40 {
                    let (x, y) = ((random() * 127.99) as usize, (random() * 127.99) as usize);
                    let cell = garment.cell(axis, x, y);
                    let want = expected[y * GRID + x];
                    assert_eq!(
                        (cell.lo.to_bits(), cell.hi.to_bits()),
                        (want.lo.to_bits(), want.hi.to_bits())
                    );
                }
                if pose % 2 == 1 {
                    for y in 0..GRID {
                        for x in 0..GRID {
                            let (cell, want) = (garment.cell(axis, x, y), expected[y * GRID + x]);
                            assert_eq!(
                                (cell.lo.to_bits(), cell.hi.to_bits()),
                                (want.lo.to_bits(), want.hi.to_bits()),
                                "pose {pose} axis {axis} cell {x}, {y}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn relief_uses_presented_time_and_releases_without_a_branch_pop() {
        let tip = Vec3::NEG_Y * 0.35;
        let forward = |q: Quat| ((0.060 - (q * tip).z).max(0.0), 0.0);
        let backward = |q: Quat| ((0.060 + (q * tip).z).max(0.0), 0.0);
        let mut relief = choose(Relief::default(), None, Vec3::NEG_Y, 1.0, 1.0, None, forward);
        assert!(forward(relief.rotation).0 < 0.004);
        assert_eq!(choose(relief, Some(0.0), Vec3::NEG_Y, 1.0, 1.0, None, backward), relief);
        for tick in 0..25 {
            let old = relief.rotation;
            relief = choose(relief, Some(0.25), Vec3::NEG_Y, 1.0, 1.0, None, backward);
            assert!(degrees_between(old, relief.rotation) < 1.14, "tick {tick}");
        }
        assert!(backward(relief.rotation).0 < 0.004);
        for _ in 0..48 {
            let before = degrees_between(relief.rotation, Quat::IDENTITY);
            relief = choose(relief, Some(1.0), Vec3::NEG_Y, 1.0, 1.0, None, |_| (0.0, 0.0));
            assert!(degrees_between(relief.rotation, Quat::IDENTITY) <= before);
        }
        assert_eq!(relief.rotation, Quat::IDENTITY);
    }

    #[test]
    fn a_clear_releasing_strand_turns_toward_the_outer_sleeve_route() {
        let along = Vec3::NEG_Y;
        let previous =
            Relief { direction: Vec3::NEG_Z, rotation: Relief::rotation(along, Vec3::NEG_Z, 18.0) };
        let route = Vec3::new(-1.0, 0.0, -0.5);
        let next = choose(previous, Some(1.0), along, -1.0, -1.0, Some(route), |_| (0.0, 0.0));
        assert!((next.rotation * along).x < -0.02);
        assert!(degrees_between(previous.rotation, next.rotation) <= 4.501);
        assert!(degrees_between(Quat::IDENTITY, next.rotation) < 18.0);
    }

    /// A solve's parts spread over the interval: those that read what others solve in order, so a strand
    /// never solves before the envelope it reads in the same interval, and the face's locks wherever is
    /// lightest, no tick taking more than an even share and a part.
    #[test]
    fn a_solves_parts_spread_over_the_interval_in_order_as_evenly_as_order_allows() {
        let heroine = |ahead: u32| -> Vec<u32> {
            [POSE_COST]
                .into_iter()
                .chain([AXIS_COST; 3])
                .chain([ahead; 3])
                .chain([STRAND_COST; 12])
                .chain([RESIDUAL_COST])
                .chain([FACE_LOCK_COST; LOCKS])
                .collect()
        };
        let ordered = Clearance::parts(12) - LOCKS;
        let free = |part: usize| part >= ordered;
        let loads = |spread: &Spread, costs: &[u32]| {
            let mut loads = vec![0; spread.interval as usize];
            for (&tick, &cost) in spread.ticks.iter().zip(costs) {
                loads[tick as usize] += cost;
            }
            loads
        };
        let mut spread = Spread::new(Clearance::parts(12));
        let costs = heroine(AHEAD_AXIS_COST);
        spread.fit(3, |part| costs[part], free);
        // The garment posed and its envelope; the forecast envelope, the strands and a face lock; the
        // residual and the other two.
        assert_eq!(loads(&spread, &costs), [545, 518, 565]);
        assert_eq!(spread.ticks[ordered..], [2, 1, 2]);
        for interval in 1..=8 {
            let costs = heroine(if interval < 4 { AHEAD_AXIS_COST } else { 0 });
            spread.fit(interval, |part| costs[part], free);
            assert!(spread.ticks[..ordered].is_sorted(), "{interval}: {:?}", spread.ticks);
            let loads = loads(&spread, &costs);
            let even = costs.iter().sum::<u32>().div_ceil(interval);
            let most = *loads.iter().max().unwrap();
            assert!(most < even.max(RESIDUAL_COST) + RESIDUAL_COST, "{interval}: {loads:?}");
        }
        spread.fit(1, |part| heroine(AHEAD_AXIS_COST)[part], free);
        assert!(spread.ticks.iter().all(|&tick| tick == 0));
    }

    /// A chest, and a strand hanging from it with a clearance turning it 24° off its pose and no
    /// garment, so a solve only releases it; and its springs, after their first frame at tick 8.
    fn turned_strand() -> (CharacterPose, SpringBones, SpringFrame, Clearance) {
        let joints = [
            Joint { name: "chest".into(), parent: None, rest: JointTransform::IDENTITY },
            Joint { name: "strand".into(), parent: Some(0), rest: JointTransform::IDENTITY },
            Joint {
                name: "end".into(),
                parent: Some(1),
                rest: JointTransform { translation: [0.0, -0.35, 0.0], ..JointTransform::IDENTITY },
            },
        ];
        let skeleton = Skeleton::new(&joints).to_cooked().unwrap();
        let mut pose = CharacterPose::new(Skeleton::read(&skeleton).unwrap(), &[], &[], Vec::new());
        let rig = CharacterRig {
            joint_count: 3,
            springs: vec![Spring {
                name: "strand".into(),
                joints: [1, 2]
                    .map(|joint| SpringJoint {
                        joint,
                        hit_radius: 0.0,
                        stiffness: 0.5,
                        gravity_power: 0.0,
                        gravity_dir: [0.0, -1.0, 0.0],
                        drag_force: 0.4,
                    })
                    .into(),
                tip: SpringTip::Joint,
                collider_groups: Vec::new(),
                center: None,
            }],
            ..CharacterRig::default()
        }
        .to_cooked()
        .unwrap();
        let mut springs = SpringBones::new(CharacterRig::read(&rig).unwrap(), &pose).unwrap();
        let frame =
            SpringFrame { world_from_model: Affine3A::IDENTITY, ticks: 8.0, active: true, pose_moved: false };
        springs.update(&mut pose, &frame);
        let clearance = Clearance {
            chest: 0,
            shirt: Vec::new(),
            shirt_morphs: Vec::new(),
            triangles: Vec::new(),
            limbs: Vec::new(),
            strands: vec![Strand {
                joints: vec![1, 2],
                samples: Vec::new(),
                side: 1.0,
                front: 1.0,
                shoulder_route: None,
                relief: Relief { direction: Vec3::Z, rotation: Quat::from_rotation_x(24.0_f32.to_radians()) },
                previous_probes: Vec::with_capacity(6),
            }],
            garment: Garment::new(&[], &[]),
            ahead_garment: Garment::new(&[], &[]),
            posed_shirt: Vec::new(),
            previous_shirt: Vec::new(),
            ahead_shirt: Vec::new(),
            posed_limbs: Vec::new(),
            previous_limbs: Vec::new(),
            ahead_limbs: Vec::new(),
            probes: Vec::with_capacity(6),
            ahead_probes: Vec::with_capacity(6),
            has_history: false,
            waking: false,
            residual: None,
            short_face: None,
            underarm: None,
            forecast: 0.0,
            pending: vec![0.0; Clearance::parts(1)],
            last: None,
            phase: 0,
            spread: Spread::new(Clearance::parts(1)),
        };
        (pose, springs, frame, clearance)
    }

    #[test]
    fn lod_wake_retains_drawn_relief_at_a_frozen_clock() {
        let (mut pose, mut springs, mut frame, mut clearance) = turned_strand();
        clearance.solve_all(&mut pose, Some(0.0));
        springs.clearance = Some(Box::new(clearance));
        let drawn = pose.locals()[1].rotation;
        assert!(degrees_between(drawn, Quat::IDENTITY) > 23.9);

        springs.update(&mut pose, &SpringFrame { active: false, ..frame });
        springs.update(&mut pose, &frame);
        assert_eq!(pose.locals()[1].rotation, drawn);

        springs.update(&mut pose, &SpringFrame { active: false, ..frame });
        frame.ticks += 0.25;
        springs.update(&mut pose, &frame);
        let release = degrees_between(pose.locals()[1].rotation, drawn);
        assert!((0.5..=1.126).contains(&release), "released {release} degrees");

        springs.reset();
        springs.update(&mut pose, &frame);
        assert_eq!(pose.locals()[1].rotation, Quat::IDENTITY);
    }

    /// A light tier's clearance spreads its solve over the interval: the garment posed first, the strand
    /// last, each taking every tick since it last solved, and on each frame the parts that don't solve
    /// write what they last solved, unchanged. An interval runs out before the next begins, however
    /// many ticks a frame draws, and a frame a whole interval late solves every part. Held, it writes
    /// and solves nothing more however long it waits.
    #[test]
    fn a_clearance_at_an_interval_spreads_its_parts_over_it_and_holds_each_between() {
        let (mut pose, _, _, mut clearance) = turned_strand();
        let rest = pose.locals()[1].rotation;
        let mut update = |clearance: &mut Clearance, ticks: f64, span: f64, interval| {
            pose.locals_mut()[1].rotation = rest;
            pose.finish();
            clearance.update(&mut pose, ticks, span, interval);
            degrees_between(pose.locals()[1].rotation, rest)
        };
        let strand = Clearance::strand_part(0);
        let pending = |clearance: &Clearance| (clearance.pending[POSE], clearance.pending[strand]);
        let full = degrees_between(clearance.strands[0].relief.rotation, Quat::IDENTITY);
        // The first update solves every part: over one tick, which releases some of the turn.
        let solved = update(&mut clearance, 8.0, 1.0, Some(2));
        assert!(solved < full - 1.0 && solved > 15.0, "{solved} of {full}");
        assert_eq!(pending(&clearance), (0.0, 0.0));
        // Then each interval of two ticks poses the garment on its first and solves the strand on its
        // second, over both ticks since it last did, releasing more than one tick did.
        assert_eq!(update(&mut clearance, 9.0, 1.0, Some(2)), solved);
        assert_eq!((clearance.spread.ticks[POSE], clearance.spread.ticks[strand]), (0, 1));
        assert_eq!(pending(&clearance), (0.0, 1.0));
        let twice = update(&mut clearance, 10.0, 1.0, Some(2));
        assert!(solved - twice > full - solved, "{twice} after {solved}");
        assert_eq!(pending(&clearance), (1.0, 0.0));
        let relief = clearance.strands[0].relief;
        assert_eq!(update(&mut clearance, 11.0, 1.0, Some(2)), twice);
        assert_eq!(clearance.strands[0].relief, relief);
        // An interval of four takes the garment's envelope a tick an axis, the strand with the last.
        update(&mut clearance, 12.0, 1.0, Some(4));
        assert_eq!(clearance.spread.ticks[..=strand], [0, 1, 2, 3, 3, 3, 3, 3]);
        assert_eq!(pending(&clearance), (0.0, 2.0));
        // Two ticks a frame: the axes on the first, then the last axis and the strand, the interval's
        // tick left over; the next frame begins the next interval.
        update(&mut clearance, 14.0, 2.0, Some(4));
        assert_eq!(pending(&clearance), (2.0, 4.0));
        assert_eq!(clearance.pending[AXES + 2], 4.0);
        update(&mut clearance, 16.0, 2.0, Some(4));
        assert_eq!(pending(&clearance), (4.0, 0.0));
        update(&mut clearance, 18.0, 2.0, Some(4));
        assert_eq!(pending(&clearance), (0.0, 2.0));
        // A frame a whole interval late solves every part (that it has: no residual or face here).
        update(&mut clearance, 22.0, 4.0, Some(4));
        assert!(clearance.pending[..=strand].iter().all(|&pending| pending == 0.0));
        // Out of range, it holds.
        let relief = clearance.strands[0].relief;
        let held = update(&mut clearance, 14.0, 1.0, None);
        for ticks in 15..40 {
            assert_eq!(update(&mut clearance, f64::from(ticks), 1.0, None), held);
        }
        assert_eq!(clearance.strands[0].relief, relief);
        // An interval of 1 solves at every update, however short.
        let mut reference = turned_strand().3;
        reference.solve_all(&mut turned_strand().0, Some(1.0));
        let (mut pose, _, _, mut every) = turned_strand();
        every.update(&mut pose, 8.25, 1.0, Some(1));
        assert_eq!(every.strands[0].relief, reference.strands[0].relief);
        every.update(&mut pose, 8.5, 0.25, Some(1));
        assert_ne!(every.strands[0].relief, reference.strands[0].relief);
    }
}
