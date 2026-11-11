//! WCAG 2.3.1's flash tests over a capture sequence (M4 A3; exit test 6): [`FlashAnalyser`] reads frames
//! of 8-bit sRGB pixels one at a time, at a known rate, and says how close the sequence comes to the general
//! and the red flash thresholds ([`FlashReport`]). `sapphire_testkit flashes` runs it over a directory of a
//! capture's frames.
//!
//! # What WCAG says (WCAG 2.2, read 2026-11-10, <https://www.w3.org/TR/WCAG22/>)
//!
//! Success criterion 2.3.1: nothing flashes more than three times in any one second, or the flash is below
//! the general flash and red flash thresholds. Content is below them if there are no more than three general
//! and no more than three red flashes in any second, or if the flashes occurring at once cover no more than
//! 25% of any 10° visual field. A general flash is a pair of opposing changes in relative luminance of 0.1 or
//! more where the darker state is below 0.8; a red flash, a pair of opposing transitions involving a
//! saturated red: a state whose R/(R + G + B) is 0.8 or more, with a change of more than 0.2 in CIE 1976 UCS
//! (u′v′) chromaticity between the two (from ISO 9241-391). A transition is the change between adjacent peaks
//! and valleys. For a screen, a 341 × 256 rectangle of a 1024 × 768 one stands for the 10° field, and fine,
//! balanced patterns under 0.1° (white noise, a fine checkerboard) don't count.
//!
//! # How it reads them
//!
//! - **Tiles.** The 10° field is a third of the capture's height tall, with WCAG's 341:256 shape, so it's
//!   341 × 256 at 1024 × 768, and on a wider screen it's sized by the height, as the eye's field is sized by
//!   the viewing distance, not the screen's width: the smaller field there is the stricter. The capture is
//!   cut into cells of a hundredth of the field's height, 0.1°, the finest pattern WCAG still counts, and a
//!   cell's mean colour in linear light is what it shows: averaging is what lets a balanced pattern finer than
//!   that through, as WCAG lets it. A capture under 300 pixels tall has a cell a pixel.
//! - **Transitions.** Each cell's relative luminance is followed through its peaks and valleys: a leg from
//!   one to the next of at least 0.1, with its darker end under 0.8, is a general transition, brightening or
//!   darkening, counted on the frame where it gets there. A cell's red transitions are its changes of more
//!   than 0.2 in u′v′ from the state it last turned from, one end a saturated red (by its linear R, G and B),
//!   to a red or from one. A cell darker than [`BLACK`] has no chromaticity and reads as the white point: our
//!   floor, not WCAG's, so a dark cell's noise doesn't read as red.
//! - **Concurrent, over a quarter of a field.** Flashes "occurring concurrently" are the same transition at
//!   the same time: cells whose newest transition was the same way, within [`CONCURRENT`] of a second, a frame
//!   or two, so a flash whose cells cross the line a frame apart is still one. A field makes a transition when
//!   those cells come to cover more than a quarter of it beyond the share changing the other way, at every
//!   place a field can stand, a cell apart. So a pattern moving across a field, its cells brightening as
//!   as many others darken (a pan over a lattice of windows), is no flash of the field's, as a fine balanced
//!   pattern isn't. That's our reading, not WCAG's words: it also lets through a coarse balanced pattern
//!   reversing in place, which can't be told from a pan over one cell by cell, and which WCAG's exemption for
//!   fine patterns suggests it would count; and a flash over a field that's also changing the other way
//!   elsewhere counts only by what it leads by.
//! - **A second.** A flash is two transitions, so more than three flashes in a second is more than six of a
//!   field's transitions in any run of a second's frames.
//!
//! The score is the most transitions any field made in a second ([`Verdict::rate`]): 6 or fewer passes.
//! [`Verdict::area`] is the largest share of a field that changed one way at once beyond the other, to read
//! how near a sequence came to a field's flash.

use crate::golden::Image;
use crate::look::{Region, linear};

/// A general flash's change in relative luminance: this or more.
pub const GENERAL_SWING: f32 = 0.1;
/// Its darker state's relative luminance: under this.
pub const DARKER_UNDER: f32 = 0.8;
/// A saturated red's share of its light, R / (R + G + B): this or more.
pub const SATURATED_RED: f32 = 0.8;
/// A red transition's change in CIE 1976 u′v′ chromaticity: more than this.
pub const RED_CHANGE: f32 = 0.2;
/// The most transitions a second that pass: three flashes.
pub const MOST_TRANSITIONS: u32 = 6;
/// The most of a 10° field flashes may cover at once and pass.
pub const MOST_AREA: f32 = 0.25;
/// Relative luminance under which a cell is black, with no chromaticity of its own (our floor).
pub const BLACK: f32 = 0.005;
/// Cells to a field's height: 0.1° each.
const FIELD_CELLS: u32 = 100;
/// Fields to a capture's height.
const FIELDS_HIGH: u32 = 3;
/// The most frames a second it reads: a cell's transitions are a bit a frame in a `u128`.
pub const MOST_FRAMES_A_SECOND: u32 = 128;
/// How far apart, in seconds, two cells' transitions the same way still happen at once.
pub const CONCURRENT: f32 = 1.0 / 30.0;
/// The D65 white point in u′v′: black's chromaticity.
const WHITE: [f32; 2] = [0.197_83, 0.468_32];

/// Why a sequence couldn't be read.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum FlashError {
    #[error("{0} frames a second: expected 1 to {MOST_FRAMES_A_SECOND}")]
    Rate(u32),
    #[error("a frame of {got:?} in a sequence of {want:?}")]
    Size { got: (u32, u32), want: (u32, u32) },
    #[error("a {0:?} frame has no pixels")]
    Empty((u32, u32)),
    #[error("a {width}x{height} frame with {bytes} bytes")]
    Bytes { width: u32, height: u32, bytes: usize },
}

/// How a capture is cut into cells, and how large a 10° field is in them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tiles {
    /// Cells across and down.
    pub columns: u32,
    pub rows: u32,
    /// A field's width and height in cells.
    pub field: (u32, u32),
    /// Where each column and row starts, in pixels, and the edge after the last.
    x_edges: Vec<u32>,
    y_edges: Vec<u32>,
}

impl Tiles {
    /// The cells of a `width` × `height` capture (see the module docs, "Tiles").
    pub fn new(width: u32, height: u32) -> Self {
        let rows = height.clamp(1, FIELD_CELLS * FIELDS_HIGH);
        let cell = f64::from(height) / f64::from(rows);
        // A cell is as wide as it's tall, near enough to land on whole pixels.
        let columns = ((f64::from(width) / cell).round() as u32).clamp(1, width.max(1));
        let field_high = (rows / FIELDS_HIGH).max(1);
        let field_wide = ((f64::from(field_high) * 341.0 / 256.0).round() as u32).clamp(1, columns);
        let edges = |pixels: u32, cells: u32| {
            (0..=cells).map(|at| (u64::from(at) * u64::from(pixels) / u64::from(cells)) as u32).collect()
        };
        Self {
            columns,
            rows,
            field: (field_wide, field_high),
            x_edges: edges(width, columns),
            y_edges: edges(height, rows),
        }
    }

    fn count(&self) -> usize {
        (self.columns * self.rows) as usize
    }

    /// The places a field can stand, a cell apart: across, and in all.
    fn places_across(&self) -> u32 {
        self.columns - self.field.0 + 1
    }

    fn places(&self) -> usize {
        (self.places_across() * (self.rows - self.field.1 + 1)) as usize
    }

    /// The pixels of the field whose top-left cell is (`column`, `row`).
    fn field_region(&self, column: u32, row: u32) -> Region {
        let (x, y) = (self.x_edges[column as usize], self.y_edges[row as usize]);
        let right = self.x_edges[(column + self.field.0) as usize];
        let bottom = self.y_edges[(row + self.field.1) as usize];
        Region { x, y, width: right - x, height: bottom - y }
    }
}

/// How close a sequence came to one of the two thresholds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Verdict {
    /// The most transitions any 10° field made in a second, each more than [`MOST_AREA`] of it changing the same
    /// way at once: twice the flashes. More than [`MOST_TRANSITIONS`] fails.
    pub rate: u32,
    /// The frame that second ended on, and the field.
    pub rate_at: Option<(u32, Region)>,
    /// The frames of that second the field made its transitions on, a bit each, that frame lowest.
    pub rate_frames: u128,
    /// The largest share of a 10° field whose cells made a transition one way at once, less the share that made
    /// one the other way: past [`MOST_AREA`], the field made one.
    pub area: f32,
    /// Where that was.
    pub area_at: Option<(u32, Region)>,
}

impl Verdict {
    /// Whether it's within WCAG 2.3.1's threshold.
    pub fn passes(&self) -> bool {
        self.rate <= MOST_TRANSITIONS
    }
}

/// What [`FlashAnalyser::report`] found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlashReport {
    pub frames: u32,
    pub general: Verdict,
    pub red: Verdict,
}

impl FlashReport {
    /// Whether the sequence passes both of WCAG 2.3.1's flash tests.
    pub fn passes(&self) -> bool {
        self.general.passes() && self.red.passes()
    }
}

/// A cell's relative luminance through its peaks and valleys: the leg it's on, from its last turn. A leg
/// begins only once it's gone [`GENERAL_SWING`] back from the last one's furthest, so wobbles smaller than a
/// transition never turn it.
#[derive(Clone, Copy, Debug, Default)]
struct Swing {
    /// +1 rising, -1 falling, 0 before its first leg.
    leg: i8,
    /// On a leg, where it turned and the furthest it's gone since; before the first, the least and the most
    /// it's been.
    turn: f32,
    reach: f32,
    /// Whether this leg has been counted as a transition.
    counted: bool,
}

impl Swing {
    fn start(luminance: f32) -> Self {
        Self { leg: 0, turn: luminance, reach: luminance, counted: false }
    }

    /// Follows the cell to `luminance`: whether a general transition got there on this frame, brightening (+1)
    /// or darkening (-1), or none (0).
    fn step(&mut self, luminance: f32) -> i8 {
        let y = luminance;
        match self.leg {
            0 => {
                let (least, most) = (self.turn.min(y), self.reach.max(y));
                if y - least >= GENERAL_SWING {
                    *self = Self { leg: 1, turn: least, reach: y, counted: false };
                } else if most - y >= GENERAL_SWING {
                    *self = Self { leg: -1, turn: most, reach: y, counted: false };
                } else {
                    (self.turn, self.reach) = (least, most);
                    return 0;
                }
            }
            leg => {
                let further = f32::from(leg) * (y - self.reach);
                if further >= 0.0 {
                    self.reach = y;
                } else if -further >= GENERAL_SWING {
                    // It turned at the leg's furthest: a new leg the other way, already a transition's length.
                    *self = Self { leg: -leg, turn: self.reach, reach: y, counted: false };
                }
            }
        }
        let (turn, now) = (self.turn, self.reach);
        if !self.counted && (now - turn).abs() >= GENERAL_SWING && turn.min(now) < DARKER_UNDER {
            self.counted = true;
            return self.leg;
        }
        0
    }
}

/// A cell's colour as the red test reads it: its chromaticity and whether it's a saturated red.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Hue {
    uv: [f32; 2],
    red: bool,
}

impl Hue {
    /// Linear `rgb`'s.
    fn of([r, g, b]: [f32; 3]) -> Self {
        let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        if luminance < BLACK {
            return Self { uv: WHITE, red: false };
        }
        let x = 0.4124 * r + 0.3576 * g + 0.1805 * b;
        let z = 0.0193 * r + 0.1192 * g + 0.9505 * b;
        let under = x + 15.0 * luminance + 3.0 * z;
        let red = r / (r + g + b) >= SATURATED_RED;
        Self { uv: [4.0 * x / under, 9.0 * luminance / under], red }
    }

    fn distance(self, other: Self) -> f32 {
        (self.uv[0] - other.uv[0]).hypot(self.uv[1] - other.uv[1])
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Cell {
    swing: Swing,
    /// The state of its last red transition, or its first.
    turned: Hue,
    /// A bit a frame, newest lowest, set where a transition got there: brightening and darkening, and to a
    /// saturated red and from one.
    general: [u128; 2],
    red: [u128; 2],
}

/// One of the two tests over every place a field can stand, a cell apart, in rows.
#[derive(Debug)]
struct Fields {
    /// Whether more than a quarter of the field was changing each way on the last frame.
    above: Vec<[bool; 2]>,
    /// A bit a frame, newest lowest, set where the field made a transition.
    transitions: Vec<u128>,
    verdict: Verdict,
    trace: Vec<Traced>,
}

impl Fields {
    fn new(places: usize) -> Self {
        Self {
            above: vec![[false; 2]; places],
            transitions: vec![0; places],
            verdict: Verdict::default(),
            trace: Vec::new(),
        }
    }
}

/// One frame of a test's trace ([`FlashAnalyser::trace`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Traced {
    /// The most transitions any field made in the second ending on this frame.
    pub most: u32,
    /// Whether some field made a transition on it.
    pub transition: bool,
}

/// Reads a sequence a frame at a time (see the module docs).
#[derive(Debug)]
pub struct FlashAnalyser {
    tiles: Tiles,
    size: (u32, u32),
    second: u32,
    frames: u32,
    cells: Vec<Cell>,
    /// Each cell's pixels, and this frame's linear light summed over them.
    pixels: Vec<u32>,
    sums: Vec<[f32; 3]>,
    /// Frames apart that two transitions still happen at once ([`CONCURRENT`]).
    concurrent: u32,
    /// Summed-area tables over the cells: of the pixels changing each way lately, and of all of them.
    tables: [Vec<u64>; 2],
    area_table: Vec<u64>,
    general: Fields,
    red: Fields,
}

impl FlashAnalyser {
    /// For frames of `width` × `height`, `second` of them a second.
    pub fn new(width: u32, height: u32, second: u32) -> Result<Self, FlashError> {
        if !(1..=MOST_FRAMES_A_SECOND).contains(&second) {
            return Err(FlashError::Rate(second));
        }
        if width == 0 || height == 0 {
            return Err(FlashError::Empty((width, height)));
        }
        let tiles = Tiles::new(width, height);
        let mut pixels = vec![0; tiles.count()];
        for row in 0..tiles.rows as usize {
            for column in 0..tiles.columns as usize {
                let wide = tiles.x_edges[column + 1] - tiles.x_edges[column];
                let high = tiles.y_edges[row + 1] - tiles.y_edges[row];
                pixels[row * tiles.columns as usize + column] = wide * high;
            }
        }
        let area_table = summed(&tiles, |cell| u64::from(pixels[cell]));
        let places = tiles.places();
        Ok(Self {
            size: (width, height),
            second,
            frames: 0,
            cells: vec![Cell::default(); tiles.count()],
            sums: vec![[0.0; 3]; tiles.count()],
            concurrent: ((second as f32 * CONCURRENT).round() as u32).max(1),
            tables: [vec![0; area_table.len()], vec![0; area_table.len()]],
            area_table,
            pixels,
            tiles,
            general: Fields::new(places),
            red: Fields::new(places),
        })
    }

    /// The cells it reads by.
    pub fn tiles(&self) -> &Tiles {
        &self.tiles
    }

    /// Reads the next frame.
    pub fn push(&mut self, image: &Image) -> Result<(), FlashError> {
        let (width, height) = (image.width, image.height);
        if (width, height) != self.size {
            return Err(FlashError::Size { got: (width, height), want: self.size });
        }
        if image.rgba.len() != width as usize * height as usize * 4 {
            return Err(FlashError::Bytes { width, height, bytes: image.rgba.len() });
        }
        let codes: [f32; 256] = std::array::from_fn(|code| linear(code as u8));
        self.sums.fill([0.0; 3]);
        let columns = self.tiles.columns as usize;
        for row in 0..self.tiles.rows as usize {
            let (top, bottom) = (self.tiles.y_edges[row], self.tiles.y_edges[row + 1]);
            for column in 0..columns {
                let (left, right) = (self.tiles.x_edges[column], self.tiles.x_edges[column + 1]);
                let mut sum = [0.0; 3];
                for y in top..bottom {
                    let start = (y as usize * width as usize + left as usize) * 4;
                    let end = (y as usize * width as usize + right as usize) * 4;
                    for texel in image.rgba[start..end].as_chunks::<4>().0 {
                        for channel in 0..3 {
                            sum[channel] += codes[usize::from(texel[channel])];
                        }
                    }
                }
                self.sums[row * columns + column] = sum;
            }
        }
        let first = self.frames == 0;
        for ((cell, sum), &pixels) in self.cells.iter_mut().zip(&self.sums).zip(&self.pixels) {
            let rgb = sum.map(|light| light / pixels as f32);
            let luminance = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
            let hue = Hue::of(rgb);
            let (general, red) = if first {
                (cell.swing, cell.turned) = (Swing::start(luminance), hue);
                (0, None)
            } else {
                let red = (hue.red || cell.turned.red) && hue.distance(cell.turned) > RED_CHANGE;
                // To a saturated red one way, from one the other.
                let way = red.then(|| {
                    cell.turned = hue;
                    usize::from(!hue.red)
                });
                (cell.swing.step(luminance), way)
            };
            cell.general = cell.general.map(|bits| bits << 1);
            cell.red = cell.red.map(|bits| bits << 1);
            match general {
                1 => cell.general[0] |= 1,
                -1 => cell.general[1] |= 1,
                _ => {}
            }
            if let Some(way) = red {
                cell.red[way] |= 1;
            }
        }
        let frame = self.frames;
        self.frames += 1;
        self.judge(frame, Kind::General);
        self.judge(frame, Kind::Red);
        Ok(())
    }

    /// What it's found so far.
    pub fn report(&self) -> FlashReport {
        FlashReport { frames: self.frames, general: self.general.verdict, red: self.red.verdict }
    }

    /// Each frame so far, for the general test and the red: to find what made a sequence's transitions.
    pub fn trace(&self) -> [&[Traced]; 2] {
        [&self.general.trace, &self.red.trace]
    }

    /// Follows every field through `frame`, and updates its verdict with the second ending there.
    fn judge(&mut self, frame: u32, kind: Kind) {
        let Self { tiles, cells, pixels, tables, area_table, second, concurrent, .. } = self;
        let fields = match kind {
            Kind::General => &mut self.general,
            Kind::Red => &mut self.red,
        };
        let window = if *second == 128 { u128::MAX } else { (1u128 << *second) - 1 };
        let across = tiles.places_across();
        let (wide, high) = tiles.field;
        for transitions in &mut fields.transitions {
            *transitions <<= 1;
        }
        for (way, table) in tables.iter_mut().enumerate() {
            fill_summed(tiles, table, |cell| {
                let bits = match kind {
                    Kind::General => cells[cell].general,
                    Kind::Red => cells[cell].red,
                };
                // Its newest transition, if it was this way and lately.
                let newest = bits[way].trailing_zeros();
                if newest < *concurrent && newest < bits[1 - way].trailing_zeros() {
                    u64::from(pixels[cell])
                } else {
                    0
                }
            });
        }
        for row in 0..=tiles.rows - high {
            for column in 0..=tiles.columns - wide {
                let place = (row * across + column) as usize;
                let changed = tables.each_ref().map(|table| rectangle(tiles, table, column, row));
                if changed == [0, 0] && fields.above[place] == [false, false] {
                    continue;
                }
                let area = rectangle(tiles, area_table, column, row) as f32;
                for way in 0..2 {
                    // The share changing this way, less the share changing the other.
                    let share = (changed[way] as f32 - changed[1 - way] as f32) / area;
                    let verdict = &mut fields.verdict;
                    if share > verdict.area {
                        (verdict.area, verdict.area_at) =
                            (share, Some((frame, tiles.field_region(column, row))));
                    }
                    let above = share > MOST_AREA;
                    if above && !fields.above[place][way] {
                        fields.transitions[place] |= 1;
                    }
                    fields.above[place][way] = above;
                }
            }
        }
        let mut traced = Traced::default();
        for (place, transitions) in fields.transitions.iter().enumerate() {
            let rate = (transitions & window).count_ones();
            traced.most = traced.most.max(rate);
            traced.transition |= transitions & 1 != 0;
            if rate > fields.verdict.rate {
                let (column, row) = (place as u32 % across, place as u32 / across);
                fields.verdict.rate = rate;
                fields.verdict.rate_at = Some((frame, tiles.field_region(column, row)));
                fields.verdict.rate_frames = transitions & window;
            }
        }
        fields.trace.push(traced);
    }
}

#[derive(Clone, Copy)]
enum Kind {
    General,
    Red,
}

/// A summed-area table over the cells, a row and a column of zeros before them.
fn summed(tiles: &Tiles, value: impl Fn(usize) -> u64) -> Vec<u64> {
    let mut table = vec![0; ((tiles.columns + 1) * (tiles.rows + 1)) as usize];
    fill_summed(tiles, &mut table, value);
    table
}

fn fill_summed(tiles: &Tiles, table: &mut [u64], value: impl Fn(usize) -> u64) {
    let (columns, rows) = (tiles.columns as usize, tiles.rows as usize);
    let stride = columns + 1;
    for row in 0..rows {
        let mut across = 0;
        for column in 0..columns {
            across += value(row * columns + column);
            table[(row + 1) * stride + column + 1] = table[row * stride + column + 1] + across;
        }
    }
}

/// The sum over the field whose top-left cell is (`column`, `row`).
fn rectangle(tiles: &Tiles, table: &[u64], column: u32, row: u32) -> u64 {
    let stride = tiles.columns as usize + 1;
    let (left, top) = (column as usize, row as usize);
    let (right, bottom) = (left + tiles.field.0 as usize, top + tiles.field.1 as usize);
    table[bottom * stride + right] + table[top * stride + left]
        - table[top * stride + right]
        - table[bottom * stride + left]
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 60;
    /// A 4:3 capture of 120 × 90: a field of 40 × 30 cells, a cell a pixel.
    const SIZE: (u32, u32) = (120, 90);

    /// The grey code whose relative luminance is nearest `luminance`.
    fn grey(luminance: f32) -> u8 {
        (0..=255u8)
            .min_by(|a, b| (linear(*a) - luminance).abs().total_cmp(&(linear(*b) - luminance).abs()))
            .unwrap()
    }

    /// A frame of `ground`, with `patch` over the pixels `inside` says.
    fn frame(ground: [u8; 3], patch: [u8; 3], inside: impl Fn(u32, u32) -> bool) -> Image {
        let (width, height) = SIZE;
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = if inside(x, y) { patch } else { ground };
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
        Image { width, height, rgba }
    }

    /// `frames` frames alternating `ground` with `patch` where `inside`, a change every `every` frames.
    fn flicker(
        ground: [u8; 3],
        patch: [u8; 3],
        every: u32,
        frames: u32,
        inside: impl Fn(u32, u32) -> bool + Copy,
    ) -> FlashReport {
        let (off, on) = (frame(ground, ground, |_, _| false), frame(ground, patch, inside));
        let mut analyser = FlashAnalyser::new(SIZE.0, SIZE.1, RATE).unwrap();
        for at in 0..frames {
            analyser.push(if (at / every).is_multiple_of(2) { &off } else { &on }).unwrap();
        }
        analyser.report()
    }

    fn whole(_: u32, _: u32) -> bool {
        true
    }

    #[test]
    fn a_field_is_wcags_rectangle_at_1024_by_768_and_sized_by_the_height() {
        let tiles = Tiles::new(1024, 768);
        assert_eq!((tiles.columns, tiles.rows, tiles.field), (400, 300, (133, 100)));
        assert_eq!(tiles.field_region(0, 0), Region { x: 0, y: 0, width: 340, height: 256 });
        let wide = Tiles::new(1920, 1080);
        assert_eq!((wide.columns, wide.rows, wide.field), (533, 300, (133, 100)));
        assert_eq!(wide.field_region(0, 0).height, 360, "a third of the height");
        let small = Tiles::new(SIZE.0, SIZE.1);
        assert_eq!((small.columns, small.rows, small.field), (120, 90, (40, 30)), "a cell a pixel");
    }

    /// Three flashes a second pass and a fourth's first change fails, at a 0.1 swing under 0.8 over the whole
    /// frame; a change every ten frames is six a second, every nine more.
    #[test]
    fn the_general_test_counts_more_than_three_flashes_a_second() {
        let (dark, light) = ([grey(0.2); 3], [grey(0.6); 3]);
        let three = flicker(dark, light, 10, 180, whole);
        assert_eq!(three.general.rate, 6);
        assert!(three.passes(), "{three:?}");
        let more = flicker(dark, light, 9, 180, whole);
        assert_eq!(more.general.rate, 7);
        assert!(!more.passes());
        assert_eq!(more.general.area, 1.0);
        assert_eq!(more.red, Verdict::default(), "grey is never red");
    }

    /// A swing just under 0.1 is no flash at any rate, one just over is; a darker state just at 0.8 is none.
    #[test]
    fn a_general_flash_swings_a_tenth_from_under_point_eight() {
        let code = |luminance: f32, below: bool| {
            (0..=255u8)
                .filter(|code| if below { linear(*code) < luminance } else { linear(*code) >= luminance })
                .min_by(|a, b| (linear(*a) - luminance).abs().total_cmp(&(linear(*b) - luminance).abs()))
                .unwrap()
        };
        let base = code(0.4, false);
        let under = code(linear(base) + GENERAL_SWING, true);
        let over = code(linear(base) + GENERAL_SWING, false);
        assert_eq!(
            flicker([base; 3], [under; 3], 2, 120, whole).general.rate,
            0,
            "{} under",
            linear(under) - linear(base)
        );
        assert!(!flicker([base; 3], [over; 3], 2, 120, whole).passes());
        // Bright: from 0.8 up never counts, from just under it does.
        let at = code(DARKER_UNDER, false);
        let below = code(DARKER_UNDER, true);
        assert_eq!(flicker([at; 3], [255; 3], 2, 120, whole).general.rate, 0, "both states 0.8 or more");
        assert!(!flicker([below; 3], [255; 3], 2, 120, whole).passes());
    }

    /// Fast flashes over a quarter of a field pass, and over a cell more fail, wherever the field stands.
    #[test]
    fn a_quarter_of_a_field_passes_and_more_fails() {
        let (dark, light) = ([grey(0.05); 3], [grey(0.6); 3]);
        // A field is 40 × 30 cells here, 1,200; a quarter is 300 cells, a 20 × 15 patch.
        let quarter = |x: u32, y: u32| (50..70).contains(&x) && (40..55).contains(&y);
        let report = flicker(dark, light, 3, 120, quarter);
        assert_eq!(report.general.area, 0.25);
        assert!(report.passes(), "{report:?}");
        let more = |x: u32, y: u32| quarter(x, y) || (x == 70 && y == 40);
        let report = flicker(dark, light, 3, 120, more);
        assert!(report.general.area > 0.25 && !report.passes(), "{report:?}");
        let (_, field) = report.general.area_at.unwrap();
        assert!(field.x <= 50 && field.x + field.width >= 71 && field.y <= 40, "{field:?}");
        // Two quarters further apart than a field is wide: no field holds more than one.
        let left = |x: u32, y: u32| (0..20).contains(&x) && (40..55).contains(&y);
        let apart = |x: u32, y: u32| left(x, y) || left(x.wrapping_sub(100), y);
        let report = flicker(dark, light, 3, 120, apart);
        assert!(report.passes() && report.general.area == 0.25, "{report:?}");
    }

    /// A flash to a saturated red counts as red, by its chromaticity's change; to an orange, or a red too dim to
    /// have a colour, it doesn't.
    #[test]
    fn the_red_test_counts_transitions_to_and_from_a_saturated_red() {
        let dark = [20, 20, 20];
        let red = flicker(dark, [200, 0, 0], 9, 120, whole);
        assert!(!red.red.passes(), "{red:?}");
        assert_eq!(red.red.rate, 7);
        let three = flicker(dark, [200, 0, 0], 10, 120, whole);
        assert!(three.red.passes() && three.red.rate == 6);
        // Red to a red of other brightness is no change of colour: general flashes alone.
        let reds = flicker([90, 0, 0], [255, 0, 0], 9, 120, whole);
        assert_eq!(reds.red.rate, 0);
        assert!(!reds.general.passes());
        // Orange isn't a saturated red; a red this dim is black.
        assert_eq!(flicker([0, 0, 0], [255, 140, 0], 2, 120, whole).red.rate, 0);
        assert_eq!(flicker([0, 0, 0], [24, 0, 0], 2, 120, whole).red.rate, 0);
        // Red and a green of the same light: no general flash, every change a red one.
        let swap = flicker([200, 0, 0], [0, grey(linear(200) * 0.2126 / 0.7152), 0], 9, 120, whole);
        assert!(swap.general.passes() && !swap.red.passes(), "{swap:?}");
    }

    /// A whole frame changing every frame is a flash every two; a lattice moving across it, half its cells
    /// brightening as the other half darken, is none, though every cell changes every frame.
    #[test]
    fn a_strobe_flashes_and_a_lattice_moving_across_a_field_does_not() {
        let (dark, light) = ([grey(0.05); 3], [grey(0.6); 3]);
        let strobe = flicker(dark, light, 1, 120, whole);
        assert_eq!(strobe.general.rate, 60);
        let mut analyser = FlashAnalyser::new(SIZE.0, SIZE.1, RATE).unwrap();
        for at in 0..120 {
            analyser.push(&frame(dark, light, |x, _| (x + at * 4) % 8 < 4)).unwrap();
        }
        let pan = analyser.report();
        assert!(pan.passes() && pan.general.rate <= 2, "{pan:?}");
    }

    /// A slow fade from black to white and back is two transitions, and a fade's steps are one.
    #[test]
    fn a_fade_is_one_transition_however_many_frames_it_takes() {
        let mut analyser = FlashAnalyser::new(SIZE.0, SIZE.1, RATE).unwrap();
        let ramp = (0..=30).chain((0..30).rev()).map(|step| step * 8);
        for code in ramp {
            analyser.push(&frame([code; 3], [code; 3], whole)).unwrap();
        }
        assert_eq!(analyser.report().general.rate, 2);
    }

    #[test]
    fn a_frame_of_another_size_or_rate_is_refused() {
        assert_eq!(FlashAnalyser::new(10, 10, 0).unwrap_err(), FlashError::Rate(0));
        assert_eq!(FlashAnalyser::new(10, 10, 129).unwrap_err(), FlashError::Rate(129));
        let mut analyser = FlashAnalyser::new(10, 10, RATE).unwrap();
        let other = Image { width: 12, height: 10, rgba: vec![0; 480] };
        assert!(matches!(analyser.push(&other), Err(FlashError::Size { .. })));
        let short = Image { width: 10, height: 10, rgba: vec![0; 12] };
        assert!(matches!(analyser.push(&short), Err(FlashError::Bytes { .. })));
    }
}
