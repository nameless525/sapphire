//! The placeholder sheets (M6 N4a): a smoke puff, a burst of sparks, a slash streak, a splash and a flame, each
//! sixteen frames in a 4 × 4 grid at [`LAYER_SIZE`]² a frame, painted in brush marks as an anime effect is
//! drawn. Clean shapes, a soft edge a texel and a half wide, and the value in three hard steps (shade, middle,
//! light, the light falling from the top left), never photographic noise: a gradient (N4b) colours the steps.
//! A mark is a round dab whose edge wobbles as a loaded brush's does, a stroke along a line whose width
//! follows a profile, or an arc. Every texel is the same on every machine: integer hashes, the four operations,
//! `sqrt` and `sapphire_math::det`.
//!
//! [`outputs`] is the generator's table (`gen-effects`, into `content/effects/`): each sheet's painting and its
//! document. Generated sheets stand until an artist paints the real ones (m6-plan §7, item 5).

use std::f32::consts::PI;
use std::io;

use glam::Vec2;
use sapphire_assets::effect_sheet::{LAYER_SIZE, SheetTiming};
use sapphire_math::det;

use super::{EffectSheetDocument, IMAGE_SUFFIX};
use crate::color::unorm8;
use crate::generators::Output;
use crate::samples::noise::{NO_PERIOD, Rng, hash, smoothstep, value_noise};

/// The light falls from the top left: the lit step of a shape sits on this side of it.
const LIGHT: Vec2 = Vec2::new(-0.6, -0.8);

/// The value's three steps.
const SHADE: f32 = 0.38;
const MIDDLE: f32 = 0.66;
const LIT: f32 = 1.0;

/// Each sheet's grid: four by four, every cell a frame.
const COLUMNS: u32 = 4;
const FRAMES: u32 = COLUMNS * COLUMNS;

/// A placeholder sheet: its name, how it plays, and how one of its frames is painted.
struct Placeholder {
    name: &'static str,
    timing: SheetTiming,
    paint: fn(&mut Frame, Moment),
}

/// Where in its sheet a frame falls.
#[derive(Clone, Copy, Debug)]
struct Moment {
    /// 0 at the first frame, 1 at the last; a looped sheet's last frame stops one short of its first again.
    t: f32,
    /// The frame's place, for its seeds.
    frame: u32,
}

/// The sheets, in the order they're listed.
const SHEETS: [Placeholder; 5] = [
    Placeholder { name: "smoke_puff", timing: SheetTiming::Life, paint: smoke_puff },
    Placeholder { name: "spark_burst", timing: SheetTiming::Life, paint: spark_burst },
    Placeholder { name: "slash_streak", timing: SheetTiming::Life, paint: slash_streak },
    Placeholder { name: "splash", timing: SheetTiming::Life, paint: splash },
    Placeholder { name: "flame", timing: SheetTiming::PerSecond { rate: 12.0, looped: true }, paint: flame },
];

/// The names of the placeholder sheets.
pub fn names() -> impl Iterator<Item = &'static str> {
    SHEETS.iter().map(|sheet| sheet.name)
}

/// What `gen-effects` writes into `content/effects/`: each sheet's painting and document under `sheets/`.
pub fn outputs(_: &std::path::Path) -> Vec<Output> {
    SHEETS
        .iter()
        .flat_map(|sheet| {
            let image = format!("{}{IMAGE_SUFFIX}", sheet.name);
            let document = EffectSheetDocument {
                image: image.clone(),
                columns: COLUMNS,
                rows: COLUMNS,
                frames: FRAMES,
                timing: sheet.timing,
            };
            [
                Output::new(format!("sheets/{image}"), move || painting_png(sheet)),
                Output::new(format!("sheets/{}.sheet.ron", sheet.name), move || {
                    sapphire_serialize::to_ron(&document).map(String::into_bytes).map_err(io::Error::other)
                }),
            ]
        })
        .collect()
}

/// A sheet's painting as a PNG of grey and alpha: value and coverage, every frame clear within four texels of its
/// border, so no frame's mark is cut by its quad's edge at any level.
fn painting_png(sheet: &Placeholder) -> io::Result<Vec<u8>> {
    let side = COLUMNS * LAYER_SIZE;
    let mut image = image::GrayAlphaImage::new(side, side);
    for frame in 0..FRAMES {
        let canvas = painted(sheet, frame);
        let (left, top) = ((frame % COLUMNS) * LAYER_SIZE, (frame / COLUMNS) * LAYER_SIZE);
        for y in 0..LAYER_SIZE {
            for x in 0..LAYER_SIZE {
                let i = (y * LAYER_SIZE + x) as usize;
                let edge = x.min(y).min(LAYER_SIZE - 1 - x).min(LAYER_SIZE - 1 - y) as f32;
                let coverage = canvas.coverage[i] * smoothstep(4.0, 6.0, edge);
                let value = if coverage > 0.0 { canvas.value[i] } else { 0.0 };
                image.put_pixel(left + x, top + y, image::LumaA([unorm8(value), unorm8(coverage)]));
            }
        }
    }
    let mut bytes = io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).map_err(io::Error::other)?;
    Ok(bytes.into_inner())
}

/// Frame `frame` of `sheet`, painted, before its border fades.
fn painted(sheet: &Placeholder, frame: u32) -> Frame {
    let looped = matches!(sheet.timing, SheetTiming::PerSecond { looped: true, .. });
    let t = if looped { frame as f32 / FRAMES as f32 } else { frame as f32 / (FRAMES - 1) as f32 };
    let mut canvas = Frame::new();
    (sheet.paint)(&mut canvas, Moment { t, frame });
    canvas
}

/// Each frame's coverage of the sheet `name`, as painted, before the border's fade: what a test holds to
/// its cell.
#[cfg(test)]
pub(super) fn coverage(name: &str) -> Vec<Vec<f32>> {
    let sheet = SHEETS.iter().find(|sheet| sheet.name == name).expect("a placeholder");
    (0..FRAMES).map(|frame| painted(sheet, frame).coverage).collect()
}

/// How a stroke's half width runs from its start (0) to its end (1).
#[derive(Clone, Copy, Debug)]
enum Taper {
    /// Pointed at both ends, widest in the middle.
    Both,
    /// Swelling fast from a round start, then thinning to a point: a spark.
    Spark,
    /// Round at its base, pointed at its tip, widest low: a flame's tongue, a drop.
    Tongue,
}

impl Taper {
    fn width(self, t: f32) -> f32 {
        match self {
            Self::Both => det::sin(PI * t),
            Self::Spark => (t * 6.0).min(1.0).sqrt() * (1.0 - t),
            Self::Tongue => det::sin(PI * det::pow(t, 0.55)).max(0.0).sqrt(),
        }
    }
}

/// One mark of the brush, in a frame's units (its side is 1, Y down).
#[derive(Clone, Copy, Debug)]
enum Mark {
    /// A round dab whose edge wobbles by `wobble` of its radius.
    Dab { centre: Vec2, radius: f32, wobble: f32, seed: u32 },
    /// Along a line from `start`, `width` at its widest; `direction` is a unit vector.
    Stroke { start: Vec2, direction: Vec2, length: f32, width: f32, taper: Taper, seed: u32 },
    /// Along a circle about `centre` from angle `from` to `to` (radians, Y down, `from` below `to`, neither
    /// crossing ±π), widest two thirds of the way along and pointed at both ends.
    Arc { centre: Vec2, radius: f32, from: f32, to: f32, width: f32 },
}

impl Mark {
    fn stroke(start: Vec2, direction: Vec2, length: f32, width: f32, taper: Taper, seed: u32) -> Self {
        Self::Stroke { start, direction: direction.normalize(), length, width, taper, seed }
    }

    /// How far inside the mark `p` is (negative outside).
    fn inside(&self, p: Vec2) -> f32 {
        match *self {
            Self::Dab { centre, radius, wobble, seed } => {
                let offset = p - centre;
                let distance = offset.length();
                let around = if distance > 0.0 { offset / distance } else { Vec2::X };
                let wobble = 1.0
                    + wobble * (value_noise(around * 2.5 + 8.0, (NO_PERIOD, NO_PERIOD), seed) - 0.5) * 2.0;
                radius * wobble - distance
            }
            Self::Stroke { start, direction, length, width, taper, seed } => {
                let offset = p - start;
                let t = offset.dot(direction) / length;
                if !(0.0..=1.0).contains(&t) {
                    return -1.0;
                }
                let wobble = 1.0
                    + 0.16 * (value_noise(Vec2::new(t * 5.0, 0.5), (NO_PERIOD, NO_PERIOD), seed) - 0.5) * 2.0;
                0.5 * width * taper.width(t) * wobble - offset.perp_dot(direction).abs()
            }
            Self::Arc { centre, radius, from, to, width } => {
                let offset = p - centre;
                let s = (det::atan2(offset.y, offset.x) - from) / (to - from);
                if !(0.0..=1.0).contains(&s) {
                    return -1.0;
                }
                0.5 * width * det::sin(PI * det::pow(s, 1.6)) - (offset.length() - radius).abs()
            }
        }
    }

    /// Its bounds, in the frame's units: `(min, max)`.
    fn bounds(&self) -> (Vec2, Vec2) {
        let (low, high, pad) = match *self {
            Self::Dab { centre, radius, wobble, .. } => (centre, centre, radius * (1.0 + wobble)),
            Self::Stroke { start, direction, length, width, .. } => {
                let end = start + direction * length;
                (start.min(end), start.max(end), width * 0.6)
            }
            Self::Arc { centre, radius, width, .. } => (centre, centre, radius + width),
        };
        let pad = Vec2::splat(pad + 0.01);
        (low - pad, high + pad)
    }
}

/// A frame being painted: value and coverage, [`LAYER_SIZE`] texels a side.
struct Frame {
    value: Vec<f32>,
    coverage: Vec<f32>,
}

impl Frame {
    fn new() -> Self {
        let texels = (LAYER_SIZE * LAYER_SIZE) as usize;
        Self { value: vec![0.0; texels], coverage: vec![0.0; texels] }
    }

    /// Calls `apply(index, amount)` for each texel `mark` covers, by `amount`, its edge a texel and a half soft.
    fn each(&mut self, mark: &Mark, mut apply: impl FnMut(&mut Self, usize, f32)) {
        let texel = 1.0 / LAYER_SIZE as f32;
        let (low, high) = mark.bounds();
        let to_texel = |v: f32| ((v / texel).floor().max(0.0) as u32).min(LAYER_SIZE);
        for y in to_texel(low.y)..(to_texel(high.y) + 1).min(LAYER_SIZE) {
            for x in to_texel(low.x)..(to_texel(high.x) + 1).min(LAYER_SIZE) {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * texel;
                let amount = smoothstep(-0.75 * texel, 0.75 * texel, mark.inside(p));
                if amount > 0.0 {
                    apply(self, (y * LAYER_SIZE + x) as usize, amount);
                }
            }
        }
    }

    /// Paints `mark` in `value`, laying down its coverage.
    fn fill(&mut self, mark: &Mark, value: f32) {
        self.each(mark, |frame, i, amount| {
            frame.value[i] += (value - frame.value[i]) * amount;
            frame.coverage[i] = frame.coverage[i].max(amount);
        });
    }

    /// Paints `mark` in `value` only where something already covers: a step of light or shade on a shape.
    fn tone(&mut self, mark: &Mark, value: f32) {
        self.each(mark, |frame, i, amount| {
            let amount = amount * frame.coverage[i];
            frame.value[i] += (value - frame.value[i]) * amount;
        });
    }

    /// Takes `mark` out of what's painted: a hole as smoke breaks up.
    fn erase(&mut self, mark: &Mark) {
        self.each(mark, |frame, i, amount| frame.coverage[i] *= 1.0 - amount);
    }

    /// A dab in three steps: the middle, its lit side toward the light and its shaded side away.
    fn shaded_dab(&mut self, centre: Vec2, radius: f32, seed: u32) {
        let wobble = 0.14;
        self.fill(&Mark::Dab { centre, radius, wobble, seed }, MIDDLE);
        let away = -LIGHT.normalize();
        let shade = Mark::Dab {
            centre: centre + away * (radius * 0.45),
            radius: radius * 0.78,
            wobble,
            seed: seed ^ 1,
        };
        self.tone(&shade, SHADE);
        let lit = Mark::Dab {
            centre: centre - away * (radius * 0.38),
            radius: radius * 0.6,
            wobble,
            seed: seed ^ 2,
        };
        self.tone(&lit, LIT);
    }
}

/// Out of 1, eased: fast at first, settling.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)) * (1.0 - t.clamp(0.0, 1.0))
}

/// A puff of smoke: dabs about the middle swelling outward, three-toned, breaking into holes and thinning
/// away over its second half.
fn smoke_puff(frame: &mut Frame, at: Moment) {
    let middle = Vec2::new(0.5, 0.52);
    let grow = ease_out(at.t * 1.4);
    let mut rng = Rng(hash(0x5e0e));
    let mut dabs: Vec<(Vec2, f32, u32)> = (0..7)
        .map(|i| {
            let around = det::vec2_from_angle(i as f32 * 0.898 + rng.range(-0.2, 0.2));
            let reach = (0.06 + 0.17 * grow) * rng.range(0.8, 1.1);
            let radius = (0.07 + 0.09 * grow) * rng.range(0.85, 1.15);
            (middle + around * reach, radius, rng.0)
        })
        .collect();
    dabs.push((middle, 0.09 + 0.1 * grow, rng.0));
    // The lower dabs first, so the upper ones' light lies over them.
    dabs.sort_by(|a, b| b.0.y.total_cmp(&a.0.y));
    let shrink = 1.0 - 0.25 * ((at.t - 0.6) / 0.4).clamp(0.0, 1.0);
    for (centre, radius, seed) in dabs {
        frame.shaded_dab(centre, radius * shrink, seed);
    }
    let breaking = ((at.t - 0.45) / 0.55).clamp(0.0, 1.0);
    if breaking > 0.0 {
        let mut holes = Rng(hash(0x401e));
        for _ in 0..9 {
            let centre = middle + Vec2::new(holes.range(-0.28, 0.28), holes.range(-0.26, 0.26));
            let radius = holes.range(0.03, 0.08) * breaking * 1.6;
            frame.erase(&Mark::Dab { centre, radius, wobble: 0.25, seed: holes.0 });
        }
    }
}

/// A burst of sparks: a flash at the middle for the first few frames, then streaks flying out from it,
/// each a bright core in a middle tone, shortening and thinning as they go.
fn spark_burst(frame: &mut Frame, at: Moment) {
    let middle = Vec2::splat(0.5);
    let flash = 1.0 - at.t * 4.0;
    if flash > 0.0 {
        frame.fill(&Mark::Dab { centre: middle, radius: 0.16 * flash + 0.02, wobble: 0.1, seed: 3 }, LIT);
    }
    let mut rng = Rng(hash(0x5a2c));
    let out = ease_out(at.t * 1.2);
    for i in 0..11 {
        let direction = det::vec2_from_angle(i as f32 * (2.0 * PI / 11.0) + rng.range(-0.25, 0.25));
        let speed = rng.range(0.7, 1.0);
        let start = middle + direction * (0.03 + 0.3 * out * speed);
        let length = (0.3 * speed * (1.0 - at.t) + 0.02) * rng.range(0.8, 1.1);
        let width = 0.055 * (1.0 - 0.7 * at.t);
        let seed = rng.0;
        frame.fill(&Mark::stroke(start, direction, length, width, Taper::Spark, seed), MIDDLE);
        frame.fill(&Mark::stroke(start, direction, length * 0.85, width * 0.45, Taper::Spark, seed ^ 1), LIT);
    }
}

/// A slash: a crescent sweeping across the top of the frame from left to right, its head bright and its
/// inner edge in shade, its tail catching up until it's gone.
fn slash_streak(frame: &mut Frame, at: Moment) {
    const RADIUS: f32 = 0.38;
    let centre = Vec2::new(0.5, 0.74);
    let (start, end) = (-0.94 * PI, -0.06 * PI);
    let head = start + (end - start) * ease_out(0.06 + at.t * 1.7);
    let tail = start + (end - start) * ease_out((at.t - 0.3) * 1.25);
    if head - tail < 0.02 {
        return;
    }
    let width = 0.11 * (1.0 - 0.6 * ((at.t - 0.5) / 0.5).clamp(0.0, 1.0));
    frame.fill(&Mark::Arc { centre, radius: RADIUS, from: tail, to: head, width }, MIDDLE);
    // The outer edge lit, the inner in shade.
    let lit = Mark::Arc { centre, radius: RADIUS + width * 0.18, from: tail, to: head, width: width * 0.5 };
    frame.tone(&lit, LIT);
    let shade = Mark::Arc { centre, radius: RADIUS - width * 0.3, from: tail, to: head, width: width * 0.35 };
    frame.tone(&shade, SHADE);
}

/// A splash: a crown of drops rising from a ring on the water, the ring spreading, the drops parting from
/// it at the top of their rise and falling back.
fn splash(frame: &mut Frame, at: Moment) {
    let base = Vec2::new(0.5, 0.8);
    let spread = 0.1 + 0.14 * ease_out(at.t * 1.3);
    let ring = (1.0 - at.t).max(0.0);
    if ring > 0.05 {
        let mut rng = Rng(hash(0x5b1a));
        for i in 0..14 {
            let side = (i as f32 + 0.5) / 14.0 * 2.0 - 1.0;
            let centre = base + Vec2::new(side * spread, rng.range(-0.01, 0.01) + side * side * 0.02);
            frame.fill(&Mark::Dab { centre, radius: 0.035 * ring + 0.01, wobble: 0.2, seed: rng.0 }, SHADE);
        }
    }
    let mut rng = Rng(hash(0x5d70));
    let rise = (at.t / 0.5).min(1.0);
    for i in 0..9 {
        let side = (i as f32 + 0.5) / 9.0 * 2.0 - 1.0;
        let root = base + Vec2::new(side * spread, 0.0);
        let lean = Vec2::new(side * 0.35, -1.0);
        let height = rng.range(0.28, 0.42) * ease_out(rise);
        let seed = rng.0;
        if at.t < 0.5 {
            let stroke = Mark::stroke(root, lean, height, 0.05 + 0.02 * (1.0 - rise), Taper::Both, seed);
            frame.fill(&stroke, MIDDLE);
            frame.tone(&Mark::stroke(root, lean, height * 0.9, 0.025, Taper::Both, seed ^ 1), LIT);
        } else {
            // Parted: a drop at the top of its rise, falling.
            let fall = (at.t - 0.5) / 0.5;
            let top = root + lean.normalize() * height + Vec2::new(side * 0.03 * fall, 0.3 * fall * fall);
            let radius = 0.03 * (1.0 - 0.6 * fall);
            frame.fill(&Mark::Dab { centre: top, radius, wobble: 0.1, seed }, MIDDLE);
            frame.tone(
                &Mark::Dab {
                    centre: top + LIGHT * (radius * 0.3),
                    radius: radius * 0.5,
                    wobble: 0.1,
                    seed: seed ^ 1,
                },
                LIT,
            );
        }
    }
}

/// A flame that loops: a tongue swaying from its base, two smaller ones beside it, a bright core inside and a
/// wisp parting from its tip once a cycle. Everything is periodic in the cycle, so the last frame leads back
/// into the first.
fn flame(frame: &mut Frame, at: Moment) {
    let cycle = 2.0 * PI * at.t;
    let base = Vec2::new(0.5, 0.9);
    let sway = |phase: f32, reach: f32| Vec2::new(det::sin(cycle + phase) * reach, -1.0);
    let height = 0.62 + 0.06 * det::sin(cycle * 2.0);
    let seed = hash(0xf1a3 ^ at.frame);
    for (offset, phase, scale) in [(-0.1, 1.9, 0.55), (0.11, 4.1, 0.5)] {
        let root = base + Vec2::new(offset, -0.02);
        let tongue =
            Mark::stroke(root, sway(phase, 0.3), height * scale, 0.2 * scale + 0.04, Taper::Tongue, seed);
        frame.fill(&tongue, SHADE);
    }
    let tongue = Mark::stroke(base, sway(0.0, 0.18), height, 0.3, Taper::Tongue, seed ^ 1);
    frame.fill(&tongue, MIDDLE);
    let core = Mark::stroke(
        base + Vec2::new(0.0, -0.02),
        sway(0.6, 0.12),
        height * 0.55,
        0.16,
        Taper::Tongue,
        seed ^ 2,
    );
    frame.tone(&core, LIT);
    // A wisp parting from the tip and rising, a quarter of the cycle out of four.
    let wisp = (at.t * 4.0).fract();
    let tip = base + sway(0.0, 0.18).normalize() * height;
    let rising = tip + Vec2::new(det::sin(cycle) * 0.04, -0.12 * wisp);
    frame.fill(
        &Mark::Dab { centre: rising, radius: 0.035 * (1.0 - wisp), wobble: 0.2, seed: seed ^ 3 },
        MIDDLE,
    );
}
