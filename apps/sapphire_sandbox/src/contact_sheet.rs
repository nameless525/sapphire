//! `--contact-sheet <clip> <png>` (M2 O2): a clip on the look-dev character at evenly spaced ticks,
//! seen from the front and the side through a long lens, as one image to read motion from. It draws
//! through the renderer as every frame does, in one process, and everything drawn over the frames
//! (each cell's tick, a ground line with a mark every half meter) is plain pixels afterwards, so it
//! asks nothing of the renderer. No window, and no golden depends on it.
//!
//! The camera stays put, so root motion shows as travel: `--clip-in-place` leaves it out, and
//! `--zoom <factor>` pulls back for a clip that leaves the frame. `--sheet-cells` is how many ticks
//! across and `--size` one cell. `--sheet-ticks` names the ticks instead, and `--sheet-views` the
//! views, as yaws in degrees (`tools/motion/sheet.py` reads a move this way, from behind and the side).
//! It draws without fog and, unless told otherwise, at midday with no ambient occlusion on MSAA, since
//! a cell has no temporal history to resolve.

use std::path::Path;

use sapphire_math::Vec3;
use sapphire_platform::AppInitError;
use sapphire_render::quality::OcclusionSetting;
use sapphire_rhi::wgpu;
use sapphire_time::TICK_HZ;

use crate::LOG_TARGET;
use crate::capture::{Readback, write_png};
use crate::offscreen::Stage;
use crate::options::{Antialiasing, Options};
use crate::orbit::OrbitCamera;

/// How many ticks across, unless `--sheet-cells` says.
pub(crate) const DEFAULT_CELLS: u32 = 8;
/// One cell's size, unless `--size` says: eight of them across fit what an image reader shows whole.
pub(crate) const DEFAULT_CELL: (u32, u32) = (192, 240);

/// What the camera looks at, at the character's middle.
const LOOK_HEIGHT: f32 = 0.92;
/// How much of the world a cell's height shows, in meters, at a zoom of 1.
const SHOWN_HEIGHT: f32 = 2.2;
/// The lens, long enough that a foot near the camera and a foot far from it read alike.
const FOV_Y_DEGREES: f32 = 15.0;
/// The views, by the camera's yaw: the front, and the character's left side (facing left).
const VIEW_YAWS_DEGREES: [f32; 2] = [0.0, 90.0];

/// The ground line's color, and each tick label's ink and paper.
const GROUND: [u8; 3] = [255, 64, 64];
const INK: [u8; 3] = [255, 255, 255];
const PAPER: [u8; 3] = [0, 0, 0];
/// A tick label's digit scale, in pixels per font pixel.
const DIGIT_SCALE: usize = 2;

/// A 3×5 font's digits, a row of three bits each.
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// Fills in what `--contact-sheet` implies, and what it isn't told otherwise.
pub(crate) fn pin_defaults(options: &mut Options) {
    options.look_dev = Some(crate::lookdev::Shot::Hero);
    options.aa.get_or_insert(Antialiasing::Msaa);
    options.time_of_day.get_or_insert(12.0);
    options.fog_off = true;
    options.ambient_occlusion.get_or_insert(OcclusionSetting::Off);
}

/// One cell's size and what it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Framing {
    width: u32,
    height: u32,
    /// How much of the world the cell's height shows, in meters.
    shown_height: f32,
}

impl Framing {
    /// For a cell this size, `zoom` times as far back as usual.
    pub fn new(width: u32, height: u32, zoom: Option<f32>) -> Self {
        Self { width, height, shown_height: SHOWN_HEIGHT * zoom.unwrap_or(1.0) }
    }

    /// The camera for a view at `yaw` degrees.
    fn camera(&self, yaw: f32) -> OrbitCamera {
        let fov_y = FOV_Y_DEGREES.to_radians();
        let distance = self.shown_height * 0.5 / (fov_y * 0.5).tan();
        let target = Vec3::new(0.0, LOOK_HEIGHT, 0.0);
        OrbitCamera::shot(target, yaw.to_radians(), 0.0, distance, fov_y)
    }

    /// How many pixels a meter takes at the character's distance.
    fn pixels_per_meter(&self) -> f32 {
        self.height as f32 / self.shown_height
    }

    /// The row where the ground at the character's distance falls.
    fn ground_row(&self) -> usize {
        (self.height as f32 * 0.5 + LOOK_HEIGHT * self.pixels_per_meter()).round() as usize
    }
}

/// What a sheet draws.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout<'a> {
    /// How many evenly spaced ticks, unless [`Self::ticks`] names them.
    pub cells: u32,
    pub ticks: &'a [u32],
    /// The views, as yaws in degrees: [`VIEW_YAWS_DEGREES`] if empty.
    pub yaws: &'a [i32],
}

/// A place in a clip to draw: the fixed step on the character's clock, and the clip's tick there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sample {
    step: u32,
    tick: u32,
}

/// Where to sample a clip of `ticks` ticks at `rate`, in `cells` evenly spaced places from its first
/// tick to its last whole step, short of the end: a player's position wraps there. A pose or a clip
/// shorter than the cells gets what it has.
fn samples(ticks: u32, rate: u32, cells: u32) -> Vec<Sample> {
    let steps = u64::from(ticks) * u64::from(TICK_HZ) / u64::from(rate.max(1));
    let last = steps.saturating_sub(1);
    let count = u64::from(cells).min(last + 1).max(1);
    (0..count)
        .map(|cell| {
            let step = if count == 1 { 0 } else { (cell * last + (count - 1) / 2) / (count - 1) };
            let tick = (step * u64::from(rate) / u64::from(TICK_HZ)) as u32;
            Sample { step: step as u32, tick }
        })
        .collect()
}

/// The clip's ticks `named`, in order, each once. A tick past the last whole step draws that step: a
/// player's position wraps there.
fn samples_at(ticks: u32, rate: u32, named: &[u32]) -> Vec<Sample> {
    let rate = u64::from(rate.max(1));
    let steps = u64::from(ticks) * u64::from(TICK_HZ) / rate;
    let last = steps.saturating_sub(1);
    let mut chosen: Vec<u64> =
        named.iter().map(|&tick| (u64::from(tick) * u64::from(TICK_HZ) / rate).min(last)).collect();
    chosen.sort_unstable();
    chosen.dedup();
    chosen
        .into_iter()
        .map(|step| Sample { step: step as u32, tick: (step * rate / u64::from(TICK_HZ)) as u32 })
        .collect()
}

/// The image being laid out: tightly packed RGBA rows.
struct Sheet {
    framing: Framing,
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

impl Sheet {
    fn new(framing: Framing, columns: usize, rows: usize) -> Self {
        let (width, height) = (framing.width as usize * columns, framing.height as usize * rows);
        let mut pixels = vec![0; width * height * 4];
        pixels.as_chunks_mut::<4>().0.iter_mut().for_each(|pixel| pixel[3] = 255);
        Self { framing, width, height, pixels }
    }

    fn set(&mut self, x: usize, y: usize, color: [u8; 3]) {
        if x < self.width && y < self.height {
            self.pixels[(y * self.width + x) * 4..][..3].copy_from_slice(&color);
        }
    }

    /// Lays `frame`, tightly packed RGBA rows of one cell, into its place, with its marks over it.
    fn place(&mut self, column: usize, row: usize, frame: &[u8], tick: u32) {
        let (cell_width, cell_height) = (self.framing.width as usize, self.framing.height as usize);
        let (left, top) = (column * cell_width, row * cell_height);
        for (y, line) in frame.chunks_exact(cell_width * 4).enumerate() {
            self.pixels[((top + y) * self.width + left) * 4..][..cell_width * 4].copy_from_slice(line);
        }
        self.ground(left, top);
        self.label(left + 2, top + 2, tick);
        // A hairline between cells.
        for y in top..top + cell_height {
            self.set(left, y, PAPER);
        }
    }

    /// A line where the ground falls, with a mark every half meter from the cell's middle.
    fn ground(&mut self, left: usize, top: usize) {
        let framing = self.framing;
        let (cell_width, cell_height) = (framing.width as usize, framing.height as usize);
        let row = framing.ground_row();
        if row >= cell_height {
            return;
        }
        for x in 0..cell_width {
            self.set(left + x, top + row, GROUND);
        }
        let middle = cell_width as f32 * 0.5;
        let half_meter = framing.pixels_per_meter() * 0.5;
        let marks = (middle / half_meter) as i32;
        for mark in -marks..=marks {
            let x = (middle + mark as f32 * half_meter).round() as usize;
            for y in row.saturating_sub(3)..=(row + 3).min(cell_height - 1) {
                self.set(left + x.min(cell_width - 1), top + y, GROUND);
            }
        }
    }

    /// `value`'s digits on a small dark card, its corner at `x`, `y`.
    fn label(&mut self, x: usize, y: usize, value: u32) {
        let text = value.to_string();
        let advance = 4 * DIGIT_SCALE;
        let (width, height) = (text.len() * advance + DIGIT_SCALE, 7 * DIGIT_SCALE);
        for down in 0..height {
            for across in 0..width {
                self.set(x + across, y + down, PAPER);
            }
        }
        for (index, digit) in text.bytes().enumerate() {
            let glyph = DIGITS[usize::from(digit - b'0')];
            for (down, bits) in glyph.iter().enumerate() {
                for across in (0..3).filter(|across| bits & (0b100 >> across) != 0) {
                    for pixel in 0..DIGIT_SCALE * DIGIT_SCALE {
                        let (dy, dx) = (pixel / DIGIT_SCALE, pixel % DIGIT_SCALE);
                        let at_x = x + DIGIT_SCALE + index * advance + across * DIGIT_SCALE + dx;
                        self.set(at_x, y + DIGIT_SCALE + down * DIGIT_SCALE + dy, INK);
                    }
                }
            }
        }
    }
}

/// Renders the clip the stage's character plays into `path`. See the [module docs](self).
pub(crate) fn run(
    stage: &mut Stage,
    path: &Path,
    layout: &Layout<'_>,
    framing: Framing,
) -> Result<(), AppInitError> {
    let (ticks, rate) = stage
        .content
        .clip()
        .map(|(clip, _)| (u32::from(clip.ticks.to_native()), u32::from(clip.rate.to_native())))
        .ok_or_else(|| AppInitError::msg("the clip isn't loaded"))?;
    let samples = match layout.ticks {
        [] => samples(ticks, rate, layout.cells),
        named => samples_at(ticks, rate, named),
    };
    let yaws: Vec<f32> = match layout.yaws {
        [] => VIEW_YAWS_DEGREES.to_vec(),
        named => named.iter().map(|&yaw| yaw as f32).collect(),
    };
    let mut sheet = Sheet::new(framing, samples.len(), yaws.len());
    let target = stage.target(framing.width, framing.height, wgpu::TextureUsages::COPY_SRC);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut step = 0;
    for (column, sample) in samples.iter().enumerate() {
        while step < sample.step {
            stage.step();
            // Spring bones follow every pose on the way, as a player's frames would show them.
            stage.scene.animate(&stage.content, 1.0);
            step += 1;
        }
        for (row, &yaw) in yaws.iter().enumerate() {
            *stage.scene.camera_mut() = framing.camera(yaw);
            let readback = Readback::new(&stage.gpu, framing.width, framing.height);
            stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
            let frame = readback.read(&stage.gpu).map_err(AppInitError::msg)?;
            stage.scene.end_frame();
            sheet.place(column, row, &frame, sample.tick);
        }
    }
    write_png(path, sheet.width as u32, sheet.height as u32, &sheet.pixels)?;
    tracing::info!(
        target: LOG_TARGET,
        path = %path.display(),
        ticks,
        rate,
        cells = samples.len(),
        "wrote contact sheet"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_cover_the_clip_evenly_and_stop_short_of_its_wrap() {
        let steps: Vec<u32> = samples(60, 60, 4).iter().map(|sample| sample.step).collect();
        assert_eq!(steps, [0, 20, 39, 59], "the last whole step, not the end, where a player wraps");
        // A clip at half the rate: two steps a tick.
        let at_30 = samples(30, 30, 3);
        assert_eq!(at_30.last(), Some(&Sample { step: 59, tick: 29 }));
        // A pose has one place to draw; a clip shorter than the cells has what it has.
        assert_eq!(samples(0, 60, 8), [Sample { step: 0, tick: 0 }]);
        assert_eq!(samples(3, 60, 8).len(), 3);
    }

    #[test]
    fn named_ticks_draw_in_order_once_each_short_of_the_wrap() {
        let steps =
            |named: &[u32]| samples_at(60, 60, named).iter().map(|sample| sample.step).collect::<Vec<_>>();
        assert_eq!(steps(&[9, 3, 3, 0]), [0, 3, 9]);
        assert_eq!(steps(&[59, 60, 200]), [59], "the last whole step");
        assert_eq!(samples_at(30, 30, &[29]), [Sample { step: 58, tick: 29 }]);
    }

    #[test]
    fn the_ground_falls_where_the_camera_puts_it() {
        let framing = Framing::new(192, 240, None);
        // The camera's level, so the ground is the look height under the middle.
        let meters_below_middle = (framing.ground_row() as f32 - 120.0) / framing.pixels_per_meter();
        assert!((meters_below_middle - LOOK_HEIGHT).abs() < 0.01, "{meters_below_middle}");
        let wider = Framing::new(192, 240, Some(2.0));
        assert!(wider.ground_row() < framing.ground_row(), "pulling back brings it up");
        let camera = framing.camera(0.0);
        let half_height = camera.eye().distance(Vec3::new(0.0, LOOK_HEIGHT, 0.0))
            * (FOV_Y_DEGREES.to_radians() * 0.5).tan();
        assert!((half_height * 2.0 - SHOWN_HEIGHT).abs() < 1e-3, "the lens shows what it says");
    }

    #[test]
    fn a_cell_lands_in_its_place_with_its_marks() {
        let framing = Framing::new(32, 40, None);
        let mut sheet = Sheet::new(framing, 2, 2);
        let frame = vec![9; 32 * 40 * 4];
        sheet.place(1, 1, &frame, 7);
        let at = |x: usize, y: usize| sheet.pixels[(y * sheet.width + x) * 4];
        assert_eq!(at(10, 10), 0, "another cell stays black");
        assert_eq!(at(32 + 16, 40 + 20), 9, "this one has the frame");
        assert_eq!(at(32 + 16, 40 + framing.ground_row()), GROUND[0], "the ground line");
        // The top row of a 7: three lit font pixels on the card.
        assert_eq!(at(32 + 2 + DIGIT_SCALE, 40 + 2 + DIGIT_SCALE), INK[0], "the label's ink");
        assert_eq!(at(32 + 2, 40 + 2), PAPER[0], "the label's card");
    }

    #[test]
    fn every_digit_has_its_own_glyph() {
        for (first, a) in DIGITS.iter().enumerate() {
            for b in &DIGITS[first + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
