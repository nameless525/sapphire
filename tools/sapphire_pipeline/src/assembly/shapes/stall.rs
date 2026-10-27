//! `#stall`: a market stall, with its counter, shelves, sign and goods.
//!
//! A stall ([`Stall`]) stands on four square timber posts on stone feet, joined by beams round
//! their tops, its front toward +Z. Its awning ([`Awning`]) is either tiled, a roof built as
//! `#roof`'s and sized to the frame (its eaves sweeping up at the corners, a ceiling of rafters
//! under it), or cloth ([`ClothAwning`]): draped over the front and back beams and a ridge pole
//! on king posts (or, for a shed, falling from a pole on the back posts to the front beam),
//! sagging between them, its corners swept up like the roofs' and a scalloped valance hanging
//! from its eaves. A counter of boards under a timber top runs across its front, shelves along a
//! boarded back, and a sign may hang from the front beam on cords. Its goods ([`Goods`]) stand
//! on the counter, the shelves and the ground before it.
//!
//! A stall's paper lanterns aren't part of it: they're `#lantern`s placed with it, under its
//! front beam.

use std::f32::consts::PI;

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

use super::goods::{Goods, Place, Surface, validate_goods};
use super::roof::{Ridge, Roof, RoofKind, Tiles};
use super::{Detail, Frame, Mesh, Part, check, finite, smoothstep};
use crate::assembly::ShapeUv;

/// Beams' width, as a share of the posts'.
const BEAM_WIDTH: f32 = 0.8;
/// Stone feet's width, as a share of the posts'.
const FOOT_WIDTH: f32 = 1.4;
/// How far behind the posts' fronts the counter's front stands, as a share of their width:
/// between their faces, so it shares a plane with neither.
const COUNTER_SET: f32 = 0.55;
/// Shelf boards' thickness, and the back's boards'.
const SHELF: f32 = 0.04;
const PANEL: f32 = 0.03;
/// The least room left between the counter and the shelves, for the seller.
const AISLE: f32 = 0.15;
/// The cloth's thickness and its valance's.
const CLOTH: f32 = 0.02;
const VALANCE: f32 = 0.012;
/// A cloth awning's poles: the ridge pole, its king posts and a shed's back pole.
const POLE: f32 = 0.06;
/// Rows of a cloth awning's slopes from the eaves to where it rests on the beams, and between
/// there and its ridge, in full detail (light detail takes about half).
const CLOTH_EAVE_ROWS: usize = 3;
const CLOTH_SAG_ROWS: usize = 5;
/// Columns across a cloth awning, about this many meters apart in full detail.
const CLOTH_COLUMN: f32 = 0.25;
/// Points along each of a valance's scallops in full detail.
const SCALLOP_POINTS: usize = 6;
/// A sign's timber frame: its width round the board and its thickness; the board's thickness;
/// the cords it hangs from.
const SIGN_FRAME: [f32; 2] = [0.035, 0.05];
const SIGN_BOARD: f32 = 0.03;
const CORD: f32 = 0.012;

/// A market stall (`#stall`); see the module docs. It stands on its origin, its front toward
/// +Z, its posts' outer faces `size` apart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Stall {
    /// The frame's footprint to its posts' outer faces: width (X) and depth (Z), meters.
    pub size: [f32; 2],
    /// From the ground to the frame's top, where the awning rests.
    pub height: f32,
    /// The posts' width (they're square) and their stone feet's height (none at 0).
    pub posts: [f32; 2],
    /// The beams' height round the frame's top.
    pub beam: f32,
    /// The counter across its front.
    pub counter: Option<Counter>,
    pub shelves: Shelves,
    pub awning: Awning,
    /// A board hanging from the front beam.
    pub sign: Option<Sign>,
    pub goods: Vec<Goods>,
}

impl Default for Stall {
    fn default() -> Self {
        Self {
            size: [2.6, 1.6],
            height: 2.3,
            posts: [0.12, 0.08],
            beam: 0.14,
            counter: Some(Counter::default()),
            shelves: Shelves::default(),
            awning: Awning::default(),
            sign: None,
            goods: Vec::new(),
        }
    }
}

/// A stall's counter, between its front posts.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Counter {
    /// Its top's height, and its depth from front to back, meters.
    pub height: f32,
    pub depth: f32,
    /// Its top board's thickness, and how far the board overhangs its front.
    pub top: f32,
    pub overhang: f32,
}

impl Default for Counter {
    fn default() -> Self {
        Self { height: 0.9, depth: 0.5, top: 0.05, overhang: 0.04 }
    }
}

/// A stall's shelves along its back, between its back posts.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shelves {
    /// Boards spaced evenly up to the beams (none at 0), and how deep they are.
    pub count: u32,
    pub depth: f32,
    /// Whether boards close the back between the back posts.
    pub back: bool,
}

impl Default for Shelves {
    fn default() -> Self {
        Self { count: 2, depth: 0.3, back: true }
    }
}

/// A stall's awning.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Awning {
    /// Tiles: a roof on the frame.
    Tiles(TiledAwning),
    /// Cloth over poles.
    Cloth(ClothAwning),
}

impl Default for Awning {
    fn default() -> Self {
        Self::Tiles(TiledAwning::default())
    }
}

/// A tiled awning: a `#roof` over the frame, its numbers as `#roof`'s (its size is the
/// frame's), smaller by default as a stall's is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TiledAwning {
    /// A hip, hip-and-gable or gable roof.
    pub kind: RoofKind,
    pub overhang: f32,
    pub barge: f32,
    pub rise: f32,
    pub curve: f32,
    pub sweep: f32,
    pub sweep_reach: f32,
    pub thickness: f32,
    pub tiles: Tiles,
    pub ridge: Ridge,
}

impl Default for TiledAwning {
    fn default() -> Self {
        Self {
            kind: RoofKind::Hip,
            overhang: 0.5,
            barge: 0.3,
            rise: 0.85,
            curve: 0.45,
            sweep: 0.28,
            sweep_reach: 0.9,
            thickness: 0.15,
            tiles: Tiles { period: 0.24, depth: 0.035 },
            ridge: Ridge { height: 0.18, width: 0.16, ends: 0.4 },
        }
    }
}

impl TiledAwning {
    /// The awning as a roof over a frame `size` meters across.
    fn roof(&self, size: [f32; 2]) -> Roof {
        Roof {
            size,
            kind: self.kind,
            overhang: self.overhang,
            barge: self.barge,
            rise: self.rise,
            curve: self.curve,
            sweep: self.sweep,
            sweep_reach: self.sweep_reach,
            thickness: self.thickness,
            tiles: self.tiles,
            ridge: self.ridge,
        }
    }
}

/// A cloth awning (see the module docs): `overhang` past the frame's front and back (half that
/// past its ends), `rise` from the beams to the ridge pole (a shed's back pole), sagging `sag`
/// between them; its corners sweep up `sweep`, fading over `sweep_reach` along the eaves. Its
/// valance hangs `valance[0]` to its scallops' tips, each scallop `valance[1]` wide (none at a
/// drop of 0).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClothAwning {
    pub kind: ClothKind,
    pub overhang: f32,
    pub rise: f32,
    pub sag: f32,
    pub sweep: f32,
    pub sweep_reach: f32,
    pub valance: [f32; 2],
}

impl Default for ClothAwning {
    fn default() -> Self {
        Self {
            kind: ClothKind::Gable,
            overhang: 0.35,
            rise: 0.5,
            sag: 0.05,
            sweep: 0.16,
            sweep_reach: 0.6,
            valance: [0.2, 0.24],
        }
    }
}

/// A cloth awning's form.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClothKind {
    /// Two slopes from a ridge pole over the middle, a valance along both eaves.
    #[default]
    Gable,
    /// One slope from a pole on the back posts down to the front, a valance along the front.
    Shed,
}

/// A board hanging from a stall's front beam on two cords: blank, in a timber frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sign {
    /// The board's width and height, meters.
    pub size: [f32; 2],
    /// Its middle along the beam, from the stall's middle.
    pub at: f32,
    /// How far under the beam it hangs.
    pub drop: f32,
}

impl Default for Sign {
    fn default() -> Self {
        Self { size: [0.9, 0.3], at: 0.0, drop: 0.06 }
    }
}

impl Stall {
    pub fn validate(&self) -> Result<(), String> {
        let [width, depth] = self.size;
        let [post, foot] = self.posts;
        let mut numbers = vec![width, depth, self.height, post, foot, self.beam, self.shelves.depth];
        if let Some(counter) = self.counter {
            numbers.extend([counter.height, counter.depth, counter.top, counter.overhang]);
        }
        if let Some(sign) = self.sign {
            numbers.extend([sign.size[0], sign.size[1], sign.at, sign.drop]);
        }
        check(finite(&numbers), || "a stall's numbers must be finite".to_owned())?;
        check(width > 0.0 && depth > 0.0 && self.height > 0.0 && post > 0.0 && self.beam > 0.0, || {
            format!("a stall's size {:?}, height, posts and beams must be positive", self.size)
        })?;
        check(post < 0.25 * width.min(depth), || {
            format!("a stall's posts {post} m wide leave no room between them in {:?}", self.size)
        })?;
        check(foot >= 0.0 && foot < 0.25 * self.height && self.beam < 0.25 * self.height, || {
            "a stall's feet and beams must be short beside its height".to_owned()
        })?;
        let plan = self.plan();
        if let Some(counter) = self.counter {
            check(counter.depth > 0.0 && counter.top > 0.0 && counter.top < counter.height, || {
                "a counter needs a depth and a top board thinner than it is high".to_owned()
            })?;
            check(counter.height > 0.3 && counter.height < plan.beam_low - 0.3, || {
                format!("a counter {} m high must stand well under the beams", counter.height)
            })?;
            check(counter.overhang >= 0.0 && counter.overhang < 0.5 * post, || {
                "a counter's top overhangs its front by less than half a post".to_owned()
            })?;
        }
        let shelves = self.shelves;
        check(shelves.count <= 6 && (shelves.count == 0 || shelves.depth > 0.0), || {
            "a stall takes up to 6 shelves, each with a depth".to_owned()
        })?;
        // From the back (its boards, its shelves' front edge) to the counter's back.
        let back = match (shelves.count, shelves.back) {
            (0, false) => -plan.inner.y,
            (0, true) => -plan.middle.y + 0.5 * PANEL,
            _ => plan.shelf_front(),
        };
        let front = match self.counter {
            Some(counter) => plan.counter_front - counter.depth,
            None => plan.inner.y,
        };
        check(back + AISLE <= front, || {
            "the counter and the shelves must leave an aisle between them".to_owned()
        })?;
        match self.awning {
            Awning::Tiles(tiles) => {
                check(!matches!(tiles.kind, RoofKind::Skirt(_)), || {
                    "a stall's tiled awning is a hip, hip-and-gable or gable roof".to_owned()
                })?;
                tiles.roof(self.size).validate()?;
            }
            Awning::Cloth(cloth) => {
                let ClothAwning { overhang, rise, sag, sweep, sweep_reach, valance, .. } = cloth;
                check(finite(&[overhang, rise, sag, sweep, sweep_reach, valance[0], valance[1]]), || {
                    "a cloth awning's numbers must be finite".to_owned()
                })?;
                check(overhang > 0.0 && rise > POLE && sweep >= 0.0 && sweep_reach > 0.0, || {
                    "a cloth awning needs an overhang, a rise over its pole, and a sweep reach".to_owned()
                })?;
                check((0.0..=0.25 * rise).contains(&sag), || {
                    format!("a cloth awning sags at most a quarter of its rise, not {sag} m")
                })?;
                let [drop, scallop] = valance;
                check(drop >= 0.0 && (drop == 0.0 || scallop >= 0.05), || {
                    "a valance needs a drop of 0 or more, and scallops 5 cm wide or more".to_owned()
                })?;
                let drape = Drape::new(self, cloth, Detail::Full);
                check(drape.underside(0.0, 0.0) - drop > 0.5 * self.height, || {
                    "a cloth awning's eaves and valance must hang in the stall's upper half".to_owned()
                })?;
            }
        }
        if let Some(sign) = self.sign {
            let [sign_width, sign_height] = sign.size;
            check(sign_width > 0.0 && sign_height > 0.0 && sign.drop >= 0.0, || {
                "a sign needs a size, and a drop of 0 or more".to_owned()
            })?;
            check(sign.at.abs() + 0.5 * sign_width <= plan.inner.x, || {
                "a sign must fit along the front beam between the posts".to_owned()
            })?;
            let floor = self.counter.map_or(0.5, |counter| counter.height + 0.1);
            check(plan.beam_low - sign.drop - sign_height > floor, || {
                "a sign must hang clear over the counter".to_owned()
            })?;
        }
        validate_goods(&self.goods, &|surface| self.place(surface), (-plan.footprint(), plan.footprint()))
    }

    /// The stall's numbers, worked out once.
    fn plan(&self) -> StallPlan {
        let [post, _] = self.posts;
        let half = Vec2::from(self.size) * 0.5;
        StallPlan {
            half,
            post,
            middle: half - Vec2::splat(0.5 * post),
            inner: half - Vec2::splat(post),
            beam_low: self.height - self.beam,
            counter_front: half.y - COUNTER_SET * post,
            shelves: self.shelves,
        }
    }

    /// Where goods on `surface` stand.
    fn place(&self, surface: Surface) -> Result<Place, String> {
        let plan = self.plan();
        match surface {
            Surface::Counter => {
                let counter = self.counter.ok_or("the stall has no counter")?;
                let (back, front) =
                    (plan.counter_front - counter.depth, plan.counter_front + counter.overhang);
                Ok(Place {
                    middle: Vec3::new(0.0, counter.height, 0.5 * (back + front)),
                    half: Vec2::new(plan.inner.x, 0.5 * (front - back)),
                    room: plan.beam_low - counter.height,
                })
            }
            Surface::Shelf(index) => {
                check(index < self.shelves.count, || {
                    format!(
                        "the stall has no shelf {index}: its shelves count from 0 to {}",
                        self.shelves.count
                    )
                })?;
                let top = plan.shelf_top(index);
                let above = if index + 1 < self.shelves.count {
                    plan.shelf_top(index + 1) - SHELF
                } else {
                    plan.beam_low
                };
                let back = plan.shelf_front() - self.shelves.depth;
                Ok(Place {
                    middle: Vec3::new(0.0, top, back + 0.5 * self.shelves.depth),
                    half: Vec2::new(plan.inner.x, 0.5 * self.shelves.depth),
                    room: above - top,
                })
            }
            Surface::Ground => Ok(Place::ground()),
            Surface::Bed => Err("a stall has no bed".to_owned()),
        }
    }

    pub(super) fn build(&self, mesh: &mut Mesh, detail: Detail) -> Result<(), String> {
        let plan = self.plan();
        let [post, foot] = self.posts;
        let h = self.height;
        let tiled = matches!(self.awning, Awning::Tiles(_));
        let shed = matches!(self.awning, Awning::Cloth(ClothAwning { kind: ClothKind::Shed, .. }));
        let drape = match self.awning {
            Awning::Cloth(cloth) => Some(Drape::new(self, cloth, detail)),
            Awning::Tiles(_) => None,
        };
        // Posts on their feet: under a tiled awning their tops are under its ceiling; a shed's
        // back posts reach up to its back pole.
        for (x, z) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            let middle = Vec3::new(x * plan.middle.x, 0.0, z * plan.middle.y);
            if foot > 0.0 {
                let reach = Vec3::new(0.5 * FOOT_WIDTH * post, 0.0, 0.5 * FOOT_WIDTH * post);
                mesh.cuboid(Part::Plinth, middle - reach, middle + reach + Vec3::Y * foot, &[-Vec3::Y]);
            }
            let (top, skip): (f32, &[Vec3]) = match &drape {
                Some(drape) if shed && z < 0.0 => (drape.pole_top - POLE, &[-Vec3::Y, Vec3::Y]),
                _ if tiled => (h, &[-Vec3::Y, Vec3::Y]),
                _ => (h, &[-Vec3::Y]),
            };
            let reach = Vec3::new(0.5 * post, 0.0, 0.5 * post);
            mesh.cuboid(Part::Timber, middle - reach + Vec3::Y * foot, middle + reach + Vec3::Y * top, skip);
        }
        // Beams between the posts: front and back along X, the sides along Z.
        let beam = 0.5 * BEAM_WIDTH * post;
        let top_skip: &[Vec3] = if tiled { &[Vec3::Y] } else { &[] };
        for z in [1.0, -1.0] {
            let z = z * plan.middle.y;
            let skip: Vec<Vec3> = [Vec3::X, -Vec3::X].into_iter().chain(top_skip.iter().copied()).collect();
            mesh.cuboid(
                Part::Timber,
                Vec3::new(-plan.inner.x, plan.beam_low, z - beam),
                Vec3::new(plan.inner.x, h, z + beam),
                &skip,
            );
        }
        for x in [1.0, -1.0] {
            let x = x * plan.middle.x;
            let skip: Vec<Vec3> = [Vec3::Z, -Vec3::Z].into_iter().chain(top_skip.iter().copied()).collect();
            mesh.cuboid(
                Part::Timber,
                Vec3::new(x - beam, plan.beam_low, -plan.inner.y),
                Vec3::new(x + beam, h, plan.inner.y),
                &skip,
            );
        }
        if let Some(counter) = self.counter {
            let front = plan.counter_front;
            let board = counter.height - counter.top;
            mesh.cuboid(
                Part::Boards,
                Vec3::new(-plan.inner.x, 0.0, front - counter.depth),
                Vec3::new(plan.inner.x, board, front),
                &[-Vec3::Y, Vec3::Y],
            );
            mesh.cuboid(
                Part::Timber,
                Vec3::new(-plan.inner.x, board, front - counter.depth),
                Vec3::new(plan.inner.x, counter.height, front + counter.overhang),
                &[],
            );
        }
        self.build_shelves(mesh, &plan);
        if let Some(sign) = self.sign {
            build_sign(mesh, &plan, sign);
        }
        match (self.awning, &drape) {
            (Awning::Tiles(tiles), _) => {
                // A ceiling of rafters under the roof, which it meets at the walls' line.
                let [hx, hz] = plan.half.to_array();
                let corners = [
                    Vec3::new(-hx, h, -hz),
                    Vec3::new(hx, h, -hz),
                    Vec3::new(hx, h, hz),
                    Vec3::new(-hx, h, hz),
                ];
                mesh.quad(Part::Eaves, corners, -Vec3::Y, [0.7; 4]);
                tiles.roof(self.size).build(mesh, Vec3::Y * h, detail)?;
            }
            (Awning::Cloth(_), Some(drape)) => drape.build(mesh, &plan, self.height)?,
            (Awning::Cloth(_), None) => {}
        }
        for goods in &self.goods {
            let place = self.place(goods.on)?;
            goods.build(mesh, &place, detail);
        }
        Ok(())
    }

    fn build_shelves(&self, mesh: &mut Mesh, plan: &StallPlan) {
        let shelves = self.shelves;
        if shelves.back {
            // Boards between the back posts, under the back beam: the inside face, shaded as
            // the stall's inside is, and the outside.
            let (x, y) = (plan.inner.x, plan.beam_low);
            for (z, out, shade) in [
                (-plan.middle.y + 0.5 * PANEL, Vec3::Z, [0.55, 0.55, 0.75, 0.75]),
                (-plan.middle.y - 0.5 * PANEL, -Vec3::Z, [1.0; 4]),
            ] {
                let corners =
                    [Vec3::new(-x, 0.0, z), Vec3::new(x, 0.0, z), Vec3::new(x, y, z), Vec3::new(-x, y, z)];
                mesh.quad(Part::Back, corners, out, shade);
            }
        }
        let back = plan.shelf_front() - shelves.depth;
        for index in 0..shelves.count {
            let top = plan.shelf_top(index);
            let skip: &[Vec3] = if shelves.back { &[-Vec3::Z] } else { &[] };
            mesh.cuboid(
                Part::Timber,
                Vec3::new(-plan.inner.x, top - SHELF, back),
                Vec3::new(plan.inner.x, top, plan.shelf_front()),
                skip,
            );
        }
    }
}

/// A stall's numbers.
struct StallPlan {
    /// Half its footprint (X, Z) to the posts' outer faces, to their middles and to their inner
    /// faces.
    half: Vec2,
    middle: Vec2,
    inner: Vec2,
    post: f32,
    /// The beams' undersides.
    beam_low: f32,
    /// The counter's front face.
    counter_front: f32,
    shelves: Shelves,
}

impl StallPlan {
    /// The shelves' front edge: their depth out from the back's boards.
    fn shelf_front(&self) -> f32 {
        -self.middle.y + 0.5 * PANEL + self.shelves.depth
    }

    /// Shelf `index`'s top: spaced evenly up to the beams.
    fn shelf_top(&self, index: u32) -> f32 {
        self.beam_low * (index + 1) as f32 / (self.shelves.count + 1) as f32
    }

    /// The footprint goods on the ground keep out of: the frame and its feet, either way.
    fn footprint(&self) -> Vec2 {
        self.half + Vec2::splat(0.5 * (FOOT_WIDTH - 1.0) * self.post)
    }
}

/// A sign under the front beam: a blank board in a timber frame, on two cords.
fn build_sign(mesh: &mut Mesh, plan: &StallPlan, sign: Sign) {
    let [width, height] = sign.size;
    let [frame, thickness] = SIGN_FRAME;
    let top = plan.beam_low - sign.drop;
    let middle = Vec3::new(sign.at, top - 0.5 * height, plan.middle.y);
    let half = Vec3::new(0.5 * width, 0.5 * height, 0.5 * thickness);
    // The board, its edges inside the frame.
    let board = Vec3::new(half.x - 0.005, half.y - 0.005, 0.5 * SIGN_BOARD);
    mesh.cuboid(Part::Sign, middle - board, middle + board, &[]);
    // Stiles the full height, rails between them.
    for side in [1.0, -1.0] {
        let x = middle.x + side * (half.x - 0.5 * frame);
        let reach = Vec3::new(0.5 * frame, half.y, half.z);
        mesh.cuboid(
            Part::Timber,
            Vec3::new(x, middle.y, middle.z) - reach,
            Vec3::new(x, middle.y, middle.z) + reach,
            &[],
        );
        let y = middle.y + side * (half.y - 0.5 * frame);
        let reach = Vec3::new(half.x - frame, 0.5 * frame, half.z);
        mesh.cuboid(
            Part::Timber,
            Vec3::new(middle.x, y, middle.z) - reach,
            Vec3::new(middle.x, y, middle.z) + reach,
            &[Vec3::X, -Vec3::X],
        );
    }
    if sign.drop > 0.0 {
        for side in [1.0, -1.0] {
            let x = middle.x + side * (half.x - 0.5 * frame);
            let reach = Vec3::new(0.5 * CORD, 0.0, 0.5 * CORD);
            mesh.cuboid(
                Part::Frame,
                Vec3::new(x, top, middle.z) - reach,
                Vec3::new(x, plan.beam_low, middle.z) + reach,
                &[-Vec3::Y, Vec3::Y],
            );
        }
    }
}

/// A cloth awning's numbers over its stall, worked out once. The cloth's slopes run from their
/// eave (`s` 0) to the ridge (`s` 1); a shed's one slope from its front eave to its back edge.
struct Drape {
    cloth: ClothAwning,
    detail: Detail,
    /// Half the cloth's extent along X and Z.
    half: Vec2,
    /// The run of each slope.
    run: f32,
    /// The frame's top, where the cloth rests on the front (and back) beam's outer edge, at `s`
    /// `rest`; the ridge pole (or a shed's back pole) holds it at `s` `high`.
    top: f32,
    rest: f32,
    high: f32,
    /// The pole's top and its middle along Z.
    pole_top: f32,
    pole_z: f32,
}

impl Drape {
    fn new(stall: &Stall, cloth: ClothAwning, detail: Detail) -> Self {
        let plan = stall.plan();
        let half = Vec2::new(plan.half.x + 0.5 * cloth.overhang, plan.half.y + cloth.overhang);
        // It rests on the front beam's outer top edge.
        let rest_z = plan.middle.y + 0.5 * BEAM_WIDTH * plan.post;
        let (run, high, pole_z) = match cloth.kind {
            ClothKind::Gable => (half.y, 1.0, 0.0),
            // On the back pole's front edge, the back posts' middles under the pole.
            ClothKind::Shed => {
                let run = 2.0 * half.y;
                (run, (half.y + plan.middle.y - 0.5 * POLE) / run, -plan.middle.y)
            }
        };
        let mut drape = Self {
            cloth,
            detail,
            half,
            run,
            top: stall.height,
            rest: (half.y - rest_z) / run,
            high,
            pole_top: 0.0,
            pole_z,
        };
        // The pole's top stands clear under the cloth: under the ridge's crease at the pole's
        // edges, or at a shed's pole's front edge.
        drape.pole_top = match cloth.kind {
            ClothKind::Gable => drape.underside(0.0, 1.0 - 0.5 * POLE / run),
            ClothKind::Shed => drape.underside(0.0, high),
        } - 0.003;
        drape
    }

    /// The cloth's underside at `x` and `s`.
    fn underside(&self, x: f32, s: f32) -> f32 {
        let cloth = &self.cloth;
        let climb = cloth.rise / (self.high - self.rest);
        let mut y = self.top + climb * (s - self.rest);
        if s > self.rest && s < self.high {
            y -= cloth.sag * (PI * (s - self.rest) / (self.high - self.rest)).sin();
        }
        if s < self.rest && cloth.sweep > 0.0 {
            let along = 1.0 - smoothstep(0.0, cloth.sweep_reach, self.half.x - x.abs());
            y += cloth.sweep * along * (1.0 - s / self.rest).powi(2);
        }
        y
    }

    /// Z at `s` on the slope facing `side` (+1 the front, −1 the back).
    fn z(&self, s: f32, side: f32) -> f32 {
        side * (self.half.y - s * self.run)
    }

    /// The cloth's upward normal at `x` and `s` on the slope facing `side`.
    fn normal(&self, x: f32, s: f32, side: f32) -> Vec3 {
        let (dx, ds) = (1e-3, 1e-3);
        let dy_dx = (self.underside(x + dx, s) - self.underside(x - dx, s)) / (2.0 * dx);
        let (s0, s1) = ((s - ds).max(0.0), (s + ds).min(1.0));
        let dy_ds = (self.underside(x, s1) - self.underside(x, s0)) / (s1 - s0);
        // Z falls by `run` per unit of s on the front slope, rises on the back.
        let dy_dz = -dy_ds * side / self.run;
        Vec3::new(-dy_dx, 1.0, -dy_dz).normalize()
    }

    /// The slopes: their sides (+1 the front) and whether each has a valance.
    fn slopes(&self) -> Vec<(f32, bool)> {
        let valance = self.cloth.valance[0] > 0.0;
        match self.cloth.kind {
            ClothKind::Gable => vec![(1.0, valance), (-1.0, valance)],
            ClothKind::Shed => vec![(1.0, valance)],
        }
    }

    /// Columns across the cloth.
    fn columns(&self) -> Vec<f32> {
        let spacing = if self.detail == Detail::Light { 2.0 * CLOTH_COLUMN } else { CLOTH_COLUMN };
        let count = (2.0 * self.half.x / spacing).ceil().max(2.0) as usize;
        (0..=count).map(|k| self.half.x * (2.0 * k as f32 / count as f32 - 1.0)).collect()
    }

    /// Rows up a slope: the eave, the valance's inner face, the eaves band to the beam, the sag
    /// to the pole, and a shed's back overhang.
    fn rows(&self) -> Vec<f32> {
        let light = self.detail == Detail::Light;
        let (eave_rows, sag_rows) =
            if light { (CLOTH_EAVE_ROWS - 1, CLOTH_SAG_ROWS / 2) } else { (CLOTH_EAVE_ROWS, CLOTH_SAG_ROWS) };
        let mut rows = vec![0.0, 1.0];
        if self.cloth.valance[0] > 0.0 {
            rows.push(VALANCE / self.run);
        }
        rows.extend((1..=eave_rows).map(|k| self.rest * k as f32 / eave_rows as f32));
        rows.extend((1..=sag_rows).map(|k| self.rest + (self.high - self.rest) * k as f32 / sag_rows as f32));
        rows.sort_by(f32::total_cmp);
        rows.dedup_by(|a, b| (*a - *b).abs() < 1e-5);
        rows
    }

    fn build(&self, mesh: &mut Mesh, plan: &StallPlan, height: f32) -> Result<(), String> {
        let columns = self.columns();
        let rows = self.rows();
        for (side, valance) in self.slopes() {
            let point =
                |x: f32, s: f32, over: f32| Vec3::new(x, self.underside(x, s) + over, self.z(s, side));
            // The top and the underside, cell by cell.
            for i in 0..columns.len() - 1 {
                for j in 0..rows.len() - 1 {
                    let cell = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
                    for (over, up, shade) in [(CLOTH, 1.0, 1.0), (0.0, -1.0, 0.8)] {
                        let corners = cell.map(|(i, j)| {
                            let (x, s) = (columns[i], rows[j]);
                            let uv = Vec2::new(x, s * self.run);
                            (point(x, s, over), self.normal(x, s, side) * up, uv, shade)
                        });
                        smooth_quad(mesh, Part::Cloth, corners);
                    }
                }
            }
            // The eave's edge, the ends' and a shed's back edge.
            let edge = |mesh: &mut Mesh, a: (f32, f32), b: (f32, f32), out: Vec3| {
                let corners = [
                    point(a.0, a.1, 0.0),
                    point(b.0, b.1, 0.0),
                    point(b.0, b.1, CLOTH),
                    point(a.0, a.1, CLOTH),
                ];
                mesh.quad(Part::Cloth, corners, out, [1.0; 4]);
            };
            for pair in columns.windows(2) {
                edge(mesh, (pair[0], 0.0), (pair[1], 0.0), Vec3::Z * side);
                if self.cloth.kind == ClothKind::Shed {
                    edge(mesh, (pair[0], 1.0), (pair[1], 1.0), -Vec3::Z);
                }
            }
            for pair in rows.windows(2) {
                for x in [self.half.x, -self.half.x] {
                    edge(mesh, (x, pair[0]), (x, pair[1]), Vec3::X * x.signum());
                }
            }
            if valance {
                self.valance(mesh, &columns, side)?;
            }
        }
        // The poles: a gable's ridge pole on king posts standing on the side beams, a shed's
        // back pole on its back posts.
        let pole = Vec3::new(plan.half.x, 0.0, 0.5 * POLE);
        let pole_low = self.pole_top - POLE;
        mesh.cuboid(
            Part::Timber,
            Vec3::new(-pole.x, pole_low, self.pole_z - pole.z),
            Vec3::new(pole.x, self.pole_top, self.pole_z + pole.z),
            &[],
        );
        if self.cloth.kind == ClothKind::Gable {
            let king = 0.5 * BEAM_WIDTH * plan.post;
            for x in [plan.middle.x, -plan.middle.x] {
                mesh.cuboid(
                    Part::Timber,
                    Vec3::new(x - king, height, -king),
                    Vec3::new(x + king, pole_low, king),
                    &[-Vec3::Y, Vec3::Y],
                );
            }
        }
        Ok(())
    }

    /// The valance along the eave facing `side`: a strip of cloth hanging from the underside's
    /// edge, scalloped along its bottom, its inside meeting the underside along the row a
    /// valance's thickness in.
    fn valance(&self, mesh: &mut Mesh, columns: &[f32], side: f32) -> Result<(), String> {
        let [drop, scallop] = self.cloth.valance;
        let width = 2.0 * self.half.x;
        let count = (width / scallop).round().max(1.0) as usize;
        let points = if self.detail == Detail::Light { SCALLOP_POINTS / 2 } else { SCALLOP_POINTS };
        let depth = (0.4 * width / count as f32).min(0.6 * drop);
        // Along the bottom: each scallop's cusps and its rounded tip between.
        let bottom: Vec<(f32, f32)> = (0..=count * points)
            .map(|k| {
                let x = width * k as f32 / (count * points) as f32 - self.half.x;
                let phase = (k % points) as f32 / points as f32;
                let y = self.underside(x, 0.0) - (drop - depth) - depth * (PI * phase).sin();
                (x, y)
            })
            .collect();
        let inner = VALANCE / self.run;
        let (outer_z, inner_z) = (self.z(0.0, side), self.z(inner, side));
        // Each face in its plane, X to the right seen from its front.
        for (z, s, facing, shade) in [(outer_z, 0.0, side, 1.0), (inner_z, inner, -side, 0.8)] {
            let frame = Frame { origin: Vec3::Z * z, right: Vec3::X * facing, up: Vec3::Y };
            let to_2d = |x: f32, y: f32| Vec2::new(x * facing, y);
            let mut outline: Vec<Vec2> = bottom.iter().map(|&(x, y)| to_2d(x, y)).collect();
            outline.extend(columns.iter().rev().map(|&x| to_2d(x, self.underside(x, s))));
            mesh.fill(Part::Valance, &frame, &outline, &[], &|_| shade)?;
        }
        // Its scalloped bottom edge, facing down and out, and its ends.
        for pair in bottom.windows(2) {
            let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
            let out = Vec3::new(y1 - y0, -(x1 - x0), 0.0);
            let corners = [
                Vec3::new(x0, y0, outer_z),
                Vec3::new(x1, y1, outer_z),
                Vec3::new(x1, y1, inner_z),
                Vec3::new(x0, y0, inner_z),
            ];
            mesh.quad(Part::Valance, corners, out, [0.8; 4]);
        }
        for (x, y) in [bottom[0], bottom[bottom.len() - 1]] {
            let corners = [
                Vec3::new(x, y, outer_z),
                Vec3::new(x, y, inner_z),
                Vec3::new(x, self.underside(x, inner), inner_z),
                Vec3::new(x, self.underside(x, 0.0), outer_z),
            ];
            mesh.quad(Part::Valance, corners, Vec3::X * x.signum(), [0.9; 4]);
        }
        Ok(())
    }
}

/// A quad of corners (position, normal, texture coordinates in meters, occlusion), in order
/// round it, as two triangles wound to face its normals.
fn smooth_quad(mesh: &mut Mesh, part: Part, corners: [(Vec3, Vec3, Vec2, f32); 4]) {
    for tri in [[0, 1, 2], [0, 2, 3]] {
        let mut tri = tri;
        let p = tri.map(|k| corners[k].0);
        let normal = tri.iter().map(|&k| corners[k].1).sum::<Vec3>();
        if (p[1] - p[0]).cross(p[2] - p[0]).dot(normal) < 0.0 {
            tri.swap(1, 2);
        }
        mesh.triangle(
            part,
            tri.map(|k| corners[k].0),
            tri.map(|k| corners[k].1),
            ShapeUv::Meters(tri.map(|k| corners[k].2)),
            tri.map(|k| corners[k].3),
        );
    }
}
