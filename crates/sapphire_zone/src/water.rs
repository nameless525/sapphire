//! A zone's water (M6 W2): lakes, rivers and the sea as data, and the query the simulation asks of them.
//!
//! A body is a flat surface at rest over a shape on the ground plane. A **lake** (a pond is a small one) is an
//! outline and a height; a **river** is a polyline of its own, each point with a height, a width and a speed,
//! running from its first point to its last, its surface falling along it; the **sea** is a height everywhere in
//! the zone, at most one a zone, where the ground decides the coast. Waves are presentation, an offset over
//! the rest surface the simulation never sees; so is the current, which a sample reports and nothing moves by.
//!
//! ```ron
//! water: [
//!     (name: "mill_pond", shape: Lake(outline: [(4.0, 4.0), (20.0, 6.0), (18.0, 22.0), (2.0, 18.0)], height: 1.2)),
//!     // Straight between its points: a bend is more points. Its surface falls from 3.0 to 1.2 m.
//!     (name: "mill_race", shape: River(points: [
//!         (at: (-30.0, 3.0, 10.0), width: 4.0, speed: 1.5),
//!         (at: (-10.0, 2.0, 12.0), width: 5.0, speed: 2.0),
//!         (at: (4.0, 1.2, 12.0), width: 6.0, speed: 0.5),
//!     ]), swim: false),
//!     (name: "bay", shape: Ocean(height: 0.0), material: Some("…"), probe: Some("harbour"), shore_margin: 4.0),
//!     (name: "ice_pond", shape: Lake(outline: […], height: 9.0), surface: Frozen),
//! ],
//! ```
//!
//! **Halves:** what the simulation reads (each body's shape, its surface's state and whether it swims) cooks into
//! [`ZoneSimulation::water`](crate::ZoneSimulation::water) and the digest; what only drawing reads (its material,
//! its reflection probe, the margin its surface runs onto land) into
//! [`ZonePresentation::water`](crate::ZonePresentation::water). A river's points are its own, never one of the
//! zone's splines, which are camera rails outside the digest. A zone with no water writes no water section in
//! its digest, so its digest is the one it had before water existed.
//!
//! **Drawn** (M6 W3a): each lake and river cooks with its zone into a mesh and an info texture of its own
//! ([`drawn`]), and one that names no probe has one placed for it ([`WaterBody::placed_probe`]).
//!
//! **The query:** [`WaterVolumes::at`] says whether a point is in water and which, with the surface over it, the
//! current there and how deep the water stands over the floor. The floor is the first thing a ray straight down
//! from the surface meets, which the caller's [`Floor`] casts (`sapphire_physics`' `PhysicsWorld::floor_below`
//! against the zone's statics), so the bed is the colliders' and needs no data of its own: a point under that
//! floor, as a sea's column under a hill is, isn't in water. The query is plain arithmetic over the cooked
//! values, with no transcendental and no allocation, so a fixed step may ask it.

use bevy_reflect::Reflect;
use sapphire_serialize::AssetId;
use serde::{Deserialize, Serialize};

use crate::document::{check_name, check_position};
use crate::grid::MAX_COORDINATE;
use crate::reflections::{ParallaxBox, ReflectionProbe};

pub mod drawn;

pub use drawn::{ArchivedWaterMesh, InfoTexture, WATER_MESH_EXTENSION, WaterMesh, WaterVertex, water_part};

/// The most water bodies a zone may hold (spec §3.4).
pub const MAX_BODIES: usize = 16;
/// The most points a lake's outline may hold.
pub const MAX_OUTLINE_POINTS: usize = 256;
/// The most points a river may hold.
pub const MAX_RIVER_POINTS: usize = 256;
/// The narrowest a river may run, in metres.
pub const MIN_RIVER_WIDTH: f32 = 0.1;
/// The widest a river may run, in metres.
pub const MAX_RIVER_WIDTH: f32 = 1024.0;
/// The fastest a river's current may run, in metres a second.
pub const MAX_RIVER_SPEED: f32 = 32.0;
/// The shortest a river's stretch may be across the ground, in metres: a fall is a steep stretch, never a
/// vertical one, so its current has a way to point.
pub const MIN_STRETCH: f32 = 0.01;
/// The smallest area a lake's outline may enclose, in square metres.
pub const MIN_LAKE_AREA: f32 = 0.01;
/// The widest margin a body's surface may run onto land, in metres.
pub const MAX_SHORE_MARGIN: f32 = 64.0;
/// The margin a body's surface runs onto land when its document doesn't say.
pub const DEFAULT_SHORE_MARGIN: f32 = 1.0;
/// How far under a body's surface [`WaterVolumes::at`] looks for its floor, in metres. Water deeper than this
/// reports no depth.
pub const FLOOR_REACH: f32 = 256.0;

/// What kind of water a body is.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[type_path = "sapphire::zone"]
#[rkyv(derive(Debug, PartialEq, Eq), compare(PartialEq))]
pub enum WaterKind {
    Lake,
    River,
    Ocean,
}

/// What state a body's surface is in.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[type_path = "sapphire::zone"]
#[rkyv(derive(Debug, PartialEq, Eq), compare(PartialEq))]
pub enum WaterSurface {
    #[default]
    Liquid,
    /// Ice over the water. Whether anyone stands on it is a collider's business: the query still reports the
    /// water under it, frozen.
    Frozen,
}

/// One water body, as a zone document places it (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct WaterBody {
    /// Unique among the zone's water: its identity, whose hash a sample's body is cooked in the order of.
    pub name: String,
    pub shape: WaterShape,
    #[serde(default, skip_serializing_if = "is_liquid")]
    pub surface: WaterSurface,
    /// The material its surface draws with, by its sidecar's ID: the water family's look. With none, the
    /// renderer's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<AssetId>,
    /// Whether a character swims in it rather than wading through.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub swim: bool,
    /// The reflection probe its surface reflects, by its name in the zone's `reflections`. With none, the
    /// cook places one over its surface ([`WaterBody::placed_probe`]), but for the sea's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<String>,
    /// How far its drawn surface runs onto land past its shape, in metres, for the shore's wet film.
    #[serde(default = "default_shore_margin")]
    pub shore_margin: f32,
}

/// The ground plane's shape a body's surface lies over, and its height.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
#[rkyv(derive(Debug))]
pub enum WaterShape {
    /// Inside a simple polygon of `(x, z)` points, in either winding, at `height` metres.
    Lake { outline: Vec<[f32; 2]>, height: f32 },
    /// Along straight stretches between its points, from the first to the last.
    River { points: Vec<RiverPoint> },
    /// Everywhere in the zone at `height` metres, under which the ground is land.
    Ocean { height: f32 },
}

/// A point of a river's polyline. Between two, each value runs straight from one to the other.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    Reflect,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
#[rkyv(derive(Debug))]
pub struct RiverPoint {
    /// Its surface's middle, in metres in the zone's space.
    pub at: [f32; 3],
    /// From bank to bank, in metres.
    pub width: f32,
    /// How fast its current runs downstream, in metres a second.
    #[serde(default)]
    pub speed: f32,
}

fn is_liquid(surface: &WaterSurface) -> bool {
    *surface == WaterSurface::Liquid
}

fn yes() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde's `skip_serializing_if` passes a reference")]
fn is_true(value: &bool) -> bool {
    *value
}

fn default_shore_margin() -> f32 {
    DEFAULT_SHORE_MARGIN
}

impl WaterShape {
    #[must_use]
    pub fn kind(&self) -> WaterKind {
        match self {
            Self::Lake { .. } => WaterKind::Lake,
            Self::River { .. } => WaterKind::River,
            Self::Ocean { .. } => WaterKind::Ocean,
        }
    }

    /// Whether it's a shape the query can answer over, or what's wrong with it, by the field at fault.
    pub(crate) fn check(&self) -> Result<(), (String, String)> {
        match self {
            Self::Lake { outline, height } => {
                check_height(*height).map_err(|problem| ("shape.height".to_owned(), problem))?;
                check_outline(outline.len(), MAX_OUTLINE_POINTS, |index| outline[index])
                    .map_err(|problem| ("shape.outline".to_owned(), problem))
            }
            Self::River { points } => check_river(points.len(), |index| points[index]),
            Self::Ocean { height } => {
                check_height(*height).map_err(|problem| ("shape.height".to_owned(), problem))
            }
        }
    }

    /// The rest surface over `(x, z)` and the current there, if it covers that column: what
    /// [`ArchivedCookedWaterVolume::column`] answers of the same shape cooked, by the same arithmetic.
    #[must_use]
    pub fn column(&self, x: f32, z: f32) -> Option<(f32, [f32; 2])> {
        let (min, max) = self.bounds();
        if !(min[0] <= x && x <= max[0] && min[1] <= z && z <= max[1]) {
            return None;
        }
        match self {
            Self::Lake { outline, height } => {
                inside(outline.len(), |index| outline[index], x, z).then_some((*height, [0.0; 2]))
            }
            Self::River { points } => river_column(points.len(), |index| points[index], x, z),
            Self::Ocean { height } => Some((*height, [0.0; 2])),
        }
    }

    /// Its rest surface's lowest and highest heights, in metres: a lake's and the sea's one height, a river's
    /// over its points.
    #[must_use]
    pub fn heights(&self) -> (f32, f32) {
        match self {
            Self::Lake { height, .. } | Self::Ocean { height } => (*height, *height),
            Self::River { points } => {
                points.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), point| {
                    (low.min(point.at[1]), high.max(point.at[1]))
                })
            }
        }
    }

    /// Around it on the ground plane: `[min x, min z]` and `[max x, max z]`, a river's banks included, the sea's
    /// the whole zone. Only comparisons and a halving, so it's the same on every machine.
    #[must_use]
    pub fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        match self {
            Self::Lake { outline, .. } => bounds_of(outline.iter().map(|&point| (point, 0.0))),
            Self::River { points } => {
                bounds_of(points.iter().map(|point| ([point.at[0], point.at[2]], point.width * 0.5)))
            }
            Self::Ocean { .. } => ([-MAX_COORDINATE; 2], [MAX_COORDINATE; 2]),
        }
    }
}

/// Around points, each grown by its own reach.
fn bounds_of(points: impl Iterator<Item = ([f32; 2], f32)>) -> ([f32; 2], [f32; 2]) {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for ([x, z], reach) in points {
        for (axis, value) in [x, z].into_iter().enumerate() {
            if value - reach < min[axis] {
                min[axis] = value - reach;
            }
            if value + reach > max[axis] {
                max[axis] = value + reach;
            }
        }
    }
    (min, max)
}

impl WaterBody {
    /// Whether it's a body a cook can take, or what's wrong with it, by the field at fault. Its probe's name
    /// is the document's to check, against its reflections.
    pub(crate) fn check(&self) -> Result<(), (String, String)> {
        check_name(&self.name).map_err(|problem| ("name".to_owned(), problem))?;
        self.shape.check()?;
        check_shore_margin(self.shore_margin).map_err(|problem| ("shore_margin".to_owned(), problem))?;
        if let Some(probe) = &self.probe {
            check_name(probe).map_err(|problem| ("probe".to_owned(), problem))?;
        }
        Ok(())
    }
}

/// How high over a body's surface the probe the cook places for it stands, in metres.
pub const PROBE_LIFT: f32 = 2.0;
/// How far past a body's drawn surface, on the ground plane, the box of the probe the cook places for it
/// reaches, in metres.
pub const PROBE_REACH: f32 = 8.0;
/// How far under a body's lowest surface the box of the probe the cook places for it reaches, in metres.
pub const PROBE_DEPTH: f32 = 4.0;
/// How far over a body's highest surface the box of the probe the cook places for it reaches, in metres.
pub const PROBE_HEIGHT: f32 = 16.0;

impl WaterBody {
    /// The reflection probe the cook places for it (M6 W3a), if it names none and isn't the sea, under its own
    /// name, which the zone's `reflections` then mustn't use: [`PROBE_LIFT`] over its surface, a lake's over
    /// the middle of its outline's bounds (or, where that's land, of the widest span of water through it) and a
    /// river's over the point halfway along it; its box its drawn
    /// surface's bounds (its shape's grown by its `shore_margin`) grown by [`PROBE_REACH`], from
    /// [`PROBE_DEPTH`] under its lowest surface to [`PROBE_HEIGHT`] over its highest. Halvings, sums and a
    /// river's stretches' square roots, each correctly rounded, so it's the same on every machine. The sea
    /// reflects the sky until it has a grid of its own (H1b).
    #[must_use]
    pub fn placed_probe(&self) -> Option<ReflectionProbe> {
        if self.probe.is_some() {
            return None;
        }
        let ([x, z], y) = match &self.shape {
            WaterShape::Lake { outline, height } => (lake_middle(outline), *height),
            WaterShape::River { points } => river_halfway(points),
            WaterShape::Ocean { .. } => return None,
        };
        let (min, max) = self.shape.bounds();
        let (low, high) = self.shape.heights();
        let reach = self.shore_margin + PROBE_REACH;
        Some(ReflectionProbe {
            name: self.name.clone(),
            position: [x, y + PROBE_LIFT, z],
            parallax: ParallaxBox {
                min: [min[0] - reach, low - PROBE_DEPTH, min[1] - reach],
                max: [max[0] + reach, high + PROBE_HEIGHT, max[1] + reach],
            },
        })
    }
}

/// Where over a lake its probe stands: the middle of its outline's bounds if that's water, or else, as for an L
/// or a crescent, the middle of the widest span of water along the line through it.
fn lake_middle(outline: &[[f32; 2]]) -> [f32; 2] {
    let (min, max) = bounds_of(outline.iter().map(|&point| (point, 0.0)));
    let [x, z] = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
    let point = |index: usize| outline[index];
    if inside(outline.len(), point, x, z) {
        return [x, z];
    }
    let mut crossings: Vec<f32> = Vec::new();
    let mut previous = outline[outline.len() - 1];
    for &next in outline {
        if (previous[1] > z) != (next[1] > z) {
            crossings
                .push(previous[0] + (z - previous[1]) * (next[0] - previous[0]) / (next[1] - previous[1]));
        }
        previous = next;
    }
    crossings.sort_by(f32::total_cmp);
    // Even-odd, as `inside` counts: water runs between the first and second crossings, the third and fourth.
    let widest = crossings.as_chunks::<2>().0.iter().fold(None::<[f32; 2]>, |widest, &span| match widest {
        Some(best) if best[1] - best[0] >= span[1] - span[0] => Some(best),
        _ => Some(span),
    });
    widest.map_or([x, z], |[from, to]| [(from + to) * 0.5, z])
}

/// The middle `(x, z)` of a river's centreline, by its length across the ground, and its surface there.
fn river_halfway(points: &[RiverPoint]) -> ([f32; 2], f32) {
    let length = |from: &RiverPoint, to: &RiverPoint| {
        let (dx, dz) = (to.at[0] - from.at[0], to.at[2] - from.at[2]);
        (dx * dx + dz * dz).sqrt()
    };
    let total: f32 = points.windows(2).map(|pair| length(&pair[0], &pair[1])).sum();
    let mut left = total * 0.5;
    for pair in points.windows(2) {
        let stretch = length(&pair[0], &pair[1]);
        if left <= stretch {
            // A stretch is at least `MIN_STRETCH` long, so the share is a number.
            let t = left / stretch;
            let along = |axis: usize| pair[0].at[axis] + (pair[1].at[axis] - pair[0].at[axis]) * t;
            return ([along(0), along(2)], along(1));
        }
        left -= stretch;
    }
    let last = points[points.len() - 1].at;
    ([last[0], last[2]], last[1])
}

/// Whether `points` are an outline a lake may have, or what's wrong with them: a simple polygon of some area,
/// of 3 to twice [`MAX_OUTLINE_POINTS`] points (a grown corner may be two) within the zone's reach. The cook
/// holds a lake's outline grown by its margin to it, as the document's check holds the outline.
pub fn check_outline_points(points: &[[f32; 2]]) -> Result<(), String> {
    check_outline(points.len(), 2 * MAX_OUTLINE_POINTS, |index| points[index])
}

fn check_height(height: f32) -> Result<(), String> {
    check_position([0.0, height, 0.0])
        .map_err(|_| format!("{height} isn't a finite height within {MAX_COORDINATE} m"))
}

pub(crate) fn check_shore_margin(margin: f32) -> Result<(), String> {
    if margin.is_finite() && (0.0..=MAX_SHORE_MARGIN).contains(&margin) {
        Ok(())
    } else {
        Err(format!("{margin} isn't from 0 to {MAX_SHORE_MARGIN} m"))
    }
}

/// Whether an outline of `count` points is a simple polygon of some area, or what's wrong with it: no two of
/// its edges meet but neighbours at their shared corner.
fn check_outline(count: usize, most: usize, point: impl Fn(usize) -> [f32; 2]) -> Result<(), String> {
    if !(3..=most).contains(&count) {
        return Err(format!("{count} points isn't from 3 to {most}"));
    }
    for index in 0..count {
        let [x, z] = point(index);
        check_position([x, 0.0, z]).map_err(|problem| format!("point {index}: {problem}"))?;
    }
    let edge = |index: usize| (point(index), point((index + 1) % count));
    let mut twice_area = 0.0f32;
    for first in 0..count {
        let (a, b) = edge(first);
        if a == b {
            return Err(format!("points {first} and {} are one point", (first + 1) % count));
        }
        twice_area += a[0] * b[1] - b[0] * a[1];
        // Every later edge but the two that share a corner with this one.
        for second in first + 2..count {
            if first == 0 && second == count - 1 {
                continue;
            }
            let (c, d) = edge(second);
            if segments_meet(a, b, c, d) {
                return Err(format!("its edges from point {first} and from point {second} cross or touch"));
            }
        }
    }
    // Every point is finite and within the zone's reach, so the area is a number.
    if twice_area.abs() * 0.5 < MIN_LAKE_AREA {
        return Err(format!("it encloses less than {MIN_LAKE_AREA} m²"));
    }
    Ok(())
}

/// Whether a river of `count` points is one the query can follow, or what's wrong with it, by the field at
/// fault.
fn check_river(count: usize, point: impl Fn(usize) -> RiverPoint) -> Result<(), (String, String)> {
    if !(2..=MAX_RIVER_POINTS).contains(&count) {
        return Err((
            "shape.points".to_owned(),
            format!("{count} points isn't from 2 to {MAX_RIVER_POINTS}"),
        ));
    }
    for index in 0..count {
        let field = |name: &str| format!("shape.points[{index}].{name}");
        let RiverPoint { at, width, speed } = point(index);
        check_position(at).map_err(|problem| (field("at"), problem))?;
        if !(width.is_finite() && (MIN_RIVER_WIDTH..=MAX_RIVER_WIDTH).contains(&width)) {
            return Err((
                field("width"),
                format!("{width} isn't from {MIN_RIVER_WIDTH} to {MAX_RIVER_WIDTH} m"),
            ));
        }
        if !(speed.is_finite() && (0.0..=MAX_RIVER_SPEED).contains(&speed)) {
            return Err((field("speed"), format!("{speed} isn't from 0 to {MAX_RIVER_SPEED} m/s")));
        }
        if index > 0 {
            let before = point(index - 1).at;
            let (dx, dz) = (at[0] - before[0], at[2] - before[2]);
            if dx * dx + dz * dz < MIN_STRETCH * MIN_STRETCH {
                let problem =
                    format!("it's less than {MIN_STRETCH} m across the ground from the point before");
                return Err((field("at"), problem));
            }
        }
    }
    Ok(())
}

/// Twice the signed area of the triangle `a`, `b`, `c`: its sign says which side of `a`→`b` `c` lies on.
fn orient(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Whether `c`, on the line through `a` and `b`, lies between them.
fn within(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    (0..2).all(|axis| a[axis].min(b[axis]) <= c[axis] && c[axis] <= a[axis].max(b[axis]))
}

/// Whether segments `a`→`b` and `c`→`d` share a point.
fn segments_meet(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
    let (abc, abd) = (orient(a, b, c), orient(a, b, d));
    let (cda, cdb) = (orient(c, d, a), orient(c, d, b));
    if ((abc > 0.0 && abd < 0.0) || (abc < 0.0 && abd > 0.0))
        && ((cda > 0.0 && cdb < 0.0) || (cda < 0.0 && cdb > 0.0))
    {
        return true;
    }
    (abc == 0.0 && within(a, b, c))
        || (abd == 0.0 && within(a, b, d))
        || (cda == 0.0 && within(c, d, a))
        || (cdb == 0.0 && within(c, d, b))
}

/// A body as the simulation reads it: its shape on the ground and the bounds around it, in
/// [`ZoneSimulation::water`](crate::ZoneSimulation::water), by ascending ID.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedWaterVolume {
    /// The name's hash ([`Name`](sapphire_core::Name)).
    pub id: u64,
    pub name: String,
    pub shape: WaterShape,
    pub surface: WaterSurface,
    pub swim: bool,
    /// Around the shape on the ground plane, `[x, z]` (a river's grown by half its width; an ocean's the
    /// zone's whole reach), which a reader checks against the shape.
    pub bounds_min: [f32; 2],
    pub bounds_max: [f32; 2],
}

/// A body as only drawing reads it, in [`ZonePresentation::water`](crate::ZonePresentation::water), in the
/// simulation's order.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedWaterSurface {
    /// Its volume's ID.
    pub id: u64,
    /// The material its surface draws with.
    pub material: Option<AssetId>,
    /// The probe it reflects: its place in the zone's reflection probes, which keep the document's order.
    pub probe: Option<u32>,
    /// How far its drawn surface runs onto land, in metres.
    pub shore_margin: f32,
    /// Its surface as drawn (`.swater`, M6 W3a): its mesh and its info texture, a sub-asset of the zone
    /// ([`drawn`]). None for the sea, whose grid isn't cooked (H1b), and in a zone cooked before version 14.
    pub mesh: Option<AssetId>,
}

/// What [`WaterVolumes::at`] found at a point in water.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterSample {
    /// The rest surface over the point, in metres: flat, waves being presentation's.
    pub surface_y: f32,
    /// The current there, `[x, z]` in metres a second: downstream along a river, still elsewhere.
    pub flow: [f32; 2],
    /// Which body: its place in the zone's water ([`WaterVolumes::body`]).
    pub body: u16,
    pub kind: WaterKind,
    pub surface: WaterSurface,
    /// Whether a character swims in it.
    pub swim: bool,
    /// How deep the water stands over the floor under the point, in metres; `None` when the floor is further
    /// down than [`FLOOR_REACH`], or there is none.
    pub depth: Option<f32>,
}

/// Where the floor is under a body's surface: the height of the first thing straight down from `(x, from_y, z)`
/// within `reach` metres, if anything. A closure taking those four is one; the simulation's casts a ray against
/// the zone's static colliders (`sapphire_physics`' `PhysicsWorld::floor_below`).
pub trait Floor {
    fn below(&self, x: f32, from_y: f32, z: f32, reach: f32) -> Option<f32>;
}

impl<F: Fn(f32, f32, f32, f32) -> Option<f32>> Floor for F {
    fn below(&self, x: f32, from_y: f32, z: f32, reach: f32) -> Option<f32> {
        self(x, from_y, z, reach)
    }
}

/// A zone's water, asked where the water is (see the [module docs](self)). Borrows the cooked zone and a
/// [`Floor`]; building one and asking it allocate nothing.
#[derive(Clone, Copy)]
pub struct WaterVolumes<'a, F> {
    bodies: &'a [ArchivedCookedWaterVolume],
    floor: F,
}

impl<'a, F: Floor> WaterVolumes<'a, F> {
    /// Over a cooked zone's simulation half, which [`Zone::read`](crate::Zone::read) checked.
    pub fn new(simulation: &'a crate::ArchivedZoneSimulation, floor: F) -> Self {
        Self { bodies: simulation.water.as_slice(), floor }
    }

    /// Whether the zone has any water.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    /// The body a sample names.
    #[must_use]
    pub fn body(&self, body: u16) -> Option<&'a ArchivedCookedWaterVolume> {
        self.bodies.get(usize::from(body))
    }

    /// The water at a point, if it's in any: at or under a body's surface over its shape, and not under the
    /// floor a ray down from that surface meets. Where bodies overlap, the one whose surface is highest over the
    /// point answers, the first in the zone's order on a tie. One floor ray at most, whatever the bodies.
    #[must_use]
    pub fn at(&self, x: f32, y: f32, z: f32) -> Option<WaterSample> {
        let mut best: Option<(usize, f32, [f32; 2])> = None;
        for (index, body) in self.bodies.iter().enumerate() {
            let Some((surface_y, flow)) = body.column(x, z) else {
                continue;
            };
            if y <= surface_y && best.is_none_or(|(_, highest, _)| surface_y > highest) {
                best = Some((index, surface_y, flow));
            }
        }
        let (index, surface_y, flow) = best?;
        let floor = self.floor.below(x, surface_y, z, FLOOR_REACH);
        if floor.is_some_and(|floor| y < floor) {
            return None;
        }
        let body = &self.bodies[index];
        Some(WaterSample {
            surface_y,
            flow,
            body: index as u16,
            kind: body.kind(),
            surface: body.surface(),
            swim: body.swim,
            depth: floor.map(|floor| surface_y - floor),
        })
    }
}

impl ArchivedWaterShape {
    #[must_use]
    pub fn kind(&self) -> WaterKind {
        match self {
            Self::Lake { .. } => WaterKind::Lake,
            Self::River { .. } => WaterKind::River,
            Self::Ocean { .. } => WaterKind::Ocean,
        }
    }
}

impl ArchivedWaterSurface {
    #[must_use]
    pub fn to_native(&self) -> WaterSurface {
        match self {
            Self::Liquid => WaterSurface::Liquid,
            Self::Frozen => WaterSurface::Frozen,
        }
    }
}

impl ArchivedRiverPoint {
    #[must_use]
    pub fn to_native(&self) -> RiverPoint {
        RiverPoint {
            at: self.at.each_ref().map(|value| value.to_native()),
            width: self.width.to_native(),
            speed: self.speed.to_native(),
        }
    }
}

impl ArchivedCookedWaterVolume {
    #[must_use]
    pub fn kind(&self) -> WaterKind {
        self.shape.kind()
    }

    #[must_use]
    pub fn surface(&self) -> WaterSurface {
        self.surface.to_native()
    }

    /// Around its shape on the ground plane, `[x, z]`.
    #[must_use]
    pub fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        let native = |values: &rkyv::Archived<[f32; 2]>| values.each_ref().map(|value| value.to_native());
        (native(&self.bounds_min), native(&self.bounds_max))
    }

    /// The rest surface over `(x, z)` and the current there, if its shape covers that column.
    #[must_use]
    pub fn column(&self, x: f32, z: f32) -> Option<(f32, [f32; 2])> {
        let (min, max) = self.bounds();
        if !(min[0] <= x && x <= max[0] && min[1] <= z && z <= max[1]) {
            return None;
        }
        match &self.shape {
            ArchivedWaterShape::Lake { outline, height } => {
                let point = |index: usize| outline[index].each_ref().map(|value| value.to_native());
                inside(outline.len(), point, x, z).then(|| (height.to_native(), [0.0; 2]))
            }
            ArchivedWaterShape::River { points } => {
                river_column(points.len(), |index| points[index].to_native(), x, z)
            }
            ArchivedWaterShape::Ocean { height } => Some((height.to_native(), [0.0; 2])),
        }
    }
}

/// Whether `(x, z)` lies inside a polygon, by the crossings of a ray toward +X: an odd count is inside. A point
/// on an edge may count either way, the same way every time.
fn inside(count: usize, point: impl Fn(usize) -> [f32; 2], x: f32, z: f32) -> bool {
    let mut odd = false;
    let mut previous = point(count - 1);
    for index in 0..count {
        let next = point(index);
        if (previous[1] > z) != (next[1] > z) {
            let crossing =
                previous[0] + (z - previous[1]) * (next[0] - previous[0]) / (next[1] - previous[1]);
            if x < crossing {
                odd = !odd;
            }
        }
        previous = next;
    }
    odd
}

/// The surface and current over `(x, z)` from the river's stretch nearest it whose banks hold it, the first such
/// on a tie. A stretch reaches past its ends by half its width there, so its stretches join with no gap.
fn river_column(
    count: usize,
    point: impl Fn(usize) -> RiverPoint,
    x: f32,
    z: f32,
) -> Option<(f32, [f32; 2])> {
    let mut best: Option<(f32, f32, [f32; 2])> = None;
    let mut from = point(0);
    for index in 1..count {
        let to = point(index);
        let (ax, az) = (from.at[0], from.at[2]);
        let (dx, dz) = (to.at[0] - ax, to.at[2] - az);
        let length_squared = dx * dx + dz * dz;
        let along = ((x - ax) * dx + (z - az) * dz) / length_squared;
        // A stretch is at least `MIN_STRETCH` long, so `along` is a number.
        let t = along.clamp(0.0, 1.0);
        let (ox, oz) = (x - (ax + dx * t), z - (az + dz * t));
        let off_squared = ox * ox + oz * oz;
        let half_width = (from.width + (to.width - from.width) * t) * 0.5;
        if off_squared <= half_width * half_width && best.is_none_or(|(nearest, _, _)| off_squared < nearest)
        {
            let surface_y = from.at[1] + (to.at[1] - from.at[1]) * t;
            let speed = from.speed + (to.speed - from.speed) * t;
            let per_metre = speed / length_squared.sqrt();
            best = Some((off_squared, surface_y, [dx * per_metre, dz * per_metre]));
        }
        from = to;
    }
    best.map(|(_, surface_y, flow)| (surface_y, flow))
}

#[cfg(test)]
mod tests;
