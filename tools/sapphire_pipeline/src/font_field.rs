//! Display faces' glyphs as a multi-channel signed distance field (M3 U8, spec §5.17), made at cook
//! time with `fdsm` so the runtime only samples it: a title stays crisp at any size from one texture,
//! and nothing is rasterized while it shows.
//!
//! The field is made from the cut font a pack ships, never the whole file, because a subset numbers
//! its glyphs afresh and the runtime finds a field's glyphs by the ids it shaped. Each glyph's outline
//! comes through `skrifa` at the weight the sidecar names, is colored by `fdsm`'s simple edge
//! coloring with a fixed seed, generated as an MTSDF (red, green and blue whose median is the
//! distance, and the true distance in alpha), corrected for interpolation artifacts and sign, and
//! rounded to bytes. Cells go on shelves in an atlas whose side is the smallest power of two that holds
//! them, so a cook makes the same bytes every time.
//!
//! `fdsm` brings `nalgebra` and `image`, which is why this lives in the cooker
//! (`[tool_only_dependencies]` in xtask/layers.toml): the runtime gets plain texels.

use fdsm::bezier::scanline::FillRule;
use fdsm::bezier::{Point, Segment};
use fdsm::correct_error::{ErrorCorrectionConfig, correct_error_mtsdf};
use fdsm::generate::generate_mtsdf;
use fdsm::render::correct_sign_mtsdf;
use fdsm::shape::{Contour, Shape};
use image::{ImageBuffer, Rgba};
use sapphire_assets::font::{FieldGlyph, FontField};
use skrifa_for_skera::instance::{Location, Size};
use skrifa_for_skera::outline::{DrawSettings, OutlinePen};
use skrifa_for_skera::{FontRef, GlyphId, MetadataProvider as _};

/// Texels to the em a field is made at. 48 keeps the strokes of a dense kanji in Dela Gothic One
/// apart, and it's also the smallest size in pixels a field draws at: under it, coverage is sharper.
pub const EM: u16 = 48;
/// How far from an edge, in texels, a field reaches: room for an outline or a glow of up to an eighth
/// of an em, past which it clamps.
pub const SPREAD: f32 = 6.0;
/// Empty texels between cells, so the atlas's filter never reads a neighbour.
const GAP: u32 = 1;
/// The sides an atlas may have: a small field needs no big texture, and every device takes 2048.
const MIN_SIDE: u32 = 128;
const MAX_SIDE: u32 = 2048;
/// The corner threshold of `fdsm`'s edge coloring (the sine of the smallest angle that is a corner),
/// as its own example colors fonts, and the seed it switches colors by, fixed so a cook repeats.
const CORNER_SIN: f64 = 0.03;
const COLORING_SEED: u64 = 0;

/// A field that couldn't be made.
#[derive(Debug, thiserror::Error)]
pub enum FontFieldError {
    #[error("not a font: {0}")]
    NotAFont(String),
    #[error("the cut font has no glyph for {character:?} (U+{code:04X}), a display character", code = u32::from(*character))]
    Missing { character: char },
    #[error("can't draw glyph {id}: {problem}")]
    Outline { id: u32, problem: String },
    #[error(
        "{glyphs} display glyphs don't fit an atlas of {MAX_SIDE} square: show fewer characters on the field"
    )]
    TooBig { glyphs: usize },
}

/// One glyph's field before it's placed.
struct Cell {
    id: u32,
    width: u32,
    height: u32,
    /// RGBA bytes, row by row from the top.
    texels: Vec<u8>,
    left: f32,
    top: f32,
}

/// The field of `characters` in `font`, a cut font file, at `weight` along its `wght` axis (the
/// font's default where `None`, or where it has no such axis). Every character must have a glyph.
pub fn field(font: &[u8], characters: &str, weight: Option<f32>) -> Result<FontField, FontFieldError> {
    let font = FontRef::new(font).map_err(|error| FontFieldError::NotAFont(error.to_string()))?;
    let location = match weight {
        Some(weight) => font.axes().location([("wght", weight)]),
        None => Location::new(font.axes().len()),
    };
    let charmap = font.charmap();
    let mut ids = characters
        .chars()
        .map(|character| {
            charmap
                .map(character)
                .filter(|glyph| glyph.to_u32() != 0)
                .map(GlyphId::to_u32)
                .ok_or(FontFieldError::Missing { character })
        })
        .collect::<Result<Vec<_>, _>>()?;
    ids.sort_unstable();
    ids.dedup();
    let units_per_em = f64::from(font.metrics(Size::unscaled(), &location).units_per_em.max(1));
    let outlines = font.outline_glyphs();
    let cells = ids
        .iter()
        .map(|&id| {
            let mut pen = Pen::default();
            let glyph = outlines
                .get(GlyphId::new(id))
                .ok_or(FontFieldError::Outline { id, problem: "no outline".to_owned() })?;
            glyph
                .draw(DrawSettings::unhinted(Size::unscaled(), &location), &mut pen)
                .map_err(|error| FontFieldError::Outline { id, problem: error.to_string() })?;
            pen.close();
            Ok(cell(id, pen.contours, f64::from(EM) / units_per_em))
        })
        .collect::<Result<Vec<_>, FontFieldError>>()?;
    let (width, height, places) = pack(&cells).ok_or(FontFieldError::TooBig { glyphs: cells.len() })?;
    let mut texels = vec![0; (width * height * 4) as usize];
    let mut glyphs = Vec::with_capacity(cells.len());
    for (cell, &(x, y)) in cells.iter().zip(&places) {
        let row_bytes = (cell.width * 4) as usize;
        for row in 0..cell.height as usize {
            let to = ((y as usize + row) * width as usize + x as usize) * 4;
            texels[to..to + row_bytes].copy_from_slice(&cell.texels[row * row_bytes..(row + 1) * row_bytes]);
        }
        glyphs.push(FieldGlyph {
            id: cell.id,
            x: x as u16,
            y: y as u16,
            width: cell.width as u16,
            height: cell.height as u16,
            left: cell.left,
            top: cell.top,
        });
    }
    Ok(FontField {
        em: EM,
        spread: SPREAD,
        coords: location.coords().iter().map(|coord| coord.to_bits()).collect(),
        width: width as u16,
        height: height as u16,
        texels,
        glyphs,
    })
}

/// A curve's control points, two to four, in font units, y up.
type Curve = Vec<[f64; 2]>;

/// A glyph's contours as `skrifa` draws them.
#[derive(Default)]
struct Pen {
    contours: Vec<Vec<Curve>>,
    current: Vec<Curve>,
    start: [f64; 2],
    at: [f64; 2],
}

impl Pen {
    fn push(&mut self, mut curve: Curve) {
        let to = *curve.last().expect("a curve ends somewhere");
        curve.insert(0, self.at);
        self.current.push(curve);
        self.at = to;
    }

    fn close(&mut self) {
        if self.at != self.start {
            self.current.push(vec![self.at, self.start]);
            self.at = self.start;
        }
        if !self.current.is_empty() {
            self.contours.push(std::mem::take(&mut self.current));
        }
    }
}

fn point(x: f32, y: f32) -> [f64; 2] {
    [f64::from(x), f64::from(y)]
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        Pen::close(self);
        self.start = point(x, y);
        self.at = self.start;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        // A segment of no length has no direction, which edge coloring can't read.
        if point(x, y) != self.at {
            self.push(vec![point(x, y)]);
        }
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.push(vec![point(cx0, cy0), point(x, y)]);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.push(vec![point(cx0, cy0), point(cx1, cy1), point(x, y)]);
    }

    fn close(&mut self) {
        Pen::close(self);
    }
}

/// `contours` as a cell of field, `scale` texels to a font unit.
fn cell(id: u32, contours: Vec<Vec<Curve>>, scale: f64) -> Cell {
    let (mut min, mut max) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in contours.iter().flatten().flatten() {
        min = [min[0].min(p[0]), min[1].min(p[1])];
        max = [max[0].max(p[0]), max[1].max(p[1])];
    }
    if !(min[0] < max[0] && min[1] < max[1]) {
        return Cell { id, width: 0, height: 0, texels: Vec::new(), left: 0.0, top: 0.0 };
    }
    let spread = f64::from(SPREAD);
    let width = ((max[0] - min[0]) * scale + 2.0 * spread).ceil() as u32;
    let height = ((max[1] - min[1]) * scale + 2.0 * spread).ceil() as u32;
    // Font units to the cell's texels, still y up: the rows flip when they're copied out.
    let to_cell =
        |p: &[f64; 2]| Point::new((p[0] - min[0]) * scale + spread, (p[1] - min[1]) * scale + spread);
    let segment = |curve: &Curve| match curve.as_slice() {
        [a, b] => Segment::line(to_cell(a), to_cell(b)),
        [a, b, c] => Segment::quad(to_cell(a), to_cell(b), to_cell(c)),
        [a, b, c, d] => Segment::cubic(to_cell(a), to_cell(b), to_cell(c), to_cell(d)),
        _ => unreachable!("the pen makes curves of two to four points"),
    };
    let shape = Shape {
        contours: contours
            .iter()
            .map(|contour| Contour { segments: contour.iter().map(segment).collect() })
            .collect(),
    };
    let colored = Shape::edge_coloring_simple(shape, CORNER_SIN, COLORING_SEED);
    let prepared = colored.prepare();
    let range = 2.0 * spread;
    let mut image = ImageBuffer::<Rgba<f32>, Vec<f32>>::new(width, height);
    generate_mtsdf(&prepared, range, &mut image);
    correct_error_mtsdf(&mut image, &colored, &prepared, range, &ErrorCorrectionConfig::default());
    correct_sign_mtsdf(&mut image, &prepared, FillRule::Nonzero);
    let mut texels = Vec::with_capacity((width * height * 4) as usize);
    for row in (0..height).rev() {
        for column in 0..width {
            let Rgba(channels) = *image.get_pixel(column, row);
            texels.extend(channels.map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8));
        }
    }
    let em = f64::from(EM);
    Cell {
        id,
        width,
        height,
        texels,
        left: ((min[0] * scale - spread) / em) as f32,
        // The cell's top edge is `height - spread` texels above the outline's lowest point.
        top: (-(min[1] * scale - spread + f64::from(height)) / em) as f32,
    }
}

/// An atlas's width and height, and each cell's corner in it, in the cells' order.
type Packing = (u32, u32, Vec<(u32, u32)>);

/// Where each cell goes on shelves, tallest first, in the smallest square-or-wider power-of-two atlas
/// that holds them.
fn pack(cells: &[Cell]) -> Option<Packing> {
    let mut order: Vec<usize> = (0..cells.len()).collect();
    order.sort_by_key(|&at| (std::cmp::Reverse(cells[at].height), cells[at].id));
    let mut side = MIN_SIDE;
    while side <= MAX_SIDE {
        let mut places = vec![(0, 0); cells.len()];
        let (mut x, mut y, mut shelf) = (0, 0, 0);
        let mut fits = true;
        for &at in &order {
            let (width, height) = (cells[at].width, cells[at].height);
            if width == 0 {
                continue;
            }
            if x + width > side {
                (x, y, shelf) = (0, y + shelf + GAP, 0);
            }
            if width > side || y + height > side {
                fits = false;
                break;
            }
            places[at] = (x, y);
            x += width + GAP;
            shelf = shelf.max(height);
        }
        if fits {
            // As tall as its cells reach, so a few glyphs don't ship a square of nothing.
            let height = (y + shelf).max(1);
            return Some((side, height, places));
        }
        side *= 2;
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::font_subset;

    fn third_party(relative: &str) -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/third_party").join(relative);
        std::fs::read(path).expect("the font is in the tree")
    }

    /// The field's value at a texel of `glyph`'s cell: the median of three, and the true distance.
    fn sample(field: &FontField, glyph: &FieldGlyph, x: u32, y: u32) -> (u8, u8) {
        let at = (((u32::from(glyph.y) + y) * u32::from(field.width) + u32::from(glyph.x) + x) * 4) as usize;
        let [r, g, b, a] = [0, 1, 2, 3].map(|channel| field.texels[at + channel]);
        (r.max(g).min(r.min(g).max(b)), a)
    }

    #[test]
    fn a_glyph_s_cell_is_inside_where_its_ink_is_and_outside_around_it() {
        let oxanium = font_subset::subset(&third_party("oxanium/Oxanium[wght].ttf"), "I").unwrap();
        let field = field(&oxanium, "I", None).unwrap();
        let glyph = field.glyphs[0];
        let (width, height) = (u32::from(glyph.width), u32::from(glyph.height));
        // An I is a bar: its middle is ink, a texel or two from either side of a thin stem at the
        // default weight, and the cell's corners are past the spread.
        let (middle, true_middle) = sample(&field, &glyph, width / 2, height / 2);
        assert!(middle > 140 && true_middle > 140, "inside the bar: {middle}, {true_middle}");
        let (beside, _) = sample(&field, &glyph, width / 2 - 3, height / 2);
        assert!(beside < 128, "outside the bar three texels to the left: {beside}");
        assert_eq!(sample(&field, &glyph, 0, 0), (0, 0), "a corner is a spread from any ink");
        // The bar's foot sits on the baseline: the cell's bottom is a spread under it, in ems.
        let bottom = glyph.top + f32::from(glyph.height) / f32::from(EM);
        assert!((bottom - SPREAD / f32::from(EM)).abs() < 1.0 / f32::from(EM), "the cell's bottom {bottom}");
        assert!(glyph.top < -0.6, "an I's cap height is above the baseline: {}", glyph.top);
    }

    #[test]
    fn a_space_holds_its_place_with_an_empty_cell_and_a_missing_character_is_named() {
        let oxanium = font_subset::subset(&third_party("oxanium/Oxanium[wght].ttf"), "A ").unwrap();
        let field = field(&oxanium, "A A ", None).unwrap();
        assert_eq!(field.glyphs.len(), 2, "each glyph once");
        assert!(field.glyphs.iter().any(|glyph| glyph.width == 0 && glyph.height == 0), "the space");
        let error = super::field(&oxanium, "Aあ", None).unwrap_err().to_string();
        assert!(error.contains("'あ'") && error.contains("U+3042"), "{error}");
    }

    #[test]
    fn a_weight_is_an_instance_of_the_variable_font() {
        let oxanium = font_subset::subset(&third_party("oxanium/Oxanium[wght].ttf"), "H").unwrap();
        let regular = field(&oxanium, "H", None).unwrap();
        let bold = field(&oxanium, "H", Some(700.0)).unwrap();
        assert_eq!(regular.coords.len(), 1, "Oxanium has one axis");
        assert_ne!(regular.coords, bold.coords);
        assert_ne!(regular.texels, bold.texels, "a heavier H");
    }

    /// The same font and characters make the same bytes, and those bytes are pinned: a change to the
    /// cut, the outlines' reader, `fdsm` or the packing shows here before it moves a pack.
    #[test]
    fn a_field_is_the_same_bytes_every_time_and_they_are_pinned() {
        use std::io::Write as _;

        const SHOWN: &str = "VICTORY 0123 Settings";
        let oxanium = font_subset::subset(&third_party("oxanium/Oxanium[wght].ttf"), SHOWN).unwrap();
        let first = field(&oxanium, SHOWN, Some(700.0)).unwrap();
        let again = field(&oxanium, SHOWN, Some(700.0)).unwrap();
        assert_eq!(first, again);
        let mut hasher = blake3::Hasher::new();
        hasher.update(&first.texels);
        for glyph in &first.glyphs {
            hasher.update(&glyph.id.to_le_bytes());
            hasher.update(&[glyph.x, glyph.y, glyph.width, glyph.height].map(u16::to_le_bytes).concat());
            hasher.update(&[glyph.left, glyph.top].map(f32::to_le_bytes).concat());
        }
        let digest = hasher.finalize().to_hex();
        let digest = &digest[..16];
        let line = format!(
            "font_field digest {digest} for {} glyphs in {} x {} on {}\n",
            first.glyphs.len(),
            first.width,
            first.height,
            std::env::consts::ARCH
        );
        std::io::stderr().write_all(line.as_bytes()).expect("stderr takes it");
        assert_eq!(digest, "9b572b853c94e832", "the field moved; {line}");
    }

    #[test]
    fn cells_pack_into_the_smallest_power_of_two_that_holds_them() {
        let cell = |id, width, height| Cell { id, width, height, texels: Vec::new(), left: 0.0, top: 0.0 };
        let (width, height, places) = pack(&[cell(1, 60, 20), cell(2, 60, 30), cell(3, 0, 0)]).unwrap();
        assert_eq!((width, height), (128, 30), "side by side, as tall as the taller");
        assert_eq!(places[1], (0, 0), "tallest first");
        assert_eq!(places[0], (61, 0), "a texel apart");
        let many: Vec<_> = (0..100).map(|id| cell(id, 100, 100)).collect();
        let (width, _, _) = pack(&many).unwrap();
        assert_eq!(width, 1024);
        assert!(pack(&[cell(1, 2049, 1)]).is_none());
    }
}
