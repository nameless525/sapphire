//! A species' leaf atlas, painted in brush strokes: see the [tree module's docs](super).
//!
//! The atlas is a grid of cells ([`cells`]), each a variant of the species' sprig. A stroke is a brush's
//! mark along a line: a width that follows its brush's profile from its start to its end, an edge that
//! wobbles a little as a loaded brush's does, and an edge a texel and a half soft. Strokes paint flat
//! colours over one another, in order, in linear light; the leaves' strokes lay down coverage, and the
//! lighter strokes on their lit sides paint only inside them. Every texel is the same on every machine:
//! integer hashes, the four operations, `sqrt` and `sapphire_math::det`.

use std::f32::consts::{PI, TAU};
use std::io;

use glam::{Vec2, Vec3};
use sapphire_math::det;

use super::mesh::STRAND_ASPECT;
use super::{Brush, TreeSpecies, color};
use crate::color::{linear_to_srgb, unorm8};
use crate::samples::noise::{NO_PERIOD, Rng, smoothstep, value_noise};

/// The light falls on the sprig from the top left: the lighter stroke on each leaf sits on this side.
const LIGHT: Vec2 = Vec2::new(-0.6, -0.8);

/// The atlas's columns and rows for `brush`: a willow's strands are cells [`STRAND_ASPECT`] times as tall as
/// they are wide, side by side, so every atlas is square.
pub fn cells(brush: Brush) -> (u32, u32) {
    match brush {
        Brush::Strand => (4, 1),
        Brush::Broad | Brush::Petal | Brush::Needle => (2, 2),
    }
}

/// How a stroke's half width runs from its start (0) to its end (1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Profile {
    /// Widest a third of the way out, pointed at its tip.
    Leaf,
    /// Round at both ends: a petal, a dab.
    Petal,
    /// Blunt at its base, tapering to a point.
    Needle,
    /// Even, thinning a little toward its end: a stem.
    Stem,
}

impl Profile {
    fn width(self, t: f32) -> f32 {
        match self {
            Self::Leaf => det::sin(PI * det::pow(t, 0.7)),
            Self::Petal => det::sin(PI * t).max(0.0).sqrt(),
            Self::Needle => det::pow(1.0 - t, 0.6) * (t * 8.0).min(1.0),
            Self::Stem => 1.0 - 0.5 * t,
        }
    }
}

/// One mark of the brush, in a cell's units (its width is 1, Y down).
#[derive(Clone, Copy, Debug)]
struct Stroke {
    start: Vec2,
    /// Unit length.
    direction: Vec2,
    length: f32,
    width: f32,
    profile: Profile,
    /// The edge's wobble, a share of the half width, and its noise's seed.
    wobble: f32,
    seed: u32,
}

impl Stroke {
    fn new(start: Vec2, direction: Vec2, length: f32, width: f32, profile: Profile, seed: u32) -> Self {
        Self { start, direction: direction.normalize(), length, width, profile, wobble: 0.18, seed }
    }

    /// A narrower stroke on this one's lit side, over `span` of its length from `from`: its lighter tone.
    fn lit_side(&self, from: f32, span: f32, width: f32) -> Self {
        let across = Vec2::new(-self.direction.y, self.direction.x);
        let side = if across.dot(LIGHT) >= 0.0 { across } else { -across };
        Self {
            start: self.start + self.direction * (from * self.length) + side * (self.width * 0.16),
            length: self.length * span,
            width: self.width * width,
            seed: self.seed ^ 0x5a5a,
            ..*self
        }
    }

    /// How much of the point `p` this stroke covers, 0 to 1, a texel `texel` wide.
    fn coverage(&self, p: Vec2, texel: f32) -> f32 {
        let offset = p - self.start;
        let along = offset.dot(self.direction);
        let t = along / self.length;
        if !(0.0..=1.0).contains(&t) {
            return 0.0;
        }
        let across = offset.perp_dot(self.direction).abs();
        let wobble = 1.0
            + self.wobble
                * (value_noise(Vec2::new(t * 5.0, 0.5), (NO_PERIOD, NO_PERIOD), self.seed) - 0.5)
                * 2.0;
        let half = 0.5 * self.width * self.profile.width(t) * wobble;
        smoothstep(-0.75 * texel, 0.75 * texel, half - across)
    }

    /// Its bounds, in the cell's units: `(min, max)`.
    fn bounds(&self) -> (Vec2, Vec2) {
        let end = self.start + self.direction * self.length;
        let pad = Vec2::splat(self.width * (0.5 + self.wobble) + 0.02);
        (self.start.min(end) - pad, self.start.max(end) + pad)
    }
}

/// A cell being painted: linear colour and coverage, a texel a cell's `1 / width` wide.
struct Cell {
    width: u32,
    height: u32,
    color: Vec<Vec3>,
    coverage: Vec<f32>,
}

impl Cell {
    fn new(width: u32, height: u32, ground: Vec3) -> Self {
        let texels = (width * height) as usize;
        Self { width, height, color: vec![ground; texels], coverage: vec![0.0; texels] }
    }

    fn texel(&self) -> f32 {
        1.0 / self.width as f32
    }

    /// Paints `stroke` in `color`, laying down its coverage.
    fn paint(&mut self, stroke: &Stroke, color: Vec3) {
        self.paint_within(stroke, None, color);
    }

    /// Paints `stroke` in `color` only where `within` covers, laying down no coverage of its own: a leaf's
    /// lit side stays on the leaf.
    fn paint_on(&mut self, stroke: &Stroke, within: &Stroke, color: Vec3) {
        self.paint_within(stroke, Some(within), color);
    }

    fn paint_within(&mut self, stroke: &Stroke, within: Option<&Stroke>, color: Vec3) {
        let texel = self.texel();
        let (low, high) = stroke.bounds();
        let to_texel = |v: f32, size: u32| ((v / texel).floor().max(0.0) as u32).min(size);
        let (x0, x1) = (to_texel(low.x, self.width), to_texel(high.x, self.width) + 1);
        let (y0, y1) = (to_texel(low.y, self.height), to_texel(high.y, self.height) + 1);
        for y in y0..y1.min(self.height) {
            for x in x0..x1.min(self.width) {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * texel;
                let mut amount = stroke.coverage(p, texel);
                if let Some(within) = within {
                    amount = amount.min(within.coverage(p, texel));
                }
                if amount <= 0.0 {
                    continue;
                }
                let index = (y * self.width + x) as usize;
                self.color[index] = self.color[index].lerp(color, amount);
                if within.is_none() {
                    self.coverage[index] = self.coverage[index].max(amount);
                }
            }
        }
    }
}

/// The species' palette, in linear light.
struct Palette {
    fill: Vec3,
    light: Vec3,
    shade: Vec3,
    accent: Vec3,
}

/// The atlas of `species`, `size` texels a side, as a PNG: colour in sRGB, coverage in alpha.
pub fn atlas_png(species: &TreeSpecies, size: u32) -> io::Result<Vec<u8>> {
    let leaves = &species.leaves;
    let rgb = |text: &str| color(text).ok_or_else(|| io::Error::other(format!("{text:?} isn't a colour")));
    let palette = Palette {
        fill: rgb(&leaves.fill)?,
        light: rgb(&leaves.light)?,
        shade: rgb(&leaves.shade)?,
        accent: rgb(&leaves.accent)?,
    };
    let (columns, rows) = cells(leaves.brush);
    let (cell_width, cell_height) = (size / columns, size / rows);
    let mut image = image::RgbaImage::new(size, size);
    for index in 0..columns * rows {
        let mut cell = Cell::new(cell_width, cell_height, palette.shade);
        let mut rng =
            Rng(crate::samples::noise::hash(species.seed ^ 0xa71a5 ^ index.wrapping_mul(0x9e37_79b9)));
        match leaves.brush {
            Brush::Broad => sprig(&mut cell, &palette, &mut rng),
            Brush::Petal => blossoms(&mut cell, &palette, &mut rng),
            Brush::Needle => frond(&mut cell, &palette, &mut rng),
            Brush::Strand => strands(&mut cell, &palette, &mut rng),
        }
        let (left, top) = ((index % columns) * cell_width, (index / columns) * cell_height);
        for y in 0..cell_height {
            for x in 0..cell_width {
                let i = (y * cell_width + x) as usize;
                // Clear at the cell's border, so neither a sample nor a mip reaches a neighbour's leaves.
                let edge = x.min(y).min(cell_width - 1 - x).min(cell_height - 1 - y) as f32;
                let alpha = cell.coverage[i] * smoothstep(0.5, 2.5, edge);
                let [r, g, b] = cell.color[i].to_array().map(|c| unorm8(linear_to_srgb(c)));
                image.put_pixel(left + x, top + y, image::Rgba([r, g, b, unorm8(alpha)]));
            }
        }
    }
    let mut bytes = io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).map_err(io::Error::other)?;
    Ok(bytes.into_inner())
}

/// A tone between `shade` (0) and `fill` (1), a little lighter or darker by `rng`.
fn tone(palette: &Palette, out: f32, rng: &mut Rng) -> Vec3 {
    palette.shade.lerp(palette.fill, out.clamp(0.0, 1.0)) * rng.range(0.93, 1.07)
}

/// A broadleaf's sprig: a twig, then pointed leaves around the middle, the inner ones first and in the
/// shade, each with its lit side.
fn sprig(cell: &mut Cell, palette: &Palette, rng: &mut Rng) {
    let middle = Vec2::splat(0.5);
    let twig = Stroke::new(
        Vec2::new(0.5, 0.97),
        Vec2::new(rng.range(-0.15, 0.15), -1.0),
        0.45,
        0.035,
        Profile::Stem,
        rng.0,
    );
    cell.paint(&twig, palette.accent);
    let count = 26;
    let mut leaves: Vec<(f32, Stroke)> = (0..count)
        .map(|i| {
            let turns = i as f32 * 0.618_034 + rng.range(-0.05, 0.05);
            let out = det::vec2_from_angle(turns * TAU);
            let reach = rng.range(0.0, 0.2);
            let direction = det::vec2_from_angle(turns * TAU + rng.range(-0.45, 0.45));
            let length = rng.range(0.2, 0.27);
            let width = rng.range(0.1, 0.14);
            (reach, Stroke::new(middle + out * reach, direction, length, width, Profile::Leaf, rng.0))
        })
        .collect();
    leaves.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (reach, leaf) in &leaves {
        cell.paint(leaf, tone(palette, 0.3 + reach * 4.0, rng));
        cell.paint_on(&leaf.lit_side(0.15, 0.7, 0.45), leaf, palette.light);
    }
}

/// A blossom's puff: five-petalled flowers crowded about the middle, the inner ones in the shade, each
/// petal with its lit side and each flower a dab of the accent at its heart.
fn blossoms(cell: &mut Cell, palette: &Palette, rng: &mut Rng) {
    let middle = Vec2::splat(0.5);
    let mut flowers: Vec<(f32, Vec2, f32, f32)> = (0..11)
        .map(|i| {
            let turns = i as f32 * 0.618_034 + rng.range(-0.05, 0.05);
            let reach = if i == 0 { 0.0 } else { rng.range(0.1, 0.28) };
            let at = middle + det::vec2_from_angle(turns * TAU) * reach;
            (reach, at, rng.range(0.085, 0.115), rng.range(0.0, TAU))
        })
        .collect();
    flowers.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (reach, at, radius, turn) in flowers {
        let base = tone(palette, 0.35 + reach * 3.0, rng);
        for petal in 0..5 {
            let direction = det::vec2_from_angle(turn + petal as f32 * TAU / 5.0);
            let stroke = Stroke::new(at, direction, radius, radius * 0.95, Profile::Petal, rng.0);
            cell.paint(&stroke, base);
            cell.paint_on(&stroke.lit_side(0.3, 0.6, 0.45), &stroke, palette.light);
        }
        let heart = Stroke::new(
            at - Vec2::X * (radius * 0.14),
            Vec2::X,
            radius * 0.28,
            radius * 0.28,
            Profile::Petal,
            rng.0,
        );
        cell.paint(&heart, palette.accent);
    }
}

/// A conifer's frond: a stem up the cell (its tip at the top, where the card's twig ends), needles both
/// sides of it pointing up and out, shorter toward the tip, the lower ones in the shade.
fn frond(cell: &mut Cell, palette: &Palette, rng: &mut Rng) {
    let lean = rng.range(-0.08, 0.08);
    let (base, tip) = (Vec2::new(0.5 - lean, 0.96), Vec2::new(0.5 + lean, 0.06));
    let up = (tip - base).normalize();
    let length = tip.distance(base);
    cell.paint(&Stroke::new(base, up, length, 0.03, Profile::Stem, rng.0), palette.accent);
    let count = 16;
    for i in 0..count {
        let t = (i as f32 + 0.5) / count as f32 * 0.95;
        let at = base + up * (length * t);
        for side in [-1.0f32, 1.0] {
            let angle = side * rng.range(0.85, 1.1);
            let (sin, cos) = det::sin_cos(angle);
            let direction = Vec2::new(up.x * cos - up.y * sin, up.x * sin + up.y * cos);
            let needle = Stroke::new(
                at,
                direction,
                0.36 * (1.0 - 0.7 * t) + 0.04,
                rng.range(0.04, 0.055),
                Profile::Needle,
                rng.0,
            );
            cell.paint(&needle, tone(palette, 0.25 + 0.75 * t, rng));
            if side * LIGHT.x < 0.0 || rng.next() < 0.35 {
                cell.paint_on(&needle.lit_side(0.1, 0.6, 0.4), &needle, palette.light);
            }
        }
    }
}

/// A willow's strands: two or three stems hanging from the cell's top, narrow leaves along them pointing
/// down and out, the ones nearer the top in the shade.
fn strands(cell: &mut Cell, palette: &Palette, rng: &mut Rng) {
    let bottom = STRAND_ASPECT;
    let count = 2 + (rng.next() * 2.0) as u32;
    for strand in 0..count {
        let x = (strand as f32 + 0.5) / count as f32 + rng.range(-0.08, 0.08);
        let sway = rng.range(-0.12, 0.12);
        let (top, end) = (Vec2::new(x, 0.02), Vec2::new(x + sway, bottom * rng.range(0.88, 0.97)));
        let down = (end - top).normalize();
        let length = end.distance(top);
        cell.paint(&Stroke::new(top, down, length, 0.03, Profile::Stem, rng.0), palette.accent);
        let leaves = (length / 0.11) as u32;
        for i in 0..leaves {
            let t = (i as f32 + 0.5) / leaves as f32;
            let at = top + down * (length * t);
            let side = if (i + strand) % 2 == 0 { 1.0f32 } else { -1.0 };
            let (sin, cos) = det::sin_cos(side * rng.range(0.55, 0.85));
            let direction = Vec2::new(down.x * cos - down.y * sin, down.x * sin + down.y * cos);
            let leaf =
                Stroke::new(at, direction, rng.range(0.26, 0.36), rng.range(0.1, 0.13), Profile::Leaf, rng.0);
            cell.paint(&leaf, tone(palette, 0.2 + 0.8 * t, rng));
            cell.paint_on(&leaf.lit_side(0.15, 0.65, 0.4), &leaf, palette.light);
        }
    }
}
