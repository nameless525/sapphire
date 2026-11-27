//! What of a zone a rule's plants keep off (M6 G1b): its splines, its placements' footprints and its water.
//!
//! A cook makes [`Surroundings`] once a zone, from its document and the footprints it measures of its
//! placements, and placement asks each rule's [`Keep`] of a candidate's root. Everything is across the ground,
//! in X and Z, in `f32` sums, products and quotients with no root, but for an ocean's shore, which is the
//! ground's height round a root against its surface. Nothing reads a platform's maths library: a footprint's
//! turn is `det`'s sine and cosine. Its box is the prefab's import, which every machine makes alike, as it does
//! the cells' meshes from the same import; it's rounded to a 64th of a metre before it's turned only as a grain,
//! so the footprint is no finer than a placement is placed.

use std::collections::BTreeMap;

use sapphire_math::det;

use super::{Avoid, Ground};
use crate::document::{ZoneDocument, ZoneError};
use crate::water::WaterShape;

/// A bucket's side, in metres: a root's keep-offs are found among its bucket's.
const BUCKET: f32 = 16.0;

/// The most buckets one thing may span before it's asked of every root instead: a 1 km square at 16 m.
const MAX_BUCKETS: i64 = 1 << 12;

/// How many stretches a spline's curve is sampled to between two of its points.
const SPLINE_STEPS: u32 = 8;

/// The grain a footprint's box is rounded to, in metres.
const FOOTPRINT_GRAIN: f32 = 1.0 / 64.0;

/// Eight directions round a root, exactly unit length in `f32` to within a rounding: where an ocean's shore is
/// looked for.
const AROUND: [[f32; 2]; 8] = [
    [1.0, 0.0],
    [std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2],
    [0.0, 1.0],
    [-std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2],
    [-1.0, 0.0],
    [-std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2],
    [0.0, -1.0],
    [std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2],
];

/// A placement's footprint on the ground: a box in X and Z, turned about +Y.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Footprint {
    /// Its middle, `[x, z]`, in metres in the zone's space.
    pub centre: [f32; 2],
    /// Its half extents along its own axes.
    pub half: [f32; 2],
    /// The cosine and sine of its turn.
    pub turn: [f32; 2],
}

impl Footprint {
    /// The footprint of the box `min` to `max`, in a prefab's space, placed by `at · turn · scale` as a zone
    /// places a prefab (`turn` in degrees about +Y, counter-clockwise seen from above). Its box is rounded to a
    /// 64th of a metre first, so the last bits of a mesh read on another machine place the same plants.
    #[must_use]
    pub fn placed(min: [f32; 3], max: [f32; 3], at: [f32; 3], turn: f32, scale: [f32; 3]) -> Self {
        let round = |value: f32| (value / FOOTPRINT_GRAIN).round() * FOOTPRINT_GRAIN;
        let (min, max) = ([round(min[0]), round(min[2])], [round(max[0]), round(max[2])]);
        let middle = [(min[0] + max[0]) * 0.5 * scale[0], (min[1] + max[1]) * 0.5 * scale[2]];
        let half = [((max[0] - min[0]) * 0.5 * scale[0]).abs(), ((max[1] - min[1]) * 0.5 * scale[2]).abs()];
        let (sin, cos) = det::sin_cos(turn.to_radians());
        // About +Y: x' = x cos + z sin, z' = z cos − x sin.
        let centre = [at[0] + middle[0] * cos + middle[1] * sin, at[2] + middle[1] * cos - middle[0] * sin];
        Self { centre, half, turn: [cos, sin] }
    }

    /// How far `(x, z)` lies outside it along each of its axes, nought inside.
    fn outside(&self, x: f32, z: f32) -> [f32; 2] {
        let (dx, dz) = (x - self.centre[0], z - self.centre[1]);
        let [cos, sin] = self.turn;
        let local = [dx * cos - dz * sin, dx * sin + dz * cos];
        [(local[0].abs() - self.half[0]).max(0.0), (local[1].abs() - self.half[1]).max(0.0)]
    }

    /// Around it, `[x, z]` low and high.
    fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        let [cos, sin] = self.turn.map(f32::abs);
        let reach = [self.half[0] * cos + self.half[1] * sin, self.half[0] * sin + self.half[1] * cos];
        let [x, z] = self.centre;
        ([x - reach[0], z - reach[1]], [x + reach[0], z + reach[1]])
    }
}

/// What of a zone placement keeps plants off: its splines, sampled, its placements' footprints and its water's
/// shapes. Empty for a zone that has none of them, or a caller that places with no zone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Surroundings {
    /// Each spline's name and its curve, sampled into a line across the ground.
    splines: Vec<(String, Vec<[f32; 2]>)>,
    footprints: Vec<Footprint>,
    water: Vec<WaterShape>,
}

impl Surroundings {
    /// A zone's: its document's splines and water, and `footprints`, which a cook measures of its placements
    /// (only it has their geometry).
    #[must_use]
    pub fn new(zone: &ZoneDocument, footprints: Vec<Footprint>) -> Self {
        let splines =
            zone.splines.iter().map(|spline| (spline.name.clone(), sampled(&spline.points, spline.closed)));
        let water = zone.water.iter().map(|body| body.shape.clone()).collect();
        Self { splines: splines.collect(), footprints, water }
    }

    /// Whether every spline a foliage document keeps off by name is one of these: the zone's to say, so the
    /// cook's to ask.
    pub fn check(&self, foliage: &super::FoliageDocument) -> Result<(), ZoneError> {
        let rules = foliage
            .rules
            .iter()
            .map(|rule| &rule.avoid)
            .enumerate()
            .map(|(at, avoid)| (format!("rules[{at}]"), avoid));
        let grass = foliage
            .grass
            .iter()
            .map(|grass| &grass.avoid)
            .enumerate()
            .map(|(at, avoid)| (format!("grass[{at}]"), avoid));
        for (owner, avoids) in rules.chain(grass) {
            for (which, avoid) in avoids.iter().enumerate() {
                let Avoid::Splines { names, .. } = avoid else { continue };
                for (index, name) in names.iter().enumerate() {
                    if !self.splines.iter().any(|(known, _)| known == name) {
                        let field = format!("{owner}.avoid[{which}].names[{index}]");
                        let problem =
                            format!("it keeps off a spline `{name}`, and the zone has none of that name");
                        return Err(ZoneError::invalid(&field, problem));
                    }
                }
            }
        }
        Ok(())
    }
}

/// The uniform Catmull-Rom through `points`, ends held, sampled at [`SPLINE_STEPS`] a stretch, across the
/// ground. One point is itself.
fn sampled(points: &[[f32; 3]], closed: bool) -> Vec<[f32; 2]> {
    let flat: Vec<[f32; 2]> = points.iter().map(|&[x, _, z]| [x, z]).collect();
    let count = flat.len();
    if count < 2 {
        return flat;
    }
    let at = |index: isize| {
        let index = if closed {
            index.rem_euclid(count as isize) as usize
        } else {
            index.clamp(0, count as isize - 1) as usize
        };
        flat[index]
    };
    let stretches = if closed { count } else { count - 1 };
    let mut line = Vec::with_capacity(stretches * SPLINE_STEPS as usize + 1);
    for stretch in 0..stretches as isize {
        let [p0, p1, p2, p3] = [at(stretch - 1), at(stretch), at(stretch + 1), at(stretch + 2)];
        for step in 0..SPLINE_STEPS {
            let t = step as f32 / SPLINE_STEPS as f32;
            let (t2, t3) = (t * t, t * t * t);
            let axis = |a: usize| {
                0.5 * (2.0 * p1[a]
                    + (p2[a] - p0[a]) * t
                    + (2.0 * p0[a] - 5.0 * p1[a] + 4.0 * p2[a] - p3[a]) * t2
                    + (3.0 * p1[a] - p0[a] - 3.0 * p2[a] + p3[a]) * t3)
            };
            line.push([axis(0), axis(1)]);
        }
    }
    line.push(at(stretches as isize));
    line
}

/// One thing a rule keeps off.
enum Item<'a> {
    /// A stretch from `a` to `b`, `half` wide each side at each end: a spline's (nought wide), a lake's shore
    /// or a river's.
    Stretch {
        a: [f32; 2],
        b: [f32; 2],
        half: [f32; 2],
    },
    Footprint(Footprint),
    /// The inside of a lake's outline.
    Inside(&'a [[f32; 2]]),
}

/// What one rule keeps its plants off, bucketed (see the [module docs](self)).
pub(crate) struct Keep<'a> {
    distance: f32,
    items: Vec<Item<'a>>,
    buckets: BTreeMap<(i32, i32), Vec<u32>>,
    /// Those too wide to bucket, asked of every root.
    everywhere: Vec<u32>,
    /// The heights of the oceans it keeps off.
    oceans: Vec<f32>,
}

impl<'a> Keep<'a> {
    /// What `avoid` keeps off of `around`, if it's one of the zone's.
    pub(crate) fn new(avoid: &Avoid, around: &'a Surroundings) -> Option<Self> {
        let (mut items, mut oceans) = (Vec::new(), Vec::new());
        let distance = match avoid {
            Avoid::Kinds { .. } | Avoid::Area { .. } => return None,
            Avoid::Splines { names, distance } => {
                let named =
                    around.splines.iter().filter(|(name, _)| names.is_empty() || names.contains(name));
                for (_, line) in named {
                    if let [only] = line[..] {
                        items.push(Item::Stretch { a: only, b: only, half: [0.0; 2] });
                    }
                    for pair in line.windows(2) {
                        items.push(Item::Stretch { a: pair[0], b: pair[1], half: [0.0; 2] });
                    }
                }
                *distance
            }
            Avoid::Placements { distance } => {
                items.extend(around.footprints.iter().copied().map(Item::Footprint));
                *distance
            }
            Avoid::Water { distance } => {
                for shape in &around.water {
                    match shape {
                        WaterShape::Lake { outline, .. } => {
                            items.push(Item::Inside(outline));
                            let mut previous = outline[outline.len() - 1];
                            for &point in outline {
                                items.push(Item::Stretch { a: previous, b: point, half: [0.0; 2] });
                                previous = point;
                            }
                        }
                        WaterShape::River { points } => {
                            for pair in points.windows(2) {
                                let [from, to] = [pair[0], pair[1]];
                                let (a, b) = ([from.at[0], from.at[2]], [to.at[0], to.at[2]]);
                                items.push(Item::Stretch { a, b, half: [from.width * 0.5, to.width * 0.5] });
                            }
                        }
                        WaterShape::Ocean { height } => oceans.push(*height),
                    }
                }
                *distance
            }
        };
        let mut keep = Self { distance, items, buckets: BTreeMap::new(), everywhere: Vec::new(), oceans };
        keep.bucket();
        Some(keep)
    }

    fn bucket(&mut self) {
        let bucket = |value: f32| (value / BUCKET).floor() as i32;
        for (index, item) in self.items.iter().enumerate() {
            let (low, high) = match item {
                Item::Stretch { a, b, half } => {
                    let reach = half[0].max(half[1]);
                    (
                        [a[0].min(b[0]) - reach, a[1].min(b[1]) - reach],
                        [a[0].max(b[0]) + reach, a[1].max(b[1]) + reach],
                    )
                }
                Item::Footprint(footprint) => footprint.bounds(),
                Item::Inside(outline) => {
                    outline.iter().fold(([f32::MAX; 2], [f32::MIN; 2]), |(low, high), point| {
                        (
                            [low[0].min(point[0]), low[1].min(point[1])],
                            [high[0].max(point[0]), high[1].max(point[1])],
                        )
                    })
                }
            };
            let (x0, z0) = (bucket(low[0] - self.distance), bucket(low[1] - self.distance));
            let (x1, z1) = (bucket(high[0] + self.distance), bucket(high[1] + self.distance));
            let span = (i64::from(x1) - i64::from(x0) + 1) * (i64::from(z1) - i64::from(z0) + 1);
            if span > MAX_BUCKETS {
                self.everywhere.push(index as u32);
                continue;
            }
            for x in x0..=x1 {
                for z in z0..=z1 {
                    self.buckets.entry((x, z)).or_default().push(index as u32);
                }
            }
        }
    }

    /// Whether a root at `position` stands within its distance of what it keeps off, or in it.
    pub(crate) fn near(&self, position: [f32; 3], ground: &impl Ground) -> bool {
        let [x, y, z] = position;
        let key = ((x / BUCKET).floor() as i32, (z / BUCKET).floor() as i32);
        let bucketed = self.buckets.get(&key).map_or(&[][..], Vec::as_slice);
        let near_item = |&index: &u32| match &self.items[index as usize] {
            Item::Stretch { a, b, half } => {
                let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
                let length = dx * dx + dz * dz;
                let t = if length > 0.0 {
                    (((x - a[0]) * dx + (z - a[1]) * dz) / length).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (ox, oz) = (x - (a[0] + dx * t), z - (a[1] + dz * t));
                let reach = half[0] + (half[1] - half[0]) * t + self.distance;
                ox * ox + oz * oz < reach * reach
            }
            Item::Footprint(footprint) => {
                let [ox, oz] = footprint.outside(x, z);
                ox * ox + oz * oz < self.distance * self.distance
            }
            Item::Inside(outline) => inside(outline, x, z),
        };
        if bucketed.iter().chain(&self.everywhere).any(near_item) {
            return true;
        }
        // An ocean is everywhere under its surface: a root in it, or with ground in it in eight directions at
        // its distance and at half of it, which is how near a shore is looked for.
        let reaches = [self.distance, self.distance * 0.5];
        self.oceans.iter().any(|&height| {
            y < height
                || reaches.iter().any(|&reach| {
                    AROUND.iter().any(|[ux, uz]| {
                        ground.at(x + ux * reach, z + uz * reach).is_some_and(|point| point.height < height)
                    })
                })
        })
    }
}

/// Whether `(x, z)` is inside `outline`, by crossings in `f64`, as a layer's are counted.
fn inside(outline: &[[f32; 2]], x: f32, z: f32) -> bool {
    let (px, pz) = (f64::from(x), f64::from(z));
    let mut odd = false;
    let mut previous = outline[outline.len() - 1].map(f64::from);
    for point in outline {
        let point = point.map(f64::from);
        if (point[1] > pz) != (previous[1] > pz) {
            let crossing = point[0] + (pz - point[1]) * (previous[0] - point[0]) / (previous[1] - point[1]);
            if px < crossing {
                odd = !odd;
            }
        }
        previous = point;
    }
    odd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_footprint_turns_as_a_placement_does_and_rounds_its_box() {
        // A 4 by 2 box, a hair off whole metres, turned a quarter about +Y and moved to (10, 0, 20).
        let footprint = Footprint::placed(
            [-2.000_001, 0.0, -1.0],
            [2.0, 3.0, 0.999_999],
            [10.0, 0.0, 20.0],
            90.0,
            [1.0; 3],
        );
        assert_eq!(footprint.half, [2.0, 1.0], "rounded to a 64th");
        assert!((footprint.centre[0] - 10.0).abs() < 1e-5 && (footprint.centre[1] - 20.0).abs() < 1e-5);
        // Turned a quarter, its long side runs along Z.
        assert_eq!(footprint.outside(10.0, 21.9), [0.0, 0.0]);
        assert!(footprint.outside(11.5, 20.0)[0] > 0.0 || footprint.outside(11.5, 20.0)[1] > 0.0);
        let (low, high) = footprint.bounds();
        assert!((low[0] - 9.0).abs() < 1e-5 && (high[1] - 22.0).abs() < 1e-5, "{low:?} {high:?}");
        // An off-centre box moves with its turn.
        let off = Footprint::placed([2.0, 0.0, -1.0], [4.0, 1.0, 1.0], [0.0; 3], 90.0, [1.0; 3]);
        assert!((off.centre[0]).abs() < 1e-5 && (off.centre[1] + 3.0).abs() < 1e-5, "{off:?}");
    }

    #[test]
    fn a_spline_is_sampled_through_its_points() {
        let line = sampled(&[[0.0, 5.0, 0.0], [8.0, 5.0, 0.0], [8.0, 5.0, 8.0]], false);
        assert_eq!(line.len(), 2 * SPLINE_STEPS as usize + 1);
        assert_eq!(line[0], [0.0, 0.0]);
        assert_eq!(line[SPLINE_STEPS as usize], [8.0, 0.0], "through each point");
        assert_eq!(line[line.len() - 1], [8.0, 8.0]);
        let ring = sampled(&[[0.0; 3], [4.0, 0.0, 0.0], [4.0, 0.0, 4.0]], true);
        assert_eq!(ring.len(), 3 * SPLINE_STEPS as usize + 1);
        assert_eq!(ring[ring.len() - 1], [0.0, 0.0], "a closed one comes back round");
    }
}
