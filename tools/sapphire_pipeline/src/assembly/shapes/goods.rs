//! Goods for the market's stalls and carts (`#stall`, `#cart`).
//!
//! Goods ([`Goods`]) stand on a stall's counter or shelves, a cart's bed or the ground, in rows:
//! round wicker baskets and crates of boards, heaped with greens or fruit or empty, lidded jars,
//! and stacks of rolled cloth. Each is a few turned or boxed surfaces; in full detail a heap
//! carries a few round fruit or heads of greens on top. Goods keep inside their surface and
//! clear of each other and of what stands over them, which
//! [`Stall::validate`](super::Stall::validate) and [`Cart::validate`](super::Cart::validate)
//! check, so none can share a plane with the stall's or the cart's own faces.

use std::f32::consts::FRAC_PI_2;

use glam::{Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use super::{Detail, Mesh, Part, check, finite};

/// Goods keep this far inside their surface's edges and from each other; a row's goods stand
/// [`GAP`] apart.
const MARGIN: f32 = 0.01;
const GAP: f32 = 0.03;
/// A roll of cloth's radius, as a share of its length.
const BOLT: f32 = 0.16;
/// Goods under this size across carry one round fruit on their heap, not five.
const SMALL_GOODS: f32 = 0.35;

/// Goods for sale: `count` of a kind in a row along X, standing on a surface.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fill {
    /// A heap of greens.
    Greens,
    /// A heap of fruit.
    #[default]
    Fruit,
    Empty,
}

/// What goods stand on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

impl Surface {
    fn name(self) -> String {
        match self {
            Self::Counter => "counter".to_owned(),
            Self::Shelf(index) => format!("shelf {index}"),
            Self::Bed => "bed".to_owned(),
            Self::Ground => "ground".to_owned(),
        }
    }
}

/// Where goods may stand on a surface: its middle (on its top), how far it reaches either way
/// along X and Z, and the room over it.
pub(super) struct Place {
    pub middle: Vec3,
    pub half: Vec2,
    pub room: f32,
}

impl Place {
    pub fn ground() -> Self {
        Self { middle: Vec3::ZERO, half: Vec2::splat(f32::INFINITY), room: f32::INFINITY }
    }
}

impl GoodsKind {
    /// How far one reaches either way along X and Z from its middle, and how high it stands,
    /// for its size.
    fn extent(self, size: f32) -> (Vec2, f32) {
        match self {
            Self::Basket(_) => (Vec2::splat(0.5 * size), 0.5 * size),
            Self::Crate(_) => (Vec2::new(0.5, 0.35) * size, 0.48 * size),
            Self::Jar => (Vec2::splat(0.5 * size), 1.04 * size),
            Self::Bolts => (Vec2::new(0.5, 2.0 * BOLT) * size, (2.0 + 3f32.sqrt()) * BOLT * size),
        }
    }
}

impl Goods {
    /// Its row's middles about the row's.
    fn offsets(&self) -> impl Iterator<Item = f32> + '_ {
        let (half, _) = self.kind.extent(self.size);
        let pitch = 2.0 * half.x + GAP;
        (0..self.count).map(move |k| (k as f32 - 0.5 * (self.count as f32 - 1.0)) * pitch)
    }

    /// Its row's footprint about `place`'s middle, least and greatest corners in X and Z.
    fn footprint(&self) -> (Vec2, Vec2) {
        let (half, _) = self.kind.extent(self.size);
        let reach = Vec2::new(0.5 * (self.count as f32 - 1.0) * (2.0 * half.x + GAP), 0.0) + half;
        let at = Vec2::from(self.at);
        (at - reach, at + reach)
    }

    pub(super) fn build(&self, mesh: &mut Mesh, place: &Place, detail: Detail) {
        for offset in self.offsets() {
            let base = place.middle + Vec3::new(self.at[0] + offset, 0.0, self.at[1]);
            match self.kind {
                GoodsKind::Basket(fill) => basket(mesh, base, self.size, fill, detail),
                GoodsKind::Crate(fill) => crate_box(mesh, base, self.size, fill, detail),
                GoodsKind::Jar => jar(mesh, base, self.size, detail),
                GoodsKind::Bolts => bolts(mesh, base, self.size, detail),
            }
        }
    }
}

/// Checks goods: their numbers, that each row stands on a surface `place` finds (failing with
/// its message where the shape has none), inside it and with room over it, apart from the
/// rows on the same surface, and on the ground clear of `keep_out` (least and greatest corners
/// in X and Z).
pub(super) fn validate_goods(
    goods: &[Goods],
    place: &dyn Fn(Surface) -> Result<Place, String>,
    keep_out: (Vec2, Vec2),
) -> Result<(), String> {
    for (index, row) in goods.iter().enumerate() {
        check(finite(&[row.size, row.at[0], row.at[1]]) && row.size > 0.05 && row.count >= 1, || {
            format!("goods {index} need a size over 5 cm, finite places and a count of at least 1")
        })?;
        let surface = place(row.on).map_err(|message| format!("goods {index}: {message}"))?;
        let (least, greatest) = row.footprint();
        let (_, height) = row.kind.extent(row.size);
        let name = row.on.name();
        check(
            least.x >= -surface.half.x + MARGIN
                && greatest.x <= surface.half.x - MARGIN
                && least.y >= -surface.half.y + MARGIN
                && greatest.y <= surface.half.y - MARGIN,
            || format!("goods {index} reach past their {name}'s edges"),
        )?;
        check(height < surface.room - MARGIN, || {
            format!("goods {index} stand too tall for the room over their {name}")
        })?;
        if row.on == Surface::Ground {
            let (out_least, out_greatest) = keep_out;
            let apart = greatest.x + MARGIN <= out_least.x
                || least.x - MARGIN >= out_greatest.x
                || greatest.y + MARGIN <= out_least.y
                || least.y - MARGIN >= out_greatest.y;
            check(apart, || format!("goods {index} on the ground must stand outside the frame"))?;
        }
        for (other_index, other) in goods[..index].iter().enumerate() {
            if other.on != row.on {
                continue;
            }
            let (other_least, other_greatest) = other.footprint();
            let apart = greatest.x + MARGIN <= other_least.x
                || least.x - MARGIN >= other_greatest.x
                || greatest.y + MARGIN <= other_least.y
                || least.y - MARGIN >= other_greatest.y;
            check(apart, || format!("goods {index} overlap goods {other_index} on their {name}"))?;
        }
    }
    Ok(())
}

/// Faces round a turned good `size` across: one about every 3 cm across it, 6 to 12 of them, or
/// three fifths of that (at least 5) in light detail.
fn sides(size: f32, detail: Detail) -> u32 {
    let full = (size / 0.033).round().clamp(6.0, 12.0) as u32;
    if detail == Detail::Light { (full * 3 / 5).max(5) } else { full }
}

/// A heap's round fruit or heads of greens on top, in full detail: a ring of four round a
/// middle one, or the middle one alone on goods under [`SMALL_GOODS`] across.
fn crown(mesh: &mut Mesh, part: Part, size: f32, top: Vec3, ring: [Vec2; 4], detail: Detail) {
    if detail == Detail::Light {
        return;
    }
    let r = 0.09 * size;
    round(mesh, part, top, r);
    if size >= SMALL_GOODS {
        for at in ring {
            round(mesh, part, top + Vec3::new(at.x, -0.05 * size, at.y) * size, r);
        }
    }
}

/// A round fruit or head of greens of `radius` about `center`.
fn round(mesh: &mut Mesh, part: Part, center: Vec3, radius: f32) {
    let r = radius;
    let profile = [(0.0, -r), (0.71 * r, -0.71 * r), (r, 0.0), (0.71 * r, 0.71 * r), (0.0, r)];
    mesh.lathe(part, center, &profile, 6, [false, false], false);
}

/// The part a heap of `fill` draws in, and whether it has one.
fn heap(fill: Fill) -> Option<Part> {
    match fill {
        Fill::Greens => Some(Part::Produce),
        Fill::Fruit => Some(Part::Fruit),
        Fill::Empty => None,
    }
}

/// A round wicker basket `size` across, standing at `base`, heaped with `fill`.
fn basket(mesh: &mut Mesh, base: Vec3, size: f32, fill: Fill, detail: Detail) {
    let sides = sides(size, detail);
    // Up its outside, over its rim and down its inside to the heap (or its floor).
    let mut wall = vec![(0.36, 0.0), (0.47, 0.16), (0.5, 0.3), (0.5, 0.34), (0.45, 0.34), (0.43, 0.24)];
    let part = heap(fill);
    if part.is_none() {
        wall.extend([(0.37, 0.06), (0.0, 0.06)]);
    }
    let scaled =
        |points: &[(f32, f32)]| points.iter().map(|&(r, y)| (r * size, y * size)).collect::<Vec<_>>();
    mesh.lathe(Part::Wicker, base, &scaled(&wall), sides, [false, false], false);
    if let Some(part) = part {
        let mound = [(0.43, 0.24), (0.36, 0.33), (0.22, 0.4), (0.0, 0.43)];
        mesh.lathe(part, base, &scaled(&mound), sides, [false, false], false);
        let ring =
            [Vec2::new(0.14, 0.14), Vec2::new(-0.14, 0.14), Vec2::new(-0.14, -0.14), Vec2::new(0.14, -0.14)];
        crown(mesh, part, size, base + Vec3::Y * 0.42 * size, ring, detail);
    }
}

/// A crate of boards `size` wide (X) and `0.7 size` deep, standing at `base`, heaped with `fill`.
fn crate_box(mesh: &mut Mesh, base: Vec3, size: f32, fill: Fill, detail: Detail) {
    let half = Vec2::new(0.5, 0.35) * size;
    let height = 0.42 * size;
    let wall = (0.06 * size).min(0.025);
    let outer = Vec3::new(half.x, 0.0, half.y);
    mesh.cuboid(Part::Boards, base - outer, base + outer + Vec3::Y * height, &[-Vec3::Y, Vec3::Y]);
    // Its rim (front and back the full width, the ends between them), then its inside down to
    // the heap (or its floor).
    let inner = half - Vec2::splat(wall);
    let top = |x: f32, z: f32| base + Vec3::new(x, height, z);
    for (x, z) in [
        ([-half.x, half.x], [inner.y, half.y]),
        ([-half.x, half.x], [-half.y, -inner.y]),
        ([inner.x, half.x], [-inner.y, inner.y]),
        ([-half.x, -inner.x], [-inner.y, inner.y]),
    ] {
        let corners = [top(x[0], z[0]), top(x[1], z[0]), top(x[1], z[1]), top(x[0], z[1])];
        mesh.quad(Part::Boards, corners, Vec3::Y, [1.0; 4]);
    }
    let part = heap(fill);
    let floor = if part.is_some() { 0.8 * height } else { 0.03 };
    let corner = |x: f32, y: f32, z: f32| base + Vec3::new(x * inner.x, y, z * inner.y);
    for (out, a, b) in [
        (-Vec3::Z, (1.0, 1.0), (-1.0, 1.0)),
        (Vec3::Z, (-1.0, -1.0), (1.0, -1.0)),
        (-Vec3::X, (1.0, -1.0), (1.0, 1.0)),
        (Vec3::X, (-1.0, 1.0), (-1.0, -1.0)),
    ] {
        let corners = [
            corner(a.0, floor, a.1),
            corner(b.0, floor, b.1),
            corner(b.0, height, b.1),
            corner(a.0, height, a.1),
        ];
        mesh.quad(Part::Boards, corners, out, [0.7, 0.7, 0.95, 0.95]);
    }
    let bottom = [
        corner(-1.0, floor, -1.0),
        corner(1.0, floor, -1.0),
        corner(1.0, floor, 1.0),
        corner(-1.0, floor, 1.0),
    ];
    match part {
        Some(part) => {
            mesh.quad(part, bottom, Vec3::Y, [0.85; 4]);
            let ring = [
                Vec2::new(0.25, 0.13),
                Vec2::new(-0.25, 0.13),
                Vec2::new(-0.25, -0.13),
                Vec2::new(0.25, -0.13),
            ];
            crown(mesh, part, size, base + Vec3::Y * (floor + 0.04 * size), ring, detail);
        }
        None => mesh.quad(Part::Boards, bottom, Vec3::Y, [0.7; 4]),
    }
}

/// A lidded jar `size` across, standing at `base`.
fn jar(mesh: &mut Mesh, base: Vec3, size: f32, detail: Detail) {
    // Its belly, shoulder and neck, a lip, and a domed lid.
    let profile: Vec<(f32, f32)> = [
        (0.34, 0.0),
        (0.48, 0.18),
        (0.5, 0.36),
        (0.42, 0.62),
        (0.24, 0.8),
        (0.21, 0.88),
        (0.25, 0.93),
        (0.13, 0.99),
        (0.0, 1.04),
    ]
    .iter()
    .map(|&(r, y)| (r * size, y * size))
    .collect();
    mesh.lathe(Part::Pottery, base, &profile, sides(size, detail), [false, false], false);
}

/// Three rolls of cloth `size` long, two under one, lying along X at `base`: the top one in the
/// awning's cloth, the others in its valance's.
fn bolts(mesh: &mut Mesh, base: Vec3, size: f32, detail: Detail) {
    let r = BOLT * size;
    let sides = sides(2.0 * r, detail);
    // Lying along X: turned about +Y, then +Y turned to +X.
    let lay = Quat::from_rotation_z(-FRAC_PI_2);
    for (z, y, part) in
        [(-r, r, Part::Valance), (r, r, Part::Valance), (0.0, r * (1.0 + 3f32.sqrt()), Part::Cloth)]
    {
        let mut roll = Mesh::default();
        roll.lathe(part, Vec3::ZERO, &[(r, -0.5 * size), (r, 0.5 * size)], sides, [true, true], false);
        mesh.append(roll, lay, base + Vec3::new(0.0, y, z));
    }
}
