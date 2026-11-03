//! Turning shapes into quads, in points: what every widget draws through.

use sapphire_look::UiQuad;
use sapphire_math::Vec2;

use crate::atlas::{SPREAD, Sprite, Sprites};
use crate::layout::Rect;

/// How much of a value a color's alpha keeps; alpha is what fades do.
pub fn faded(color: [u8; 4], share: f32) -> [u8; 4] {
    let [r, g, b, a] = color;
    [r, g, b, (f32::from(a) * share.clamp(0.0, 1.0)).round() as u8]
}

/// How a shape draws, in points where it's a size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paint {
    pub color: [u8; 4],
    pub outline_color: [u8; 4],
    /// The outline's width, outward from the shape's edge.
    pub outline: f32,
    /// How far the edges feather, each way.
    pub softness: f32,
    /// How far the shape's edge moves out, or in if negative.
    pub grow: f32,
    /// A share of a turn clockwise from the top that draws; 0 draws it all.
    pub sweep: f32,
}

impl Paint {
    /// A plain fill.
    pub const fn fill(color: [u8; 4]) -> Self {
        Self { color, outline_color: [0; 4], outline: 0.0, softness: 0.0, grow: 0.0, sweep: 0.0 }
    }

    /// A fill with an outline around it.
    pub const fn outlined(color: [u8; 4], outline_color: [u8; 4], outline: f32) -> Self {
        Self { color, outline_color, outline, softness: 0.0, grow: 0.0, sweep: 0.0 }
    }

    /// A ring: the band `width` wide just inside the shape's edge.
    pub const fn ring(color: [u8; 4], width: f32) -> Self {
        Self { color: [0; 4], outline_color: color, outline: width, softness: 0.0, grow: -width, sweep: 0.0 }
    }

    pub const fn soft(self, softness: f32) -> Self {
        Self { softness, ..self }
    }

    pub const fn grown(self, grow: f32) -> Self {
        Self { grow, ..self }
    }

    pub const fn swept(self, sweep: f32) -> Self {
        Self { sweep, ..self }
    }

    /// The quad's field values, for a shape drawn at `points_per_texel`.
    pub(crate) fn apply(&self, quad: &mut UiQuad, points_per_texel: f32) {
        let value = |points: f32| points / points_per_texel / (2.0 * SPREAD);
        quad.color = self.color;
        quad.outline_color = self.outline_color;
        quad.edge = (0.5 - value(self.grow)).clamp(MIN_EDGE, 1.0);
        // Whatever draws must fade out before the cell's border, where the field reaches its
        // spread and stops: past that, a feather or outline would end in the quad's hard edge. A
        // small shape's spread is few points, so these give way to it.
        quad.outline = value(self.outline).min(quad.edge - MIN_EDGE);
        quad.softness = value(self.softness).min(quad.edge - quad.outline - MIN_EDGE);
        quad.sweep = self.sweep;
    }
}

/// The lowest field value anything draws at, a little above where the field clamps.
const MIN_EDGE: f32 = 0.02;

/// How a row of digits sits on its position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// A digit's width beside its height, as a share of that height: tabular.
const DIGIT_ADVANCE: f32 = 0.71;

/// Appends quads to a draw list, with shapes from an atlas.
#[derive(Debug)]
pub struct Painter<'a> {
    quads: &'a mut Vec<UiQuad>,
    sprites: &'a Sprites,
}

impl<'a> Painter<'a> {
    pub fn new(quads: &'a mut Vec<UiQuad>, sprites: &'a Sprites) -> Self {
        Self { quads, sprites }
    }

    pub fn sprites(&self) -> &'a Sprites {
        self.sprites
    }

    /// `sprite` at `size` points (the shape's, without its cell's margin) centered on `center`,
    /// its top leaning `slant` points to the right per point of height. A slanted quad's lean is
    /// around its center, so it stays centered and its field stays true along the slant.
    pub fn sprite(&mut self, sprite: Sprite, center: Vec2, size: Vec2, slant: f32, paint: Paint) {
        let per_texel = size / Vec2::from(sprite.size);
        let cell = size + per_texel * 2.0 * SPREAD;
        let axis_x = Vec2::new(cell.x, 0.0);
        let axis_y = Vec2::new(-slant * cell.y, cell.y);
        self.push(sprite, center, axis_x, axis_y, per_texel.y, paint);
    }

    /// `sprite` at `size` points centered on `center`, its top pointing along `up`, a unit
    /// vector in UI space (+y down).
    pub fn turned(&mut self, sprite: Sprite, center: Vec2, size: Vec2, up: Vec2, paint: Paint) {
        let per_texel = size / Vec2::from(sprite.size);
        let cell = size + per_texel * 2.0 * SPREAD;
        // Its up is `up`, so its down is -`up` and its right, a quarter turn from its up.
        let right = Vec2::new(-up.y, up.x);
        self.push(sprite, center, right * cell.x, -up * cell.y, per_texel.y, paint);
    }

    /// The bar shape filling `rect`, leaning `slant`, drawing only the range `from` to `to` of its
    /// length (0 to 1), cut along the slant. Its corners keep their shape at any length. Nothing
    /// draws for an empty range.
    pub fn bar(&mut self, rect: Rect, slant: f32, (from, to): (f32, f32), paint: Paint) {
        if to <= from || rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }
        let sprite = self.sprites.bar;
        let per_texel = rect.height() / sprite.size[1];
        let margin = SPREAD * per_texel;
        let cell = Vec2::new(rect.width(), rect.height()) + Vec2::splat(margin * 2.0);
        let cap = (sprite.size[0] * 0.5 + SPREAD) * per_texel;
        let index = self.quads.len();
        self.push(
            sprite,
            rect.center(),
            Vec2::new(cell.x, 0.0),
            Vec2::new(-slant * cell.y, cell.y),
            per_texel,
            paint,
        );
        let share = |t: f32| (margin + t * rect.width()) / cell.x;
        let quad = &mut self.quads[index];
        quad.slice = (cap / cell.x).min(0.5);
        quad.clip = [if from <= 0.0 { 0.0 } else { share(from) }, if to >= 1.0 { 1.0 } else { share(to) }];
    }

    /// The bar shape `length` long and `width` across, centered on `center` and running along `along`,
    /// a unit vector in UI space (+y down). Its corners keep their shape at any length. Nothing draws
    /// for an empty one.
    pub fn bar_along(&mut self, center: Vec2, along: Vec2, length: f32, width: f32, paint: Paint) {
        if length <= 0.0 || width <= 0.0 {
            return;
        }
        let sprite = self.sprites.bar;
        let per_texel = width / sprite.size[1];
        let cell = Vec2::new(length, width) + Vec2::splat(SPREAD * per_texel * 2.0);
        let cap = (sprite.size[0] * 0.5 + SPREAD) * per_texel;
        let across = Vec2::new(-along.y, along.x);
        let index = self.quads.len();
        self.push(sprite, center, along * cell.x, across * cell.y, per_texel, paint);
        self.quads[index].slice = (cap / cell.x).min(0.5);
    }

    /// `value` in digits `height` points tall, leaning `slant`, with `at` the row's left, middle or
    /// right on its vertical middle. Returns the row's width. Writes no text: `u32`'s ten digits at
    /// most, from the last.
    pub fn digits(
        &mut self,
        value: u32,
        at: Vec2,
        height: f32,
        align: Align,
        slant: f32,
        paint: Paint,
    ) -> f32 {
        let mut digits = [0_u8; 10];
        let mut count = 0;
        let mut rest = value;
        while rest > 0 || count == 0 {
            digits[count] = (rest % 10) as u8;
            rest /= 10;
            count += 1;
        }
        let advance = height * DIGIT_ADVANCE;
        let width = advance * count as f32;
        let left = match align {
            Align::Left => at.x,
            Align::Center => at.x - width * 0.5,
            Align::Right => at.x - width,
        };
        let sprites = self.sprites;
        for (index, &digit) in digits[..count].iter().rev().enumerate() {
            let sprite = sprites.digits[usize::from(digit)];
            let size = Vec2::new(height * sprite.size[0] / sprite.size[1], height);
            let center = Vec2::new(left + advance * (index as f32 + 0.5), at.y);
            self.sprite(sprite, center, size, slant, paint);
        }
        width
    }

    fn push(
        &mut self,
        sprite: Sprite,
        center: Vec2,
        axis_x: Vec2,
        axis_y: Vec2,
        points_per_texel: f32,
        paint: Paint,
    ) {
        let origin = center - (axis_x + axis_y) * 0.5;
        let mut quad =
            UiQuad::new(origin.to_array(), axis_x.to_array(), axis_y.to_array(), sprite.uv, paint.color);
        paint.apply(&mut quad, points_per_texel);
        self.quads.push(quad);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atlas::Atlas;

    fn painted(paint: impl FnOnce(&mut Painter<'_>)) -> Vec<UiQuad> {
        let atlas = Atlas::paint();
        let mut quads = Vec::new();
        paint(&mut Painter::new(&mut quads, &atlas.sprites));
        quads
    }

    /// The quad's four corners.
    fn corners(quad: &UiQuad) -> [Vec2; 4] {
        let origin = Vec2::from(quad.origin);
        let (x, y) = (Vec2::from(quad.axis_x), Vec2::from(quad.axis_y));
        [origin, origin + x, origin + y, origin + x + y]
    }

    #[test]
    fn a_sprite_s_shape_lands_on_its_rectangle_with_its_margin_around_it() {
        let center = Vec2::new(100.0, 50.0);
        let quads = painted(|painter| {
            let disc = painter.sprites().disc;
            painter.sprite(disc, center, Vec2::splat(26.0), 0.0, Paint::fill([255; 4]));
        });
        // 26 points from a 52-texel disc: half a point a texel, so the margin is 3 points.
        let [first, _, _, last] = corners(&quads[0]);
        assert_eq!(first, center - Vec2::splat(13.0 + 3.0));
        assert_eq!(last, center + Vec2::splat(13.0 + 3.0));
    }

    #[test]
    fn a_slant_keeps_a_shape_centered() {
        let center = Vec2::new(100.0, 50.0);
        let quads = painted(|painter| {
            let disc = painter.sprites().disc;
            painter.sprite(disc, center, Vec2::splat(26.0), 0.3, Paint::fill([255; 4]));
        });
        let [top_left, _, _, bottom_right] = corners(&quads[0]);
        assert_eq!((top_left + bottom_right) * 0.5, center);
        assert!(top_left.x > center.x - 16.0, "the top leans right");
    }

    #[test]
    fn field_values_follow_points_at_any_scale() {
        let quads = painted(|painter| {
            let disc = painter.sprites().disc;
            // A ring 2 points wide on a disc at half a point a texel and at two points a texel.
            for size in [26.0, 104.0] {
                painter.sprite(disc, Vec2::ZERO, Vec2::splat(size), 0.0, Paint::ring([255; 4], 2.0));
            }
        });
        let band = |quad: &UiQuad| quad.outline * 2.0 * SPREAD;
        assert_eq!(band(&quads[0]), 4.0, "4 texels at half a point a texel");
        assert_eq!(band(&quads[1]), 1.0, "1 texel at two points a texel");
        for quad in &quads {
            // The band ends on the shape's edge: the fill's edge is its width inside.
            assert!((quad.edge - quad.outline - 0.5).abs() < 1e-5, "{quad:?}");
        }
    }

    #[test]
    fn a_bar_s_range_cuts_along_its_length_and_leaves_its_ends_alone() {
        let rect = Rect::new(Vec2::new(10.0, 10.0), Vec2::new(210.0, 22.0));
        let quads = painted(|painter| {
            painter.bar(rect, 0.3, (0.0, 1.0), Paint::fill([255; 4]));
            painter.bar(rect, 0.3, (0.0, 0.5), Paint::fill([255; 4]));
            painter.bar(rect, 0.3, (0.25, 0.75), Paint::fill([255; 4]));
            painter.bar(rect, 0.3, (0.75, 0.25), Paint::fill([255; 4]));
        });
        assert_eq!(quads.len(), 3, "an empty range draws nothing");
        assert_eq!(quads[0].clip, [0.0, 1.0], "the whole bar is uncut");
        // 12 points from 16 texels: 0.75 a texel, so a 4.5-point margin on a 209-point cell.
        let share = |t: f32| (4.5 + t * 200.0) / 209.0;
        assert_eq!(quads[1].clip, [0.0, share(0.5)]);
        assert_eq!(quads[2].clip, [share(0.25), share(0.75)]);
        // Each end's half a cell (14 texels) at that scale.
        assert!((quads[0].slice - 10.5 / 209.0).abs() < 1e-6);
    }

    #[test]
    fn a_bar_along_a_direction_stands_on_its_center_with_its_ends_kept() {
        let center = Vec2::new(100.0, 50.0);
        let quads = painted(|painter| {
            let up = Vec2::new(0.0, -1.0);
            painter.bar_along(center, up, 60.0, 16.0, Paint::fill([255; 4]));
            painter.bar_along(center, up, 0.0, 16.0, Paint::fill([255; 4]));
            painter.bar_along(center, up, 60.0, 0.0, Paint::fill([255; 4]));
        });
        assert_eq!(quads.len(), 1, "an empty one draws nothing");
        // 16 points from 16 texels: a point a texel, so a 6-point margin all round.
        assert_eq!(quads[0].axis_x, [0.0, -72.0]);
        assert_eq!(quads[0].axis_y, [28.0, 0.0]);
        let [first, _, _, last] = corners(&quads[0]);
        assert_eq!((first + last) * 0.5, center);
        // Each end's half a cell of 14 texels, out of 72 points.
        assert_eq!(quads[0].slice, 14.0 / 72.0);
        assert_eq!(quads[0].clip, [0.0, 1.0], "uncut");
    }

    #[test]
    fn digits_write_most_significant_first_on_their_alignment() {
        let center_x = |quad: &UiQuad| {
            let [first, _, _, last] = corners(quad);
            (first.x + last.x) * 0.5
        };
        let quads = painted(|painter| {
            let width =
                painter.digits(305, Vec2::new(100.0, 20.0), 20.0, Align::Left, 0.0, Paint::fill([255; 4]));
            assert_eq!(width, 3.0 * 20.0 * DIGIT_ADVANCE);
        });
        let atlas = Atlas::paint();
        assert_eq!(quads.len(), 3);
        assert_eq!(quads[0].uv, atlas.sprites.digits[3].uv);
        assert_eq!(quads[1].uv, atlas.sprites.digits[0].uv);
        assert_eq!(quads[2].uv, atlas.sprites.digits[5].uv);
        assert!(center_x(&quads[0]) < center_x(&quads[1]) && center_x(&quads[1]) < center_x(&quads[2]));

        let zero = painted(|painter| {
            painter.digits(0, Vec2::ZERO, 20.0, Align::Right, 0.0, Paint::fill([255; 4]));
        });
        assert_eq!(zero.len(), 1, "zero is one digit");
        assert_eq!(zero[0].uv, atlas.sprites.digits[0].uv);
        let many = painted(|painter| {
            painter.digits(u32::MAX, Vec2::ZERO, 20.0, Align::Center, 0.0, Paint::fill([255; 4]));
        });
        assert_eq!(many.len(), 10);
    }

    #[test]
    fn a_turned_sprite_points_where_it_was_told() {
        let quads = painted(|painter| {
            let wedge = painter.sprites().wedge;
            painter.turned(wedge, Vec2::new(50.0, 50.0), Vec2::splat(20.0), Vec2::X, Paint::fill([255; 4]));
        });
        // The cell's top (its origin to axis_x's end) is on the far side along +x.
        let [top_left, top_right, bottom_left, _] = corners(&quads[0]);
        assert!(top_left.x > 50.0 && top_right.x > 50.0, "{top_left} {top_right}");
        assert!(bottom_left.x < 50.0);
        // Not mirrored: from its top-left, its right is a quarter turn clockwise on screen from up.
        assert!(top_right.y > top_left.y);
    }
}
