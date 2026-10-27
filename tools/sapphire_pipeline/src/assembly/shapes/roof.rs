//! `#roof`: tiled roofs in the East Asian manner, built over a building's walls.
//!
//! A roof covers walls `size` meters wide (X, along its ridge) and deep (Z), sitting on their
//! tops: its origin is the middle of the walls' top. Its eaves reach `overhang` past the walls
//! all round and its slopes climb `rise` from the eaves to the ridge, steeper toward the ridge
//! than at the eaves (`curve`: the slope's height is `(1 − curve) s + curve s²` of the rise at
//! `s` of the way up). Near each corner the eaves sweep up (`sweep` meters at the corner,
//! fading over `sweep_reach` along the eave and to nothing at the walls), the silhouette's
//! signature. The kinds:
//!
//! - **Hip** (庑殿): four slopes; the end ones triangles under hips at 45°. On a square, a
//!   pyramid (攒尖) under a finial.
//! - **HipGable(at)** (歇山): a hip up to `at` of the way up its end slopes, then a gable set
//!   back over them, its upper slopes reaching `barge` past the gable face.
//! - **Gable** (悬山): two slopes running `barge` past gable ends that fill the walls' ends.
//! - **Skirt(run)** (腰檐): a hip's lower `run` meters only, round the foot of a storey standing
//!   on the walls: its walls rise from the skirt's top edge, `size + 2 (overhang − run)` across.
//!
//! The surface is tiled: rows of cover tiles running down each slope over flat channels, modeled
//! (`tiles`: their spacing and height), the rows' ends showing along the eaves as a scalloped
//! edge. Each slope is a grid over (along the eave, up the slope) cut to its outline in that
//! plane, every corner placed by the surface's own function, so curves stay true at cut edges.
//! Under the eaves a timber soffit runs back to the walls, a fascia board closes the eaves' edge,
//! and a corner beam runs under each hip out past the eaves. Ridges sit on top: the main ridge
//! (its top rising toward its ends, an ornament on each), hip ridges ending in an upturned tip,
//! verge ridges along gables, and a ridge along a hip-and-gable's gable foot or a skirt's top.
//! The seams where slopes meet (hips, the ridge) are left to those ridges and beams to cover.

use glam::{Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use super::polygon::{self, HalfPlane};
use super::{
    Detail, Frame, Mesh, PLASTER_REPEAT, Part, TILE_COURSES, TILE_PERIODS, check, finite, smoothstep,
};
use crate::assembly::ShapeUv;

/// A tiled roof (`#roof`); see the module docs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Roof {
    /// The walls it covers: width along its ridge (X) and depth (Z), meters.
    pub size: [f32; 2],
    pub kind: RoofKind,
    /// How far the eaves reach past the walls.
    pub overhang: f32,
    /// How far a gable's slopes reach past its face.
    pub barge: f32,
    /// From the eaves' edge to the ridge (to the top edge, for a skirt).
    pub rise: f32,
    /// How much steeper the slope grows toward the ridge, 0 (straight) to 1.
    pub curve: f32,
    /// How far the eaves sweep up at the corners, and how far along the eaves the sweep reaches.
    pub sweep: f32,
    pub sweep_reach: f32,
    /// From the tiles' top to the soffit, measured upright.
    pub thickness: f32,
    pub tiles: Tiles,
    pub ridge: Ridge,
}

impl Default for Roof {
    fn default() -> Self {
        Self {
            size: [8.0, 6.0],
            kind: RoofKind::Hip,
            overhang: 0.9,
            barge: 0.45,
            rise: 2.4,
            curve: 0.4,
            sweep: 0.4,
            sweep_reach: 2.2,
            thickness: 0.26,
            tiles: Tiles::default(),
            ridge: Ridge::default(),
        }
    }
}

/// The roof's form (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum RoofKind {
    #[default]
    Hip,
    /// Where the gable starts, as a fraction of the slope's run from the eaves (0..1).
    HipGable(f32),
    Gable,
    /// How far the skirt runs in from its eaves, meters.
    Skirt(f32),
}

/// Rows of tiles: their spacing along the eaves and how high the cover tiles stand over the
/// channels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tiles {
    pub period: f32,
    pub depth: f32,
}

impl Default for Tiles {
    fn default() -> Self {
        Self { period: 0.36, depth: 0.055 }
    }
}

/// The main ridge's height and width (the others are scaled from it), and its end ornaments'
/// height (none at 0).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ridge {
    pub height: f32,
    pub width: f32,
    pub ends: f32,
}

impl Default for Ridge {
    fn default() -> Self {
        Self { height: 0.3, width: 0.26, ends: 0.6 }
    }
}

/// A row of tiles' profile across one period: a flat channel, then a cover tile peaking at
/// `CREST`. Each period has corners at these fractions.
const CHANNEL: f32 = 0.4;
const CREST: f32 = 0.7;
/// Occlusion in the tiles' channels (1 on the crests).
const CHANNEL_SHADE: f32 = 0.8;
/// Rows of the slope's grid: about this many meters of run apart above the walls' line.
const ROW_RUN: f32 = 1.1;
/// The eaves band (from the edge to the walls' line) takes this many rows.
const EAVE_ROWS: usize = 3;
/// How far ridges sink into the tiles under them.
const EMBED: f32 = 0.06;
/// Samples along a curved ridge or beam.
const PATH_SAMPLES: usize = 6;
/// How far ridges run past the verges they end at.
const RIDGE_PAST: f32 = 0.04;
/// The fascia board's share of the roof's thickness under the tile ends.
const BOARD: f32 = 0.42;

/// One slope, in its own frame: X along its eave (to the right seen from outside), Z out
/// through its eave, rotated into the roof by `turn`.
struct Slope {
    turn: Quat,
    /// The eave line's distance out from the roof's middle, and half its length.
    eave: f32,
    half: f32,
    /// Whether the eave's corners are hips (they draw in as the slope climbs) or verges.
    hips: bool,
    /// The tiled surface: convex regions of (along, up) = (c, s).
    parts: Vec<Region>,
    /// Past this far along the eave either way, above the walls' line, its underside shows.
    open_beyond: f32,
    /// Open edges, which take a fascia down to the soffit.
    edges: Vec<Edge>,
}

#[derive(Clone)]
struct Region {
    planes: Vec<HalfPlane>,
    c: [f32; 2],
    s: [f32; 2],
}

impl Region {
    /// The box `c` × `s`, cut by `planes`.
    fn new(c: [f32; 2], s: [f32; 2], mut planes: Vec<HalfPlane>) -> Self {
        planes.extend([
            HalfPlane::new(Vec2::X, c[1]),
            HalfPlane::new(-Vec2::X, -c[0]),
            HalfPlane::new(Vec2::Y, s[1]),
            HalfPlane::new(-Vec2::Y, -s[0]),
        ]);
        Self { planes, c, s }
    }

    /// The same region, cut further.
    fn and(&self, plane: HalfPlane) -> Self {
        let mut planes = self.planes.clone();
        planes.push(plane);
        Self { planes, ..self.clone() }
    }
}

/// An open edge of a slope's tiles.
#[derive(Clone, Copy)]
enum Edge {
    /// At height `s`, from `c[0]` to `c[1]` along the eave, facing down the slope.
    Along { s: f32, c: [f32; 2] },
    /// At `c` along the eave, from `s[0]` to `s[1]` up the slope, facing out along the eave.
    Across { c: f32, s: [f32; 2] },
}

/// The roof's numbers, worked out once.
struct Plan<'a> {
    roof: &'a Roof,
    detail: Detail,
    origin: Vec3,
    /// Half the eaves' extent along X (to the verges for a gable) and Z.
    half: Vec2,
    /// The horizontal run of each slope from its eave to its ridge (or a skirt's top edge).
    run: f32,
    /// The walls' line, as a fraction of the run up from the eaves.
    wall: f32,
    /// The eaves' height at their edge, over the walls' top.
    eave_height: f32,
    /// The arc length up the slope at even steps of `s`, for texture coordinates.
    arc: Vec<f32>,
}

impl Roof {
    pub fn validate(&self) -> Result<(), String> {
        let [width, depth] = self.size;
        let numbers = [
            width,
            depth,
            self.overhang,
            self.barge,
            self.rise,
            self.curve,
            self.sweep,
            self.sweep_reach,
            self.thickness,
        ];
        check(
            finite(&numbers)
                && finite(&[
                    self.tiles.period,
                    self.tiles.depth,
                    self.ridge.height,
                    self.ridge.width,
                    self.ridge.ends,
                ]),
            || "a roof's numbers must be finite".to_owned(),
        )?;
        check(width > 0.0 && depth > 0.0 && self.rise > 0.0 && self.thickness > 0.0, || {
            format!("a roof's size {:?}, rise and thickness must be positive", self.size)
        })?;
        check(
            self.overhang > 0.0 && self.barge >= 0.0 && self.sweep >= 0.0 && self.sweep_reach > 0.0,
            || {
                "a roof's overhang and sweep reach must be positive, its barge and sweep not negative"
                    .to_owned()
            },
        )?;
        check((0.0..=1.0).contains(&self.curve), || {
            format!("a roof's curve {} must be in 0..=1", self.curve)
        })?;
        check(
            self.tiles.period >= 0.1 && self.tiles.depth >= 0.0 && self.tiles.depth < 0.5 * self.thickness,
            || "tiles need a period of 0.1 m or more and a depth under half the roof's thickness".to_owned(),
        )?;
        check(self.ridge.height >= 0.0 && self.ridge.width >= 0.0 && self.ridge.ends >= 0.0, || {
            "a ridge's sizes can't be negative".to_owned()
        })?;
        let plan = Plan::new(self, Vec3::ZERO, Detail::Full);
        match self.kind {
            RoofKind::Hip | RoofKind::HipGable(_) => {
                check(width >= depth, || {
                    format!(
                        "a hip roof's ridge runs along its width: {width} m must be at least its depth {depth} m"
                    )
                })?;
            }
            RoofKind::Gable => {}
            RoofKind::Skirt(run) => {
                check(
                    run.is_finite() && run > self.overhang + 0.1 && run < plan.half.min_element() - 0.2,
                    || {
                        format!(
                            "a skirt's run {run} m must reach past its overhang and leave a storey inside ({:?} m half-eaves)",
                            plan.half
                        )
                    },
                )?;
            }
        }
        check(plan.wall < 0.9, || {
            format!("the overhang {} m must be well under the slope's run of {} m", self.overhang, plan.run)
        })?;
        if let RoofKind::HipGable(at) = self.kind {
            check(at.is_finite() && at > plan.wall + 0.05 && at < 0.9, || {
                format!(
                    "a hip-and-gable's gable must start above the walls' line ({:.2}) and under 0.9: {at}",
                    plan.wall
                )
            })?;
            check(plan.half.x - at * plan.run > self.ridge.width, || {
                "a hip-and-gable's gable must stand clear of the middle".to_owned()
            })?;
        }
        Ok(())
    }

    /// Builds the roof with its origin at `origin`.
    pub(super) fn build(&self, mesh: &mut Mesh, origin: Vec3, detail: Detail) -> Result<(), String> {
        let plan = Plan::new(self, origin, detail);
        let slopes = plan.slopes();
        for slope in &slopes {
            plan.tiles(mesh, slope);
            plan.soffit(mesh, slope);
            for &edge in &slope.edges {
                plan.fascia(mesh, slope, edge);
            }
        }
        plan.gables(mesh)?;
        plan.ridges(mesh, &slopes)?;
        Ok(())
    }
}

impl<'a> Plan<'a> {
    fn new(roof: &'a Roof, origin: Vec3, detail: Detail) -> Self {
        let [width, depth] = roof.size;
        let o = roof.overhang;
        let half = match roof.kind {
            RoofKind::Gable => Vec2::new(0.5 * width + roof.barge, 0.5 * depth + o),
            _ => Vec2::new(0.5 * width + o, 0.5 * depth + o),
        };
        let run = match roof.kind {
            RoofKind::Skirt(run) => run,
            _ => half.y,
        };
        let wall = o / run;
        let mut plan = Self { roof, detail, origin, half, run, wall, eave_height: 0.0, arc: Vec::new() };
        // The soffit meets the walls' top at their line.
        plan.eave_height = roof.thickness - roof.rise * plan.profile(wall);
        let steps = 64;
        let mut length = 0.0;
        plan.arc.push(0.0);
        for k in 0..steps {
            let (s0, s1) = (k as f32 / steps as f32, (k + 1) as f32 / steps as f32);
            length += Vec2::new(plan.run * (s1 - s0), plan.height(s1) - plan.height(s0)).length();
            plan.arc.push(length);
        }
        plan
    }

    /// The slope's profile: its height at `s` of the way up, as a share of the rise.
    fn profile(&self, s: f32) -> f32 {
        let c = self.roof.curve;
        (1.0 - c) * s + c * s * s
    }

    /// The tiles' height at `s` (without sweep or tiles), over the walls' top.
    fn height(&self, s: f32) -> f32 {
        self.eave_height + self.roof.rise * self.profile(s)
    }

    /// The arc length up the slope from the eaves to `s`.
    fn arc_length(&self, s: f32) -> f32 {
        let steps = (self.arc.len() - 1) as f32;
        let x = (s.clamp(0.0, 1.0) * steps).min(steps - 1e-4);
        let k = x as usize;
        self.arc[k] + (self.arc[k + 1] - self.arc[k]) * (x - k as f32)
    }

    /// How far a hip-and-gable's gable face stands from the middle.
    fn gable_x(&self) -> f32 {
        match self.roof.kind {
            RoofKind::HipGable(at) => self.half.x - at * self.run,
            _ => 0.5 * self.roof.size[0],
        }
    }

    fn slopes(&self) -> Vec<Slope> {
        let Vec2 { x: hx, y: hz } = self.half;
        let run = self.run;
        let turn = |quarters: f32| Quat::from_rotation_y(quarters * std::f32::consts::FRAC_PI_2);
        // A slope whose eave is `half` long each way and whose ends are hips, up to `top`.
        let hipped = |half: f32, top: f32| {
            Region::new(
                [-half, half],
                [0.0, top],
                vec![HalfPlane::new(Vec2::new(1.0, run), half), HalfPlane::new(Vec2::new(-1.0, run), half)],
            )
        };
        let [width, depth] = self.roof.size;
        let mut slopes = Vec::new();
        match self.roof.kind {
            RoofKind::Hip | RoofKind::Skirt(_) => {
                for (quarter, eave, half, wall) in [(0.0, hz, hx, 0.5 * width), (1.0, hx, hz, 0.5 * depth)] {
                    for flip in [0.0, 2.0] {
                        slopes.push(Slope {
                            turn: turn(quarter + flip),
                            eave,
                            half,
                            hips: true,
                            parts: vec![hipped(half, 1.0)],
                            open_beyond: wall,
                            edges: vec![Edge::Along { s: 0.0, c: [-half, half] }],
                        });
                    }
                }
            }
            RoofKind::Gable => {
                for flip in [0.0, 2.0] {
                    slopes.push(Slope {
                        turn: turn(flip),
                        eave: hz,
                        half: hx,
                        hips: false,
                        parts: vec![Region::new([-hx, hx], [0.0, 1.0], Vec::new())],
                        open_beyond: 0.5 * width,
                        edges: vec![
                            Edge::Along { s: 0.0, c: [-hx, hx] },
                            Edge::Across { c: hx, s: [0.0, 1.0] },
                            Edge::Across { c: -hx, s: [0.0, 1.0] },
                        ],
                    });
                }
            }
            RoofKind::HipGable(at) => {
                let verge = self.gable_x() + self.roof.barge;
                let gable = self.gable_x();
                for flip in [0.0, 2.0] {
                    slopes.push(Slope {
                        turn: turn(flip),
                        eave: hz,
                        half: hx,
                        hips: true,
                        parts: vec![hipped(hx, at), Region::new([-verge, verge], [at, 1.0], Vec::new())],
                        open_beyond: gable.min(0.5 * width),
                        edges: vec![
                            Edge::Along { s: 0.0, c: [-hx, hx] },
                            Edge::Across { c: verge, s: [at, 1.0] },
                            Edge::Across { c: -verge, s: [at, 1.0] },
                            Edge::Along { s: at, c: [gable, verge] },
                            Edge::Along { s: at, c: [-verge, -gable] },
                        ],
                    });
                    slopes.push(Slope {
                        turn: turn(1.0 + flip),
                        eave: hx,
                        half: hz,
                        hips: true,
                        parts: vec![hipped(hz, at)],
                        open_beyond: 0.5 * depth,
                        edges: vec![Edge::Along { s: 0.0, c: [-hz, hz] }],
                    });
                }
            }
        }
        slopes
    }

    /// Where along the eave a slope's corner lies at height `s`.
    fn corner(&self, slope: &Slope, s: f32) -> f32 {
        if slope.hips { slope.half - s * self.run } else { slope.half }
    }

    /// The eaves' sweep at (c, s) on `slope`.
    fn lift(&self, slope: &Slope, c: f32, s: f32) -> f32 {
        let roof = self.roof;
        if roof.sweep <= 0.0 || s >= self.wall {
            return 0.0;
        }
        let distance = (self.corner(slope, s) - c.abs()).max(0.0);
        let along = 1.0 - smoothstep(0.0, roof.sweep_reach, distance);
        let fall = (1.0 - s / self.wall).powi(2);
        roof.sweep * along * fall
    }

    /// The tile rows' spacing: wider in light detail.
    fn period(&self) -> f32 {
        if self.detail == Detail::Light { 1.4 * self.roof.tiles.period } else { self.roof.tiles.period }
    }

    /// Where along each period the tiles' corners are: a flat channel and a cover tile's two
    /// faces, or in light detail one face up and one down.
    fn phases(&self) -> &'static [f32] {
        if self.detail == Detail::Light { &[0.5 * CHANNEL, CREST] } else { &[0.0, CHANNEL, CREST] }
    }

    /// The tiles' height over a channel at `c` along the eave: 0 in the channels, 1 on crests.
    fn wave(&self, c: f32) -> f32 {
        let phase = (c / self.period() + CREST).rem_euclid(1.0);
        if self.detail == Detail::Light {
            // Rows of two faces: up from the channel's middle to the crest and down again.
            let from = 0.5 * CHANNEL;
            let up = (phase - from).rem_euclid(1.0);
            let rise = CREST - from;
            return if up < rise { up / rise } else { (1.0 - up) / (1.0 - rise) };
        }
        if phase < CHANNEL {
            0.0
        } else if phase < CREST {
            (phase - CHANNEL) / (CREST - CHANNEL)
        } else {
            (1.0 - phase) / (1.0 - CREST)
        }
    }

    /// The smooth surface (no tiles) at (c, s) in the slope's frame: (c, height, out).
    fn smooth_local(&self, slope: &Slope, c: f32, s: f32) -> Vec3 {
        Vec3::new(c, self.height(s) + self.lift(slope, c, s), slope.eave - s * self.run)
    }

    fn to_roof(&self, slope: &Slope, local: Vec3) -> Vec3 {
        self.origin + slope.turn * local
    }

    /// The smooth surface's upward normal at (c, s), the tiles' slope along the eave added.
    fn normal(&self, slope: &Slope, c: f32, s: f32, tile_slope: f32) -> Vec3 {
        let h = 1e-3;
        let y = |c: f32, s: f32| self.height(s) + self.lift(slope, c, s);
        let dy_dc = (y(c + h, s) - y(c - h, s)) / (2.0 * h);
        let dy_ds = (y(c, s + h) - y(c, s - h)) / (2.0 * h);
        let along = Vec3::new(1.0, dy_dc + tile_slope, 0.0);
        let up = Vec3::new(0.0, dy_ds, -self.run);
        slope.turn * along.cross(up).normalize()
    }

    /// Positions of tile rows' corners along the eave, from `from` to `to`, with both ends.
    fn columns(&self, from: f32, to: f32, phases: &[f32]) -> Vec<f32> {
        let period = self.period();
        let mut columns = vec![from, to];
        let first = (from / period).floor() as i64 - 1;
        let last = (to / period).ceil() as i64 + 1;
        for k in first..=last {
            for &phase in phases {
                let c = (k as f32 + phase - CREST) * period;
                if c > from + 1e-4 && c < to - 1e-4 {
                    columns.push(c);
                }
            }
        }
        columns.sort_by(f32::total_cmp);
        columns
    }

    /// Rows up the slope from `from` to `to`: three across the eaves band, then about every
    /// [`ROW_RUN`] meters, with both ends and the gable's foot.
    fn rows(&self, from: f32, to: f32, coarse: bool) -> Vec<f32> {
        let mut rows = vec![from, to];
        // The sweep curls the eaves band: rows enough to follow it, or one where it's straight.
        let band = match (coarse, self.roof.sweep > 0.0) {
            (_, false) => 1,
            (true, true) => 2,
            (false, true) => {
                if self.detail == Detail::Light {
                    2
                } else {
                    EAVE_ROWS
                }
            }
        };
        for k in 1..=band {
            rows.push(self.wall * k as f32 / band as f32);
        }
        let spacing = if coarse { 2.0 * ROW_RUN } else { ROW_RUN }
            * if self.detail == Detail::Light { 1.5 } else { 1.0 };
        let above = ((1.0 - self.wall) * self.run / spacing).ceil().max(2.0) as usize;
        for k in 1..above {
            rows.push(self.wall + (1.0 - self.wall) * k as f32 / above as f32);
        }
        if let RoofKind::HipGable(at) = self.roof.kind {
            rows.push(at);
        }
        rows.retain(|&s| s >= from - 1e-6 && s <= to + 1e-6);
        rows.sort_by(f32::total_cmp);
        rows.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        rows
    }

    /// Fills `region` on a grid of `columns` × `rows`, each cell cut to the region, its corners
    /// placed by `place` (c, s, the cell's column index) and the cell triangulated as a fan.
    fn grid(&self, region: &Region, columns: &[f32], rows: &[f32], place: &mut dyn FnMut(&[Vec2], usize)) {
        for i in 0..columns.len().saturating_sub(1) {
            let (c0, c1) = (columns[i], columns[i + 1]);
            if c1 <= region.c[0] || c0 >= region.c[1] {
                continue;
            }
            for j in 0..rows.len().saturating_sub(1) {
                let (s0, s1) = (rows[j], rows[j + 1]);
                let cell = [Vec2::new(c0, s0), Vec2::new(c1, s0), Vec2::new(c1, s1), Vec2::new(c0, s1)];
                let cut = polygon::clip(&cell, &region.planes);
                if cut.len() >= 3 && polygon::signed_area(&cut) > 1e-8 {
                    place(&cut, i);
                }
            }
        }
    }

    /// A slope's tiles.
    fn tiles(&self, mesh: &mut Mesh, slope: &Slope) {
        let depth = self.roof.tiles.depth;
        for region in &slope.parts {
            let columns = self.columns(region.c[0], region.c[1], self.phases());
            let rows = self.rows(region.s[0], region.s[1], false);
            self.grid(region, &columns, &rows, &mut |cell, column| {
                let (c0, c1) = (columns[column], columns[column + 1]);
                let tile_slope = depth * (self.wave(c1) - self.wave(c0)) / (c1 - c0);
                let corner = |p: Vec2| {
                    let local = self.smooth_local(slope, p.x, p.y) + Vec3::Y * depth * self.wave(p.x);
                    let position = self.to_roof(slope, local);
                    let normal = self.normal(slope, p.x, p.y, tile_slope);
                    // In tile periods across, so the map's channels lie on the modeled ones.
                    let across = (p.x / self.period() + CREST) / TILE_PERIODS;
                    let uv = Vec2::new(across, self.arc_length(p.y) / TILE_COURSES);
                    let shade = CHANNEL_SHADE + (1.0 - CHANNEL_SHADE) * self.wave(p.x);
                    (position, normal, uv, shade)
                };
                fan(mesh, Part::Tiles, cell, &corner, false, ShapeUv::Tiles);
            });
        }
    }

    /// A slope's soffit: under the eaves band and wherever its underside shows past the walls.
    fn soffit(&self, mesh: &mut Mesh, slope: &Slope) {
        let thickness = self.roof.thickness;
        let open = slope.open_beyond;
        for region in &slope.parts {
            let columns = self.columns(region.c[0], region.c[1], &[CREST]);
            let rows = self.rows(region.s[0], region.s[1], true);
            let pieces = [
                region.and(HalfPlane::new(Vec2::Y, self.wall)),
                region.and(HalfPlane::new(-Vec2::Y, -self.wall)).and(HalfPlane::new(-Vec2::X, -open)),
                region.and(HalfPlane::new(-Vec2::Y, -self.wall)).and(HalfPlane::new(Vec2::X, -open)),
            ];
            for piece in &pieces {
                self.grid(piece, &columns, &rows, &mut |cell, _| {
                    let corner = |p: Vec2| {
                        let local = self.smooth_local(slope, p.x, p.y) - Vec3::Y * thickness;
                        let position = self.to_roof(slope, local);
                        let normal = -self.normal(slope, p.x, p.y, 0.0);
                        let uv = Vec2::new(p.x, p.y * self.run);
                        let shade = if p.y <= self.wall { 0.85 - 0.3 * p.y / self.wall } else { 0.6 };
                        (position, normal, uv, shade)
                    };
                    fan(mesh, Part::Eaves, cell, &corner, true, ShapeUv::Meters);
                });
            }
        }
    }

    /// The fascia along an open edge: the tiles' ends over a timber board, down to the soffit.
    fn fascia(&self, mesh: &mut Mesh, slope: &Slope, edge: Edge) {
        let thickness = self.roof.thickness;
        let depth = self.roof.tiles.depth;
        let board = BOARD * thickness;
        // Points along the edge: the top's (every tile corner or row) and the soffit's.
        let (top_points, low_points, out): (Vec<Vec2>, Vec<Vec2>, Vec3) = match edge {
            Edge::Along { s, c } => {
                let top =
                    self.columns(c[0], c[1], self.phases()).into_iter().map(|c| Vec2::new(c, s)).collect();
                let low = self.columns(c[0], c[1], &[CREST]).into_iter().map(|c| Vec2::new(c, s)).collect();
                (top, low, Vec3::Z)
            }
            Edge::Across { c, s } => {
                let top = self.rows(s[0], s[1], false).into_iter().map(|s| Vec2::new(c, s)).collect();
                let low = self.rows(s[0], s[1], true).into_iter().map(|s| Vec2::new(c, s)).collect();
                (top, low, Vec3::X * c.signum())
            }
        };
        let along = |p: Vec2| match edge {
            Edge::Along { .. } => p.x,
            Edge::Across { .. } => p.y * self.run,
        };
        let top: Vec<(f32, Vec3)> = top_points
            .iter()
            .map(|&p| {
                (
                    along(p),
                    self.to_roof(
                        slope,
                        self.smooth_local(slope, p.x, p.y) + Vec3::Y * depth * self.wave(p.x),
                    ),
                )
            })
            .collect();
        let at = |p: Vec2, over: f32| {
            (along(p), self.to_roof(slope, self.smooth_local(slope, p.x, p.y) - Vec3::Y * (thickness - over)))
        };
        // The board's top takes the tiles' points, so the tile ends above it (rising and
        // falling with the rows) are quads; the board's bottom the soffit's, so they share an
        // edge. Neither run of the board bends, so the strip between them never folds.
        let board_top: Vec<(f32, Vec3)> = top_points.iter().map(|&p| at(p, board)).collect();
        let bottom: Vec<(f32, Vec3)> = low_points.iter().map(|&p| at(p, 0.0)).collect();
        let out = slope.turn * out;
        mesh.zipper(Part::Tiles, &top, &board_top, out, [1.0, 0.9]);
        mesh.zipper(Part::Eaves, &board_top, &bottom, out, [0.95, 0.8]);
    }

    /// The gables' faces: a gable roof's over the walls' ends, a hip-and-gable's set back over
    /// its end slopes, each up to the soffit under its slopes.
    fn gables(&self, mesh: &mut Mesh) -> Result<(), String> {
        let (x, foot) = match self.roof.kind {
            RoofKind::Gable => (0.5 * self.roof.size[0], self.wall),
            RoofKind::HipGable(at) => (self.gable_x(), at),
            _ => return Ok(()),
        };
        let thickness = self.roof.thickness;
        let soffit = |s: f32| self.height(s) - thickness;
        let rows = self.rows(foot, 1.0, true);
        let reach = self.half.y - foot * self.run;
        // In the face's plane, seen from outside its +X end: right is -Z.
        let mut outline = vec![Vec2::new(-reach, soffit(foot)), Vec2::new(reach, soffit(foot))];
        // Up the back slope's underside, then down the front's.
        for &s in rows.iter().skip(1) {
            outline.push(Vec2::new(self.half.y - s * self.run, soffit(s)));
        }
        for &s in rows.iter().rev().skip(1) {
            if s <= foot + 1e-6 {
                continue;
            }
            outline.push(Vec2::new(-(self.half.y - s * self.run), soffit(s)));
        }
        outline.dedup_by(|a, b| a.distance_squared(*b) < 1e-10);
        let (low, high) = (soffit(foot), soffit(1.0));
        let shade = move |p: Vec2| 1.0 - 0.25 * ((p.y - low) / (high - low).max(1e-3)).clamp(0.0, 1.0);
        for side in [1.0, -1.0] {
            let frame =
                Frame { origin: self.origin + Vec3::X * (side * x), right: -Vec3::Z * side, up: Vec3::Y };
            // Plaster's map from its top (streaks under the ridge) to a little past its middle.
            let uv =
                move |p: Vec2| Vec2::new(p.x / PLASTER_REPEAT, 0.6 * (high - p.y) / (high - low).max(1e-3));
            mesh.fill_mapped(Part::Gable, &frame, &outline, &[], &shade, Some(&uv))?;
        }
        Ok(())
    }

    /// Ridges, ornaments, hip ridges, verge ridges and corner beams.
    fn ridges(&self, mesh: &mut Mesh, slopes: &[Slope]) -> Result<(), String> {
        let samples = if self.detail == Detail::Light { PATH_SAMPLES - 2 } else { PATH_SAMPLES };
        let ridge = self.roof.ridge;
        let top = self.height(1.0);
        // The main ridge, its top rising toward its ends.
        let length = match self.roof.kind {
            RoofKind::Hip => self.half.x - self.half.y,
            // Past the verges a little, so its ends never share a plane with their fascias.
            RoofKind::Gable => self.half.x + RIDGE_PAST,
            RoofKind::HipGable(_) => self.gable_x() + self.roof.barge + RIDGE_PAST,
            RoofKind::Skirt(_) => 0.0,
        };
        let rising = 0.5 * ridge.height;
        if ridge.height > 0.0 && length > 0.5 * ridge.width {
            let path: Vec<Vec3> = (0..=2 * samples)
                .map(|k| {
                    let t = k as f32 / samples as f32 - 1.0;
                    self.origin + Vec3::new(t * length, top + rising * t.abs().powi(4), 0.0)
                })
                .collect();
            mesh.sweep(Part::Ridge, &path, ridge.width, ridge.height, EMBED + rising, false);
            if ridge.ends > 0.0 {
                for side in [1.0, -1.0] {
                    let end = self.origin + Vec3::new(side * length, top + rising + ridge.height, 0.0);
                    ornament(mesh, end, side, ridge)?;
                }
            }
        } else if matches!(self.roof.kind, RoofKind::Hip) && ridge.height > 0.0 {
            finial(mesh, self.origin + Vec3::Y * (top - EMBED), ridge);
        }
        for slope in slopes {
            // Hips: from where they meet the ridge (or the gable's foot) down to the corner, and
            // an upturned tip out past it; a corner beam under each, from the walls' corner out.
            if slope.hips {
                let to = match self.roof.kind {
                    RoofKind::HipGable(at) => at,
                    _ => 1.0,
                };
                let hip = |s: f32| self.to_roof(slope, self.smooth_local(slope, self.corner(slope, s), s));
                let out = slope.turn * Vec3::new(1.0, 0.0, 1.0).normalize();
                let tip_rise = 0.25 * ridge.height + 0.25 * self.roof.sweep;
                let mut path: Vec<Vec3> =
                    (0..=samples).map(|k| hip(to * (1.0 - k as f32 / samples as f32))).collect();
                let corner = hip(0.0);
                for (k, reach) in [(1.0, 0.18), (2.0, 0.32)] {
                    path.push(corner + out * reach + Vec3::Y * tip_rise * k * k / 4.0);
                }
                if ridge.height > 0.0 {
                    mesh.sweep(Part::Ridge, &path, 0.8 * ridge.width, 0.75 * ridge.height, EMBED, true);
                }
                let soffit = |s: f32| hip(s) - Vec3::Y * self.roof.thickness;
                let beam_height = 0.6 * self.roof.thickness;
                let mut beam: Vec<Vec3> = (0..=samples / 2)
                    .map(|k| soffit(self.wall * (1.05 - 1.05 * k as f32 / (samples / 2) as f32)))
                    .collect();
                let beam_corner = soffit(0.0);
                beam.push(beam_corner + out * 0.2 + Vec3::Y * 0.5 * tip_rise);
                mesh.sweep(Part::Eaves, &beam, 0.6 * ridge.width.max(0.2), 0.02, beam_height, true);
            }
            if ridge.height <= 0.0 {
                continue;
            }
            // Verge ridges along a gable's slopes.
            for &edge in &slope.edges {
                if let Edge::Across { c, s } = edge {
                    let inset = c - c.signum() * 0.5 * ridge.width;
                    let path: Vec<Vec3> = (0..=samples)
                        .map(|k| {
                            // Past the open edge it ends at, a little, as the main ridge.
                            let low = s[0] - RIDGE_PAST / self.run;
                            let s = s[1] - (s[1] - low) * k as f32 / samples as f32;
                            self.to_roof(
                                slope,
                                self.smooth_local(slope, inset, s) + Vec3::Y * self.roof.tiles.depth,
                            )
                        })
                        .collect();
                    mesh.sweep(Part::Ridge, &path, 0.8 * ridge.width, 0.6 * ridge.height, EMBED, true);
                }
            }
            // A ridge along a hip-and-gable's gable foot, or a skirt's top edge against its storey.
            let along = match self.roof.kind {
                RoofKind::HipGable(at) if slope.half < self.half.x => Some(at),
                RoofKind::Skirt(_) => Some(1.0 - 0.5 * ridge.width / self.run),
                _ => None,
            };
            if let Some(s) = along {
                // Stopping short of the corners, where the hips' ridges meet it, so its ends never
                // share a plane with a hip-and-gable's fascia across the gable's foot.
                let reach = self.corner(slope, s) - 0.5 * ridge.width;
                let path: Vec<Vec3> = (0..=2)
                    .map(|k| {
                        let c = reach * (k as f32 - 1.0);
                        self.to_roof(slope, self.smooth_local(slope, c, s) + Vec3::Y * self.roof.tiles.depth)
                    })
                    .collect();
                mesh.sweep(Part::Ridge, &path, ridge.width, 0.6 * ridge.height, EMBED, false);
            }
        }
        Ok(())
    }
}

/// Fans a convex cell of a slope's grid, each corner placed by `corner` (position, normal,
/// texture coordinates, occlusion), the coordinates read as `uv` says (meters or tiles);
/// `under` faces it down.
fn fan(
    mesh: &mut Mesh,
    part: Part,
    cell: &[Vec2],
    corner: &dyn Fn(Vec2) -> (Vec3, Vec3, Vec2, f32),
    under: bool,
    uv: fn([Vec2; 3]) -> ShapeUv,
) {
    let corners: Vec<(Vec3, Vec3, Vec2, f32)> = cell.iter().map(|&p| corner(p)).collect();
    for k in 1..corners.len() - 1 {
        let mut tri = [0, k, k + 1];
        if under {
            tri.swap(1, 2);
        }
        let positions = tri.map(|i| corners[i].0);
        let normals = tri.map(|i| corners[i].1);
        // Keep the winding facing the normals (the cell is counter-clockwise in (c, s)).
        let face = (positions[1] - positions[0]).cross(positions[2] - positions[0]);
        if face.dot(normals[0] + normals[1] + normals[2]) < 0.0 {
            tri.swap(1, 2);
        }
        mesh.triangle(
            part,
            tri.map(|i| corners[i].0),
            tri.map(|i| corners[i].1),
            uv(tri.map(|i| corners[i].2)),
            tri.map(|i| corners[i].3),
        );
    }
}

/// A main ridge's end ornament (a stylized upturned tail), standing on the ridge's top at `end`
/// and curling in toward the middle; `side` is +1 at the +X end.
fn ornament(mesh: &mut Mesh, end: Vec3, side: f32, ridge: Ridge) -> Result<(), String> {
    let (h, a) = (ridge.ends, 0.7 * ridge.ends);
    // In (out along the ridge, up) from the ridge's end at its top, counter-clockwise.
    let profile: Vec<Vec2> = [
        (0.08, -0.25),
        (0.14, 0.2),
        (0.22, 0.5),
        (0.16, 0.78),
        (-0.02, 0.98),
        (-0.3, 1.0),
        (-0.48, 0.86),
        (-0.36, 0.8),
        (-0.24, 0.62),
        (-0.3, 0.42),
        (-0.62, 0.22),
        (-0.7, -0.25),
    ]
    .iter()
    .map(|&(x, y)| Vec2::new(x * a, y * h))
    .collect();
    // Out along the ridge is +X at its +X end and -X at the other; extruded across the ridge's
    // width either side of its middle.
    let frame = Frame { origin: end, right: Vec3::X * side, up: Vec3::Y };
    let half = 0.42 * ridge.width;
    mesh.prism(Part::Ridge, &frame, &profile, -half, half)
}

/// A pyramid roof's finial: a gourd on the apex.
fn finial(mesh: &mut Mesh, apex: Vec3, ridge: Ridge) {
    let r = 0.5 * ridge.width.max(0.2);
    let h = ridge.ends.max(2.5 * r);
    let profile: Vec<(f32, f32)> = [
        (1.2, 0.0),
        (1.0, 0.12),
        (0.7, 0.2),
        (1.25, 0.42),
        (0.95, 0.62),
        (0.45, 0.72),
        (0.6, 0.82),
        (0.35, 0.94),
        (0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y)| (x * r, y * h))
    .collect();
    mesh.lathe(Part::Ridge, apex, &profile, 10, [false, false], false);
}
