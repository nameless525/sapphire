//! Ground cover's atlases, painted in the trees' brush strokes ([`atlas`]): a fern's fronds, a flower's heads
//! and leaves, and a rock's flat swatches. See the [cover module's docs](super).

use std::f32::consts::TAU;
use std::io;

use glam::{Vec2, Vec3};
use sapphire_math::det;

use super::super::atlas::{self, Cell, LIGHT, Palette, Profile, Stroke, tone};
use super::Flower;
use crate::color::{linear_to_srgb, unorm8};
use crate::samples::noise::Rng;

/// A fern's atlas: four fronds side by side, each cell [`FROND_ASPECT`] times as tall as it's wide, so the
/// atlas is square.
pub const FROND_CELLS: (u32, u32) = (4, 1);
/// A frond's cell's height over its width, and so its strip's length over its width at most.
pub const FROND_ASPECT: f32 = 4.0;

/// A flower's atlas: three heads, and a fan of leaves in its last cell ([`LEAF_CELL`]).
pub const FLOWER_CELLS: (u32, u32) = (2, 2);
/// The flower atlas's cell of leaves.
pub const LEAF_CELL: u32 = 3;

/// A rock's swatches: four tones across (the shade, two fills, the lit bevel), the bare stone's row over the
/// moss's.
pub const SWATCHES: (u32, u32) = (4, 2);

/// A fern's atlas, `size` texels a side.
pub fn fern(seed: u32, palette: &Palette, size: u32) -> io::Result<Vec<u8>> {
    atlas::paint(seed, FROND_CELLS, size, |_| palette.shade, |_, cell, rng| frond(cell, palette, rng))
}

/// A flower's atlas, `size` texels a side: its heads in `bloom`'s colours, its leaves in `leaf`'s.
pub fn flower(seed: u32, flower: &Flower, bloom: &Palette, leaf: &Palette, size: u32) -> io::Result<Vec<u8>> {
    // Each cell over its own shade: the heads' soft edges blend from the petals' shade, not the leaves'.
    let ground = |index| if index == LEAF_CELL { leaf.shade } else { bloom.shade };
    atlas::paint(seed, FLOWER_CELLS, size, ground, |index, cell, rng| {
        if index == LEAF_CELL {
            leaf_fan(cell, leaf, rng);
        } else {
            head(cell, bloom, flower, rng);
        }
    })
}

/// A frond: a stalk up the cell (its root at the bottom, where the strip leaves the fern's heart), leaflets
/// both sides pointing out and a little toward its tip, longest a third of the way up, the lower ones in the
/// shade.
fn frond(cell: &mut Cell, palette: &Palette, rng: &mut Rng) {
    let lean = rng.range(-0.05, 0.05);
    let (base, tip) = (Vec2::new(0.5 - lean, FROND_ASPECT - 0.04), Vec2::new(0.5 + lean, 0.05));
    let up = (tip - base).normalize();
    let length = tip.distance(base);
    cell.paint(&Stroke::new(base, up, length, 0.045, Profile::Stem, rng.0), palette.accent);
    let pairs = 22;
    for i in 0..pairs {
        let t = (i as f32 + 0.5) / pairs as f32 * 0.97;
        let at = base + up * (length * t);
        let reach = 0.05 + 0.4 * Profile::Leaf.width(t);
        for side in [-1.0f32, 1.0] {
            let (sin, cos) = det::sin_cos(side * rng.range(1.05, 1.3));
            let direction = Vec2::new(up.x * cos - up.y * sin, up.x * sin + up.y * cos);
            let leaflet =
                Stroke::new(at, direction, reach, reach * rng.range(0.32, 0.4), Profile::Leaf, rng.0);
            cell.paint(&leaflet, tone(palette, 0.25 + 0.75 * t, rng));
            if side * LIGHT.x < 0.0 || rng.next() < 0.4 {
                cell.paint_on(&leaflet.lit_side(0.15, 0.6, 0.4), &leaflet, palette.light);
            }
        }
    }
}

/// A flower's head, seen from above: a ring of petals round the middle (behind them, when there are six or
/// more, a second ring half a petal round in the shade, the head's depth), each with its lit side, and its
/// heart a dab of the accent with a lighter dab on its lit side.
fn head(cell: &mut Cell, palette: &Palette, flower: &Flower, rng: &mut Rng) {
    let middle = Vec2::splat(0.5);
    let length = 0.44;
    let petals = flower.petals;
    let turn = rng.range(0.0, TAU);
    let step = TAU / petals as f32;
    if petals >= 6 {
        for petal in 0..petals {
            let direction = det::vec2_from_angle(turn + (petal as f32 + 0.5) * step);
            let width = length * flower.petal_width;
            let stroke = Stroke::new(middle, direction, length * 0.9, width, Profile::Petal, rng.0);
            cell.paint(&stroke, tone(palette, 0.1, rng));
        }
    }
    for petal in 0..petals {
        let direction = det::vec2_from_angle(turn + petal as f32 * step + rng.range(-0.08, 0.08));
        let reach = length * rng.range(0.9, 1.0);
        let stroke = Stroke::new(middle, direction, reach, reach * flower.petal_width, Profile::Petal, rng.0);
        cell.paint(&stroke, tone(palette, rng.range(0.75, 1.0), rng));
        cell.paint_on(&stroke.lit_side(0.3, 0.6, 0.45), &stroke, palette.light);
    }
    let radius = flower.heart * length;
    let heart =
        Stroke::new(middle - Vec2::X * radius, Vec2::X, 2.0 * radius, 2.0 * radius, Profile::Petal, rng.0);
    cell.paint(&heart, palette.accent);
    let lit = middle + LIGHT * (0.35 * radius) - Vec2::X * (0.5 * radius);
    let dab = Stroke::new(lit, Vec2::X, radius, radius, Profile::Petal, rng.0);
    cell.paint_on(&dab, &heart, palette.accent.lerp(palette.light, 0.5));
}

/// The leaves at a flower's foot: a fan of pointed leaves from the cell's bottom middle, the outer ones
/// behind and in the shade, each with its lit side.
fn leaf_fan(cell: &mut Cell, palette: &Palette, rng: &mut Rng) {
    let foot = Vec2::new(0.5, 0.97);
    let count = 7;
    let mut leaves: Vec<(f32, Stroke)> = (0..count)
        .map(|i| {
            let angle = (i as f32 / (count - 1) as f32 - 0.5) * 1.6 + rng.range(-0.08, 0.08);
            let (sin, cos) = det::sin_cos(angle);
            let direction = Vec2::new(sin, -cos);
            let length = rng.range(0.6, 0.85) * (1.0 - 0.3 * angle.abs());
            (angle.abs(), Stroke::new(foot, direction, length, rng.range(0.13, 0.17), Profile::Leaf, rng.0))
        })
        .collect();
    leaves.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (out, leaf) in &leaves {
        cell.paint(leaf, tone(palette, 1.0 - out * 0.8, rng));
        cell.paint_on(&leaf.lit_side(0.15, 0.65, 0.42), leaf, palette.light);
    }
}

/// A rock's swatches, `size` texels a side, opaque: across, the shade, two fills and the lit bevel's tone;
/// down, the bare stone (`fill`, `light`, `shade`) and the moss (`accent`, darker and lighter). Each a flat
/// colour: a face samples one swatch's middle.
pub fn swatches(stone: &Palette, size: u32) -> io::Result<Vec<u8>> {
    let rows: [[Vec3; 4]; 2] = [
        [stone.shade, stone.shade.lerp(stone.fill, 0.55), stone.fill, stone.light],
        [stone.accent * 0.62, stone.accent * 0.82, stone.accent, (stone.accent * 1.3).min(Vec3::ONE)],
    ];
    let (columns, count) = SWATCHES;
    let (width, height) = (size / columns, size / count);
    let mut image = image::RgbaImage::new(size, size);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let tone = rows[((y / height).min(count - 1)) as usize][((x / width).min(columns - 1)) as usize];
        let [r, g, b] = tone.to_array().map(|c| unorm8(linear_to_srgb(c)));
        *pixel = image::Rgba([r, g, b, 255]);
    }
    let mut bytes = io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).map_err(io::Error::other)?;
    Ok(bytes.into_inner())
}

/// The middle of swatch `(column, row)`, as a UV.
pub fn swatch(column: u32, row: u32) -> [f32; 2] {
    let (columns, rows) = SWATCHES;
    [(column as f32 + 0.5) / columns as f32, (row as f32 + 0.5) / rows as f32]
}
