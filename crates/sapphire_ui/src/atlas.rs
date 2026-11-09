//! The generated atlas: every shape the HUD draws, painted by code at load as a signed distance
//! field, with no art to license or cook.
//!
//! One channel, 320 × 216: 68 KiB (the crate docs have its measured cost). Each shape is exact
//! geometry, so the field is true distance to its edge, and only square roots and arithmetic go
//! into it, which every platform computes alike.
//!
//! - **Digits** are strokes of constant width along chamfered center lines, tabular so a number
//!   that changes doesn't jitter. An offset polygon with mitered corners keeps their corners
//!   sharper than round joins would; the bilinear filter rounds a corner by about a texel, and a
//!   phone draws a 28-point digit about twice the texel's size, so a corner rounds by about 2
//!   pixels of 84.
//! - **Disc** is a circle: a ring is its outline with no fill ([`Paint`](crate::Paint)), at any
//!   thickness the field's spread reaches, so a thin ring isn't a thin feature in the texture.
//! - **Bar** is a square with small rounded corners, three-sliced by the shader to any length.
//! - **Button** is a square with two corners cut hard and two barely, read as a blade's cut.
//! - **Icons:** a sword (attack), two chevrons (dodge), a four-point spark (skill) and a wedge (the
//!   stick's direction).

use std::sync::Arc;

use sapphire_look::{UiAtlas, UiFieldFormat};
use sapphire_math::Vec2;

/// The atlas's size. The shelves below fill it nearly to its last row.
pub const ATLAS_WIDTH: u32 = 320;
pub const ATLAS_HEIGHT: u32 = 216;
/// How far the field reaches from an edge, in texels: the most an outline or glow can spread.
pub const SPREAD: f32 = 6.0;

/// A digit's size in texels. Its height decides how crisp a large one is.
const DIGIT: [f32; 2] = [28.0, 44.0];
const BUTTON: f32 = 76.0;
const ICON: f32 = 60.0;
const DISC: f32 = 52.0;
const WEDGE: f32 = 32.0;
const BAR: f32 = 16.0;
/// The bar's corner radius, in texels.
pub(crate) const BAR_CORNER: f32 = 3.0;

/// The digits' stroke width and grid, on which `0` is 5 wide and 9 tall along its center lines.
const STROKE: f32 = 1.7;
const GRID: [f32; 2] = [5.0, 9.0];

/// One shape's place in the atlas.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sprite {
    /// The whole cell, its margin of [`SPREAD`] texels included: `u0, v0, u1, v1`.
    pub uv: [f32; 4],
    /// The shape's size in texels, without the margin.
    pub size: [f32; 2],
}

/// Every shape in the atlas.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sprites {
    pub digits: [Sprite; 10],
    pub disc: Sprite,
    pub bar: Sprite,
    pub button: Sprite,
    pub sword: Sprite,
    pub chevrons: Sprite,
    pub spark: Sprite,
    pub wedge: Sprite,
}

/// The painted atlas and where its shapes are.
#[derive(Clone, Debug)]
pub struct Atlas {
    /// What the frame packet's draw list names.
    pub image: Arc<UiAtlas>,
    pub sprites: Sprites,
}

impl Atlas {
    /// Paints every shape. A load-time cost, once per process in practice: the image is shared by
    /// every list that names it, and a new renderer uploads it again from the same bytes.
    pub fn paint() -> Self {
        profiling::scope!("ui atlas");
        let mut canvas = Canvas::new();
        let digit_shapes = digits();
        let mut sprites = Sprites::default();
        for (sprite, parts) in sprites.digits.iter_mut().zip(&digit_shapes) {
            *sprite = canvas.polygon(DIGIT, parts);
        }
        sprites.disc = canvas.shape([DISC; 2], |p| {
            let centered = p - Vec2::splat(DISC * 0.5);
            DISC * 0.5 - centered.length()
        });
        sprites.wedge = canvas.polygon([WEDGE; 2], &[scaled(&WEDGE_POINTS, WEDGE)]);
        sprites.bar = canvas.shape([BAR; 2], |p| {
            let inner = BAR * 0.5 - BAR_CORNER;
            let q = (p - Vec2::splat(BAR * 0.5)).abs() - Vec2::splat(inner);
            BAR_CORNER - q.max(Vec2::ZERO).length() - q.x.max(q.y).min(0.0)
        });
        sprites.button = canvas.polygon([BUTTON; 2], &[scaled(&BUTTON_CUT, BUTTON)]);
        sprites.sword = canvas.polygon([ICON; 2], &sword());
        sprites.chevrons = canvas.polygon([ICON; 2], &chevrons());
        sprites.spark = canvas.polygon([ICON; 2], &[scaled(&SPARK, ICON)]);
        let image = UiAtlas {
            width: ATLAS_WIDTH,
            height: ATLAS_HEIGHT,
            spread: SPREAD,
            format: UiFieldFormat::Single,
            texels: canvas.texels,
        };
        Self { image: Arc::new(image), sprites }
    }
}

/// The atlas as it fills: shapes placed on shelves, left to right.
struct Canvas {
    texels: Vec<u8>,
    x: u32,
    y: u32,
    row: u32,
}

impl Canvas {
    fn new() -> Self {
        Self { texels: vec![0; (ATLAS_WIDTH * ATLAS_HEIGHT) as usize], x: 0, y: 0, row: 0 }
    }

    /// Paints a shape `size` texels large, from its distance at each texel's center, in texels
    /// from the shape's top-left and positive inside.
    fn shape(&mut self, size: [f32; 2], distance: impl Fn(Vec2) -> f32) -> Sprite {
        let cell_w = (size[0] + SPREAD * 2.0).ceil() as u32;
        let cell_h = (size[1] + SPREAD * 2.0).ceil() as u32;
        if self.x + cell_w > ATLAS_WIDTH {
            self.y += self.row;
            self.x = 0;
            self.row = 0;
        }
        assert!(self.y + cell_h <= ATLAS_HEIGHT, "the atlas is full at a shape of {size:?}");
        for row in 0..cell_h {
            let start = ((self.y + row) * ATLAS_WIDTH + self.x) as usize;
            let line = &mut self.texels[start..start + cell_w as usize];
            for (column, texel) in line.iter_mut().enumerate() {
                let p = Vec2::new(column as f32 + 0.5, row as f32 + 0.5) - Vec2::splat(SPREAD);
                let value = 0.5 + distance(p) / (2.0 * SPREAD);
                *texel = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
        let (width, height) = (ATLAS_WIDTH as f32, ATLAS_HEIGHT as f32);
        let uv = [
            self.x as f32 / width,
            self.y as f32 / height,
            (self.x + cell_w) as f32 / width,
            (self.y + cell_h) as f32 / height,
        ];
        self.x += cell_w;
        self.row = self.row.max(cell_h);
        Sprite { uv, size }
    }

    /// Paints the union of `parts`.
    fn polygon(&mut self, size: [f32; 2], parts: &[Part]) -> Sprite {
        self.shape(size, |p| parts.iter().map(|part| part.distance(p)).fold(f32::MIN, f32::max))
    }
}

/// Closed loops of points, filled even-odd, so a hole is a second loop inside the first.
#[derive(Clone, Debug, Default)]
struct Part {
    loops: Vec<Vec<Vec2>>,
}

impl Part {
    /// The signed distance to the nearest edge of any loop, positive inside.
    fn distance(&self, p: Vec2) -> f32 {
        let mut nearest = f32::MAX;
        let mut inside = false;
        for points in &self.loops {
            let Some(&last) = points.last() else { continue };
            let mut a = last;
            for &b in points {
                let (edge, from_a) = (b - a, p - a);
                let along = (from_a.dot(edge) / edge.length_squared()).clamp(0.0, 1.0);
                nearest = nearest.min((from_a - edge * along).length_squared());
                if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y) {
                    inside = !inside;
                }
                a = b;
            }
        }
        if inside { nearest.sqrt() } else { -nearest.sqrt() }
    }
}

/// A loop from points in −1..1 on both axes, in a square `size` texels on a side.
fn scaled(points: &[[f32; 2]], size: f32) -> Part {
    Part { loops: vec![points.iter().map(|&[x, y]| (Vec2::new(x, y) + Vec2::ONE) * size * 0.5).collect()] }
}

/// A stroke `width` wide along `points`, with mitered corners and square-cut ends: a closed one is
/// two loops, an open one a single loop down one side and back up the other.
fn stroke(points: &[Vec2], closed: bool, width: f32) -> Part {
    let half = width * 0.5;
    let count = points.len();
    let normal = |from: Vec2, to: Vec2| (to - from).normalize().perp();
    let (mut left, mut right) = (Vec::with_capacity(count), Vec::with_capacity(count));
    for (index, &point) in points.iter().enumerate() {
        let before = if index > 0 || closed { Some(points[(index + count - 1) % count]) } else { None };
        let after = if index + 1 < count || closed { Some(points[(index + 1) % count]) } else { None };
        let incoming = before.map(|before| normal(before, point));
        let outgoing = after.map(|after| normal(point, after));
        let (first, second) = match (incoming, outgoing) {
            (Some(first), Some(second)) => (first, second),
            (Some(only), None) | (None, Some(only)) => (only, only),
            (None, None) => unreachable!("a stroke has at least two points"),
        };
        let miter = (first + second).normalize();
        // A hairpin's miter would spike: at most 2.5 half widths out.
        let length = (half / miter.dot(second)).min(half * 2.5);
        left.push(point + miter * length);
        right.push(point - miter * length);
    }
    if closed {
        Part { loops: vec![left, right] }
    } else {
        right.reverse();
        left.append(&mut right);
        Part { loops: vec![left] }
    }
}

/// The ten digits, in a [`DIGIT`] box.
fn digits() -> [Vec<Part>; 10] {
    // Open center lines on the digits' grid, +y down. Each digit is one or two strokes.
    let scale = Vec2::new(DIGIT[0] / (GRID[0] + STROKE), DIGIT[1] / (GRID[1] + STROKE));
    let offset = Vec2::splat(STROKE * 0.5);
    let open = |points: &[[f32; 2]]| {
        let points: Vec<Vec2> = points.iter().map(|&[x, y]| (Vec2::new(x, y) + offset) * scale).collect();
        stroke(&points, false, STROKE * scale.y)
    };
    let closed = |points: &[[f32; 2]]| {
        let points: Vec<Vec2> = points.iter().map(|&[x, y]| (Vec2::new(x, y) + offset) * scale).collect();
        stroke(&points, true, STROKE * scale.y)
    };
    const SIX: [[f32; 2]; 10] = [
        [5.0, 0.0],
        [1.0, 0.0],
        [0.0, 1.0],
        [0.0, 8.0],
        [1.0, 9.0],
        [4.0, 9.0],
        [5.0, 8.0],
        [5.0, 5.3],
        [4.0, 4.3],
        [0.0, 4.3],
    ];
    let nine = SIX.map(|[x, y]| [GRID[0] - x, GRID[1] - y]);
    [
        vec![closed(&[
            [1.0, 0.0],
            [4.0, 0.0],
            [5.0, 1.0],
            [5.0, 8.0],
            [4.0, 9.0],
            [1.0, 9.0],
            [0.0, 8.0],
            [0.0, 1.0],
        ])],
        vec![open(&[[0.8, 1.9], [3.0, 0.0], [3.0, 9.0]])],
        vec![open(&[
            [0.0, 2.3],
            [0.0, 1.0],
            [1.0, 0.0],
            [4.0, 0.0],
            [5.0, 1.0],
            [5.0, 3.2],
            [0.0, 7.4],
            [0.0, 9.0],
            [5.2, 9.0],
        ])],
        vec![
            open(&[[0.2, 0.0], [4.0, 0.0], [5.0, 1.0], [5.0, 3.5], [4.0, 4.5], [1.5, 4.5]]),
            open(&[[3.5, 4.5], [4.0, 4.5], [5.0, 5.5], [5.0, 8.0], [4.0, 9.0], [0.2, 9.0]]),
        ],
        vec![open(&[[3.9, 9.0], [3.9, 0.0]]), open(&[[3.9, 0.4], [0.0, 6.2], [5.4, 6.2]])],
        vec![open(&[
            [5.0, 0.0],
            [0.4, 0.0],
            [0.2, 4.3],
            [4.0, 4.3],
            [5.0, 5.3],
            [5.0, 8.0],
            [4.0, 9.0],
            [0.0, 9.0],
        ])],
        vec![open(&SIX)],
        vec![open(&[[0.0, 0.0], [5.0, 0.0], [5.0, 1.2], [1.6, 9.0]])],
        vec![
            closed(&[
                [1.0, 0.0],
                [4.0, 0.0],
                [5.0, 1.0],
                [5.0, 3.5],
                [4.0, 4.5],
                [5.0, 5.5],
                [5.0, 8.0],
                [4.0, 9.0],
                [1.0, 9.0],
                [0.0, 8.0],
                [0.0, 5.5],
                [1.0, 4.5],
                [0.0, 3.5],
                [0.0, 1.0],
            ]),
            open(&[[1.0, 4.5], [4.0, 4.5]]),
        ],
        vec![open(&nine)],
    ]
}

/// A square with its top-left and bottom-right corners cut hard, the others barely.
const BUTTON_CUT: [[f32; 2]; 8] = [
    [-0.42, -1.0],
    [0.86, -1.0],
    [1.0, -0.86],
    [1.0, 0.42],
    [0.42, 1.0],
    [-0.86, 1.0],
    [-1.0, 0.86],
    [-1.0, -0.42],
];

/// A four-point spark.
const SPARK: [[f32; 2]; 8] =
    [[0.0, -1.0], [0.2, -0.2], [1.0, 0.0], [0.2, 0.2], [0.0, 1.0], [-0.2, 0.2], [-1.0, 0.0], [-0.2, -0.2]];

/// A wedge pointing up, its base notched.
const WEDGE_POINTS: [[f32; 2]; 4] = [[0.0, -1.0], [1.0, 0.7], [0.0, 0.15], [-1.0, 0.7]];

/// A sword from bottom-left to top-right: blade, guard and grip.
fn sword() -> Vec<Part> {
    // (along the blade from the guard, across it), to texels. `SQRT` is written out: no libm.
    const SQRT_HALF: f32 = 0.707_106_77;
    let place = |along: f32, across: f32| {
        let unit = Vec2::new(along + across, -along + across) * SQRT_HALF;
        (unit * 0.92 + Vec2::ONE) * ICON * 0.5
    };
    let quad = |points: &[[f32; 2]]| Part {
        loops: vec![points.iter().map(|&[along, across]| place(along, across)).collect()],
    };
    vec![
        quad(&[[-0.28, -0.13], [0.8, -0.13], [1.08, 0.0], [0.8, 0.13], [-0.28, 0.13]]),
        quad(&[[-0.36, -0.4], [-0.24, -0.4], [-0.24, 0.4], [-0.36, 0.4]]),
        quad(&[[-0.82, -0.08], [-0.3, -0.08], [-0.3, 0.08], [-0.82, 0.08]]),
        quad(&[[-1.02, -0.14], [-0.82, -0.14], [-0.82, 0.14], [-1.02, 0.14]]),
    ]
}

/// Two chevrons running to the right.
fn chevrons() -> Vec<Part> {
    let at = |x: f32, y: f32| (Vec2::new(x, y) + Vec2::ONE) * ICON * 0.5;
    let chevron = |x: f32| stroke(&[at(x, -0.72), at(x + 0.56, 0.0), at(x, 0.72)], false, ICON * 0.15);
    vec![chevron(-0.72), chevron(0.04)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texel(atlas: &Atlas, u: f32, v: f32) -> u8 {
        let image = &atlas.image;
        let x = ((u * image.width as f32) as u32).min(image.width - 1);
        let y = ((v * image.height as f32) as u32).min(image.height - 1);
        image.texels[(y * image.width + x) as usize]
    }

    /// The field value at a point of `sprite`, as shares of its cell.
    fn sample(atlas: &Atlas, sprite: Sprite, x: f32, y: f32) -> u8 {
        let [u0, v0, u1, v1] = sprite.uv;
        texel(atlas, u0 + (u1 - u0) * x, v0 + (v1 - v0) * y)
    }

    #[test]
    fn the_atlas_is_small_and_every_shape_fits() {
        let atlas = Atlas::paint();
        assert_eq!(atlas.image.texels.len(), 69_120, "one channel, 320 × 216");
        let sprites = atlas.sprites;
        let mut all = sprites.digits.to_vec();
        all.extend([
            sprites.disc,
            sprites.bar,
            sprites.button,
            sprites.sword,
            sprites.chevrons,
            sprites.spark,
            sprites.wedge,
        ]);
        for (index, a) in all.iter().enumerate() {
            assert!(a.uv[2] <= 1.0 && a.uv[3] <= 1.0, "{a:?} leaves the atlas");
            for b in &all[index + 1..] {
                let apart =
                    a.uv[2] <= b.uv[0] || b.uv[2] <= a.uv[0] || a.uv[3] <= b.uv[1] || b.uv[3] <= a.uv[1];
                assert!(apart, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn a_field_is_half_at_its_edge_and_clamps_past_its_spread() {
        let atlas = Atlas::paint();
        let disc = atlas.sprites.disc;
        let cell = DISC + SPREAD * 2.0;
        // The middle is deep inside; the cell's corner, far outside.
        assert_eq!(sample(&atlas, disc, 0.5, 0.5), 255);
        assert_eq!(sample(&atlas, disc, 0.01, 0.01), 0);
        // On the circle, to within a texel's distance in field values.
        let on_edge = sample(&atlas, disc, (SPREAD + 0.5) / cell, 0.5);
        assert!((i32::from(on_edge) - 128).abs() <= 22, "{on_edge}");
    }

    #[test]
    fn digits_differ_and_have_their_holes() {
        let atlas = Atlas::paint();
        let digit_cell = |digit: usize| {
            let sprite = atlas.sprites.digits[digit];
            let [u0, v0, u1, v1] = sprite.uv;
            let (width, height) = (atlas.image.width as f32, atlas.image.height as f32);
            let (x0, y0, x1, y1) =
                ((u0 * width) as u32, (v0 * height) as u32, (u1 * width) as u32, (v1 * height) as u32);
            let mut cells = Vec::new();
            for y in y0..y1 {
                let start = (y * atlas.image.width + x0) as usize;
                cells.extend(atlas.image.texels[start..start + (x1 - x0) as usize].iter().map(|&t| t >= 128));
            }
            cells
        };
        let cells: Vec<Vec<bool>> = (0..10).map(digit_cell).collect();
        for a in 0..10 {
            let inked = cells[a].iter().filter(|&&on| on).count();
            assert!(inked > cells[a].len() / 8, "{a} has so little ink");
            for b in a + 1..10 {
                let different = cells[a].iter().zip(&cells[b]).filter(|(x, y)| x != y).count();
                assert!(different > cells[a].len() / 30, "{a} and {b} read alike ({different})");
            }
        }
        // Each digit's middle column, top to bottom: strokes crossed, as a reader counts them.
        let crossings = |digit: usize| {
            let sprite = atlas.sprites.digits[digit];
            let mut count = 0;
            let mut last = false;
            for step in 0..100 {
                let on = sample(&atlas, sprite, 0.5, step as f32 / 99.0) >= 128;
                count += usize::from(on && !last);
                last = on;
            }
            count
        };
        assert_eq!(crossings(0), 2, "0 is open in the middle");
        assert_eq!(crossings(8), 3, "8 has its waist");
        assert_eq!(crossings(3), 3);
    }

    #[test]
    fn painting_is_repeatable() {
        let (first, second) = (Atlas::paint(), Atlas::paint());
        assert_eq!(first.image, second.image);
        assert_eq!(first.sprites, second.sprites);
    }
}
