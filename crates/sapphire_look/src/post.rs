//! Plain-data post-processing input (rule R4): the frame packet's `post`. Everything here is
//! `Copy` data extraction fills once per frame; from M1 W1 on, the time-of-day system resolves it
//! from its keys, and the world's effect stack resolves the screen effects. The renderer's `post`
//! module draws it.

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
    /// Combat's screen effects for this frame (M1 P3; look pillar 8), resolved by the world's
    /// effect stack from what gameplay pushed: radial blur, chromatic aberration, inversion, a
    /// duotone and speed lines. [`PostEffects::NONE`], the default, draws none and costs
    /// nothing.
    pub effects: PostEffects,
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
            effects: PostEffects::NONE,
        }
    }
}

impl PostParams {
    /// These parameters without the compositing layers (M1 P2): no diffusion glow, characters
    /// blooming as the world does, no gradient layers or lens flare, a black vignette and no
    /// sharpening. Without effects, post draws them exactly as it did before the layers existed,
    /// through the same shader code (the fused pass's plain variant). The screen effects
    /// ([`Self::effects`], M1 P3) stay: they are gameplay's, not the look's.
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
    /// toward blue. The renderer takes -2 to 2 of it and of `tint`; in that square's corners, past
    /// about 1.7 of temperature and 1.5 of tint together, the white it adapts from would leave
    /// sRGB's gamut, and is held just inside it, so every channel keeps a positive gain.
    pub temperature: f32,
    /// Green–magenta balance, about -1 to 1: positive toward magenta. -2 to 2, as `temperature`.
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
/// alpha 0 for characters (the instances the frame packet's `characters` lists) and every other
/// pass leaves 1 (the renderer's post module's docs, "The character mask").
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

/// Combat's screen effects for one frame (M1 P3; look pillar 8, "combat screen grammar"): the
/// radial blurs, impact frames and speed lines of Wuthering Waves' and Zenless Zone Zero's hits,
/// drawn in the fused pass on every tier, over the compositing layers. They are resolved values:
/// the world's effect stack (`sapphire_world::screen_effects`) plays each effect gameplay pushed
/// over its own timeline and writes what shows this frame here (rule R4).
///
/// Each effect's strength, amount or opacity at 0 turns it off; [`Self::NONE`], the default, has
/// none, and post then draws exactly what it did before effects existed, through the same
/// pipelines. Positions are texture coordinates: (0, 0) the top left, (1, 1) the bottom right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PostEffects {
    pub radial_blur: RadialBlur,
    pub chromatic_aberration: ChromaticAberration,
    pub inversion: Inversion,
    pub duotone: Duotone,
    pub speed_lines: SpeedLines,
}

impl PostEffects {
    /// No effect.
    pub const NONE: Self = Self {
        radial_blur: RadialBlur::NONE,
        chromatic_aberration: ChromaticAberration::NONE,
        inversion: Inversion::NONE,
        duotone: Duotone::NONE,
        speed_lines: SpeedLines::NONE,
    };
}

impl Default for PostEffects {
    fn default() -> Self {
        Self::NONE
    }
}

/// How much of an effect shows on the world and how much on characters: the character mask
/// (scene color's alpha; the post module's docs) tells them apart per pixel. Each is 0 to 1;
/// blending effects blends these too.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectCoverage {
    pub world: f32,
    pub characters: f32,
}

impl EffectCoverage {
    /// On every pixel alike: no mask read.
    pub const EVERYWHERE: Self = Self { world: 1.0, characters: 1.0 };
    /// On the world, characters left as they are ("keep characters colored", speed lines behind
    /// characters).
    pub const WORLD: Self = Self { world: 1.0, characters: 0.0 };
    /// On characters alone.
    pub const CHARACTERS: Self = Self { world: 0.0, characters: 1.0 };
}

/// A zoom blur toward a point on screen (an impact's center, the camera's heading): each pixel
/// averages the frame along the line from it toward `center`, so the frame streaks outward from
/// it, more so farther out. The fused pass's taps per pixel are the tier's
/// (the renderer's `PostEffectSettings::radial_blur_taps`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadialBlur {
    pub center: Vec2,
    /// How far toward the center each pixel's taps reach, as a share of its distance from it: 0
    /// is no blur, 0.1 a strong one; at most 1.
    pub strength: f32,
}

impl RadialBlur {
    pub const NONE: Self = Self { center: Vec2::splat(0.5), strength: 0.0 };
}

/// Chromatic aberration, a lens's colored fringes or a hit's RGB split: red is sampled nearer to
/// `center` and blue farther from it than green, by a split growing with the square of the
/// distance from it, so the middle stays clean and red fringes face outward. With the radial
/// blur on too, the split spreads the colors along the blur's streaks instead (a spectral zoom).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromaticAberration {
    pub center: Vec2,
    /// The split half a frame's diagonal from the center (at the corners, for a centered one),
    /// as a share of that distance: 0.02 is a subtle fringe, 0.1 a hard split; at most 0.25.
    pub strength: f32,
}

impl ChromaticAberration {
    pub const NONE: Self = Self { center: Vec2::splat(0.5), strength: 0.0 };
}

/// The frame's display values inverted, `1 − c`: the negative impact frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inversion {
    /// 0 is none and 1 fully inverted.
    pub amount: f32,
    pub coverage: EffectCoverage,
}

impl Inversion {
    pub const NONE: Self = Self { amount: 0.0, coverage: EffectCoverage::EVERYWHERE };
}

/// A duotone: the frame's display luminance mapped from `dark` to `light`, the two-tone impact
/// frame (black and white, or black and red), usually with characters kept in color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Duotone {
    /// 0 is none and 1 the duotone alone.
    pub amount: f32,
    /// Linear RGB for the dark side and the light side.
    pub dark: Vec3,
    pub light: Vec3,
    /// The display luminance (of sRGB-encoded values, 0 to 1) where the frame turns from dark
    /// to light.
    pub threshold: f32,
    /// The width of that turn: near 0 a hard two-tone cut, 1 a smooth gradient map.
    pub softness: f32,
    pub coverage: EffectCoverage,
}

impl Duotone {
    pub const NONE: Self = Self {
        amount: 0.0,
        dark: Vec3::ZERO,
        light: Vec3::ONE,
        threshold: 0.5,
        softness: 0.1,
        coverage: EffectCoverage::EVERYWHERE,
    };
}

/// Speed lines (anime's focus lines): thin wedges radiating from `center`, sharp at their inner
/// tips, around a clear area, drawn over the frame's display values in their own color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedLines {
    /// 0 is none and 1 the lines in their full color.
    pub opacity: f32,
    pub center: Vec2,
    /// Lines around the center: the circle is split into this many slots, one line in each
    /// (1 to 1024).
    pub count: u32,
    /// A line's width at its outer end, as a share of its slot (0 to 1); each line takes 35% to
    /// 100% of it.
    pub width: f32,
    /// The clear radius around the center, in frame heights, inside which no line reaches.
    pub inner_radius: f32,
    /// How far past the clear radius lines' tips scatter, in frame heights.
    pub spread: f32,
    /// Linear RGB.
    pub color: Vec3,
    /// Picks the lines' widths, places and tips: the effect stack moves it a few times a second,
    /// as hand-drawn lines are redrawn.
    pub seed: u32,
    pub coverage: EffectCoverage,
}

impl SpeedLines {
    pub const NONE: Self = Self {
        opacity: 0.0,
        center: Vec2::splat(0.5),
        count: 96,
        width: 0.3,
        inner_radius: 0.3,
        spread: 0.3,
        color: Vec3::ONE,
        seed: 0,
        coverage: EffectCoverage::EVERYWHERE,
    };
}
