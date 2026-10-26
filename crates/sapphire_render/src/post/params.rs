//! Plain-data post-processing input (rule R4): [`FramePacket::post`](crate::FramePacket::post).
//! Everything here is `Copy` data extraction fills once per frame; from M1 W1 on, the
//! time-of-day system resolves it from its keys.

use glam::{Vec2, Vec3};

/// The most gradient ("para") layers a frame composites ([`PostParams::gradients`]).
pub const MAX_GRADIENT_LAYERS: usize = 4;

/// What post-processing does to one frame: exposure, bloom, the tone curve and grade, the
/// vignette, and (M1 P2) the anime compositing layers over them: the diffusion glow, the
/// characters' own bloom, gradient ("para") layers and a lens flare from the sun, plus the
/// player's sharpening. [`PostParams::default`] is Sapphire's default anime look, as post drew
/// it before the compositing layers: it has none of them ([`Self::plain`]).
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
    /// grade (the tone curve alone) and 1 the full grade. It scales the gradient layers'
    /// opacity too: they are the picture's other color filter.
    pub filter_strength: f32,
    pub vignette: Vignette,
    /// The wide, soft glow screened over the graded frame (M1 P2).
    pub diffusion: Diffusion,
    /// How characters bloom, relative to the world (M1 P2).
    pub character_bloom: CharacterBloom,
    /// Screen-space gradient ("para") layers over the graded frame, composited in order
    /// (M1 P2). Layers with no opacity are skipped.
    pub gradients: [GradientLayer; MAX_GRADIENT_LAYERS],
    /// Sprites along the axis from the sun through the middle of the frame (M1 P2).
    pub lens_flare: LensFlare,
    /// The player's sharpening, 0 (off) to 1: contrast-adaptive sharpening (RCAS) in the fused
    /// pass (M1 P2). Off by default: after temporal AA it moves the image away from 4× MSAA
    /// and draws halos along outlines (spec §5.12, "As built (M1 R7)" and "(M1 P2)").
    pub sharpening: f32,
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
            diffusion: Diffusion::NONE,
            character_bloom: CharacterBloom::NEUTRAL,
            gradients: [GradientLayer::NONE; MAX_GRADIENT_LAYERS],
            lens_flare: LensFlare::NONE,
            sharpening: 0.0,
        }
    }
}

impl PostParams {
    /// These parameters without the compositing layers (M1 P2): no diffusion glow, characters
    /// blooming as the world does, no gradient layers or lens flare, a black vignette and no
    /// sharpening. Post draws them exactly as it did before the layers existed, through the
    /// same shader code (the fused pass's plain variant).
    pub fn plain(self) -> Self {
        Self {
            vignette: Vignette { color: Vec3::ZERO, ..self.vignette },
            diffusion: Diffusion::NONE,
            character_bloom: CharacterBloom::NEUTRAL,
            gradients: [GradientLayer::NONE; MAX_GRADIENT_LAYERS],
            lens_flare: LensFlare::NONE,
            sharpening: 0.0,
            ..self
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

/// Darkens the frame toward its corners, or tints it: anime vignettes are often colored, a deep
/// blue or violet at night.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vignette {
    /// How far the corners go toward `color`, 0 (not at all) to 1 (all the way).
    pub intensity: f32,
    /// Where it starts, as a fraction of the distance from the center to a corner.
    pub start: f32,
    /// Linear RGB the corners are multiplied toward (M1 P2): black darkens them, as the vignette
    /// always did; a color tints them.
    pub color: Vec3,
}

impl Default for Vignette {
    fn default() -> Self {
        Self { intensity: 0.18, start: 0.45, color: Vec3::ZERO }
    }
}

/// The diffusion glow (M1 P2; look pillar 5): a wide, soft glow over bloom, the soft focus of
/// anime compositing (Blue Protocol's "diffusion", the dreamy bloom of Wuthering Waves' and
/// Genshin's cutscenes). It spreads what bloom spreads, from the bloom chain's widest smooth
/// level, through a soft threshold of its own, and is screened over the graded frame: it lifts
/// what is dark around bright light and never pushes anything past white.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Diffusion {
    /// How fast the glow builds with the light under it: 0 turns it off.
    pub intensity: f32,
    /// Light in the bloom chain's wide level (after exposure, as bloom's threshold) below which
    /// nothing diffuses: it fades in from half this to one and a half times this.
    pub threshold: f32,
    /// The glow's color, linear RGB: what it approaches over the brightest light.
    pub color: Vec3,
}

impl Diffusion {
    /// No glow.
    pub const NONE: Self = Self { intensity: 0.0, threshold: 0.0, color: Vec3::ONE };
}

/// How characters bloom (M1 P2; look pillar 5): with a threshold and a strength of their own,
/// relative to the world's bloom, so faces lit near white don't blow out while the sky and the
/// world glow (Honkai: Star Rail controls characters' bloom apart, too).
///
/// Character pixels are the ones scene color's alpha marks: the anime character shader writes
/// alpha 0 for characters (instances [`FramePacket::characters`](crate::FramePacket::characters)
/// lists) and every other pass leaves 1 (the post module's docs, "The character mask").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterBloom {
    /// The world's bloom threshold times this: above 1, characters need brighter light to bloom.
    pub threshold: f32,
    /// The world's bloom intensity times this: 0, characters add nothing to bloom.
    pub strength: f32,
}

impl CharacterBloom {
    /// Characters bloom as the world does.
    pub const NEUTRAL: Self = Self { threshold: 1.0, strength: 1.0 };
}

/// A screen-space gradient layer (M1 P2; look pillar 3): anime compositing's "para", a tint
/// ramped across the frame (the top darkened toward the sky's blue, warm light washing in from
/// the bottom, a radial glow), blended over the graded frame's display values.
///
/// The layer's opacity ramps from full at `start` to nothing at `end`, smoothly; positions are
/// the shape's ([`GradientShape`]), and `start` may lie either side of `end`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientLayer {
    pub shape: GradientShape,
    pub blend: GradientBlend,
    /// Linear RGB; blended as its sRGB-encoded value, as compositing tools blend.
    pub color: Vec3,
    /// 0 hides the layer, 1 blends it in fully at `start`.
    pub opacity: f32,
    /// Where the layer is at full opacity.
    pub start: f32,
    /// Where it has faded out.
    pub end: f32,
}

impl GradientLayer {
    /// An empty slot: no opacity.
    pub const NONE: Self = Self {
        shape: GradientShape::Linear { edge: 0.0 },
        blend: GradientBlend::Normal,
        color: Vec3::ZERO,
        opacity: 0.0,
        start: 0.0,
        end: 1.0,
    };
}

/// Where a [`GradientLayer`] ramps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientShape {
    /// Straight across the frame from one side: `edge` is the side it comes from, in radians
    /// clockwise from the top (0 the top, π/2 the right, π the bottom). Positions run from 0 at
    /// that side to 1 at the opposite one (corner to corner for a diagonal edge).
    Linear { edge: f32 },
    /// Round on screen around `center`, in texture coordinates (0, 0 the top left, 1, 1 the
    /// bottom right). Positions are distances from it in half the frame's diagonal: 1 reaches
    /// the corners from the middle.
    Radial { center: Vec2 },
}

/// How a [`GradientLayer`] blends over the frame: the compositing modes, on sRGB-encoded
/// display values `a` (the frame) and `b` (the layer's color).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GradientBlend {
    /// `b`, over `a`.
    #[default]
    Normal,
    /// `a × b`: darkens toward the color.
    Multiply,
    /// `1 − (1 − a)(1 − b)`: lightens toward it, never past white.
    Screen,
    /// Multiply in the darks and screen in the lights, by `a`: deepens contrast in the color.
    Overlay,
    /// A gentle overlay (the W3C compositing formula): tints without crushing blacks or whites.
    SoftLight,
    /// `a + b`, clamped to white: light added, as a dodge.
    Add,
}

/// A lens flare from the sun (M1 P2; look pillar 5): a halo, a streak and ghosts along the axis
/// from the sun through the middle of the frame, as anime photography adds them, big and
/// colorful. They fade with how much of the sun's disc is visible, measured on the frame itself
/// (the post module's docs, "Lens flares"), so clouds and characters in front of the sun put it
/// out. The flare's elements are the renderer's default lens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensFlare {
    /// Its brightness, as light added to the scene before the tone curve: 0 turns it off.
    pub intensity: f32,
    /// Linear RGB tinting every element.
    pub color: Vec3,
}

impl LensFlare {
    /// No flare.
    pub const NONE: Self = Self { intensity: 0.0, color: Vec3::ONE };
}
