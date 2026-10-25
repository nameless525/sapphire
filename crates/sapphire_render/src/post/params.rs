//! Plain-data post-processing input (rule R4): [`FramePacket::post`](crate::FramePacket::post).
//! Everything here is `Copy` data extraction fills once per frame; from M1 W1 on, the
//! time-of-day system resolves it from its keys.

use glam::Vec3;

/// What post-processing does to one frame: exposure, bloom, the tone curve and grade, and the
/// vignette. [`PostParams::default`] is Sapphire's default anime look.
///
/// The renderer compares this with the previous frame's by value. A change to `tone_curve`,
/// either grade or `filter_strength` re-bakes a grading LUT, a few milliseconds of CPU
/// spread over the job pool, so those are meant to change at keys (a time-of-day key, a zone,
/// a settings slider) rather than every frame. `grade_blend` and everything else animate for
/// free.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PostParams {
    /// Exposure in stops, applied to the scene before bloom and the tone curve (0 is ×1).
    pub exposure: f32,
    pub bloom: BloomParams,
    /// Scene-linear to display, shared by both grades.
    pub tone_curve: ToneCurve,
    /// The grade baked into LUT A.
    pub grade_a: ColorGrade,
    /// The grade baked into LUT B: the next time-of-day key, or a zone's override. Not baked
    /// or sampled while `grade_blend` is 0.
    pub grade_b: ColorGrade,
    /// 0 shows grade A alone and 1 grade B alone; in between, the two LUTs' outputs blend.
    pub grade_blend: f32,
    /// The player's color-filter strength, 0 to 1, as both target games offer: 0 is the neutral
    /// grade (the tone curve alone) and 1 the full grade.
    pub filter_strength: f32,
    pub vignette: Vignette,
}

impl Default for PostParams {
    /// A clean, bright anime grade: authored colors at their own brightness, a little more
    /// saturated, a late shoulder, and bloom only on what is genuinely brighter than lit white.
    fn default() -> Self {
        let grade = ColorGrade {
            saturation: 1.15,
            contrast: 1.05,
            // Slightly lifted, cool shadows, as cel animation paints them.
            lift: Vec3::new(0.0, 0.005, 0.015),
            ..ColorGrade::NEUTRAL
        };
        Self {
            exposure: 0.0,
            bloom: BloomParams::default(),
            tone_curve: ToneCurve::default(),
            grade_a: grade,
            grade_b: grade,
            grade_blend: 0.0,
            filter_strength: 1.0,
            vignette: Vignette::default(),
        }
    }
}

/// Bloom: what is brighter than a threshold spreads into a wide, soft glow.
///
/// It is computed down a dual-filter chain of half-resolution levels, as many as
/// `RenderSettings::bloom_levels` asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BloomParams {
    /// Brightness after exposure (the largest of R, G and B) where bloom starts. Lit white
    /// is about 1, so a threshold above it keeps bloom to what is genuinely bright: the sky,
    /// highlights and, later, emissive materials.
    pub threshold: f32,
    /// Softens the threshold, 0 to 1: 0 cuts hard at `threshold`, 1 fades bloom in from 0 to
    /// twice the threshold.
    pub knee: f32,
    /// How much bloom is added to the scene. 0 skips the bloom passes.
    pub intensity: f32,
    /// How far bloom spreads, 0 to 1: how much of each level comes from the wider levels below
    /// it rather than its own.
    pub scatter: f32,
}

impl Default for BloomParams {
    fn default() -> Self {
        Self { threshold: 1.0, knee: 0.5, intensity: 0.25, scatter: 0.7 }
    }
}

/// The tone curve, from scene-linear to display-linear light, per channel.
///
/// It is the GT curve (Hajime Uchimura, "HDR theory and practice", CEDEC 2017): a toe, a
/// linear midtone section and a soft exponential shoulder that approaches `max_brightness`.
/// With `contrast` 1 the midtones pass through unchanged, so authored colors keep their
/// values and saturation: the anime look is not a filmic one, and ACES's contrast and hue
/// skew are deliberately avoided (spec §5.12; Genshin rejected ACES for the same reason).
///
/// Per channel, the shoulder pulls bright saturated colors toward white, so the default
/// keeps the shoulder short and late and the grade restores saturation on display values.
/// A hue-stable shoulder (AgX-style) mixes channels in a way a 32³ table can't follow near
/// white; see the `post` module's later milestones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneCurve {
    /// What very bright input approaches: 1 is display white.
    pub max_brightness: f32,
    /// Slope of the linear section: 1 leaves midtones as they are.
    pub contrast: f32,
    /// Where the linear section starts. Below it, the toe curves into black.
    pub linear_start: f32,
    /// How much of the range between `linear_start` and `max_brightness` stays linear, 0 to
    /// just under 1. The shoulder starts after it.
    pub linear_length: f32,
    /// The toe's exponent, 1 to 3: 1 has no toe, higher values deepen the blacks.
    pub black_tightness: f32,
}

impl Default for ToneCurve {
    /// Linear up to about 0.7 and a short shoulder above, so lit colors stay bright and clean
    /// and only highlights compress; a gentle toe.
    fn default() -> Self {
        Self {
            max_brightness: 1.0,
            contrast: 1.0,
            linear_start: 0.2,
            linear_length: 0.6,
            black_tightness: 1.15,
        }
    }
}

/// A color grade. [`ColorGrade::NEUTRAL`] changes nothing.
///
/// The scene-referred stages run before the tone curve, on exposed linear color: white
/// balance, the color filter, contrast and saturation. Lift, gamma and gain run after it, on
/// display values (sRGB-encoded, as color-wheel grading tools apply them).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorGrade {
    /// White balance, about -1 to 1: positive warms the picture toward amber, negative cools it
    /// toward blue.
    pub temperature: f32,
    /// Green–magenta balance, about -1 to 1: positive toward magenta.
    pub tint: f32,
    /// Multiplies the scene color: a colored filter in linear RGB.
    pub filter: Vec3,
    /// Contrast around middle grey (0.18): 1 is unchanged.
    pub contrast: f32,
    /// 1 is unchanged and 0 is greyscale.
    pub saturation: f32,
    /// Raises the blacks per channel, with less effect toward white: 0 is unchanged.
    pub lift: Vec3,
    /// Midtone power per channel: 1 is unchanged, higher brightens.
    pub gamma: Vec3,
    /// Scales the display values per channel: 1 is unchanged.
    pub gain: Vec3,
}

impl ColorGrade {
    /// No grading: the tone curve alone.
    pub const NEUTRAL: Self = Self {
        temperature: 0.0,
        tint: 0.0,
        filter: Vec3::ONE,
        contrast: 1.0,
        saturation: 1.0,
        lift: Vec3::ZERO,
        gamma: Vec3::ONE,
        gain: Vec3::ONE,
    };
}

impl Default for ColorGrade {
    fn default() -> Self {
        Self::NEUTRAL
    }
}

/// Darkens the frame toward its corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vignette {
    /// Darkening at the corners, 0 (none) to 1 (black).
    pub intensity: f32,
    /// Where darkening starts, as a fraction of the distance from the center to a corner.
    pub start: f32,
}

impl Default for Vignette {
    fn default() -> Self {
        Self { intensity: 0.18, start: 0.45 }
    }
}
