//! A node's look as quads: fills, nine-slices, gradients and bars, all of them plain [`UiQuad`]s over
//! the interface's distance field atlas.

use sapphire_assets::ui::{UiFill, UiLook, UiShape, UiSprite};
use sapphire_look::UiQuad;
use sapphire_math::Vec2;

use super::PROBLEM_COLOR;
use crate::atlas::{SPREAD, Sprite, Sprites};
use crate::layout::Rect;
use crate::paint::{Paint, faded};

/// The width of a problem's frame, in points.
const PROBLEM_FRAME: f32 = 2.0;

/// `rect` with each edge on a whole pixel.
fn snapped(rect: Rect, scale: f32) -> Rect {
    let snap = |value: f32| (value * scale).round() / scale;
    Rect::new(
        Vec2::new(snap(rect.min.x), snap(rect.min.y)),
        Vec2::new(snap(rect.max.x), snap(rect.max.y)),
    )
}

/// Whether anything of a node at `rect` can show. What can't costs no quads and no glyphs.
pub(super) fn on_screen(rect: Rect, mask: Option<[f32; 4]>, size: Vec2) -> bool {
    let [x0, y0, x1, y1] = mask.unwrap_or([0.0, 0.0, size.x, size.y]);
    rect.min.x < x1 && rect.max.x > x0 && rect.min.y < y1 && rect.max.y > y0
}

fn sprite_of(sprites: &Sprites, sprite: UiSprite) -> Sprite {
    match sprite {
        UiSprite::Bar => sprites.bar,
        UiSprite::Button => sprites.button,
        UiSprite::Disc => sprites.disc,
        UiSprite::Sword => sprites.sword,
        UiSprite::Chevrons => sprites.chevrons,
        UiSprite::Spark => sprites.spark,
        UiSprite::Wedge => sprites.wedge,
    }
}

/// A point of `sprite`'s cell where the field is deep inside the shape everywhere near: what a plain
/// rectangle reads, so it needs no texture of its own.
fn solid_uv(sprites: &Sprites) -> [f32; 4] {
    let [u0, v0, u1, v1] = sprites.bar.uv;
    let (u, v) = ((u0 + u1) * 0.5, (v0 + v1) * 0.5);
    [u, v, u, v]
}

/// A quad over `piece` of the frame, reading `uv`.
fn quad(piece: Rect, uv: [f32; 4], color: [u8; 4]) -> UiQuad {
    UiQuad::new(
        piece.min.to_array(),
        [piece.width(), 0.0],
        [0.0, piece.height()],
        uv,
        color,
    )
}

/// `from` and `to` across `rect`, for each quad pushed since `first`: its piece of the gradient.
fn apply_gradient(
    quads: &mut [UiQuad],
    rect: Rect,
    to: [u8; 4],
    angle_degrees: f32,
) {
    let (sin, cos) = angle_degrees.to_radians().sin_cos();
    let along = Vec2::new(cos, sin);
    let length = (rect.width() * cos).abs() + (rect.height() * sin).abs();
    if length <= 0.0 {
        return;
    }
    let center = rect.center();
    for quad in quads {
        let origin = Vec2::from(quad.origin);
        // Before any slant, each quad's axes are its own width and height.
        quad.color_end = to;
        quad.gradient = [
            along.x * quad.axis_x[0] / length,
            along.y * quad.axis_y[1] / length,
            (origin - center).dot(along) / length + 0.5,
        ];
    }
}

/// Leans `quads` around `center_y`, their tops to the right.
fn apply_slant(quads: &mut [UiQuad], slant: f32, center_y: f32) {
    for quad in quads {
        quad.origin[0] -= slant * (quad.origin[1] - center_y);
        quad.axis_y[0] = -slant * quad.axis_y[1];
    }
}

/// Cuts `quads` at `share` of `rect`'s width, with the shader's antialiased cut.
fn apply_fill(quads: &mut Vec<UiQuad>, first: usize, rect: Rect, share: f32) {
    let cut = rect.min.x + rect.width() * share;
    let mut keep = first;
    for at in first..quads.len() {
        let mut quad = quads[at];
        let (start, width) = (quad.origin[0], quad.axis_x[0]);
        if start >= cut {
            continue;
        }
        if start + width > cut {
            quad.clip = [0.0, (cut - start) / width];
        }
        quads[keep] = quad;
        keep += 1;
    }
    quads.truncate(keep);
}

/// Adds what `look` draws over `rect`, at `opacity`, and how much of it `fill` says.
pub(super) fn look(
    look: &UiLook,
    rect: Rect,
    fill: f32,
    opacity: f32,
    scale: f32,
    sprites: &Sprites,
    quads: &mut Vec<UiQuad>,
) {
    if opacity <= 0.0 || fill <= 0.0 {
        return;
    }
    let rect = snapped(rect, scale);
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let (near, far, angle) = match look.fill {
        UiFill::Color(color) => (faded(color, opacity), None, 0.0),
        UiFill::Gradient { from, to, angle } => (faded(from, opacity), Some(faded(to, opacity)), angle),
    };
    let paint = look.outline.map_or(Paint::fill(near), |outline| {
        Paint::outlined(near, faded(outline.color, opacity), outline.width)
    });
    let first = quads.len();
    match look.shape {
        UiShape::Rect => quads.push(quad(rect, solid_uv(sprites), near)),
        UiShape::Slice { sprite, corner } => {
            nine_slice(sprite_of(sprites, sprite), rect, corner, paint, quads);
        }
        UiShape::Sprite(sprite) => {
            let sprite = sprite_of(sprites, sprite);
            let per_texel = Vec2::new(rect.width(), rect.height()) / Vec2::from(sprite.size);
            let cell = rect_grown(rect, per_texel * SPREAD);
            let mut drawn = quad(cell, sprite.uv, near);
            paint.apply(&mut drawn, per_texel.y);
            quads.push(drawn);
        }
    }
    if let Some(far) = far {
        apply_gradient(&mut quads[first..], rect, far, angle);
    }
    if fill < 1.0 {
        apply_fill(quads, first, rect, fill);
    }
    if look.slant != 0.0 {
        apply_slant(&mut quads[first..], look.slant, rect.center().y);
    }
}

fn rect_grown(rect: Rect, by: Vec2) -> Rect {
    Rect::new(rect.min - by, rect.max + by)
}

/// `sprite` over `rect` in nine pieces: each of its cell's quarters at a corner, keeping its shape at
/// `corner` points across the shape's half, and the row and column through the cell's middle stretched
/// between them. The field's edges keep their one pixel, and nine quads share their edges exactly.
fn nine_slice(sprite: Sprite, rect: Rect, corner: f32, paint: Paint, quads: &mut Vec<UiQuad>) {
    let half = Vec2::from(sprite.size) * 0.5;
    // A box smaller than its corners takes smaller corners, keeping them square to the texels.
    let corner = corner.min(rect.width() * 0.5).min(rect.height() * 0.5);
    let per_texel = corner / half.x.min(half.y);
    let margin = SPREAD * per_texel;
    let inner = half * per_texel;
    let inner = Vec2::new(inner.x.min(rect.width() * 0.5), inner.y.min(rect.height() * 0.5));
    let xs = [rect.min.x - margin, rect.min.x + inner.x, rect.max.x - inner.x, rect.max.x + margin];
    let ys = [rect.min.y - margin, rect.min.y + inner.y, rect.max.y - inner.y, rect.max.y + margin];
    let [u0, v0, u1, v1] = sprite.uv;
    let (um, vm) = ((u0 + u1) * 0.5, (v0 + v1) * 0.5);
    let us = [u0, um, um, u1];
    let vs = [v0, vm, vm, v1];
    for row in 0..3 {
        for column in 0..3 {
            let piece = Rect::new(Vec2::new(xs[column], ys[row]), Vec2::new(xs[column + 1], ys[row + 1]));
            if piece.width() <= 0.0 || piece.height() <= 0.0 {
                continue;
            }
            let mut drawn = quad(piece, [us[column], vs[row], us[column + 1], vs[row + 1]], paint.color);
            paint.apply(&mut drawn, per_texel);
            quads.push(drawn);
        }
    }
}

/// The frame around a node with something wrong: four bars of a color no screen picks.
pub(super) fn problem_frame(rect: Rect, scale: f32, sprites: &Sprites, quads: &mut Vec<UiQuad>) {
    let rect = snapped(rect, scale);
    let uv = solid_uv(sprites);
    let (min, max, width) = (rect.min, rect.max, PROBLEM_FRAME);
    for piece in [
        Rect::new(min, Vec2::new(max.x, min.y + width)),
        Rect::new(Vec2::new(min.x, max.y - width), max),
        Rect::new(min, Vec2::new(min.x + width, max.y)),
        Rect::new(Vec2::new(max.x - width, min.y), max),
    ] {
        quads.push(quad(piece, uv, PROBLEM_COLOR));
    }
}
