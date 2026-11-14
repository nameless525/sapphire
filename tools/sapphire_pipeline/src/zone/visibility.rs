//! Visible sets by cell, cooked from a zone's document: M5 D0's prototype, which D7 may build into the zone
//! cook. It writes [`sapphire_zone::visibility`]'s file, beside a cook, and nothing a shipping build reads.
//!
//! # What's an object, and what hides one
//!
//! **Objects** are what the main view culls one by one: each cell's merged mesh and each instance, by an
//! axis-aligned box round the geometry of both cooks (desktop's full detail and mobile's light), grown by
//! [`OBJECT_SLACK`]. **Occluders** are halls (`#hall`, a building's walls): a hall's walls and the backings of
//! its recessed openings close a box [`HALL_INSET`] inside its faces, from its plinth's top to its walls' top,
//! where the roof or the next storey closes it, so any segment from a camera outside the building through that
//! box meets a face that's drawn toward it before it leaves the box (the tests cast segments at every building
//! of the district to hold it). The plinth stays out: where it stops at a door's jambs it leaves slivers into
//! the wall, low enough that only a ray going down passes them, and such a ray never rises into the box. A
//! roof (`#roof`) adds two boxes under its tiles over the walls' top (`roof_boxes`). Only a hall or roof
//! placed at a quarter turn is one, so its box stays a box. Nothing else hides anything: props, plants,
//! freestanding walls (a lane's closure opens at run time, and a wall's openings go through) and the ground
//! are only ever objects.
//!
//! # Points sample a cell's space, and occluders shrink to make them conservative
//!
//! A camera may stand anywhere over a cell, at any height in [`Sampling::heights`]. The cook samples that
//! box on a grid, each sample the middle of a small box `ε` across on each axis ([`Sampling::reach`]: half the
//! spacing across, half a layer upward), and asks of each sample what it sees past the occluders **shrunk by
//! ε on each axis**. That makes the union over the samples conservative for the whole box (occluder
//! shrinking, as in Wonka, Wimmer and Schmalstieg's urban visibility, 2000): if every segment from a sample `p`
//! to an object meets a shrunk occluder, the segment from a camera `q` in `p`'s small box to the same point
//! lies within that box's offset of `p`'s at every step, so it meets the occluder itself. Shrinking by axis
//! rather than by the diagonal keeps a wall's top where it matters: a walk's camera rides 1 m under a
//! storey's. A camera inside an occluder's whole bounds sees walls from behind, which aren't drawn, so the
//! runtime falls back to the frustum there (`CellVisibility::keep_out`).
//!
//! # How one sample decides
//!
//! An object is hidden from `p` if each piece of its box is hidden by a single shrunk occluder `B`: the box's
//! eight corners' rays from `p` all meet `B` (so, `B` being convex, every ray into the box does), and `B` is
//! met first: a plane along an axis has `B` and `p` on one side and the piece on the other, or the piece is
//! farther from `p` than all of `B`. A piece no single occluder hides splits along its longest side, down to
//! [`Sampling::piece`]; where pieces meet is where occluders fuse. An object some sample sees is in the cell's
//! set and isn't asked about again.
//!
//! # The same bytes everywhere
//!
//! The four operations, comparisons, `floor` and `sqrt` on `f32`, in a fixed order, with transforms through
//! the cook's own (`placement_transform`); boxes snap outward (objects) or inward (occluders) to a 64th
//! of a metre so the last bits of a transform don't reach a set. Cells cook on threads, each alone and
//! in order, so how many there are changes nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use glam::Vec3;
use sapphire_assets::AssetId;
use sapphire_zone::document::ZoneDocument;
use sapphire_zone::grid::{Bounds, Place};
use sapphire_zone::visibility::{CellVisibility, VisibleCell};

use super::{part_name, placement_transform};
use crate::assembly::{self, Assembly, Detail, Roof, RoofKind, Scale, Shape};

/// How far an object's box reaches past its geometry, in metres: the cooked positions' rounding, and
/// transforms' last bits.
pub const OBJECT_SLACK: f32 = 0.02;
/// How far inside a hall's faces its occluder stands, beyond the recess of its openings' backings, in metres.
pub const HALL_INSET: f32 = 0.02;
/// What every box snaps to, outward for objects and inward for occluders: a 64th of a metre.
const SNAP: f32 = 64.0;
/// How much further than ε an occluder shrinks, so no rounding in a ray's test can find it where it isn't.
const MARGIN: f32 = 0.001;
/// How far under the ground a grounded occluder reaches: nothing between two points above the ground passes
/// under it, and a box reaching down keeps a shrunk one on the ground.
const BELOW: f32 = -16.0;
/// The side of the buckets occluders are found by, in metres.
const BUCKET: f32 = 16.0;

/// An axis-aligned box, least and greatest corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    fn of_points(points: impl IntoIterator<Item = [f32; 3]>) -> Option<Self> {
        points.into_iter().fold(None, |found: Option<Self>, point| {
            let point = Self { min: point, max: point };
            Some(found.map_or(point, |found| found.union(point)))
        })
    }

    fn union(self, other: Self) -> Self {
        Self {
            min: std::array::from_fn(|axis| self.min[axis].min(other.min[axis])),
            max: std::array::from_fn(|axis| self.max[axis].max(other.max[axis])),
        }
    }

    fn corners(&self) -> [[f32; 3]; 8] {
        std::array::from_fn(|corner| {
            std::array::from_fn(|axis| if corner >> axis & 1 == 0 { self.min[axis] } else { self.max[axis] })
        })
    }

    /// Moved in by `by` on every side, or `None` if nothing is left.
    fn shrunk(&self, by: [f32; 3]) -> Option<Self> {
        let shrunk = Self {
            min: std::array::from_fn(|a| self.min[a] + by[a]),
            max: std::array::from_fn(|a| self.max[a] - by[a]),
        };
        (0..3).all(|axis| shrunk.min[axis] < shrunk.max[axis]).then_some(shrunk)
    }

    fn grown(&self, by: f32) -> Self {
        Self { min: self.min.map(|v| v - by), max: self.max.map(|v| v + by) }
    }

    /// Snapped outward to [`SNAP`].
    fn snapped_out(&self) -> Self {
        Self {
            min: self.min.map(|v| (v * SNAP).floor() / SNAP),
            max: self.max.map(|v| (v * SNAP).ceil() / SNAP),
        }
    }

    /// Snapped inward to [`SNAP`].
    fn snapped_in(&self) -> Self {
        Self {
            min: self.min.map(|v| (v * SNAP).ceil() / SNAP),
            max: self.max.map(|v| (v * SNAP).floor() / SNAP),
        }
    }

    pub fn contains(&self, point: [f32; 3]) -> bool {
        (0..3).all(|axis| self.min[axis] <= point[axis] && point[axis] <= self.max[axis])
    }

    fn holds(&self, other: &Self) -> bool {
        (0..3).all(|axis| self.min[axis] <= other.min[axis] && other.max[axis] <= self.max[axis])
    }

    fn overlaps(&self, other: &Self) -> bool {
        (0..3).all(|axis| self.min[axis] <= other.max[axis] && other.min[axis] <= self.max[axis])
    }

    /// The square of the distance from `point` to its nearest point.
    fn distance2(&self, point: [f32; 3]) -> f32 {
        let mut sum = 0.0;
        for ((&at, &min), &max) in point.iter().zip(&self.min).zip(&self.max) {
            let outside = if at < min {
                min - at
            } else if at > max {
                at - max
            } else {
                0.0
            };
            sum += outside * outside;
        }
        sum
    }

    /// The square of the distance from `point` to its farthest corner.
    fn farthest2(&self, point: [f32; 3]) -> f32 {
        let mut sum = 0.0;
        for ((&at, &min), &max) in point.iter().zip(&self.min).zip(&self.max) {
            let far = (at - min).abs().max((max - at).abs());
            sum += far * far;
        }
        sum
    }

    /// Whether the ray from `from` through `to` meets it.
    fn met_by_ray(&self, from: [f32; 3], to: [f32; 3]) -> bool {
        let (mut enter, mut leave) = (0.0_f32, f32::INFINITY);
        for axis in 0..3 {
            let step = to[axis] - from[axis];
            if step == 0.0 {
                if from[axis] < self.min[axis] || from[axis] > self.max[axis] {
                    return false;
                }
                continue;
            }
            let a = (self.min[axis] - from[axis]) / step;
            let b = (self.max[axis] - from[axis]) / step;
            let (near, far) = if a < b { (a, b) } else { (b, a) };
            enter = enter.max(near);
            leave = leave.min(far);
            if enter > leave {
                return false;
            }
        }
        true
    }

    fn to_bounds(self) -> Bounds {
        Bounds { min: self.min, max: self.max }
    }
}

/// A building, or whatever else a hall stands in: the boxes that hide what's behind them, and its whole
/// bounds, which a camera must be outside of.
#[derive(Clone, Debug, PartialEq)]
pub struct Blocker {
    pub solids: Vec<Aabb>,
    pub bounds: Aabb,
}

/// One thing the main view culls by its bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Object {
    pub mesh: AssetId,
    pub bounds: Aabb,
}

/// A cell of the cooked zone, in its order, with what it draws in its order.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneCell {
    pub coord: [i32; 2],
    pub objects: Vec<Object>,
}

/// What a zone's sets are cooked from.
#[derive(Clone, Debug, PartialEq)]
pub struct VisibilityScene {
    pub cell_size: f32,
    pub cells: Vec<SceneCell>,
    pub blockers: Vec<Blocker>,
    /// The zone's splines' points by name, for walking one ([`probe_walk`]).
    pub splines: BTreeMap<String, Vec<[f32; 3]>>,
}

impl VisibilityScene {
    /// Every object, numbered as the sets number them.
    pub fn objects(&self) -> impl Iterator<Item = &Object> {
        self.cells.iter().flat_map(|cell| &cell.objects)
    }

    /// Reads a zone (`zone`, its source's ID, and its document) and the assemblies its placements and instances
    /// name, by ID, each with the path it's read from. Fails on what the prototype doesn't handle: a split
    /// placement, an assembly it isn't given.
    pub fn read(
        zone: AssetId,
        document: &ZoneDocument,
        assemblies: &BTreeMap<AssetId, (String, Assembly)>,
    ) -> Result<Self, String> {
        // Each assembly's shape, read once.
        let mut shapes = BTreeMap::new();
        for id in document
            .placements
            .iter()
            .map(|placement| placement.prefab)
            .chain(document.instances.iter().map(|instance| instance.mesh))
        {
            if let std::collections::btree_map::Entry::Vacant(entry) = shapes.entry(id) {
                let (path, assembly) =
                    assemblies.get(&id).ok_or_else(|| format!("{id} isn't an assembly it was given"))?;
                entry.insert(Shaped::of(path, assembly)?);
            }
        }
        let shape_of = |id: AssetId| -> Result<&Shaped, String> { Ok(&shapes[&id]) };
        let mut places: BTreeMap<Place, (Option<Aabb>, Vec<Object>)> = BTreeMap::new();
        let mut blockers = Vec::new();
        for (index, placement) in document.placements.iter().enumerate() {
            if placement.split.is_some() {
                return Err(format!("placements[{index}] is split, which the prototype doesn't handle"));
            }
            let shaped = shape_of(placement.prefab)?;
            let bounds = placed(shaped.bounds, placement.at, placement.turn, placement.scale);
            let (mesh, _) = places.entry(document.place_of(placement.at, placement.backdrop)).or_default();
            *mesh = Some(mesh.map_or(bounds, |mesh| mesh.union(bounds)));
        }
        for instance in &document.instances {
            let shaped = shape_of(instance.mesh)?;
            let bounds = placed(shaped.bounds, instance.at, instance.turn, instance.scale);
            let (_, objects) = places.entry(document.place_of(instance.at, instance.backdrop)).or_default();
            objects.push(Object { mesh: instance.mesh, bounds: bounds.grown(OBJECT_SLACK).snapped_out() });
            if let Some(solids) = quarter(instance.turn, instance.scale).then(|| {
                shaped
                    .halls
                    .iter()
                    .map(|hall| placed(*hall, instance.at, instance.turn, instance.scale))
                    .collect::<Vec<_>>()
            }) && !solids.is_empty()
            {
                let solids = grounded(solids)
                    .into_iter()
                    .filter_map(|solid| solid.shrunk([MARGIN; 3]))
                    .map(|solid| solid.snapped_in())
                    .collect();
                blockers.push(Blocker { solids, bounds: bounds.grown(OBJECT_SLACK).snapped_out() });
            }
        }
        for light in &document.lights {
            places.entry(document.place_of(light.position, light.backdrop)).or_default();
        }
        let cells = places
            .into_iter()
            .filter_map(|(place, (mesh, instances))| {
                let Place::Cell(coord) = place else { return None };
                let merged = mesh.map(|bounds| Object {
                    mesh: zone.sub_asset(&part_name(place)),
                    bounds: bounds.grown(OBJECT_SLACK).snapped_out(),
                });
                Some(SceneCell {
                    coord: [coord.x, coord.z],
                    objects: merged.into_iter().chain(instances).collect(),
                })
            })
            .collect();
        let splines =
            document.splines.iter().map(|spline| (spline.name.clone(), spline.points.clone())).collect();
        Ok(Self { cell_size: document.grid.cell_size, cells, blockers, splines })
    }
}

/// An assembly's box round both cooks' geometry, and its halls' and roofs' occluder boxes, in its own space.
struct Shaped {
    bounds: Aabb,
    halls: Vec<Aabb>,
}

impl Shaped {
    fn of(path: &str, assembly: &Assembly) -> Result<Self, String> {
        let mut bounds: Option<Aabb> = None;
        for detail in [Detail::Full, Detail::Light] {
            let parts = assembly::import_parts(Path::new(path), assembly, detail)
                .map_err(|error| format!("{path}: {error:#}"))?;
            let more = Aabb::of_points(parts.mesh.vertices.iter().map(|vertex| vertex.position));
            bounds = match (bounds, more) {
                (Some(bounds), Some(more)) => Some(bounds.union(more)),
                (bounds, more) => bounds.or(more),
            };
        }
        let bounds = bounds.ok_or_else(|| format!("{path} draws nothing"))?;
        let halls = assembly
            .place
            .iter()
            .filter(|placement| {
                placement.piece.is_empty() && quarter(placement.turn, scale_axes(placement.scale))
            })
            .filter_map(|placement| match &placement.shape {
                Some(Shape::Hall(hall)) => {
                    let [width, depth, height] = hall.size;
                    let inset = hall.recess + HALL_INSET;
                    let foot = if hall.plinth.height > 0.0 { hall.plinth.height + HALL_INSET } else { 0.0 };
                    let local = Aabb {
                        min: [-0.5 * width, foot, -0.5 * depth],
                        max: [0.5 * width, height, 0.5 * depth],
                    };
                    let local = local.shrunk([inset, 0.0, inset])?;
                    Some(vec![placed(local, placement.at, placement.turn, scale_axes(placement.scale))])
                }
                Some(Shape::Roof(roof)) => Some(
                    roof_boxes(roof)
                        .into_iter()
                        .map(|local| placed(local, placement.at, placement.turn, scale_axes(placement.scale)))
                        .collect(),
                ),
                _ => None,
            })
            .flatten()
            .collect();
        Ok(Self { bounds, halls })
    }
}

/// How far up its slope each of a roof's occluder boxes reaches, as shares of the way from the eaves to the
/// ridge: two boxes, a wider and lower one and a narrower and higher one.
const ROOF_REACH: [f32; 2] = [0.5, 0.8];

/// Boxes under a hip, hip-and-gable or gable roof's tiles, over its walls' top (its origin): each as wide
/// as the slopes let it stand at its share of the way up ([`ROOF_REACH`]), and as high as the tiles' channels
/// there, less a margin. The tiles close them from above, the soffit and fascia under the eaves, the gables'
/// fill and the walls below. A skirt has none: a storey stands in it. The slope's height repeats
/// `assembly::shapes::roof`'s plan: the soffit meets the walls' top at their line, `thickness` under the tiles.
fn roof_boxes(roof: &Roof) -> Vec<Aabb> {
    if matches!(roof.kind, RoofKind::Skirt(_)) {
        return Vec::new();
    }
    let [width, depth] = roof.size;
    let profile = |s: f32| (1.0 - roof.curve) * s + roof.curve * s * s;
    let run = 0.5 * depth + roof.overhang;
    let eave = roof.thickness - roof.rise * profile(roof.overhang / run);
    let inset = HALL_INSET;
    ROOF_REACH
        .iter()
        .filter_map(|&reach| {
            let top = eave + roof.rise * profile(reach) - roof.tiles.depth - HALL_INSET;
            let half_z = ((1.0 - reach) * run).min(0.5 * depth - inset);
            // A hip's end slopes climb the same run from the eaves past the walls' ends.
            let half_x = match roof.kind {
                RoofKind::Gable => 0.5 * width - inset,
                _ => (0.5 * width + roof.overhang - reach * run).min(0.5 * width - inset),
            };
            (top > 0.0 && half_x > 0.0 && half_z > 0.0)
                .then_some(Aabb { min: [-half_x, 0.0, -half_z], max: [half_x, top, half_z] })
        })
        .collect()
}

fn scale_axes(scale: Scale) -> [f32; 3] {
    match scale {
        Scale::Uniform(scale) => [scale; 3],
        Scale::Axes(axes) => axes,
    }
}

/// Whether a placement keeps a box a box: a quarter turn and a positive scale.
fn quarter(turn: f32, scale: [f32; 3]) -> bool {
    turn % 90.0 == 0.0 && scale.iter().all(|axis| *axis > 0.0)
}

/// `local` placed by the cook's own transform, as the box round its corners.
fn placed(local: Aabb, at: [f32; 3], turn: f32, scale: [f32; 3]) -> Aabb {
    let transform = placement_transform(at, turn, scale);
    let corners = local.corners().map(|corner| transform.transform_point3(Vec3::from(corner)).to_array());
    Aabb::of_points(corners).expect("a box has corners")
}

/// A building's hall boxes, each reaching down through the halls under it that hold its footprint, and under
/// the ground from the ground (a hall with no plinth): storeys narrowing upward each stand on the one below.
fn grounded(mut solids: Vec<Aabb>) -> Vec<Aabb> {
    solids.sort_by(|a, b| a.min[1].total_cmp(&b.min[1]));
    for index in 0..solids.len() {
        let mut bottom = solids[index].min[1];
        for under in &solids[..index] {
            let footprint = (0..3)
                .step_by(2)
                .all(|a| under.min[a] <= solids[index].min[a] && solids[index].max[a] <= under.max[a]);
            if footprint && under.max[1] >= bottom - MARGIN && under.min[1] < bottom {
                bottom = under.min[1];
            }
        }
        if bottom <= MARGIN {
            bottom = BELOW;
        }
        solids[index].min[1] = bottom;
    }
    solids
}

/// Where a cook's samples stand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sampling {
    /// The most metres between samples across a cell.
    pub spacing: f32,
    /// The most metres between samples upward.
    pub layer: f32,
    /// The camera heights sampled, least and most, in the zone's space.
    pub heights: [f32; 2],
    /// The smallest piece of an object's box a sample asks about, in metres.
    pub piece: f32,
}

impl Sampling {
    /// A camera following a walk or a run on flat ground: the zone walk's rides 2.46 m up, 5.4 m behind.
    pub const WALK: Self = Self { spacing: 1.0, layer: 1.0, heights: [2.0, 3.0], piece: 1.0 };
    /// A fight's cameras too, pulled back and up, up to 8 m (`default_camera.ron`'s encounter rig at its
    /// longest and steepest is about 7.8 m).
    pub const COMBAT: Self = Self { spacing: 1.0, layer: 1.5, heights: [0.5, 8.0], piece: 1.0 };

    /// Samples along one side of a cell, and upward.
    fn counts(&self, cell_size: f32) -> [usize; 2] {
        let across = (cell_size / self.spacing).ceil() as usize;
        let up = ((self.heights[1] - self.heights[0]) / self.layer).ceil() as usize;
        [across.max(1), up.max(1)]
    }

    /// How far a point of a cell's space can be from its nearest sample on each axis, in metres: half the
    /// spacing across and half a layer upward, what occluders shrink by.
    pub fn reach(&self, cell_size: f32) -> [f32; 3] {
        let [across, up] = self.counts(cell_size);
        let half_across = cell_size / across as f32 * 0.5;
        let half_up = (self.heights[1] - self.heights[0]) / up as f32 * 0.5;
        [half_across, half_up, half_across]
    }

    /// The samples over cell `coord`, in a fixed order.
    pub fn points(&self, cell_size: f32, coord: [i32; 2]) -> Vec<[f32; 3]> {
        let [across, up] = self.counts(cell_size);
        let step = cell_size / across as f32;
        let rise = (self.heights[1] - self.heights[0]) / up as f32;
        let origin = [coord[0] as f32 * cell_size, coord[1] as f32 * cell_size];
        let mut points = Vec::with_capacity(across * across * up);
        for k in 0..up {
            for j in 0..across {
                for i in 0..across {
                    points.push([
                        origin[0] + (i as f32 + 0.5) * step,
                        self.heights[0] + (k as f32 + 0.5) * rise,
                        origin[1] + (j as f32 + 0.5) * step,
                    ]);
                }
            }
        }
        points
    }
}

/// Occluder boxes, found by the buckets they reach.
pub struct Occluders {
    pub boxes: Vec<Aabb>,
    origin: [f32; 2],
    size: [usize; 2],
    buckets: Vec<Vec<u32>>,
}

impl Occluders {
    /// Every blocker's solids shrunk by `by` on each axis, and `MARGIN` more, on every side but a grounded
    /// one's foot, which is far under the ground, where no camera or object is.
    pub fn new(blockers: &[Blocker], by: [f32; 3]) -> Self {
        let boxes: Vec<Aabb> = blockers
            .iter()
            .flat_map(|blocker| &blocker.solids)
            .filter_map(|solid| {
                let grounded = solid.min[1] < BELOW + 1.0;
                let mut by = by.map(|by| by + MARGIN);
                let shrunk = solid.shrunk(by)?;
                if grounded {
                    by[1] = 0.0;
                }
                (shrunk.min[1] < shrunk.max[1])
                    .then_some(Aabb { min: [shrunk.min[0], solid.min[1] + by[1], shrunk.min[2]], ..shrunk })
            })
            .collect();
        let all = Aabb::of_points(boxes.iter().flat_map(|b| [b.min, b.max]))
            .unwrap_or(Aabb { min: [0.0; 3], max: [0.0; 3] });
        let origin = [all.min[0], all.min[2]];
        let size = [
            ((all.max[0] - origin[0]) / BUCKET).floor() as usize + 1,
            ((all.max[2] - origin[1]) / BUCKET).floor() as usize + 1,
        ];
        let mut buckets = vec![Vec::new(); size[0] * size[1]];
        let mut occluders = Self { boxes: Vec::new(), origin, size, buckets: Vec::new() };
        for (index, b) in boxes.iter().enumerate() {
            let ([x0, z0], [x1, z1]) = occluders.span(b);
            for z in z0..=z1 {
                for x in x0..=x1 {
                    buckets[z * size[0] + x].push(index as u32);
                }
            }
        }
        occluders.boxes = boxes;
        occluders.buckets = buckets;
        occluders
    }

    /// The buckets a box reaches, clamped to the grid.
    fn span(&self, b: &Aabb) -> ([usize; 2], [usize; 2]) {
        let bucket = |value: f32, origin: f32, size: usize| {
            (((value - origin) / BUCKET).floor().max(0.0) as usize).min(size - 1)
        };
        (
            [bucket(b.min[0], self.origin[0], self.size[0]), bucket(b.min[2], self.origin[1], self.size[1])],
            [bucket(b.max[0], self.origin[0], self.size[0]), bucket(b.max[2], self.origin[1], self.size[1])],
        )
    }

    /// Whether `point` is inside one.
    fn inside(&self, point: [f32; 3]) -> bool {
        let at = Aabb { min: point, max: point };
        let ([x0, z0], [x1, z1]) = self.span(&at);
        (z0..=z1).any(|z| {
            (x0..=x1).any(|x| {
                self.buckets[z * self.size[0] + x].iter().any(|&b| self.boxes[b as usize].contains(point))
            })
        })
    }

    /// Whether one occluder alone hides every point of `piece` from `from`.
    fn one_hides(&self, from: [f32; 3], piece: &Aabb, scratch: &mut Scratch) -> bool {
        let hull = piece.union(Aabb { min: from, max: from });
        let ([x0, z0], [x1, z1]) = self.span(&hull);
        scratch.stamp += 1;
        let corners = piece.corners();
        for z in z0..=z1 {
            for x in x0..=x1 {
                for &index in &self.buckets[z * self.size[0] + x] {
                    let seen = &mut scratch.seen[index as usize];
                    if *seen == scratch.stamp {
                        continue;
                    }
                    *seen = scratch.stamp;
                    let b = &self.boxes[index as usize];
                    if b.overlaps(&hull) && hides(b, from, piece, &corners) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Whether occluder `b` hides every point of `piece` (whose corners are `corners`) from `from`, which is
/// outside it.
fn hides(b: &Aabb, from: [f32; 3], piece: &Aabb, corners: &[[f32; 3]; 8]) -> bool {
    if b.holds(piece) {
        return true;
    }
    if !corners.iter().all(|corner| b.met_by_ray(from, *corner)) {
        return false;
    }
    // Met before the piece: an axis plane with `b` and `from` on one side and the piece on the other.
    let separated = (0..3).any(|axis| {
        (b.max[axis] <= piece.min[axis] && from[axis] < b.max[axis])
            || (b.min[axis] >= piece.max[axis] && from[axis] > b.min[axis])
    });
    separated || piece.distance2(from) > b.farthest2(from)
}

/// A worker's scratch: which occluders a test has looked at.
pub struct Scratch {
    seen: Vec<u32>,
    stamp: u32,
    /// Pieces asked about, for the stats.
    pub pieces: u64,
}

impl Scratch {
    pub fn new(occluders: &Occluders) -> Self {
        Self { seen: vec![0; occluders.boxes.len()], stamp: 0, pieces: 0 }
    }
}

/// Whether every point of `object` is hidden from `from` by `occluders`, splitting it down to `piece`.
pub fn hidden(
    occluders: &Occluders,
    from: [f32; 3],
    object: &Aabb,
    piece: f32,
    scratch: &mut Scratch,
) -> bool {
    scratch.pieces += 1;
    if occluders.one_hides(from, object, scratch) {
        return true;
    }
    let extent: [f32; 3] = std::array::from_fn(|axis| object.max[axis] - object.min[axis]);
    let axis = if extent[0] >= extent[1] && extent[0] >= extent[2] {
        0
    } else if extent[1] >= extent[2] {
        1
    } else {
        2
    };
    if extent[axis] <= piece {
        return false;
    }
    let middle = object.min[axis] + extent[axis] * 0.5;
    let mut low = *object;
    low.max[axis] = middle;
    let mut high = *object;
    high.min[axis] = middle;
    // The half nearer the camera first: it's the likelier to show.
    let (first, second) = if from[axis] < middle { (low, high) } else { (high, low) };
    hidden(occluders, from, &first, piece, scratch) && hidden(occluders, from, &second, piece, scratch)
}

/// Where the zone walk's camera stands along `points` every `step` metres, the walk facing along each leg:
/// 5.5 m behind and 1.5 m over the walker, pitched 10° down (`sapphire_stage`'s zone walk), on flat ground.
pub fn walk_eyes(points: &[[f32; 3]], step: f32) -> Vec<[f32; 3]> {
    let (back, up) = (5.5 * 0.984_807_8, 1.5 + 5.5 * 0.173_648_2);
    let mut eyes = Vec::new();
    for leg in points.windows(2) {
        let d = [leg[1][0] - leg[0][0], leg[1][2] - leg[0][2]];
        let length = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if length == 0.0 {
            continue;
        }
        let along = [d[0] / length, d[1] / length];
        let mut at = 0.0;
        while at < length {
            let place = [leg[0][0] + along[0] * at, leg[0][1], leg[0][2] + along[1] * at];
            eyes.push([place[0] - along[0] * back, place[1] + up, place[2] - along[1] * back]);
            at += step;
        }
    }
    eyes
}

/// For each eye: the objects its cell's set holds (`None` outside the sets' space), and those it sees itself
/// past the occluders as they are: what a set for that one point would hold, the most any set could hide.
pub fn probe_walk(
    scene: &VisibilityScene,
    sets: &CellVisibility,
    eyes: &[[f32; 3]],
) -> Vec<(Option<usize>, usize)> {
    let occluders = Occluders::new(&scene.blockers, [0.0; 3]);
    let objects: Vec<Aabb> = scene.objects().map(|object| object.bounds).collect();
    let sizes = sets.set_sizes();
    let mut scratch = Scratch::new(&occluders);
    eyes.iter()
        .map(|eye| {
            let cell = sets.cell_for(*eye).ok().map(|cell| sizes[cell]);
            let (seen, _, _) = region_set(&occluders, &objects, [*eye], 0.5, &mut scratch);
            (cell, seen.iter().map(|word| word.count_ones() as usize).sum())
        })
        .collect()
}

/// What a cook did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CookStats {
    pub cells: usize,
    pub objects: usize,
    pub occluders: usize,
    /// Samples asked, and those inside a shrunk occluder, which see nothing.
    pub samples: u64,
    pub samples_inside: u64,
    /// Pieces of objects asked about.
    pub pieces: u64,
}

/// Cooks `scene`'s sets at `sampling`, on up to `workers` threads.
pub fn cook(scene: &VisibilityScene, sampling: Sampling, workers: usize) -> (CellVisibility, CookStats) {
    let occluders = Occluders::new(&scene.blockers, sampling.reach(scene.cell_size));
    let objects: Vec<Aabb> = scene.objects().map(|object| object.bounds).collect();
    let words = objects.len().div_ceil(64);
    let next = AtomicUsize::new(0);
    let cells = scene.cells.len();
    let results: Vec<(usize, Vec<u64>, [u64; 3])> = std::thread::scope(|threads| {
        let handles: Vec<_> = (0..workers.clamp(1, cells.max(1)))
            .map(|_| {
                threads.spawn(|| {
                    let mut scratch = Scratch::new(&occluders);
                    let mut done = Vec::new();
                    loop {
                        let cell = next.fetch_add(1, Ordering::Relaxed);
                        if cell >= cells {
                            return done;
                        }
                        scratch.pieces = 0;
                        let (set, samples, inside) = cell_set(
                            &occluders,
                            &objects,
                            sampling,
                            scene.cell_size,
                            scene.cells[cell].coord,
                            &mut scratch,
                        );
                        done.push((cell, set, [samples, inside, scratch.pieces]));
                    }
                })
            })
            .collect();
        handles.into_iter().flat_map(|handle| handle.join().expect("a cell's set doesn't panic")).collect()
    });
    let mut bits = vec![0_u64; cells * words];
    let mut stats =
        CookStats { cells, objects: objects.len(), occluders: occluders.boxes.len(), ..CookStats::default() };
    for (cell, set, [samples, inside, pieces]) in results {
        bits[cell * words..(cell + 1) * words].copy_from_slice(&set);
        stats.samples += samples;
        stats.samples_inside += inside;
        stats.pieces += pieces;
    }
    let sets = CellVisibility {
        cell_size: scene.cell_size,
        heights: sampling.heights,
        cells: scene
            .cells
            .iter()
            .map(|cell| VisibleCell {
                coord: cell.coord,
                objects: cell.objects.iter().map(|object| object.mesh).collect(),
            })
            .collect(),
        keep_out: keep_out(&scene.blockers),
        bits,
    };
    (sets, stats)
}

/// The blockers' bounds, each once, in order.
fn keep_out(blockers: &[Blocker]) -> Vec<Bounds> {
    let mut seen = BTreeSet::new();
    blockers
        .iter()
        .filter(|blocker| {
            seen.insert(
                blocker.bounds.min.iter().chain(&blocker.bounds.max).map(|v| v.to_bits()).collect::<Vec<_>>(),
            )
        })
        .map(|blocker| blocker.bounds.to_bounds())
        .collect()
}

/// One cell's set: a bit per object some sample sees. Also how many samples it asked, and how many stood
/// inside an occluder.
fn cell_set(
    occluders: &Occluders,
    objects: &[Aabb],
    sampling: Sampling,
    cell_size: f32,
    coord: [i32; 2],
    scratch: &mut Scratch,
) -> (Vec<u64>, u64, u64) {
    region_set(occluders, objects, sampling.points(cell_size, coord), sampling.piece, scratch)
}

/// The set seen from `points`: a bit per object some point sees past `occluders`, which are shrunk by how far
/// a camera can stand from its nearest point. Also how many points it asked, and how many stood inside an
/// occluder.
pub fn region_set(
    occluders: &Occluders,
    objects: &[Aabb],
    points: impl IntoIterator<Item = [f32; 3]>,
    piece: f32,
    scratch: &mut Scratch,
) -> (Vec<u64>, u64, u64) {
    let mut set = vec![0_u64; objects.len().div_ceil(64)];
    let mut unseen: Vec<usize> = (0..objects.len()).collect();
    let (mut samples, mut inside) = (0, 0);
    for point in points {
        if unseen.is_empty() {
            break;
        }
        samples += 1;
        if occluders.inside(point) {
            inside += 1;
            continue;
        }
        unseen.retain(|&object| {
            let seen = !hidden(occluders, point, &objects[object], piece, scratch);
            if seen {
                set[object / 64] |= 1 << (object % 64);
            }
            !seen
        });
    }
    (set, samples, inside)
}

#[cfg(test)]
mod tests;
