//! Parametric builtins' parameters (M6 T4): East Asian architecture, and a market's stalls and carts, as a
//! placement's `shape` gives them. The pipeline's `assembly::shapes` builds each from these numbers and
//! checks them, and its module docs say what each does to the geometry.

use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

/// A parametric builtin, placed by a placement's `shape`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
pub enum Shape {
    Roof(Roof),
    Hall(Hall),
    Wall(Wall),
    Column(Column),
    Lantern(Lantern),
    Stall(Stall),
    Cart(Cart),
}

impl Shape {
    /// The builtin's name, as rules and allowances name its placements.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Roof(_) => "#roof",
            Self::Hall(_) => "#hall",
            Self::Wall(_) => "#wall",
            Self::Column(_) => "#column",
            Self::Lantern(_) => "#lantern",
            Self::Stall(_) => "#stall",
            Self::Cart(_) => "#cart",
        }
    }
}

/// A tiled roof (`#roof`); see the module docs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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

/// A side of a building, by the way it faces: the front faces +Z.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
pub enum Side {
    #[default]
    Front,
    Right,
    Back,
    Left,
}

impl Side {
    pub const ALL: [Self; 4] = [Self::Front, Self::Right, Self::Back, Self::Left];
}

/// What an opening is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
pub enum OpeningKind {
    /// A rectangular window in a timber frame, its lattice before paper.
    Window,
    /// A round window in a stone ring.
    Moon,
    /// A rectangular doorway from the ground in a timber frame, a door leaf in it.
    Door,
    /// A round doorway from the ground (a moon gate) in a stone ring: `size` is its circle's
    /// width and the height of its top, the circle cut by the ground when that is less.
    Gate,
}

/// An opening in a side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
#[serde(deny_unknown_fields)]
pub struct Opening {
    /// The side it's in (a freestanding wall's openings go through it from its front).
    #[serde(default)]
    pub side: Side,
    pub kind: OpeningKind,
    /// Its middle along the side (meters right of the side's middle, seen from outside) and its
    /// foot's height (doors and gates stand on the ground: 0).
    #[serde(default)]
    pub at: [f32; 2],
    /// Its width and height; a moon window's are its diameter.
    pub size: [f32; 2],
    /// Lattice bars, upright and across (windows and moon windows).
    #[serde(default)]
    pub lattice: [u32; 2],
}

/// The stone base walls stand on: its height and how far it stands proud of the plaster (no
/// plinth at height 0).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
#[serde(default, deny_unknown_fields)]
pub struct Plinth {
    pub height: f32,
    pub proud: f32,
}

impl Default for Plinth {
    fn default() -> Self {
        Self { height: 0.5, proud: 0.06 }
    }
}

/// Timber posts proud of the walls: at the corners and between `bays` (across the width and
/// the depth), `width` wide.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
#[serde(default, deny_unknown_fields)]
pub struct Posts {
    pub width: f32,
    pub proud: f32,
    pub bays: [u32; 2],
}

impl Default for Posts {
    fn default() -> Self {
        Self { width: 0.24, proud: 0.04, bays: [1, 1] }
    }
}

/// A timber beam round all four sides, its top `at` meters up.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
#[serde(default, deny_unknown_fields)]
pub struct Beam {
    pub at: f32,
    pub height: f32,
    pub proud: f32,
}

impl Default for Beam {
    fn default() -> Self {
        Self { at: 3.0, height: 0.22, proud: 0.07 }
    }
}

/// A building's walls (`#hall`): `size` meters wide (X), deep (Z) and high, standing on its
/// origin, the front facing +Z. A roof placed `size[2]` up sits on them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
#[serde(default, deny_unknown_fields)]
pub struct Hall {
    pub size: [f32; 3],
    pub plinth: Plinth,
    pub posts: Option<Posts>,
    pub beams: Vec<Beam>,
    pub openings: Vec<Opening>,
    /// How far into the walls windows' paper and doors stand.
    pub recess: f32,
    /// The width of frames and rings round openings, and how proud they stand.
    pub frame: [f32; 2],
    /// A flat roof over it (none where a roof covers it).
    pub flat_top: bool,
}

impl Default for Hall {
    fn default() -> Self {
        Self {
            size: [6.0, 5.0, 3.6],
            plinth: Plinth::default(),
            posts: None,
            beams: Vec::new(),
            openings: Vec::new(),
            recess: 0.16,
            frame: [0.09, 0.03],
            flat_top: false,
        }
    }
}

/// A freestanding wall (`#wall`): `size` meters long (X), high and thick (Z), standing on its
/// origin, under a tiled coping. Its openings go through it, placed from its front.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
#[serde(default, deny_unknown_fields)]
pub struct Wall {
    pub size: [f32; 3],
    pub plinth: Plinth,
    pub coping: Coping,
    pub openings: Vec<Opening>,
    pub frame: [f32; 2],
    /// Whether its left and right ends are built (not where they run into a building).
    pub ends: [bool; 2],
}

impl Default for Wall {
    fn default() -> Self {
        Self {
            size: [6.0, 2.6, 0.32],
            plinth: Plinth { height: 0.4, proud: 0.05 },
            coping: Coping::default(),
            openings: Vec::new(),
            frame: [0.09, 0.03],
            ends: [true, true],
        }
    }
}

/// A freestanding wall's coping: a small gable roof along its top.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
#[serde(default, deny_unknown_fields)]
pub struct Coping {
    pub overhang: f32,
    pub barge: f32,
    pub rise: f32,
    pub thickness: f32,
    pub tiles: Tiles,
    pub ridge: Ridge,
}

impl Default for Coping {
    fn default() -> Self {
        Self {
            overhang: 0.16,
            barge: 0.1,
            rise: 0.22,
            thickness: 0.08,
            tiles: Tiles { period: 0.22, depth: 0.03 },
            ridge: Ridge { height: 0.12, width: 0.14, ends: 0.0 },
        }
    }
}

/// A round timber column (`#column`) standing on its origin: a stone drum under it and a
/// timber block on its top for beams to rest on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
#[serde(default, deny_unknown_fields)]
pub struct Column {
    /// From the ground to the block's top, meters.
    pub height: f32,
    pub radius: f32,
    /// Faces round the shaft.
    pub sides: u32,
    /// The drum's radius and height (none at height 0).
    pub base: [f32; 2],
    /// The block's width and height (none at height 0).
    pub cap: [f32; 2],
}

impl Default for Column {
    fn default() -> Self {
        Self { height: 3.2, radius: 0.15, sides: 12, base: [0.26, 0.24], cap: [0.38, 0.22] }
    }
}

/// A paper lantern (`#lantern`): its paper `size` meters across and high between caps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
#[serde(default, deny_unknown_fields)]
pub struct Lantern {
    pub kind: LanternKind,
    pub size: [f32; 2],
    /// Ribs round its paper (round and drum lanterns).
    pub sides: u32,
    pub mount: Mount,
}

impl Default for Lantern {
    fn default() -> Self {
        Self { kind: LanternKind::Round, size: [0.42, 0.5], sides: 10, mount: Mount::Hang(0.3) }
    }
}

/// A lantern's form.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
pub enum LanternKind {
    /// Swelling to its middle, narrow caps: the classic red-and-amber paper globe.
    #[default]
    Round,
    /// A cylinder bulging a little.
    Drum,
    /// Four paper panels between corner posts, under a little pyramid roof.
    Square,
}

/// How a lantern is held, and where its origin is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
pub enum Mount {
    /// Hanging this far under its origin on a cord (from an eave or a beam).
    Hang(f32),
    /// Standing on a timber post this high, its origin the post's foot.
    Post(f32),
    /// Its bottom on its origin.
    Bare,
}

impl Default for Mount {
    fn default() -> Self {
        Self::Hang(0.3)
    }
}

/// A market stall (`#stall`); see the module docs. It stands on its origin, its front toward
/// +Z, its posts' outer faces `size` apart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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

/// A cloth awning (see the module docs): `overhang` past the frame's front and back (half that
/// past its ends), `rise` from the beams to the ridge pole (a shed's back pole), sagging `sag`
/// between them; its corners sweep up `sweep`, fading over `sweep_reach` along the eaves. Its
/// valance hangs `valance[0]` to its scallops' tips, each scallop `valance[1]` wide (none at a
/// drop of 0).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
pub enum ClothKind {
    /// Two slopes from a ridge pole over the middle, a valance along both eaves.
    #[default]
    Gable,
    /// One slope from a pole on the back posts down to the front, a valance along the front.
    Shed,
}

/// A board hanging from a stall's front beam on two cords: blank, in a timber frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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

/// A hand cart (`#cart`); see the module docs. It stands on its origin, its shafts toward +Z,
/// its wheels' axle a little behind the bed's middle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
#[serde(default, deny_unknown_fields)]
pub struct Cart {
    /// The bed: width (X), length (Z), and its sides' height over its floor, meters.
    pub bed: [f32; 3],
    pub wheels: Wheels,
    /// How far the shafts reach past the bed's front.
    pub shafts: f32,
    pub goods: Vec<Goods>,
}

impl Default for Cart {
    fn default() -> Self {
        Self { bed: [1.1, 1.7, 0.3], wheels: Wheels::default(), shafts: 0.9, goods: Vec::new() }
    }
}

/// A cart's wheels: their radius to the tyre, their spokes, and their rim's depth (from the
/// tyre in) and width (along the axle).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
#[serde(default, deny_unknown_fields)]
pub struct Wheels {
    pub radius: f32,
    pub spokes: u32,
    pub rim: [f32; 2],
}

impl Default for Wheels {
    fn default() -> Self {
        Self { radius: 0.45, spokes: 10, rim: [0.07, 0.07] }
    }
}

/// Goods for sale: `count` of a kind in a row along X, standing on a surface.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
#[serde(default, deny_unknown_fields)]
pub struct Goods {
    pub kind: GoodsKind,
    pub on: Surface,
    /// The row's middle from its surface's middle (on the ground, from the shape's origin): along
    /// X and Z, meters.
    pub at: [f32; 2],
    /// Each one's size: a basket's or jar's diameter, a crate's width, a roll of cloth's length.
    pub size: f32,
    pub count: u32,
}

impl Default for Goods {
    fn default() -> Self {
        Self { kind: GoodsKind::Basket(Fill::Fruit), on: Surface::Counter, at: [0.0; 2], size: 0.4, count: 1 }
    }
}

/// What goods are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
pub enum GoodsKind {
    /// A round wicker basket.
    Basket(Fill),
    /// A crate of boards, `0.7 size` deep.
    Crate(Fill),
    /// A lidded jar, a little taller than wide.
    Jar,
    /// Three rolls of cloth `size` long, stacked two under one, lying along the row.
    Bolts,
}

/// What a basket or crate holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
pub enum Fill {
    /// A heap of greens.
    Greens,
    /// A heap of fruit.
    #[default]
    Fruit,
    Empty,
}

/// What goods stand on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
pub enum Surface {
    /// A stall's counter.
    #[default]
    Counter,
    /// A stall's shelf, counted from the lowest (0).
    Shelf(u32),
    /// A cart's bed.
    Bed,
    /// The ground, outside the stall's frame or clear of the cart.
    Ground,
}
