//! The grading LUT (spec §5.12): the tone curve and a color grade baked into a 32³ table on
//! the CPU, only when they change. The GPU-free half of post-processing, so it is unit-tested
//! without a device.
//!
//! # Encoding
//!
//! - **Input:** exposed scene-linear color through a log shaper per channel,
//!   `s = log2(1 + x / ε) / log2(1 + MAX / ε)`. Black lands exactly on the first texel and
//!   values well below ε = 1/32 stay close to linear, so blacks aren't lifted. Above it the
//!   texels are spread logarithmically up to [`SHAPER_MAX`] = 16, four stops over lit white,
//!   at about 3.4 texels per stop; brighter input clamps to the last texel, which the
//!   shoulder has taken to white. ε and the maximum were chosen by measuring the trilinear
//!   error (the tests below): a smaller ε or a larger maximum thins the texels in the
//!   shoulder, and a larger ε thins them near black.
//! - **Output:** sRGB-encoded display color, 10 bits per channel (`Rgb10a2Unorm`).
//!   Interpolating encoded values spreads the trilinear error evenly over perceived
//!   brightness; the composite decodes the result and the sRGB target encodes it again.
//!
//! # The grade
//!
//! Per texel: white balance and the color filter (per-channel gains) and contrast around
//! middle grey on scene-linear color; the tone curve and sRGB encoding; then saturation,
//! lift, gain and gamma on the encoded display value. Everything before the tone curve works
//! per channel and everything after it is a weighted sum of channels, which a trilinear table
//! follows almost exactly: the default look stays within about a display code value of
//! direct evaluation in its lit range. The player's filter strength blends the result with
//! the neutral one (the tone curve alone), so strength 0 bakes exactly the neutral LUT. Each
//! stage is skipped when its parameters are neutral, so [`ColorGrade::NEUTRAL`] is exactly
//! the tone curve.

use glam::Vec3;

use super::params::{ColorGrade, ToneCurve};

/// Texels per side of the LUT.
pub(crate) const LUT_SIZE: u32 = 32;
/// Texels in the LUT.
pub(crate) const LUT_TEXELS: usize = (LUT_SIZE * LUT_SIZE * LUT_SIZE) as usize;
/// The shaper's ε: below about this, input is close to linear.
pub(crate) const SHAPER_EPSILON: f32 = 1.0 / 32.0;
/// The brightest input the LUT tells apart, after exposure.
pub(crate) const SHAPER_MAX: f32 = 16.0;

/// `log2(1 + SHAPER_MAX / SHAPER_EPSILON)`: the shaper's range in stops.
pub(crate) fn shaper_range() -> f32 {
    (SHAPER_MAX / SHAPER_EPSILON).ln_1p() / std::f32::consts::LN_2
}

/// Scene-linear input to its LUT coordinate in [0, 1], as the composite shader computes it.
#[cfg(test)]
pub(crate) fn shape(x: f32) -> f32 {
    ((x.max(0.0) / SHAPER_EPSILON).ln_1p() / std::f32::consts::LN_2 / shaper_range()).min(1.0)
}

/// The scene-linear input at LUT coordinate `s`.
fn unshape(s: f32) -> f32 {
    SHAPER_EPSILON * ((s * shaper_range()).exp2() - 1.0)
}

/// What one LUT is baked from. Built from sanitized parameters, so comparing keys is exact.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LutKey {
    curve: ToneCurve,
    grade: ColorGrade,
    strength: f32,
}

impl LutKey {
    /// Clamps every parameter to its working range, replacing non-finite values with neutral
    /// ones: whatever the frame asks for bakes a finite, monotonic LUT.
    pub(crate) fn new(curve: &ToneCurve, grade: &ColorGrade, strength: f32) -> Self {
        let (neutral, fallback) = (ColorGrade::NEUTRAL, ToneCurve::default());
        let max_brightness = finite_clamp(curve.max_brightness, 0.1, 4.0, fallback.max_brightness);
        let curve = ToneCurve {
            max_brightness,
            contrast: finite_clamp(curve.contrast, 0.1, 4.0, fallback.contrast),
            // The toe needs room below the shoulder.
            linear_start: finite_clamp(
                curve.linear_start,
                0.001,
                0.9 * max_brightness,
                fallback.linear_start,
            )
            .min(0.9 * max_brightness),
            // Below 1, so the shoulder has room to approach the maximum.
            linear_length: finite_clamp(curve.linear_length, 0.0, 0.99, fallback.linear_length),
            black_tightness: finite_clamp(curve.black_tightness, 1.0, 3.0, fallback.black_tightness),
        };
        let grade = ColorGrade {
            temperature: finite_clamp(grade.temperature, -2.0, 2.0, neutral.temperature),
            tint: finite_clamp(grade.tint, -2.0, 2.0, neutral.tint),
            filter: finite_clamp3(grade.filter, 0.0, 16.0, neutral.filter),
            contrast: finite_clamp(grade.contrast, 0.1, 4.0, neutral.contrast),
            saturation: finite_clamp(grade.saturation, 0.0, 4.0, neutral.saturation),
            lift: finite_clamp3(grade.lift, -1.0, 1.0, neutral.lift),
            gamma: finite_clamp3(grade.gamma, 0.1, 10.0, neutral.gamma),
            gain: finite_clamp3(grade.gain, 0.0, 4.0, neutral.gain),
        };
        Self { curve, grade, strength: finite_clamp(strength, 0.0, 1.0, 1.0) }
    }
}

fn finite_clamp(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() { value.clamp(min, max) } else { fallback }
}

fn finite_clamp3(value: Vec3, min: f32, max: f32, fallback: Vec3) -> Vec3 {
    Vec3::new(
        finite_clamp(value.x, min, max, fallback.x),
        finite_clamp(value.y, min, max, fallback.y),
        finite_clamp(value.z, min, max, fallback.z),
    )
}

/// The GT tone curve with its derived constants (see [`ToneCurve`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Curve {
    /// Uchimura's names: `max_brightness` (P), `contrast` (a), `linear_start` (m) and
    /// `black_tightness` (c).
    p: f32,
    a: f32,
    m: f32,
    c: f32,
    /// Where the shoulder starts, and its value there.
    s0: f32,
    s1: f32,
    /// The shoulder's exponential rate, which matches the linear section's slope at `s0`.
    rate: f32,
}

impl Curve {
    /// `curve` must be sanitized (it comes from a [`LutKey`]).
    fn new(curve: &ToneCurve) -> Self {
        let (p, a, m) = (curve.max_brightness, curve.contrast, curve.linear_start);
        let l0 = (p - m) * curve.linear_length / a;
        let s1 = m + a * l0;
        Self { p, a, m, c: curve.black_tightness, s0: m + l0, s1, rate: a / (p - s1) }
    }

    /// Scene-linear to display-linear, per channel.
    pub(crate) fn apply(&self, x: f32) -> f32 {
        let x = x.max(0.0);
        if x >= self.s0 {
            return self.p - (self.p - self.s1) * (-self.rate * (x - self.s0)).exp();
        }
        let linear = self.m + self.a * (x - self.m);
        if x >= self.m {
            return linear;
        }
        // The toe blends into the linear section over [0, m] with a smoothstep weight.
        let t = x / self.m;
        let toe = self.m * t.powf(self.c);
        let blend = t * t * (3.0 - 2.0 * t);
        toe * (1.0 - blend) + linear * blend
    }
}

/// A [`LutKey`] prepared for evaluation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Lut {
    curve: Curve,
    grade: ColorGrade,
    /// White balance and the color filter as per-channel gains; `None` when both are neutral.
    gains: Option<Vec3>,
    strength: f32,
}

/// Rec. 709 luminance weights, for saturation.
const LUMA: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);
/// Contrast pivots around middle grey.
const MIDDLE_GREY: f32 = 0.18;
/// The CIE x of D65, the white point of sRGB.
const D65_X: f32 = 0.3127;
/// How far ±1 of temperature or tint moves the adapted-from white in CIE xy.
const WHITE_BALANCE_SCALE: f32 = 0.05;

impl Lut {
    pub(crate) fn new(key: &LutKey) -> Self {
        let grade = key.grade;
        let white_balance = (grade.temperature != 0.0 || grade.tint != 0.0)
            .then(|| white_balance(grade.temperature, grade.tint));
        let gains = match (white_balance, grade.filter == Vec3::ONE) {
            (None, true) => None,
            (white_balance, _) => Some(grade.filter * white_balance.unwrap_or(Vec3::ONE)),
        };
        Self { curve: Curve::new(&key.curve), grade, gains, strength: key.strength }
    }

    /// The baked value for scene-linear `rgb`: sRGB-encoded display color in [0, 1].
    pub(crate) fn evaluate(&self, rgb: Vec3) -> Vec3 {
        let rgb = rgb.max(Vec3::ZERO);
        let neutral = self.display(rgb);
        if self.strength == 0.0 {
            return neutral;
        }
        let graded = self.graded(rgb);
        // Exact at both ends: strength 0 is the neutral value and 1 the graded one.
        (neutral * (1.0 - self.strength) + graded * self.strength).clamp(Vec3::ZERO, Vec3::ONE)
    }

    /// The tone curve alone, encoded.
    fn display(&self, rgb: Vec3) -> Vec3 {
        rgb.map(|x| srgb_encode(self.curve.apply(x)))
    }

    fn graded(&self, rgb: Vec3) -> Vec3 {
        let grade = &self.grade;
        let mut color = match self.gains {
            Some(gains) => rgb * gains,
            None => rgb,
        };
        if grade.contrast != 1.0 {
            color = color.map(|x| MIDDLE_GREY * (x / MIDDLE_GREY).powf(grade.contrast));
        }
        let mut display = self.display(color);
        // On display values, after the shoulder: the table follows a weighted sum of
        // per-channel curves far more closely than a mix taken before them, and highlights the
        // shoulder has compressed keep their saturation.
        if grade.saturation != 1.0 {
            let luma = Vec3::splat(display.dot(LUMA));
            display = luma + (display - luma) * grade.saturation;
        }
        if grade.lift != Vec3::ZERO {
            display += grade.lift * (Vec3::ONE - display);
        }
        if grade.gain != Vec3::ONE {
            display *= grade.gain;
        }
        display = display.clamp(Vec3::ZERO, Vec3::ONE);
        if grade.gamma != Vec3::ONE {
            let exponent = grade.gamma.recip();
            display =
                Vec3::new(display.x.powf(exponent.x), display.y.powf(exponent.y), display.z.powf(exponent.z));
        }
        display
    }

    /// Fills `texels` (red fastest, then green, then blue slices) with this LUT, packed as
    /// `Rgb10a2Unorm`. Slices bake in parallel on the job pool.
    pub(crate) fn bake(&self, texels: &mut [u32]) {
        profiling::scope!("Lut::bake");
        assert_eq!(texels.len(), LUT_TEXELS, "a LUT has LUT_SIZE³ texels");
        let size = LUT_SIZE as usize;
        let inputs: [f32; LUT_SIZE as usize] =
            std::array::from_fn(|index| unshape(index as f32 / (LUT_SIZE - 1) as f32));
        sapphire_jobs::par_for_each_chunk_mut(texels, size * size, |start, slice| {
            let blue = inputs[start / (size * size)];
            for (index, texel) in slice.iter_mut().enumerate() {
                let rgb = Vec3::new(inputs[index % size], inputs[index / size], blue);
                *texel = pack_rgb10a2(self.evaluate(rgb));
            }
        });
    }
}

/// White balance as per-channel gains: a von Kries adaptation done in linear sRGB, as a
/// camera's white balance is done in its own RGB, from a white moved off D65 along the
/// daylight locus (temperature) and across it (tint) back to D65. Adapting from a bluer white
/// warms the picture, so positive temperature moves the source white toward blue.
///
/// Gains rather than a cone-space matrix keep the whole scene-referred grade per channel,
/// which the LUT follows to within its one-dimensional interpolation error; a matrix mixes
/// channels where the table is coarsest (near black and in the shoulder). The gains keep a
/// grey's luminance.
fn white_balance(temperature: f32, tint: f32) -> Vec3 {
    // Both whites are taken on the locus (D65 lies within 0.0002 of it), so zero is exactly
    // neutral.
    let white = |temperature: f32, tint: f32| {
        let x = D65_X - temperature * WHITE_BALANCE_SCALE;
        let y = daylight_locus_y(x) + tint * WHITE_BALANCE_SCALE;
        let xyz = Vec3::new(x / y, 1.0, (1.0 - x - y) / y);
        // CIE XYZ to linear sRGB, row by row.
        Vec3::new(
            Vec3::new(3.240_454_2, -1.537_138_5, -0.498_531_4).dot(xyz),
            Vec3::new(-0.969_266, 1.876_010_8, 0.041_556).dot(xyz),
            Vec3::new(0.055_643_4, -0.204_025_9, 1.057_225_2).dot(xyz),
        )
    };
    let gains = white(0.0, 0.0) / white(temperature, tint);
    gains / gains.dot(LUMA)
}

/// CIE daylight locus: y for a daylight white of chromaticity x.
fn daylight_locus_y(x: f32) -> f32 {
    -3.0 * x * x + 2.87 * x - 0.275
}

fn srgb_encode(linear: f32) -> f32 {
    if linear <= 0.003_130_8 { linear * 12.92 } else { 1.055 * linear.powf(1.0 / 2.4) - 0.055 }
}

/// Packs a color in [0, 1] as `Rgb10a2Unorm`: red in the low bits, opaque alpha on top.
fn pack_rgb10a2(rgb: Vec3) -> u32 {
    let quantize = |value: f32| (value.clamp(0.0, 1.0) * 1023.0).round() as u32;
    quantize(rgb.x) | quantize(rgb.y) << 10 | quantize(rgb.z) << 20 | 3 << 30
}

/// Which of two baked LUTs each grade reads, and which LUTs must be (re)baked.
///
/// A grade whose key is already baked in either slot reuses it, and a new key replaces the
/// least recently used slot that the other grade isn't reading. A time-of-day transition from
/// key pair (A, B) to (B, C) therefore costs one bake: B is found where it was, and C takes
/// A's old slot.
#[derive(Debug)]
pub(crate) struct LutSlots<K> {
    keys: [Option<K>; 2],
    /// Per slot: the `assign` call that last used it (0: never).
    used: [u64; 2],
    calls: u64,
}

impl<K> Default for LutSlots<K> {
    fn default() -> Self {
        Self { keys: [None, None], used: [0, 0], calls: 0 }
    }
}

/// The result of [`LutSlots::assign`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Assignment {
    /// The slot grades A and B read. A grade that isn't needed reads A's.
    pub slots: [usize; 2],
    /// Slots whose LUT must be baked from their new key before this frame samples it.
    pub stale: [bool; 2],
}

impl<K: Copy + PartialEq> LutSlots<K> {
    /// Finds or assigns a slot for each wanted key. `b` is `None` when grade B isn't sampled.
    pub(crate) fn assign(&mut self, a: K, b: Option<K>) -> Assignment {
        self.calls += 1;
        let wanted = [Some(a), b];
        let mut slots =
            wanted.map(|key| key.and_then(|key| self.keys.iter().position(|slot| *slot == Some(key))));
        let mut stale = [false; 2];
        for grade in 0..2 {
            let Some(key) = wanted[grade] else { continue };
            if slots[grade].is_some() {
                continue;
            }
            if grade == 1 && key == a {
                slots[1] = slots[0];
                continue;
            }
            let other = slots[1 - grade];
            let slot = (0..2)
                .filter(|&slot| Some(slot) != other)
                .min_by_key(|&slot| self.used[slot])
                .expect("one of two slots is free of the other grade");
            self.keys[slot] = Some(key);
            stale[slot] = true;
            slots[grade] = Some(slot);
        }
        let slot_a = slots[0].expect("grade A always gets a slot");
        let slot_b = slots[1].unwrap_or(slot_a);
        self.used[slot_a] = self.calls;
        self.used[slot_b] = self.calls;
        Assignment { slots: [slot_a, slot_b], stale }
    }

    /// The key baked into `slot`.
    pub(crate) fn key(&self, slot: usize) -> Option<K> {
        self.keys[slot]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lut(curve: ToneCurve, grade: ColorGrade, strength: f32) -> Lut {
        Lut::new(&LutKey::new(&curve, &grade, strength))
    }

    fn curve(tone: ToneCurve) -> Curve {
        Curve::new(&LutKey::new(&tone, &ColorGrade::NEUTRAL, 1.0).curve)
    }

    /// Input values spanning the LUT's range, densest where the shaper is.
    fn sweep() -> impl Iterator<Item = f32> {
        (0..=4000).map(|step| unshape(step as f32 / 4000.0))
    }

    #[test]
    fn the_tone_curve_is_monotonic_continuous_and_bounded() {
        let curves = [
            ToneCurve::default(),
            ToneCurve { contrast: 1.6, linear_length: 0.0, ..ToneCurve::default() },
            ToneCurve { contrast: 0.5, black_tightness: 3.0, ..ToneCurve::default() },
            ToneCurve { linear_start: 0.6, linear_length: 0.99, ..ToneCurve::default() },
            ToneCurve { max_brightness: 0.8, black_tightness: 1.0, ..ToneCurve::default() },
        ];
        for tone in curves {
            let curve = curve(tone);
            let (mut previous_x, mut previous) = (0.0, curve.apply(0.0));
            assert_eq!(previous, 0.0, "{tone:?}: black stays black");
            for x in sweep().skip(1) {
                let y = curve.apply(x);
                assert!(y >= previous, "{tone:?}: not monotonic at {x}");
                // A jump would show as a band; no slope is steeper than about the toe's.
                assert!(y - previous <= 4.0 * (x - previous_x) + 1e-6, "{tone:?}: a jump at {x}");
                assert!(y <= tone.max_brightness, "{tone:?}: {x} maps above the maximum");
                (previous_x, previous) = (x, y);
            }
            assert!(
                curve.apply(SHAPER_MAX) > 0.99 * tone.max_brightness,
                "{tone:?}: the shoulder reaches white"
            );
        }
    }

    #[test]
    fn the_default_curve_keeps_midtones_and_rolls_off_highlights() {
        let curve = curve(ToneCurve::default());
        for x in [0.2, 0.3, 0.5, 0.65] {
            assert!((curve.apply(x) - x).abs() < 1e-6, "{x} is in the linear section");
        }
        assert!(curve.apply(0.05) < 0.05 && curve.apply(0.05) > 0.04, "a gentle toe");
        let white = curve.apply(1.0);
        assert!(white > 0.85 && white < 0.95, "lit white stays bright but not clipped: {white}");
        assert!(curve.apply(4.0) > 0.995);
    }

    #[test]
    fn a_neutral_grade_is_exactly_the_tone_curve() {
        let neutral = lut(ToneCurve::default(), ColorGrade::NEUTRAL, 1.0);
        let curve = curve(ToneCurve::default());
        for x in sweep() {
            let rgb = Vec3::new(x, x * 0.5, x * 2.0);
            assert_eq!(neutral.evaluate(rgb), rgb.map(|c| srgb_encode(curve.apply(c))));
        }
    }

    #[test]
    fn filter_strength_zero_bakes_the_neutral_lut_bit_for_bit() {
        let strong = ColorGrade {
            temperature: 0.8,
            tint: -0.4,
            filter: Vec3::new(1.2, 0.9, 0.7),
            contrast: 1.5,
            saturation: 1.8,
            lift: Vec3::splat(0.05),
            gamma: Vec3::new(1.2, 1.0, 0.8),
            gain: Vec3::new(0.9, 1.1, 1.0),
        };
        let mut off = vec![0; LUT_TEXELS];
        let mut neutral = vec![0; LUT_TEXELS];
        let mut full = vec![0; LUT_TEXELS];
        lut(ToneCurve::default(), strong, 0.0).bake(&mut off);
        lut(ToneCurve::default(), ColorGrade::NEUTRAL, 1.0).bake(&mut neutral);
        lut(ToneCurve::default(), strong, 1.0).bake(&mut full);
        assert!(off == neutral, "strength 0 is the neutral grade");
        assert!(full != neutral, "the grade does something");
    }

    #[test]
    fn baked_values_are_clamped_and_finite_for_any_parameters() {
        let wild = [
            ColorGrade { gain: Vec3::splat(100.0), lift: Vec3::splat(-5.0), ..ColorGrade::NEUTRAL },
            ColorGrade { saturation: f32::INFINITY, contrast: f32::NAN, ..ColorGrade::NEUTRAL },
            ColorGrade { gamma: Vec3::ZERO, temperature: -50.0, tint: 50.0, ..ColorGrade::NEUTRAL },
            ColorGrade { filter: Vec3::splat(-1.0), saturation: 0.0, ..ColorGrade::NEUTRAL },
        ];
        let tone = ToneCurve { max_brightness: f32::NAN, linear_start: 9.0, ..ToneCurve::default() };
        for grade in wild {
            let lut = lut(tone, grade, 1.0);
            for x in sweep().step_by(37) {
                for rgb in [Vec3::splat(x), Vec3::new(x, 0.0, x * 0.1), Vec3::new(f32::MAX, x, -x)] {
                    let value = lut.evaluate(rgb);
                    assert!(value.is_finite() && value.min_element() >= 0.0 && value.max_element() <= 1.0);
                }
            }
        }
        assert_eq!(pack_rgb10a2(Vec3::new(2.0, -1.0, 0.5)), 1023 | 512 << 20 | 3 << 30);
    }

    #[test]
    fn white_balance_keeps_white_neutral_only_at_zero_and_warms_with_temperature() {
        assert!((white_balance(0.0, 0.0) - Vec3::ONE).abs().max_element() < 1e-5);
        let warm = white_balance(0.5, 0.0);
        assert!(warm.x > warm.y && warm.y > warm.z, "warmer: {warm}");
        assert!((warm.dot(LUMA) - 1.0).abs() < 1e-5, "grey keeps its luminance");
        let cool = white_balance(-0.5, 0.0);
        assert!(cool.x < cool.y && cool.y < cool.z, "cooler: {cool}");
        let magenta = white_balance(0.0, 0.5);
        assert!(magenta.y < magenta.x && magenta.y < magenta.z, "magenta: {magenta}");
    }

    /// The GPU's trilinear lookup, emulated on the CPU from the packed texels.
    fn sample(texels: &[u32], rgb: Vec3) -> Vec3 {
        let size = LUT_SIZE as usize;
        let coordinate = rgb.map(|c| shape(c) * (LUT_SIZE - 1) as f32);
        let base = coordinate.floor().min(Vec3::splat((LUT_SIZE - 2) as f32));
        let fraction = coordinate - base;
        let unpack = |texel: u32| {
            Vec3::new((texel & 1023) as f32, (texel >> 10 & 1023) as f32, (texel >> 20 & 1023) as f32)
                / 1023.0
        };
        let mut sum = Vec3::ZERO;
        for corner in 0..8 {
            let offset = [corner & 1, corner >> 1 & 1, corner >> 2 & 1];
            let index = (0..3).map(|axis| base[axis] as usize + offset[axis]).collect::<Vec<_>>();
            let weight: f32 = (0..3)
                .map(|axis| if offset[axis] == 1 { fraction[axis] } else { 1.0 - fraction[axis] })
                .product();
            sum += unpack(texels[(index[2] * size + index[1]) * size + index[0]]) * weight;
        }
        sum
    }

    /// The largest trilinear error of `grade`'s LUT against direct evaluation, in 8-bit display
    /// code values, over `colors(x)` for inputs from black to past the shoulder.
    fn worst_error(grade: ColorGrade, colors: impl Fn(f32) -> Vec<Vec3>) -> f32 {
        let lut = lut(ToneCurve::default(), grade, 1.0);
        let mut texels = vec![0; LUT_TEXELS];
        lut.bake(&mut texels);
        let mut worst = 0.0f32;
        for x in sweep().step_by(3).filter(|&x| x <= 8.0) {
            for rgb in colors(x) {
                worst = worst.max((sample(&texels, rgb) - lut.evaluate(rgb)).abs().max_element() * 255.0);
            }
        }
        worst
    }

    #[test]
    fn the_lut_matches_direct_evaluation_within_display_code_values() {
        let ramps =
            |x: f32| vec![Vec3::splat(x), Vec3::new(x, x * 0.6, x * 0.3), Vec3::new(x * 0.2, x * 0.5, x)];
        // A neutral grade is separable, so its error is the 1D interpolation error alone.
        let neutral = worst_error(ColorGrade::NEUTRAL, ramps);
        assert!(neutral < 1.0, "neutral: up to {neutral:.2} code values");
        // The default grade mixes channels after the curve (saturation), which the table follows
        // almost as well, except where saturation pushes a channel past display white and the
        // clamp creases it: saturated blue at full brightness is the worst case. The error is
        // smooth, never a step.
        let look = crate::post::PostParams::default().grade_a;
        let graded = worst_error(look, ramps);
        assert!(graded < 2.5, "the default look: up to {graded:.2} code values");
        // The default sky's gradient, zenith to horizon, from a quarter to twice as bright as
        // drawn: what the sky can look like under exposure and time of day.
        let sky = crate::SkyGradient::default();
        let gradient = |x: f32| {
            let colors = [sky.zenith, sky.zenith.lerp(sky.horizon, 0.5), sky.horizon];
            if (0.25..=2.0).contains(&x) { colors.map(|color| color * x).to_vec() } else { Vec::new() }
        };
        let sky = worst_error(look, gradient);
        assert!(sky < 1.5, "the sky: up to {sky:.2} code values");
    }

    #[test]
    fn slots_reuse_baked_keys_and_bake_only_what_is_new() {
        let mut slots = LutSlots::default();
        let assign = |slots: &mut LutSlots<char>, a, b| {
            let Assignment { slots: [slot_a, slot_b], stale } = slots.assign(a, b);
            assert_eq!(slots.key(slot_a), Some(a));
            if let Some(b) = b {
                assert_eq!(slots.key(slot_b), Some(b));
            } else {
                assert_eq!(slot_b, slot_a, "an unused grade B reads A's LUT");
            }
            stale.iter().filter(|stale| **stale).count()
        };
        assert_eq!(assign(&mut slots, 'a', None), 1, "first bake");
        assert_eq!(assign(&mut slots, 'a', None), 0, "unchanged");
        assert_eq!(assign(&mut slots, 'a', Some('a')), 0, "B equal to A shares its LUT");
        assert_eq!(assign(&mut slots, 'a', Some('b')), 1);
        assert_eq!(assign(&mut slots, 'b', Some('c')), 1, "a key transition bakes only the new key");
        assert_eq!(assign(&mut slots, 'c', Some('b')), 0, "swapping keys bakes nothing");
        assert_eq!(assign(&mut slots, 'b', None), 0, "B alone is still baked");
        assert_eq!(assign(&mut slots, 'd', None), 1, "replaces the least recently used: c");
        assert_eq!(assign(&mut slots, 'b', Some('d')), 0, "so b survived");
        assert_eq!(assign(&mut slots, 'e', Some('f')), 2);
    }
}
