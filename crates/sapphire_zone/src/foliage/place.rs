//! Placing a foliage document's plants: pure functions of the document, the ground and a region.
//!
//! **Candidates.** A rule's candidates lie one to each square of a lattice over the whole of world space,
//! twice its `spacing` a side, each jittered within the middle half of its square, so no two stand closer than
//! `spacing`. Grass's lattice is `1 / √per_m2` a side, each candidate anywhere in its square. Everything about
//! a candidate (where it stands, whether it's kept, its yaw, scale and tint) is a hash of the rule's name, its
//! seed and the square's coordinates, never of the region asked for: so any region holding a plant places it
//! the same, and a cell, a chunk or a thread can't move it.
//!
//! **What's kept.** A rule keeps a candidate with the chance its density gives a square (`per_100m2` of a
//! hundred square metres; grass keeps every one), times its cluster's noise if it has one; then only if it
//! stands inside one of its layers, on ground the [`Ground`] finds under it, within the rule's slope and
//! height, outside its clearings, and more than its distance from every plant of the kinds it avoids that an
//! earlier rule kept. A rule that something avoids is placed over the region grown by as far as that reaches,
//! so a plant near a region's edge is judged by the same neighbours as anywhere else.
//!
//! **The maths.** Integer hashing for every choice, a value noise of hashed corners, and `f32` and `f64`
//! arithmetic that's correctly rounded on every architecture, with `sapphire_math::det` for the one cosine
//! and logarithm: nothing reads an address, a hash map's order or a platform's maths library, so two machines
//! place the same bits.

use std::collections::BTreeMap;

use sapphire_core::Name;
use sapphire_math::det;

use super::{
    Avoid, FoliageDocument, FoliageLayer, MAX_SCALE, MAX_SET_INSTANCES, MIN_SCALE, RuleView, ScatterInstance,
    ScatterSet,
};
use crate::document::ZoneError;
use crate::grid::{CellCoord, Grid};

/// How far a rule's candidate may stray from its lattice square's centre, as a share of the square.
const JITTER: f32 = 0.5;

/// The ground under a point, as a [`Ground`] finds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundPoint {
    /// Its height, in metres.
    pub height: f32,
    /// Its normal, facing up, of any length.
    pub normal: [f32; 3],
}

/// What a plant stands on: the highest ground under `(x, z)` facing up, or `None` where there's none. The cook's
/// is a cast down through the zone's static colliders; a terrain's (W12a) will be its height field. Must be a
/// pure function of its inputs for placement to be.
pub trait Ground {
    fn at(&self, x: f32, z: f32) -> Option<GroundPoint>;
}

impl<F: Fn(f32, f32) -> Option<GroundPoint>> Ground for F {
    fn at(&self, x: f32, z: f32) -> Option<GroundPoint> {
        self(x, z)
    }
}

/// A rectangle of the ground: `min` to `max` in X and Z, each half-open, so regions that tile share no point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl Region {
    /// A cell's square on `grid`.
    #[must_use]
    pub fn cell(grid: Grid, cell: CellCoord) -> Self {
        let size = grid.cell_size;
        let min = [cell.x as f32 * size, cell.z as f32 * size];
        Self { min, max: [min[0] + size, min[1] + size] }
    }

    /// The region `by` metres larger on every side.
    #[must_use]
    pub fn grown(self, by: f32) -> Self {
        Self { min: [self.min[0] - by, self.min[1] - by], max: [self.max[0] + by, self.max[1] + by] }
    }

    /// Whether it holds `(x, z)`.
    #[must_use]
    pub fn holds(self, x: f32, z: f32) -> bool {
        self.min[0] <= x && x < self.max[0] && self.min[1] <= z && z < self.max[1]
    }
}

/// One plant placed: its kind's index in the document, its stable ID and its instance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub kind: u32,
    pub id: u64,
    pub instance: ScatterInstance,
}

/// The most plants a hundred square metres a rule of `spacing` can hold: one to a square of its lattice.
pub(crate) fn most_per_100m2(spacing: f32) -> f32 {
    let side = lattice(spacing);
    100.0 / (side * side)
}

/// A rule's lattice side for `spacing`: two squares' jittered candidates are at least `spacing` apart.
fn lattice(spacing: f32) -> f32 {
    spacing / (1.0 - JITTER)
}

/// Every plant `foliage`'s rules place in `region`, in no particular order. `foliage` must be
/// [checked](FoliageDocument::check). See the [module docs](self).
pub fn place_rules(foliage: &FoliageDocument, ground: &impl Ground, region: Region) -> Vec<Placed> {
    let margins = margins(foliage);
    let mut placed: Vec<Placed> = Vec::new();
    for (index, rule) in foliage.rules.iter().enumerate() {
        let judge = Judge::new(foliage, &rule.view(), &placed);
        let side = lattice(rule.spacing);
        let chance = rule.per_100m2 / 100.0 * side * side;
        let mut kept = Vec::new();
        for_each_square(region.grown(margins[index]), side, |x, z| {
            let draws = judge.draws(x, z);
            let [u, v] = draws.jitter;
            let at =
                [(x as f32 + 0.5 + JITTER * (u - 0.5)) * side, (z as f32 + 0.5 + JITTER * (v - 0.5)) * side];
            if let Some(plant) = judge.plant(x, z, at, &draws, chance, ground) {
                kept.push(plant);
            }
        });
        placed.extend(kept);
    }
    placed.retain(|plant| region.holds(plant.instance.position[0], plant.instance.position[2]));
    placed
}

/// How far past a region each rule must be placed for the rules after it to judge their plants there by every
/// neighbour: as far as what avoids its kind reaches, and as far again as that rule must be placed.
fn margins(foliage: &FoliageDocument) -> Vec<f32> {
    let mut margins = vec![0.0_f32; foliage.rules.len()];
    for later in (0..foliage.rules.len()).rev() {
        for avoid in &foliage.rules[later].avoid {
            let Avoid::Kinds { kinds, distance } = avoid else { continue };
            for earlier in 0..later {
                if kinds.contains(&foliage.rules[earlier].kind) {
                    margins[earlier] = margins[earlier].max(margins[later] + distance);
                }
            }
        }
    }
    margins
}

/// How far the plants grass avoids must have been placed past a region: its farthest `distance`.
#[must_use]
pub fn grass_reach(foliage: &FoliageDocument) -> f32 {
    let distances = foliage.grass.iter().flat_map(|grass| &grass.avoid);
    distances
        .filter_map(|avoid| match avoid {
            Avoid::Kinds { distance, .. } => Some(*distance),
            Avoid::Area { .. } => None,
        })
        .fold(0.0, f32::max)
}

/// Adds to `out` every clump `foliage`'s grass rule `rule` places in `region`. `near` holds what the rules
/// placed over `region` grown by [`grass_reach`], from which it keeps its distance. See the [module docs](self).
pub fn place_grass(
    foliage: &FoliageDocument,
    rule: usize,
    ground: &impl Ground,
    region: Region,
    near: &[Placed],
    out: &mut Vec<Placed>,
) {
    let grass = &foliage.grass[rule];
    let judge = Judge::new(foliage, &grass.view(), near);
    let side = 1.0 / grass.per_m2.sqrt();
    for_each_square(region, side, |x, z| {
        let draws = judge.draws(x, z);
        let [u, v] = draws.jitter;
        let at = [(x as f32 + u) * side, (z as f32 + v) * side];
        if region.holds(at[0], at[1])
            && let Some(plant) = judge.plant(x, z, at, &draws, 1.0, ground)
        {
            out.push(plant);
        }
    });
}

/// The cell `cell`'s scatter set: every plant `foliage` places whose root is in it on `grid`, grass included.
/// `foliage` must be [checked](FoliageDocument::check).
pub fn scatter_set(
    foliage: &FoliageDocument,
    ground: &impl Ground,
    grid: Grid,
    cell: CellCoord,
) -> Result<ScatterSet, ZoneError> {
    // A metre past the square, as what's in the cell is the grid's to say, not a float's comparison.
    let region = Region::cell(grid, cell).grown(1.0);
    let mut placed = place_rules(foliage, ground, region.grown(grass_reach(foliage)));
    let mut grass = Vec::new();
    for rule in 0..foliage.grass.len() {
        place_grass(foliage, rule, ground, region, &placed, &mut grass);
    }
    placed.append(&mut grass);
    placed.retain(|plant| grid.cell_of(plant.instance.position) == cell);
    if placed.len() > MAX_SET_INSTANCES {
        let problem = format!("{} plants is more than the {MAX_SET_INSTANCES} a cell may hold", placed.len());
        return Err(ZoneError::invalid(format!("foliage: cell ({}, {})", cell.x, cell.z), problem));
    }
    // One rule's IDs never meet; two rules' meet by a 2⁻⁶⁴ chance, refused rather than written.
    let mut ids: Vec<u64> = placed.iter().map(|plant| plant.id).collect();
    ids.sort_unstable();
    if let Some(pair) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
        let problem = format!("two plants share the ID {:#018x}: rename one of their rules", pair[0]);
        return Err(ZoneError::invalid(format!("foliage: cell ({}, {})", cell.x, cell.z), problem));
    }
    Ok(ScatterSet::from_placed([cell.x, cell.z], placed))
}

/// Calls `each` with every lattice square of `side` that `region` meets.
fn for_each_square(region: Region, side: f32, mut each: impl FnMut(i32, i32)) {
    let square = |value: f32| (value / side).floor() as i32;
    for z in square(region.min[1])..=square(region.max[1]) {
        for x in square(region.min[0])..=square(region.max[0]) {
            each(x, z);
        }
    }
}

/// What one rule keeps of its candidates.
struct Judge<'a> {
    kind: u32,
    /// The rule's name's hash: its plants' IDs.
    stream: u64,
    /// That and its seed: where its plants stand and what they look like.
    seeded: u64,
    layers: Vec<&'a FoliageLayer>,
    /// The cosines of the steepest and the flattest slope it grows on.
    steepest: f64,
    flattest: f64,
    height: Option<[f32; 2]>,
    /// The noise's stream, scale and contrast.
    cluster: Option<(u64, f32, f32)>,
    clearings: Vec<([f32; 3], crate::ZoneShape)>,
    avoided: Vec<Near>,
    /// The bytes its scales are chosen between.
    scales: [u8; 2],
}

impl<'a> Judge<'a> {
    fn new(foliage: &'a FoliageDocument, rule: &RuleView<'_>, placed: &[Placed]) -> Self {
        let stream = Name::new(rule.name).as_u64();
        let seeded = mix(stream ^ mix(u64::from(rule.seed) ^ 0x5eed));
        let cosine = |degrees: f32| f64::from(det::cos(degrees.to_radians()).max(0.0));
        let mut clearings = Vec::new();
        let mut avoided = Vec::new();
        for avoid in rule.avoid {
            match avoid {
                Avoid::Kinds { kinds, distance } => {
                    let indices: Vec<u32> = kinds
                        .iter()
                        .filter_map(|kind| foliage.kind_index(kind))
                        .map(|index| index as u32)
                        .collect();
                    let points = placed.iter().filter(|plant| indices.contains(&plant.kind));
                    avoided.push(Near::new(*distance, points.map(|plant| plant.instance.position)));
                }
                Avoid::Area { position, shape } => clearings.push((*position, *shape)),
            }
        }
        Self {
            kind: foliage.kind_index(rule.kind).expect("a checked rule's kind is the document's") as u32,
            stream,
            seeded,
            layers: foliage.layers.iter().filter(|layer| rule.on.contains(&layer.name)).collect(),
            steepest: cosine(rule.slope_degrees[1]),
            flattest: cosine(rule.slope_degrees[0]),
            height: rule.height,
            cluster: rule.cluster.map(|cluster| (mix(seeded ^ 0xc105_7e75), cluster.scale, cluster.contrast)),
            clearings,
            avoided,
            scales: rule.scale.map(scale_byte),
        }
    }

    /// The lattice square `(x, z)`'s draws.
    fn draws(&self, x: i32, z: i32) -> Draws {
        let first = hash(self.seeded, x, z, 0);
        let second = hash(self.seeded, x, z, 1);
        Draws { jitter: [unit(first), unit(first >> 32)], keep: unit(second), look: (second >> 32) as u32 }
    }

    /// The plant the candidate of square `(x, z)` at `at` makes, if its draws keep it against `chance`.
    fn plant(
        &self,
        x: i32,
        z: i32,
        at: [f32; 2],
        draws: &Draws,
        chance: f32,
        ground: &impl Ground,
    ) -> Option<Placed> {
        let factor = self.cluster.map_or(1.0, |(stream, scale, contrast)| {
            1.0 - contrast + contrast * 2.0 * noise(stream, scale, at[0], at[1])
        });
        if draws.keep >= chance * factor || !self.layers.iter().any(|layer| inside(layer, at)) {
            return None;
        }
        let point = ground.at(at[0], at[1])?;
        if !self.slope_holds(point.normal)
            || self.height.is_some_and(|[low, high]| !(low..=high).contains(&point.height))
        {
            return None;
        }
        let position = [at[0], point.height, at[1]];
        let cleared = self.clearings.iter().any(|(centre, shape)| {
            shape.holds([position[0] - centre[0], position[1] - centre[1], position[2] - centre[2]])
        });
        if cleared || self.avoided.iter().any(|near| near.within(position)) {
            return None;
        }
        let [yaw, scale, tint, _] = draws.look.to_le_bytes();
        let [low, high] = self.scales;
        let scale = low + (u32::from(scale) % (u32::from(high - low) + 1)) as u8;
        let id = mix(self.stream ^ square_key(x, z));
        Some(Placed { kind: self.kind, id, instance: ScatterInstance::new(position, yaw, scale, tint, 0) })
    }

    /// Whether ground of `normal` lies within its slopes: in `f64` with no root, as the navmesh bake's slope is.
    fn slope_holds(&self, normal: [f32; 3]) -> bool {
        let [x, y, z] = normal.map(f64::from);
        let (up, length) = (y * y, x * x + y * y + z * z);
        y > 0.0
            && up >= self.steepest * self.steepest * length
            && up <= self.flattest * self.flattest * length
    }
}

/// A lattice square's draws: its candidate's jitter in X and Z within the square, its chance of being kept,
/// and its look's bits (yaw, scale and tint).
struct Draws {
    jitter: [f32; 2],
    keep: f32,
    look: u32,
}

/// The byte nearest `scale`: `255 · log₄(scale / 0.5)`.
fn scale_byte(scale: f32) -> u8 {
    let scale = scale.clamp(MIN_SCALE, MAX_SCALE);
    (det::log2(scale / MIN_SCALE) * (255.0 / 2.0)).round().clamp(0.0, 255.0) as u8
}

/// Whether `at` is inside `layer`: inside an odd number of its outlines, by crossings in `f64`.
fn inside(layer: &FoliageLayer, at: [f32; 2]) -> bool {
    let [px, pz] = at.map(f64::from);
    let mut crossings = 0u32;
    for outline in &layer.outlines {
        let mut previous = outline[outline.len() - 1].map(f64::from);
        for point in outline {
            let point = point.map(f64::from);
            if (point[1] > pz) != (previous[1] > pz) {
                let x = point[0] + (pz - point[1]) * (previous[0] - point[0]) / (previous[1] - point[1]);
                if px < x {
                    crossings += 1;
                }
            }
            previous = point;
        }
    }
    crossings % 2 == 1
}

/// The plants a rule keeps its distance from, bucketed by that distance.
struct Near {
    distance: f32,
    buckets: BTreeMap<(i32, i32), Vec<[f32; 2]>>,
}

impl Near {
    fn new(distance: f32, points: impl Iterator<Item = [f32; 3]>) -> Self {
        let mut buckets: BTreeMap<(i32, i32), Vec<[f32; 2]>> = BTreeMap::new();
        for [x, _, z] in points {
            buckets.entry(bucket(distance, x, z)).or_default().push([x, z]);
        }
        Self { distance, buckets }
    }

    /// Whether a plant stands within its distance of `position`, across the ground.
    fn within(&self, position: [f32; 3]) -> bool {
        let [x, _, z] = position;
        let (bx, bz) = bucket(self.distance, x, z);
        let reach = self.distance * self.distance;
        (bz - 1..=bz + 1).any(|bz| {
            (bx - 1..=bx + 1).any(|bx| {
                self.buckets.get(&(bx, bz)).is_some_and(|points| {
                    points.iter().any(|&[px, pz]| (px - x) * (px - x) + (pz - z) * (pz - z) < reach)
                })
            })
        })
    }
}

fn bucket(side: f32, x: f32, z: f32) -> (i32, i32) {
    ((x / side).floor() as i32, (z / side).floor() as i32)
}

/// A value noise from 0 to 1 at `scale` metres: hashed corners, eased between.
fn noise(stream: u64, scale: f32, x: f32, z: f32) -> f32 {
    let (gx, gz) = (x / scale, z / scale);
    let (cx, cz) = (gx.floor(), gz.floor());
    let ease = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sz) = (ease(gx - cx), ease(gz - cz));
    let (cx, cz) = (cx as i32, cz as i32);
    let corner = |dx: i32, dz: i32| unit(hash(stream, cx.wrapping_add(dx), cz.wrapping_add(dz), 2));
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    lerp(lerp(corner(0, 0), corner(1, 0), sx), lerp(corner(0, 1), corner(1, 1), sx), sz)
}

/// SplitMix64's finalizer: every bit of the input moves every bit of the output.
fn mix(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// A lattice square's coordinates as one word.
fn square_key(x: i32, z: i32) -> u64 {
    u64::from(x as u32) | (u64::from(z as u32) << 32)
}

/// A hash of `stream`, a lattice square and which draw it is.
fn hash(stream: u64, x: i32, z: i32, draw: u64) -> u64 {
    mix(stream ^ mix(square_key(x, z) ^ mix(draw.wrapping_add(0x9e37_79b9_7f4a_7c15))))
}

/// The low 24 bits of `bits` as a number in `[0, 1)`, which an `f32` holds exactly.
fn unit(bits: u64) -> f32 {
    (bits & 0x00ff_ffff) as f32 * (1.0 / (1u32 << 24) as f32)
}
