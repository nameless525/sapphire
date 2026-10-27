//! Jinzhou's far mountains (M1 X1 part 2d): three ranges of the project's own round the look-dev
//! town, generated as heightfields and written as glTF sources under `content/lookdev/mountains/`.
//!
//! Wuthering Waves' backdrops are layered ranges of steep, sculpted peaks that recede in value
//! and hue through the haze, with mist between them; their silhouettes are soft and organic,
//! never faceted. So the ranges here are:
//!
//! | Range | Meters from the square | Heights | What |
//! |---|---|---|---|
//! | `near` | 700–1,050 | ridges about 110 m, knolls to 220 m | Forested ridges and knolls, rock showing on their steeper faces |
//! | `middle` | 1,250–1,850 | ridges about 190 m, towers to 560 m | Karst towers with sheer, fluted walls, green ledges and rounded green tops; Jinzhou's mountain among them, over the street's end |
//! | `far` | 2,200–3,200 | ridges about 380 m, massifs to 810 m | A broad, pale range, mostly rock |
//!
//! Each is a heightfield over a band round the square ([`Range`]): a spine of ridges along the
//! band, peaks (some placed to compose the stage's shots, [`Peak`], the rest scattered by a seed)
//! joined to it by a smooth maximum, ridged noise carving gullies and spurs into their slopes,
//! and stepped ledges on the upper slopes. A peak is a rounded bump or a karst tower (a rounded
//! summit, a sheer wall, a talus apron) or between the two, its footprint drawn out along a
//! random axis, lobed into spurs and fluted into buttresses. The cameras all look toward −Z
//! from the square, so a range covers the [`ARC`] either side of −Z (every shot's view with room
//! to spare) and nothing behind them.
//!
//! - **Geometry:** a polar grid per range, columns along the arc about [`Range::spacing`] apart
//!   at its middle and rows across the band, each cell split along the diagonal that follows the
//!   surface better. Normals come from the heightfield over a few cells, smoother than the grid,
//!   so slopes read as painted masses rather than facets. Ranges dip [`FOOT`] meters under the
//!   plain at their feet and ends, so no edge shows.
//! - **Chunks:** each range is cut into sectors along the arc, each its own source file and
//!   mesh. A mesh's bounding sphere is what shadows cull casters by, per cascade, with a box in
//!   light space that reaches 256 m toward the light: a whole range's sphere would hold the
//!   square, putting every mountain into every cascade, and so would a sector's if it were too
//!   wide (a quarter of the far range's reached within 800 m of the square). So the sectors are
//!   narrow enough (20 near, 20 middle, 8 far) that under any light, with the camera anywhere in
//!   the town, no mountain enters the near cascades ([`NEAR_CASCADES`]), and only the near range
//!   can enter desktop's farthest ([`FAR_CASCADE`]), which redraws every other frame. Neighbors
//!   share their edge's vertices bit for bit (positions are cooked as floats), so no crack opens
//!   between them.
//! - **Surface:** one environment material (`mountains.material.ron`) reads [`palette_png`]
//!   through texture coordinates the generator writes: V the range's band and the height within
//!   it, U how rocky the surface is (steepness, gullies against ridges, patches of noise, and a
//!   little more rock toward the crests). The map paints forest and meadow on gentle ground,
//!   rock on steep faces in strata with green along some of their ledges, and paler crests;
//!   each band paler and bluer than the last, the haze painted in as anime backdrops paint it.
//!   V runs with height inside every triangle, so a wall a few triangles tall still shows its
//!   strata as level bands, the levels wandering a little from peak to peak. Vertex color R
//!   carries gullies' occlusion.
//! - **Mobile** draws a simplified level: the sidecars ask the cooker for a LOD (meshoptimizer,
//!   within 0.28%, 0.34% and 0.3% of a near, middle and far sector's extent: at most about 1 m,
//!   2.3 m and 4.9 m, a tenth of a degree seen from the square), which the mobile cook takes as
//!   its top level (`mobile_skip`): 57% of the triangles. The sidecars lock each sector's border
//!   (`lock_border`), so the simplifier keeps the seams' vertices and neighbors still meet.
//!
//! Everything is computed in plain `f32` arithmetic with [`sapphire_math::det`]'s functions, so
//! the files come out the same bits on every platform; tests still compare the committed files to
//! the generated ones only up to float rounding, as the other generated content.

use std::f32::consts::TAU;
use std::io;

use glam::{Vec2, Vec3};
use sapphire_math::det;
use serde_json::json;

use crate::color::{linear_to_srgb, srgb_to_linear, unorm8};
use crate::gltf_writer::{Geometry, GltfWriter};
use crate::samples::noise::{NO_PERIOD, Rng, fbm, hash, smoothstep, value_noise};

/// The mountains' directory, under the look-dev directory.
pub const MOUNTAINS_DIR: &str = "mountains";

/// The palette map's file name, in [`MOUNTAINS_DIR`].
pub const PALETTE: &str = "mountains.png";

/// The glTF material every range draws with, which each sidecar binds to the mountains'
/// environment material.
pub const SLOT: &str = "Mountain";

/// How far either side of −Z (the cameras' way) the ranges reach, in degrees: every shot's view,
/// from the sky shot's right edge to the wide shot's left, on screens up to 21:9, with room to
/// orbit.
pub const ARC: f32 = 115.0;

/// How far under the plain the ranges' feet and ends sink, in meters.
pub const FOOT: f32 = -20.0;

/// Shadow cascades as the renderer fits them (`sapphire_render`'s `shadows::fit`), for keeping
/// sectors out of them: each a sphere round a slice of the view, of up to `radius` meters, its
/// center within `center` meters of the square while the camera stands within 40 m of it, as
/// every shot's does. A caster's bounding sphere enters a cascade where the box round the
/// cascade's sphere in light space, grown by the caster's radius and reaching [`CASTER_REACH`]
/// further toward the light, holds its center.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cascades {
    pub radius: f32,
    pub center: f32,
}

/// The near cascades, every one that ends within 50 m of view: desktop's first three and both
/// of a phone's, at the widest shot's lens (the sky shot's 58°) on a 19.5:9 phone. No sector
/// may enter them.
pub const NEAR_CASCADES: Cascades = Cascades { radius: 66.0, center: 90.0 };

/// Desktop's farthest cascade (120 m of view, at the same lens on a 21:9 screen), which redraws
/// every other frame. Only the near range's sectors may enter it, where they lie toward a low
/// light.
pub const FAR_CASCADE: Cascades = Cascades { radius: 170.0, center: 160.0 };

/// How far toward the light a cascade takes casters past its sphere, in meters (the renderer's
/// `CASTER_REACH`).
pub const CASTER_REACH: f32 = 256.0;

impl Cascades {
    /// Whether a caster's bounding sphere could enter one of these cascades under some light
    /// (a conservative test: the box round the cascade can't hold a center farther than its
    /// corner).
    pub fn may_hold(&self, center: Vec3, radius: f32) -> bool {
        let reach = self.radius + radius;
        let corner = (2.0 * reach * reach + (reach + CASTER_REACH).powi(2)).sqrt();
        center.length() - self.center <= corner
    }
}

/// The palette map's size: U across (rockiness), and a band of rows per range down V.
const PALETTE_WIDTH: u32 = 64;
const BAND_ROWS: u32 = 256;
/// How much of a band's rows V keeps clear at its top and bottom, so bilinear filtering never
/// reads the next band.
const BAND_MARGIN: f32 = 0.04;

/// A peak placed by hand to compose the shots: where it stands (degrees from −Z, positive toward
/// +X, and meters from the square), its height and footprint radius in meters, and how much of a
/// tower it is (0 a rounded hill, 1 sheer sides under a rounded top).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    pub azimuth: f32,
    pub distance: f32,
    pub height: f32,
    pub radius: f32,
    pub tower: f32,
}

/// A range round the square.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    /// Its files' stem: `<name>_<sector>.glb`.
    pub name: &'static str,
    /// Its band: from its front foot to its back, in meters from the square.
    pub inner: f32,
    pub outer: f32,
    /// The spine's crests: their usual height, in meters.
    pub crest: f32,
    /// The highest the range reaches, in meters: the top of its band in the palette.
    pub top: f32,
    /// Sectors along the arc, each a file of its own.
    pub sectors: u32,
    /// Meters between columns at the band's middle, and rows across the band.
    pub spacing: f32,
    pub rows: u32,
    /// How deep its gullies and spurs cut, as a share of the height there (half its crest's,
    /// half the point's).
    pub carving: f32,
    /// The ledges' step on its upper slopes, in meters, and how much of each step is ledge.
    pub ledge: f32,
    pub ledges: f32,
    /// Peaks placed to compose the shots.
    pub peaks: &'static [Peak],
    /// Peaks scattered along its band: how many, and their heights as shares of its crest.
    pub scattered: u32,
    pub scattered_height: (f32, f32),
    /// How much of a tower its scattered peaks are, at most.
    pub scattered_tower: f32,
    /// Seeds its noise and its scattered peaks.
    pub seed: u32,
}

impl Range {
    /// Its band's index in the palette: 0 near, 1 middle, 2 far.
    fn band(&self) -> usize {
        RANGES.iter().position(|range| range.name == self.name).expect("one of RANGES")
    }

    /// Columns per sector: about [`Self::spacing`] apart at the band's middle.
    fn columns(&self) -> u32 {
        let arc = 2.0 * ARC.to_radians() * 0.5 * (self.inner + self.outer);
        ((arc / self.spacing) / self.sectors as f32).ceil() as u32
    }

    /// The azimuth of column `column` of the whole range, in radians: computed from the integer
    /// alone, so neighboring sectors' shared columns agree bit for bit.
    fn azimuth(&self, column: u32) -> f32 {
        let columns = self.columns() * self.sectors;
        let arc = ARC.to_radians();
        -arc + 2.0 * arc * (column as f32 / columns as f32)
    }

    /// The radius of row `row`, in meters.
    fn radius(&self, row: u32) -> f32 {
        self.inner + (self.outer - self.inner) * (row as f32 / self.rows as f32)
    }
}

/// The ranges, nearest first.
pub const RANGES: [Range; 3] = [
    Range {
        name: "near",
        inner: 700.0,
        outer: 1050.0,
        crest: 110.0,
        top: 260.0,
        sectors: 20,
        spacing: 6.0,
        rows: 26,
        carving: 0.25,
        ledge: 30.0,
        ledges: 0.3,
        peaks: &[
            // Wooded knolls: over the left row's roofs, through the street's end, to the right.
            Peak { azimuth: -32.0, distance: 860.0, height: 220.0, radius: 230.0, tower: 0.4 },
            Peak { azimuth: 9.0, distance: 900.0, height: 160.0, radius: 200.0, tower: 0.3 },
            Peak { azimuth: 52.0, distance: 820.0, height: 200.0, radius: 200.0, tower: 0.5 },
        ],
        scattered: 18,
        scattered_height: (1.0, 1.8),
        scattered_tower: 0.5,
        seed: 0x6e01,
    },
    Range {
        name: "middle",
        inner: 1250.0,
        outer: 1850.0,
        crest: 190.0,
        top: 680.0,
        sectors: 20,
        spacing: 10.0,
        rows: 28,
        carving: 0.2,
        ledge: 45.0,
        ledges: 0.4,
        peaks: &[
            // Jinzhou's mountain: a tall tower of rock over the street's end and the gate wall,
            // on a broad, stepped massif with a lower tower beside it.
            Peak { azimuth: 0.5, distance: 1500.0, height: 620.0, radius: 270.0, tower: 1.0 },
            Peak { azimuth: -1.5, distance: 1560.0, height: 330.0, radius: 520.0, tower: 0.6 },
            Peak { azimuth: -7.0, distance: 1620.0, height: 430.0, radius: 240.0, tower: 0.9 },
            // Karst towers to the right, over the right row and in the sky shot.
            Peak { azimuth: 34.0, distance: 1400.0, height: 440.0, radius: 200.0, tower: 1.0 },
            Peak { azimuth: 45.0, distance: 1560.0, height: 520.0, radius: 240.0, tower: 1.0 },
            Peak { azimuth: 58.0, distance: 1420.0, height: 380.0, radius: 190.0, tower: 1.0 },
            // Lower to the left, under the golden hour's sun.
            Peak { azimuth: -38.0, distance: 1500.0, height: 300.0, radius: 260.0, tower: 0.8 },
            Peak { azimuth: -58.0, distance: 1420.0, height: 400.0, radius: 220.0, tower: 1.0 },
        ],
        scattered: 22,
        scattered_height: (1.0, 2.0),
        scattered_tower: 1.0,
        seed: 0x6e02,
    },
    Range {
        name: "far",
        inner: 2200.0,
        outer: 3200.0,
        crest: 380.0,
        top: 900.0,
        sectors: 8,
        spacing: 16.0,
        rows: 26,
        carving: 0.2,
        ledge: 70.0,
        ledges: 0.3,
        peaks: &[
            // Broad massifs: to the left, the hero shot's backdrop, and to the right.
            Peak { azimuth: -24.0, distance: 2700.0, height: 760.0, radius: 750.0, tower: 0.5 },
            Peak { azimuth: 22.0, distance: 2800.0, height: 680.0, radius: 700.0, tower: 0.5 },
            Peak { azimuth: 70.0, distance: 2600.0, height: 640.0, radius: 600.0, tower: 0.5 },
            Peak { azimuth: -68.0, distance: 2700.0, height: 600.0, radius: 600.0, tower: 0.5 },
        ],
        scattered: 14,
        scattered_height: (1.0, 1.6),
        scattered_tower: 0.5,
        seed: 0x6e03,
    },
];

/// One sector of a range: its file, relative to the look-dev directory, and its geometry.
pub(crate) struct Chunk {
    pub file: String,
    pub geometry: Geometry,
}

/// Every range's sectors, nearest range first, each along the arc from −X to +X.
pub(crate) fn chunks() -> Vec<Chunk> {
    RANGES
        .iter()
        .flat_map(|range| {
            let field = Field::new(range);
            (0..range.sectors)
                .map(move |sector| Chunk {
                    file: format!("{MOUNTAINS_DIR}/{}_{sector}.glb", range.name),
                    geometry: field.sector(sector),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Every generated mountain file, relative to the look-dev directory, with its bytes: the
/// sectors' glTF and the palette.
pub fn files() -> io::Result<Vec<(String, Vec<u8>)>> {
    let mut files: Vec<(String, Vec<u8>)> =
        chunks().into_iter().map(|chunk| (chunk.file, glb(&chunk.geometry))).collect();
    files.push((format!("{MOUNTAINS_DIR}/{PALETTE}"), palette_png()?));
    Ok(files)
}

/// A sector as a binary glTF: one primitive in [`SLOT`].
fn glb(geometry: &Geometry) -> Vec<u8> {
    let mut gltf = GltfWriter::default();
    let material = gltf.add_material(json!({
        "name": SLOT,
        "pbrMetallicRoughness": { "baseColorFactor": [1.0, 1.0, 1.0, 1.0], "metallicFactor": 0.0 },
    }));
    let mesh = gltf.add_mesh(SLOT, &[(geometry, Some(material))]);
    let root = gltf.add_node(json!({ "name": SLOT, "mesh": mesh }));
    gltf.add_root(root);
    gltf.to_glb()
}

/// A peak in the field: its center on the plain, height, footprint radius and tower share.
#[derive(Clone, Copy, Debug)]
struct FieldPeak {
    center: Vec2,
    height: f32,
    radius: f32,
    tower: f32,
    /// Its footprint is drawn out along `axis` (a unit vector on the plain) by `stretch`, so its
    /// faces turn every way rather than round one middle.
    axis: Vec2,
    stretch: f32,
    /// Seeds the lobes and flutes of its footprint.
    seed: u32,
}

impl FieldPeak {
    fn new(center: Vec2, height: f32, radius: f32, tower: f32, seed: u32) -> Self {
        let mut rng = Rng(seed);
        let (turn, stretch) = (rng.next(), rng.next());
        let (sin, cos) = det::sin_cos(std::f32::consts::PI * turn);
        Self { center, height, radius, tower, axis: Vec2::new(cos, sin), stretch: 1.0 + 0.6 * stretch, seed }
    }
}

/// A range's heightfield.
struct Field<'a> {
    range: &'a Range,
    peaks: Vec<FieldPeak>,
}

/// Where a point of the plain is, in meters: `+X` right of the cameras' view, `−Z` ahead.
fn on_plain(azimuth: f32, radius: f32) -> Vec2 {
    let (sin, cos) = det::sin_cos(azimuth);
    Vec2::new(radius * sin, -radius * cos)
}

impl<'a> Field<'a> {
    fn new(range: &'a Range) -> Self {
        let mut peaks: Vec<FieldPeak> = range
            .peaks
            .iter()
            .enumerate()
            .map(|(index, peak)| {
                FieldPeak::new(
                    on_plain(peak.azimuth.to_radians(), peak.distance),
                    peak.height,
                    peak.radius,
                    peak.tower,
                    range.seed ^ (index as u32 + 1) << 8,
                )
            })
            .collect();
        let mut rng = Rng(range.seed);
        let depth = range.outer - range.inner;
        for index in 0..range.scattered {
            // Every peak draws all its numbers, so changing one rule moves no other peak.
            let (along, across, height, width, tower) =
                (rng.next(), rng.next(), rng.next(), rng.next(), rng.next());
            let azimuth = (ARC - 8.0).to_radians() * (2.0 * along - 1.0);
            let radius = range.inner + depth * (0.35 + 0.35 * across);
            let (low, high) = range.scattered_height;
            let height = range.crest * (low + (high - low) * height);
            // Towers stand on narrower feet than rounded peaks.
            let footprint = height * (1.15 - 0.45 * tower) * (0.8 + 0.4 * width);
            let seed = range.seed ^ (index + 100) << 8;
            peaks.push(FieldPeak::new(
                on_plain(azimuth, radius),
                height,
                footprint,
                range.scattered_tower * tower,
                seed,
            ));
        }
        Self { range, peaks }
    }

    /// The height at `p` on the plain, in meters.
    fn height(&self, p: Vec2) -> f32 {
        let range = self.range;
        let radius = p.length();
        let azimuth = det::atan2(p.x, -p.y);
        let depth = range.outer - range.inner;
        let middle = 0.5 * (range.inner + range.outer);
        // Meters along the band's middle: the spine's noise runs along it.
        let along = azimuth * middle;

        // The spine: a ridge wandering across the band, its crest rising and falling.
        let wander = fbm(Vec2::new(along / 900.0, 0.5), (NO_PERIOD, 1), 3, range.seed ^ 0x51);
        let crest_at = range.inner + depth * (0.4 + 0.3 * (wander - 0.5));
        let rise = fbm(Vec2::new(along / 650.0, 1.5), (NO_PERIOD, 1), 4, range.seed ^ 0x52);
        let spine_height = range.crest * (0.35 + 0.75 * rise);
        let across = (radius - crest_at).abs() / (0.55 * depth);
        let mut height = spine_height * bell(across);

        // The peaks, joined to the spine and to each other by a smooth maximum.
        let blend = 0.12 * range.crest;
        for peak in &self.peaks {
            let offset = p - peak.center;
            let reach = 1.5 * peak.radius * peak.stretch;
            if offset.x.abs() > reach || offset.y.abs() > reach {
                continue;
            }
            let along = offset.dot(peak.axis) / peak.stretch;
            let across = offset.perp_dot(peak.axis);
            let distance = Vec2::new(along, across).length() / peak.radius;
            if distance >= 1.5 {
                continue;
            }
            // Lobes and flutes: the footprint pushed out and in round the peak, broad spurs and,
            // down a tower's walls, grooves between buttresses.
            let around = det::atan2(across, along) / TAU + 0.5;
            let lobes = value_noise(Vec2::new(around * 7.0, 0.5), (7, 1), peak.seed);
            let flutes = value_noise(Vec2::new(around * 19.0, 0.5), (19, 1), peak.seed ^ 0x9e);
            let distance = distance * (1.0 + 0.4 * (lobes - 0.5) + 0.25 * peak.tower * (flutes - 0.5));
            let shape = bell(distance) + (karst(distance) - bell(distance)) * peak.tower;
            height = smooth_max(height, peak.height * shape, blend);
        }

        // Gullies and spurs: ridged noise, warped, carving the upper slopes most.
        let warp = Vec2::new(
            fbm(p / 400.0, (NO_PERIOD, NO_PERIOD), 2, range.seed ^ 0x53),
            fbm(p / 400.0 + Vec2::splat(17.0), (NO_PERIOD, NO_PERIOD), 2, range.seed ^ 0x54),
        ) - Vec2::splat(0.5);
        let carved = ridged(p / 260.0 + 1.2 * warp, 5, range.seed ^ 0x55);
        let reach = smoothstep(0.05, 0.6, height / range.crest);
        let depth_of_cut = range.carving * 0.5 * (range.crest + height.max(0.0));
        height += depth_of_cut * reach * (carved - 0.45) * 2.0;

        // Ledges: the upper slopes stepped, each step a ledge and a steeper riser, the steps'
        // levels wandering so no two peaks share them.
        let level = fbm(p / 700.0, (NO_PERIOD, NO_PERIOD), 2, range.seed ^ 0x56) * range.ledge;
        let steps = (height + level) / range.ledge;
        let within = steps - steps.floor();
        let stepped = (steps.floor() + smoothstep(range.ledges, 1.0, within)) * range.ledge - level;
        let upper = smoothstep(0.25, 0.7, height / range.crest);
        height += (stepped - height) * 0.55 * upper;

        // The feet: under the plain at the band's front and back and at the arc's ends.
        let front = smoothstep(range.inner, range.inner + 0.22 * depth, radius);
        let back = 1.0 - smoothstep(range.outer - 0.18 * depth, range.outer, radius);
        let arc = ARC.to_radians();
        let ends = 1.0 - smoothstep(arc - 14f32.to_radians(), arc, azimuth.abs());
        FOOT + (height - FOOT) * front * back * ends
    }

    /// Sector `sector`'s geometry.
    fn sector(&self, sector: u32) -> Geometry {
        let range = self.range;
        let columns = range.columns();
        let first = sector * columns;
        // Normals and occlusion look this far round each vertex: a column and a half.
        let reach = 1.5 * range.spacing;
        let mut geometry = Geometry::default();
        let mut heights = Vec::new();
        for row in 0..=range.rows {
            for column in first..=first + columns {
                let p = on_plain(range.azimuth(column), range.radius(row));
                let height = self.height(p);
                let around = [
                    self.height(p + Vec2::new(reach, 0.0)),
                    self.height(p - Vec2::new(reach, 0.0)),
                    self.height(p + Vec2::new(0.0, reach)),
                    self.height(p - Vec2::new(0.0, reach)),
                ];
                let normal = Vec3::new(around[1] - around[0], 2.0 * reach, around[3] - around[2]).normalize();
                // Gullies (the neighborhood above the point) against ridges (below it).
                let hollow = (around.iter().sum::<f32>() * 0.25 - height) / reach;
                geometry.positions.push([p.x, height, p.y]);
                geometry.normals.push(normal.to_array());
                geometry.uvs.push(self.palette_uv(p, height, normal, hollow));
                let occlusion = 1.0 - 0.45 * smoothstep(0.02, 0.25, hollow);
                geometry.colors.push([unorm8(occlusion), 255, 255, 255]);
                heights.push(height);
            }
        }
        let stride = columns + 1;
        for row in 0..range.rows {
            for column in 0..columns {
                let corner = |dc: u32, dr: u32| (row + dr) * stride + column + dc;
                let [a, b, c, d] = [corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1)];
                // Split along the diagonal whose middle lies nearer the surface's.
                let middle = self.height(on_plain(
                    0.5 * (range.azimuth(first + column) + range.azimuth(first + column + 1)),
                    0.5 * (range.radius(row) + range.radius(row + 1)),
                ));
                let height = |index: u32| heights[index as usize];
                let along_ad = (0.5 * (height(a) + height(d)) - middle).abs();
                let along_bc = (0.5 * (height(b) + height(c)) - middle).abs();
                // Counter-clockwise seen from above: columns run toward +X, rows away.
                if along_ad <= along_bc {
                    geometry.indices.extend([a, b, d, a, d, c]);
                } else {
                    geometry.indices.extend([a, b, c, b, d, c]);
                }
            }
        }
        geometry
    }

    /// Where a vertex reads the palette: U its rockiness, V its range's band and its height in it,
    /// the strata's levels wandering a little so no two peaks share them.
    fn palette_uv(&self, p: Vec2, height: f32, normal: Vec3, hollow: f32) -> [f32; 2] {
        let range = self.range;
        let steep = 1.0 - normal.y;
        // Patches of plants and bare rock: broad ones, and smaller ones where plants cling to the
        // walls' ledges.
        let patches = fbm(p / 180.0, (NO_PERIOD, NO_PERIOD), 3, range.seed ^ 0x57) - 0.5;
        let clumps = fbm(p / 45.0, (NO_PERIOD, NO_PERIOD), 2, range.seed ^ 0x59) - 0.5;
        let rock = 0.8 * smoothstep(0.12, 0.5, steep) + 0.2 * smoothstep(0.8, 1.0, height / range.top)
            - 0.3 * smoothstep(0.0, 0.2, hollow)
            + 0.35 * patches
            + 0.35 * clumps;
        let u = rock.clamp(0.0, 1.0);
        let dip = (fbm(p / 650.0, (NO_PERIOD, NO_PERIOD), 2, range.seed ^ 0x58) - 0.5) * 0.12 * range.top;
        let elevation = ((height + dip) / range.top).clamp(0.0, 1.0);
        let within = BAND_MARGIN + (1.0 - 2.0 * BAND_MARGIN) * (1.0 - elevation);
        let v = (range.band() as f32 + within) / RANGES.len() as f32;
        [(u * (PALETTE_WIDTH - 1) as f32 + 0.5) / PALETTE_WIDTH as f32, v]
    }
}

/// A rounded bump, 1 at its middle and 0 from `t = 1` out, flat at both: `(1 − t²)²`.
fn bell(t: f32) -> f32 {
    let s = (1.0 - t * t).max(0.0);
    s * s
}

/// A karst tower: a rounded summit, a sheer wall from about 0.42 to 0.66 of its radius, and a
/// talus apron round its foot, out to `t = 1`.
fn karst(t: f32) -> f32 {
    let top = (1.0 - 0.9 * t * t) * (1.0 - smoothstep(0.42, 0.66, t));
    let apron = 0.3 * (1.0 - smoothstep(0.45, 1.0, t));
    smooth_max(top, apron, 0.08)
}

/// The larger of `a` and `b`, rounded over `k` where they meet.
fn smooth_max(a: f32, b: f32, k: f32) -> f32 {
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.max(b) + h * h * k * 0.25
}

/// Ridged fractal noise in 0..1 at `p` (in lattice cells): sharp crests where each octave's
/// value noise crosses its middle, later octaves weighted by the crests before them, each octave
/// turned so the lattice doesn't line up.
fn ridged(p: Vec2, octaves: u32, seed: u32) -> f32 {
    // A turn of about 37° between octaves, as a rotation's cosine and sine.
    const TURN: Vec2 = Vec2::new(0.8, 0.6);
    let (mut sum, mut total, mut amplitude, mut weight) = (0.0, 0.0, 0.5, 1.0);
    let mut at = p;
    for octave in 0..octaves {
        let n = value_noise(at, (NO_PERIOD, NO_PERIOD), seed.wrapping_add(octave));
        let crest = 1.0 - (2.0 * n - 1.0).abs();
        let crest = crest * crest * weight;
        sum += crest * amplitude;
        total += amplitude;
        weight = (2.0 * crest).clamp(0.0, 1.0);
        amplitude *= 0.5;
        at = Vec2::new(at.x * TURN.x - at.y * TURN.y, at.x * TURN.y + at.y * TURN.x) * 2.0;
    }
    sum / total
}

/// Linear RGB from 8-bit sRGB.
fn rgb(r: u8, g: u8, b: u8) -> Vec3 {
    Vec3::new(f32::from(r), f32::from(g), f32::from(b)).map(|c| srgb_to_linear(c / 255.0))
}

/// One range's colors in the palette.
struct Band {
    /// Forest on the lower slopes, and grass and scrub higher up.
    forest: Vec3,
    meadow: Vec3,
    /// Rock, a darker stratum and the crests' pale rock.
    rock: Vec3,
    stratum: Vec3,
    crest: Vec3,
}

/// The bands, nearest first: teal-green forest and warm grey rock near; bluer, paler and less
/// saturated with distance, as the haze paints them.
fn bands() -> [Band; 3] {
    [
        Band {
            forest: rgb(38, 76, 66),
            meadow: rgb(86, 128, 78),
            rock: rgb(118, 120, 116),
            stratum: rgb(92, 96, 100),
            crest: rgb(150, 152, 148),
        },
        Band {
            forest: rgb(50, 92, 92),
            meadow: rgb(90, 130, 108),
            rock: rgb(122, 132, 142),
            stratum: rgb(100, 110, 124),
            crest: rgb(168, 174, 182),
        },
        Band {
            forest: rgb(84, 116, 132),
            meadow: rgb(110, 138, 148),
            rock: rgb(140, 152, 170),
            stratum: rgb(122, 134, 156),
            crest: rgb(186, 194, 208),
        },
    ]
}

/// The mountains' palette (color): a band of rows per range, nearest at the top. Across U, from
/// forest (0) to bare rock (1); down each band, from its crests (top) to its feet. Forest is
/// darkest low and lightens to meadow higher up. Rock lies in strata: thin green ledges where
/// plants hold on, darker beds between paler ones, the ledges thinning out toward the barest rock
/// (U 1). The highest ground pales toward the crest's color.
pub fn palette_png() -> io::Result<Vec<u8>> {
    let bands = bands();
    let height = BAND_ROWS * RANGES.len() as u32;
    let image = image::RgbImage::from_fn(PALETTE_WIDTH, height, |x, y| {
        let u = (x as f32 + 0.5) / PALETTE_WIDTH as f32;
        let band = (y / BAND_ROWS) as usize;
        let within = ((y % BAND_ROWS) as f32 + 0.5) / BAND_ROWS as f32;
        let elevation = ((1.0 - within - BAND_MARGIN) / (1.0 - 2.0 * BAND_MARGIN)).clamp(0.0, 1.0);
        let colors = &bands[band];
        let seed = 0x6e10 + band as u32;
        let green = colors.forest.lerp(colors.meadow, smoothstep(0.1, 0.8, elevation));
        // Strata: beds of uneven thickness, about 1/36 of the band tall on average, each its own
        // shade; along the tops of some, a ledge of green, more of them where the rock is less
        // bare.
        let bed =
            elevation * 36.0 + 1.4 * fbm(Vec2::new(elevation * 9.0, 0.5), (NO_PERIOD, 1), 2, seed ^ 0x1d);
        let shade = value_noise(Vec2::new(bed, 0.5), (NO_PERIOD, 1), seed);
        let mut rock = colors.rock.lerp(colors.stratum, 0.75 * smoothstep(0.3, 0.8, shade));
        let within_bed = bed - bed.floor();
        if random(bed.floor() as u32, seed ^ 0x1e) < 0.55 - 0.45 * u {
            let lip = 1.0 - smoothstep(0.0, 0.3, 1.0 - within_bed);
            rock = rock.lerp(colors.meadow * 0.85, 0.8 * lip);
        }
        let mut color = green.lerp(rock, smoothstep(0.3, 0.6, u));
        color = color.lerp(colors.crest, smoothstep(0.75, 1.0, elevation) * (0.3 + 0.5 * u));
        image::Rgb(color.to_array().map(|c| unorm8(linear_to_srgb(c.max(0.0)))))
    });
    let mut bytes = io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).map_err(io::Error::other)?;
    Ok(bytes.into_inner())
}

/// A random value in 0..1 for `id` under `seed`.
fn random(id: u32, seed: u32) -> f32 {
    hash(id ^ hash(seed)) as f32 / 4_294_967_296.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed sectors cook whole on desktop and lighter on phones, where the sidecars'
    /// LOD is the mobile cook's top level: these are the mountains' budget lines, about 102k
    /// triangles on desktop and 59k on phones, the light level within 5 m of the full one (as
    /// meshoptimizer measures it, normals counted: 0.11° from the square at the far range). The
    /// light level keeps every vertex of the sector's seams (the sidecars lock its border), so
    /// neighbors still meet on phones.
    #[test]
    fn sectors_cook_lighter_for_phones() {
        use sapphire_assets::mesh::{Mesh, Positions};
        let lookdev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/lookdev");
        let triangles =
            |mesh: &Mesh| mesh.lods[0].submeshes.iter().map(|range| range.index_count).sum::<u32>() / 3;
        let (mut desktop, mut mobile, mut worst) = (0, 0, 0.0f32);
        for chunk in chunks() {
            let source = lookdev.join(&chunk.file);
            let meta = crate::meta::read(&crate::meta::sidecar_path(&source)).unwrap();
            let settings = meta.importer.mesh().expect("a mesh's sidecar");
            let full = crate::cook::cook_mesh(&source, settings, crate::cook::Cook::Desktop).unwrap();
            let light = crate::cook::cook_mesh(&source, settings, crate::cook::Cook::Mobile).unwrap();
            assert_eq!(triangles(&full) as usize, chunk.geometry.indices.len() / 3, "{}", chunk.file);
            desktop += triangles(&full);
            mobile += triangles(&light);
            worst = worst.max(light.lods[0].error);
            // Every seam vertex, the first and last column of each row, is still drawn.
            let Positions::Float(positions) = &light.streams.positions else { panic!("float positions") };
            let drawn: std::collections::HashSet<[u32; 3]> = light.lods[0]
                .submeshes
                .iter()
                .flat_map(|range| &light.indices[range.first_index as usize..][..range.index_count as usize])
                .map(|&index| positions[index as usize].get().map(f32::to_bits))
                .collect();
            let range =
                RANGES.iter().find(|range| chunk.file.contains(&format!("/{}_", range.name))).unwrap();
            let columns = range.columns() as usize + 1;
            for row in 0..=range.rows as usize {
                for column in [0, columns - 1] {
                    let seam = chunk.geometry.positions[row * columns + column].map(f32::to_bits);
                    assert!(drawn.contains(&seam), "{} row {row} column {column} on phones", chunk.file);
                }
            }
        }
        assert!((80_000..120_000).contains(&desktop), "{desktop} triangles on desktop");
        assert!(mobile * 5 < desktop * 3, "{mobile} triangles on phones, {desktop} on desktop");
        assert!(worst < 5.0, "the light level strays {worst} m");
    }

    /// A sector's bounding box, as the renderer's caster culling sees it (a sphere round it).
    fn bounding_sphere(geometry: &Geometry) -> (Vec3, f32) {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for &p in &geometry.positions {
            min = min.min(Vec3::from(p));
            max = max.max(Vec3::from(p));
        }
        ((min + max) * 0.5, (max - min).length() * 0.5)
    }

    /// No sector's bounding sphere can enter the near cascades under any light, and only the
    /// near range's can enter desktop's farthest.
    #[test]
    fn sectors_stay_out_of_the_shadow_cascades() {
        for chunk in chunks() {
            let (center, radius) = bounding_sphere(&chunk.geometry);
            assert!(!NEAR_CASCADES.may_hold(center, radius), "{} in the near cascades", chunk.file);
            let near = chunk.file.starts_with(&format!("{MOUNTAINS_DIR}/{}_", RANGES[0].name));
            assert!(near || !FAR_CASCADE.may_hold(center, radius), "{} in the far cascade", chunk.file);
        }
        // The test itself: a sphere round the square is held, and so is one toward the light,
        // as far as the reach.
        assert!(NEAR_CASCADES.may_hold(Vec3::ZERO, 1.0));
        assert!(NEAR_CASCADES.may_hold(Vec3::new(0.0, 0.0, -330.0), 1.0));
    }

    /// Neighboring sectors share the vertices along their seam bit for bit, so no crack opens.
    #[test]
    fn neighboring_sectors_meet_without_a_crack() {
        let chunks = chunks();
        for range in &RANGES {
            let columns = range.columns() as usize + 1;
            let sectors: Vec<&Chunk> = chunks
                .iter()
                .filter(|chunk| chunk.file.starts_with(&format!("{MOUNTAINS_DIR}/{}_", range.name)))
                .collect();
            assert_eq!(sectors.len(), range.sectors as usize);
            for pair in sectors.windows(2) {
                let (left, right) = (&pair[0].geometry, &pair[1].geometry);
                for row in 0..=range.rows as usize {
                    let (a, b) = (row * columns + columns - 1, row * columns);
                    assert_eq!(left.positions[a], right.positions[b], "{} row {row}", range.name);
                    assert_eq!(left.normals[a], right.normals[b], "{} row {row}", range.name);
                }
            }
        }
    }

    /// The ranges are where their table says, sink under the plain at their edges, and rise
    /// from near to far; their triangles fit the scene's budget (the town's 217k on desktop).
    #[test]
    fn ranges_rise_from_near_to_far_within_their_budget() {
        let chunks = chunks();
        let mut triangles = 0;
        let mut tops = Vec::new();
        for range in &RANGES {
            let mut top = f32::MIN;
            for chunk in chunks.iter().filter(|chunk| chunk.file.contains(&format!("/{}_", range.name))) {
                let geometry = &chunk.geometry;
                triangles += geometry.indices.len() / 3;
                for &[x, y, z] in &geometry.positions {
                    let radius = Vec2::new(x, z).length();
                    assert!((range.inner - 1.0..=range.outer + 1.0).contains(&radius), "{}", chunk.file);
                    assert!((FOOT - 1.0..=range.top).contains(&y), "{} at {y} m", chunk.file);
                    top = top.max(y);
                }
                // The front row stands under the plain.
                assert!(geometry.positions[..range.columns() as usize].iter().all(|p| p[1] < 0.0));
            }
            tops.push(top);
        }
        assert!(tops.windows(2).all(|pair| pair[0] < pair[1]), "{tops:?}");
        assert!((60_000..130_000).contains(&triangles), "{triangles} triangles");
    }

    #[test]
    fn normals_face_up_and_texture_coordinates_stay_in_their_band() {
        for chunk in chunks() {
            let band =
                RANGES.iter().position(|range| chunk.file.contains(&format!("/{}_", range.name))).unwrap();
            for (normal, uv) in chunk.geometry.normals.iter().zip(&chunk.geometry.uvs) {
                assert!(normal[1] > 0.0 && (Vec3::from(*normal).length() - 1.0).abs() < 1e-5);
                let v = uv[1] * RANGES.len() as f32 - band as f32;
                assert!(
                    (BAND_MARGIN - 1e-5..=1.0 - BAND_MARGIN + 1e-5).contains(&v),
                    "{}: {uv:?}",
                    chunk.file
                );
                assert!((0.0..=1.0).contains(&uv[0]));
            }
        }
    }
}
