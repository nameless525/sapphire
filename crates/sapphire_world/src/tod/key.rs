//! Authored keys: the look at one time, group by group, as documents spell it.

use bevy_reflect::Reflect;
use sapphire_math::Ease;
use sapphire_render::{
    BloomParams, CharacterBloom, ColorGrade, Diffusion, GradientBlend, GradientLayer, GradientShape,
    LensFlare, MAX_GRADIENT_LAYERS, Vignette,
};
use serde::{Deserialize, Serialize};

/// A linear RGB color (or, for a direction, a vector), authored as `(r, g, b)`.
pub type Rgb = [f32; 3];

/// The look at one time: the time, how the blend toward the next key goes, and the value of
/// every group the key sets.
///
/// Every group is optional, so one key type serves the base timeline (which sets every group),
/// a zone's partial override (which sets only what the zone changes: its fog and ambient, say)
/// and a cutscene's value track. Within one timeline or track, every key sets the same groups,
/// so each group always has two keys to blend between.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct TodKey {
    /// In a [`TodTimeline`](super::TodTimeline): hours since midnight, `0 ≤ time < 24`. In a
    /// [`TodCutscene`](super::TodCutscene): seconds since the shot began.
    pub time: f32,
    /// How the blend from this key to the next progresses.
    #[serde(default, skip_serializing_if = "is_linear")]
    pub ease: TodEase,
    /// The sun: its direction, and the light it gives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sun: Option<CelestialKey>,
    /// The moon, likewise. The key light is resolved from the two (see
    /// [`TodLook::key_light`](super::TodLook::key_light)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moon: Option<CelestialKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sky: Option<SkyKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient: Option<AmbientKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fog: Option<FogKey>,
    /// Linear RGB the key light keeps inside cast shadows: black is a physical shadow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_tint: Option<Rgb>,
    /// The character rig's key light.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<CharacterKey>,
    /// 0 by day, 1 at night: the ramp rows characters blend between.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_night: Option<f32>,
    /// Exposure in stops (0 is ×1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure: Option<f32>,
    /// The color grade, baked into a LUT; neighboring keys' grades blend as two LUTs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade: Option<GradeKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bloom: Option<BloomKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vignette: Option<VignetteKey>,
    /// The sky's sun and moon discs and their glows (M1 W2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discs: Option<DiscsKey>,
    /// Star brightness: 0 by day, 1 for a full night sky (M1 W2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stars: Option<f32>,
    /// The nebula's colors, under the stars (M1 X1 part 2e): black by day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nebula: Option<NebulaKey>,
    /// The clouds' light and cover (M1 W2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clouds: Option<CloudsKey>,
    /// The diffusion glow over bloom (M1 P2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diffusion: Option<DiffusionKey>,
    /// How characters bloom, relative to the world (M1 P2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character_bloom: Option<CharacterBloomKey>,
    /// Screen-space gradient ("para") layers over the graded frame, at most four, composited in
    /// order (M1 P2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradients: Option<Vec<GradientKey>>,
    /// The lens flare from the sun (M1 P2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens_flare: Option<LensFlareKey>,
    /// How much of the world's authored emission shows: environment materials' emission × this,
    /// 1 as authored (M1 W1's keyed emission; the renderer reads it as
    /// [`sapphire_render::FramePacket::emission_scale`]). Exposure can't tell a lantern from the
    /// light around it, so a lantern bright enough to bloom at night would glow at noon too;
    /// keyed down by day, paper lanterns and lit windows rest pale and come on at dusk. A
    /// material can keep a share of its emission steady at every hour (its `emission_steady`),
    /// and characters' emission never follows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emission: Option<f32>,
}

fn is_linear(ease: &TodEase) -> bool {
    *ease == TodEase::Linear
}

/// How a key blends toward the next one ([`sapphire_math::Ease`], as documents spell it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
pub enum TodEase {
    /// Constant speed: the sun moves steadily, colors change at an even pace.
    #[default]
    Linear,
    /// Slow at both ends.
    Smooth,
    /// Starts slowly.
    In,
    /// Arrives slowly.
    Out,
    /// Holds this key's look until the next key, then switches.
    Hold,
}

impl From<TodEase> for Ease {
    fn from(ease: TodEase) -> Self {
        match ease {
            TodEase::Linear => Self::Linear,
            TodEase::Smooth => Self::Smooth,
            TodEase::In => Self::In,
            TodEase::Out => Self::Out,
            TodEase::Hold => Self::Hold,
        }
    }
}

/// The sun or the moon at a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct CelestialKey {
    /// The direction its light travels, from the body into the scene, so the body sits at
    /// `-direction` in the sky. Need not be unit length. Between keys it moves along the great
    /// circle at constant angular speed.
    pub direction: Rgb,
    /// Linear RGB.
    pub color: Rgb,
    /// 0 for a body that gives no light (the sun at night, the moon by day).
    pub intensity: f32,
}

/// The sky gradient at a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct SkyKey {
    pub zenith: Rgb,
    pub horizon: Rgb,
    pub ground: Rgb,
}

/// The sun's and the moon's discs and glows at a key, as the sky draws them
/// ([`sapphire_render::SkyLayers`]). Where they are is the `sun` and `moon` groups'; their sizes
/// are the sky material's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct DiscsKey {
    pub sun: DiscKey,
    pub moon: DiscKey,
}

/// One body's disc and glow. A disc keeps its own look apart from the light the body gives: the
/// moon can show by day, a setting sun's disc can fade before its light does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct DiscKey {
    /// Linear RGB; the disc's radiance is `color × intensity`.
    pub color: Rgb,
    /// 0 hides the disc. In HDR, so a bright disc blooms: the sun's is tens.
    pub intensity: f32,
    /// The glow around it: linear RGB at its center, fading to nothing at `glow_size`.
    #[serde(default)]
    pub glow: Rgb,
    /// The glow's radius in degrees, 0 to 180.
    #[serde(default)]
    pub glow_size: f32,
}

/// The night sky's nebula at a key ([`sapphire_render::Nebula`]): the color of each kind of cloud
/// the sky material's nebula map paints, linear RGB, HDR. A channel's light is its color times
/// its density, so black hides it; between keys each color blends in OkLab, so the nebula fades
/// out through its own hues and can turn them through the night.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(default, deny_unknown_fields)]
pub struct NebulaKey {
    /// The map's R: its broad clouds.
    pub clouds: Rgb,
    /// G: the bright wisps along them.
    pub wisps: Rgb,
    /// B: sparse accents.
    pub accents: Rgb,
}

/// The clouds at a key ([`sapphire_render::CloudLight`]): painted in a lit and a shade tone with
/// a rim toward the key light, and how much of each cloud layer shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct CloudsKey {
    /// Linear RGB of their sides facing the key light.
    pub lit: Rgb,
    /// Linear RGB of their shaded sides: an authored color, never a darkened `lit`.
    pub shade: Rgb,
    /// Linear RGB of their silver lining toward the key light.
    #[serde(default)]
    pub rim: Rgb,
    /// How much of the card clouds shows, 0 (clear) to 1 (as painted).
    #[serde(default)]
    pub coverage: f32,
    /// How much of the cirrus shows, 0 to 1.
    #[serde(default)]
    pub cirrus: f32,
}

/// Hemispherical ambient light at a key: `color × intensity` from above and below.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct AmbientKey {
    pub sky: Rgb,
    pub ground: Rgb,
    pub intensity: f32,
}

/// Fog at a key ([`sapphire_render::Fog`] says how the renderer draws each part; M1 W3). A layer
/// left out has no fog, and characters take none of it unless `characters` says so.
///
/// Between keys each layer's color blends in OkLab weighted by the layers' densities, so a layer
/// one key doesn't have fades in or out in the other key's color rather than through black.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct FogKey {
    #[serde(default)]
    pub near: FogLayerKey,
    #[serde(default)]
    pub far: FogLayerKey,
    #[serde(default)]
    pub height: HeightFogKey,
    /// Light scattered toward the camera around the key light, linear RGB (color × strength).
    #[serde(default)]
    pub sun_inscatter: Rgb,
    /// How much of the fog characters take, 0 (none: they keep their colors at any distance) to
    /// 1 (as the world): the scale on their optical depth, which keeps them readable in fog.
    /// Unset, 0: keys written before it (M1 W3) leave characters clear of the fog.
    #[serde(default)]
    pub characters: f32,
}

/// One distance layer of fog. The far layer's color is a tint on the sky's horizon (white is
/// the horizon itself), which is what melts distant geometry into the sky.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct FogLayerKey {
    pub color: Rgb,
    /// Extinction per meter.
    #[serde(default)]
    pub density: f32,
    /// Meters from the camera where the layer begins.
    #[serde(default)]
    pub start: f32,
}

/// Fog that thickens toward the ground.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct HeightFogKey {
    pub color: Rgb,
    /// Extinction per meter at `base`.
    #[serde(default)]
    pub density: f32,
    /// How fast it thins with height, per meter.
    #[serde(default)]
    pub falloff: f32,
    /// The height in meters where `density` holds.
    #[serde(default)]
    pub base: f32,
}

/// The character rig's key light at a key: its color and brightness, curved through the day
/// apart from the world's light but fitted to it, as Wuthering Waves fits character brightness
/// to its environment (the default day keeps it the key light's own by day and a little
/// brighter at night). Exposure lifts it with the world's light, so a night key set well above
/// the moon's brings characters back to their noon brightness. Where the horizon dims the sun or
/// moon, characters take it dimmed with the world's light
/// ([`TodLook::character_key`](super::TodLook::character_key)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct CharacterKey {
    pub color: Rgb,
    pub intensity: f32,
}

/// A color grade ([`ColorGrade`]); every field left out is neutral, so a key writes only what
/// it changes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(default, deny_unknown_fields)]
pub struct GradeKey {
    pub temperature: f32,
    pub tint: f32,
    pub filter: Rgb,
    pub contrast: f32,
    pub saturation: f32,
    pub lift: Rgb,
    pub gamma: Rgb,
    pub gain: Rgb,
}

impl Default for GradeKey {
    fn default() -> Self {
        Self::from(ColorGrade::NEUTRAL)
    }
}

impl From<ColorGrade> for GradeKey {
    fn from(grade: ColorGrade) -> Self {
        Self {
            temperature: grade.temperature,
            tint: grade.tint,
            filter: grade.filter.to_array(),
            contrast: grade.contrast,
            saturation: grade.saturation,
            lift: grade.lift.to_array(),
            gamma: grade.gamma.to_array(),
            gain: grade.gain.to_array(),
        }
    }
}

impl From<GradeKey> for ColorGrade {
    fn from(key: GradeKey) -> Self {
        Self {
            temperature: key.temperature,
            tint: key.tint,
            filter: key.filter.into(),
            contrast: key.contrast,
            saturation: key.saturation,
            lift: key.lift.into(),
            gamma: key.gamma.into(),
            gain: key.gain.into(),
        }
    }
}

/// Bloom at a key ([`BloomParams`]); fields left out take the default look's.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(default, deny_unknown_fields)]
pub struct BloomKey {
    pub threshold: f32,
    pub knee: f32,
    pub intensity: f32,
    pub scatter: f32,
}

impl Default for BloomKey {
    fn default() -> Self {
        let BloomParams { threshold, knee, intensity, scatter } = BloomParams::default();
        Self { threshold, knee, intensity, scatter }
    }
}

impl From<BloomKey> for BloomParams {
    fn from(key: BloomKey) -> Self {
        Self { threshold: key.threshold, knee: key.knee, intensity: key.intensity, scatter: key.scatter }
    }
}

/// The vignette at a key ([`Vignette`]); fields left out take the default look's.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(default, deny_unknown_fields)]
pub struct VignetteKey {
    pub intensity: f32,
    pub start: f32,
    /// Linear RGB the corners go toward (M1 P2): black, the default, darkens them.
    #[serde(skip_serializing_if = "is_black")]
    pub color: Rgb,
}

fn is_black(color: &Rgb) -> bool {
    *color == [0.0; 3]
}

impl Default for VignetteKey {
    fn default() -> Self {
        let Vignette { intensity, start, color } = Vignette::default();
        Self { intensity, start, color: color.to_array() }
    }
}

impl From<VignetteKey> for Vignette {
    fn from(key: VignetteKey) -> Self {
        Self { intensity: key.intensity, start: key.start, color: key.color.into() }
    }
}

/// The diffusion glow at a key ([`Diffusion`]): a wide, soft glow screened over the graded frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct DiffusionKey {
    /// How fast the glow builds with the light under it: 0 turns it off.
    pub intensity: f32,
    /// Light (after exposure) where the glow starts: it fades in from half this to one and a
    /// half times this.
    #[serde(default)]
    pub threshold: f32,
    /// Linear RGB the glow builds toward over the brightest light.
    #[serde(default = "white")]
    pub color: Rgb,
}

fn white() -> Rgb {
    [1.0; 3]
}

impl From<DiffusionKey> for Diffusion {
    fn from(key: DiffusionKey) -> Self {
        Self { intensity: key.intensity, threshold: key.threshold, color: key.color.into() }
    }
}

/// How characters bloom at a key ([`CharacterBloom`]): their threshold and strength as multiples
/// of the world's bloom. Fields left out are 1, as the world's.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(default, deny_unknown_fields)]
pub struct CharacterBloomKey {
    /// The world's threshold times this: above 1, faces need brighter light to bloom.
    pub threshold: f32,
    /// The world's intensity times this: 0, characters add nothing to bloom.
    pub strength: f32,
}

impl Default for CharacterBloomKey {
    fn default() -> Self {
        let CharacterBloom { threshold, strength } = CharacterBloom::NEUTRAL;
        Self { threshold, strength }
    }
}

impl From<CharacterBloomKey> for CharacterBloom {
    fn from(key: CharacterBloomKey) -> Self {
        Self { threshold: key.threshold, strength: key.strength }
    }
}

/// One gradient ("para") layer at a key ([`GradientLayer`]): a tint ramped across the frame,
/// from full opacity at `start` to nothing at `end`, in a blend mode.
///
/// Between keys a layer blends with the one in the same place in the next key's list: colors in
/// OkLab, the rest linearly, while their shape and blend mode match. Where they differ, the one
/// fades out by halfway and the other in after it, so the picture never jumps; a layer only one
/// of the keys has fades from or to no opacity.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct GradientKey {
    pub shape: GradientShapeKey,
    #[serde(default)]
    pub blend: GradientBlendKey,
    /// Linear RGB.
    pub color: Rgb,
    /// 0 to 1.
    #[serde(default = "one")]
    pub opacity: f32,
    /// Where the layer is at full opacity, in its shape's positions.
    #[serde(default)]
    pub start: f32,
    /// Where it has faded out.
    #[serde(default = "one")]
    pub end: f32,
}

fn one() -> f32 {
    1.0
}

/// Where a gradient layer ramps ([`GradientShape`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
pub enum GradientShapeKey {
    /// Straight across the frame from the side `edge` degrees clockwise from the top (0 the top,
    /// 90 the right, 180 the bottom), positions 0 there to 1 at the opposite side.
    Linear { edge: f32 },
    /// Round around `center` (0, 0 the top left of the frame, 1, 1 its bottom right), positions
    /// in half the frame's diagonal.
    Radial { center: [f32; 2] },
}

/// A gradient layer's blend mode ([`GradientBlend`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
pub enum GradientBlendKey {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    SoftLight,
    Add,
}

impl From<GradientBlendKey> for GradientBlend {
    fn from(key: GradientBlendKey) -> Self {
        match key {
            GradientBlendKey::Normal => Self::Normal,
            GradientBlendKey::Multiply => Self::Multiply,
            GradientBlendKey::Screen => Self::Screen,
            GradientBlendKey::Overlay => Self::Overlay,
            GradientBlendKey::SoftLight => Self::SoftLight,
            GradientBlendKey::Add => Self::Add,
        }
    }
}

impl From<GradientKey> for GradientLayer {
    fn from(key: GradientKey) -> Self {
        let shape = match key.shape {
            GradientShapeKey::Linear { edge } => GradientShape::Linear { edge: edge.to_radians() },
            GradientShapeKey::Radial { center } => GradientShape::Radial { center: center.into() },
        };
        Self {
            shape,
            blend: key.blend.into(),
            color: key.color.into(),
            opacity: key.opacity,
            start: key.start,
            end: key.end,
        }
    }
}

/// The lens flare at a key ([`LensFlare`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct LensFlareKey {
    /// Light added before the tone curve: 0 turns it off.
    pub intensity: f32,
    /// Linear RGB tinting every element.
    #[serde(default = "white")]
    pub color: Rgb,
}

impl From<LensFlareKey> for LensFlare {
    fn from(key: LensFlareKey) -> Self {
        Self { intensity: key.intensity, color: key.color.into() }
    }
}

/// One group of a key: the unit a zone or cutscene overrides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TodGroup {
    Sun,
    Moon,
    Sky,
    Ambient,
    Fog,
    ShadowTint,
    Character,
    DayNight,
    Exposure,
    Grade,
    Bloom,
    Vignette,
    Discs,
    Stars,
    Clouds,
    Diffusion,
    CharacterBloom,
    Gradients,
    LensFlare,
    Emission,
    Nebula,
}

impl TodGroup {
    pub const ALL: [Self; 21] = [
        Self::Sun,
        Self::Moon,
        Self::Sky,
        Self::Ambient,
        Self::Fog,
        Self::ShadowTint,
        Self::Character,
        Self::DayNight,
        Self::Exposure,
        Self::Grade,
        Self::Bloom,
        Self::Vignette,
        Self::Discs,
        Self::Stars,
        Self::Clouds,
        Self::Diffusion,
        Self::CharacterBloom,
        Self::Gradients,
        Self::LensFlare,
        Self::Emission,
        Self::Nebula,
    ];

    /// The sky's layers over its gradient (M1 W2, the nebula M1 X1 part 2e): optional even in
    /// the base timeline, where a group left out draws nothing
    /// ([`sapphire_render::SkyLayers::default`]), so timelines written before them draw the sky
    /// as they did.
    pub const SKY_LAYERS: [Self; 4] = [Self::Discs, Self::Stars, Self::Clouds, Self::Nebula];

    /// Post's compositing layers (M1 P2): optional even in the base timeline, where a group left
    /// out draws nothing (characters bloom as the world does), so timelines written before them
    /// draw post as they did ([`sapphire_render::PostParams::plain`]).
    pub const POST_LAYERS: [Self; 4] =
        [Self::Diffusion, Self::CharacterBloom, Self::Gradients, Self::LensFlare];

    /// Every group the base timeline may leave out: the sky's layers ([`Self::SKY_LAYERS`]),
    /// post's ([`Self::POST_LAYERS`]) and the emission scale (M1 W1's keyed emission), which a
    /// timeline that leaves it out keeps at 1, as authored. Each draws where unset as every
    /// timeline written before it drew.
    pub const OPTIONAL: [Self; 9] = [
        Self::Discs,
        Self::Stars,
        Self::Clouds,
        Self::Nebula,
        Self::Diffusion,
        Self::CharacterBloom,
        Self::Gradients,
        Self::LensFlare,
        Self::Emission,
    ];

    /// The group's field name in a key.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sun => "sun",
            Self::Moon => "moon",
            Self::Sky => "sky",
            Self::Ambient => "ambient",
            Self::Fog => "fog",
            Self::ShadowTint => "shadow_tint",
            Self::Character => "character",
            Self::DayNight => "day_night",
            Self::Exposure => "exposure",
            Self::Grade => "grade",
            Self::Bloom => "bloom",
            Self::Vignette => "vignette",
            Self::Discs => "discs",
            Self::Stars => "stars",
            Self::Clouds => "clouds",
            Self::Diffusion => "diffusion",
            Self::CharacterBloom => "character_bloom",
            Self::Gradients => "gradients",
            Self::LensFlare => "lens_flare",
            Self::Emission => "emission",
            Self::Nebula => "nebula",
        }
    }

    fn bit(self) -> u32 {
        1 << self as u32
    }
}

impl std::fmt::Display for TodGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// A set of [`TodGroup`]s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TodGroups(u32);

impl TodGroups {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self((1 << TodGroup::ALL.len()) - 1);
    /// What the base timeline must set: every group but the optional ones
    /// ([`TodGroup::OPTIONAL`]: the sky's layers, post's and the emission scale).
    pub const REQUIRED: Self = {
        let mut bits = Self::ALL.0;
        let mut index = 0;
        while index < TodGroup::OPTIONAL.len() {
            bits &= !(1 << TodGroup::OPTIONAL[index] as u32);
            index += 1;
        }
        Self(bits)
    };

    pub fn contains(self, group: TodGroup) -> bool {
        self.0 & group.bit() != 0
    }

    pub fn insert(&mut self, group: TodGroup) {
        self.0 |= group.bit();
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The groups in the set, in [`TodGroup::ALL`]'s order.
    pub fn iter(self) -> impl Iterator<Item = TodGroup> {
        TodGroup::ALL.into_iter().filter(move |group| self.contains(*group))
    }

    /// The first group in `self` and not in `other`.
    pub fn first_missing_from(self, other: Self) -> Option<TodGroup> {
        self.iter().find(|group| !other.contains(*group))
    }
}

impl std::fmt::Display for TodGroups {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() {
            return f.write_str("no groups");
        }
        for (index, group) in self.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            f.write_str(group.name())?;
        }
        Ok(())
    }
}

impl TodKey {
    /// The groups this key sets.
    pub fn groups(&self) -> TodGroups {
        let mut groups = TodGroups::NONE;
        let set = [
            (TodGroup::Sun, self.sun.is_some()),
            (TodGroup::Moon, self.moon.is_some()),
            (TodGroup::Sky, self.sky.is_some()),
            (TodGroup::Ambient, self.ambient.is_some()),
            (TodGroup::Fog, self.fog.is_some()),
            (TodGroup::ShadowTint, self.shadow_tint.is_some()),
            (TodGroup::Character, self.character.is_some()),
            (TodGroup::DayNight, self.day_night.is_some()),
            (TodGroup::Exposure, self.exposure.is_some()),
            (TodGroup::Grade, self.grade.is_some()),
            (TodGroup::Bloom, self.bloom.is_some()),
            (TodGroup::Vignette, self.vignette.is_some()),
            (TodGroup::Discs, self.discs.is_some()),
            (TodGroup::Stars, self.stars.is_some()),
            (TodGroup::Clouds, self.clouds.is_some()),
            (TodGroup::Diffusion, self.diffusion.is_some()),
            (TodGroup::CharacterBloom, self.character_bloom.is_some()),
            (TodGroup::Gradients, self.gradients.is_some()),
            (TodGroup::LensFlare, self.lens_flare.is_some()),
            (TodGroup::Emission, self.emission.is_some()),
            (TodGroup::Nebula, self.nebula.is_some()),
        ];
        for (group, present) in set {
            if present {
                groups.insert(group);
            }
        }
        groups
    }

    /// Checks every value the key sets is in its range. Returns the field and what is wrong.
    pub(crate) fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        if let Some(sun) = &self.sun {
            celestial(sun, ("sun.direction", "sun.color", "sun.intensity"))?;
        }
        if let Some(moon) = &self.moon {
            celestial(moon, ("moon.direction", "moon.color", "moon.intensity"))?;
        }
        if let Some(sky) = &self.sky {
            color(sky.zenith, "sky.zenith")?;
            color(sky.horizon, "sky.horizon")?;
            color(sky.ground, "sky.ground")?;
        }
        if let Some(ambient) = &self.ambient {
            color(ambient.sky, "ambient.sky")?;
            color(ambient.ground, "ambient.ground")?;
            non_negative(ambient.intensity, "ambient.intensity")?;
        }
        if let Some(fog) = &self.fog {
            for (layer, names) in [
                (&fog.near, ("fog.near.color", "fog.near.density", "fog.near.start")),
                (&fog.far, ("fog.far.color", "fog.far.density", "fog.far.start")),
            ] {
                color(layer.color, names.0)?;
                non_negative(layer.density, names.1)?;
                non_negative(layer.start, names.2)?;
            }
            color(fog.height.color, "fog.height.color")?;
            non_negative(fog.height.density, "fog.height.density")?;
            non_negative(fog.height.falloff, "fog.height.falloff")?;
            finite(fog.height.base, "fog.height.base")?;
            color(fog.sun_inscatter, "fog.sun_inscatter")?;
            within(fog.characters, 0.0, 1.0, "fog.characters", "must be within 0..=1")?;
        }
        if let Some(tint) = self.shadow_tint {
            color(tint, "shadow_tint")?;
        }
        if let Some(character) = &self.character {
            color(character.color, "character.color")?;
            non_negative(character.intensity, "character.intensity")?;
        }
        if let Some(day_night) = self.day_night {
            within(day_night, 0.0, 1.0, "day_night", "must be within 0..=1")?;
        }
        if let Some(exposure) = self.exposure {
            within(exposure, -16.0, 16.0, "exposure", "must be within -16..=16 stops")?;
        }
        if let Some(grade) = &self.grade {
            finite(grade.temperature, "grade.temperature")?;
            finite(grade.tint, "grade.tint")?;
            color(grade.filter, "grade.filter")?;
            positive(grade.contrast, "grade.contrast")?;
            non_negative(grade.saturation, "grade.saturation")?;
            if !grade.lift.iter().all(|v| v.is_finite()) {
                return Err(("grade.lift", "must be finite"));
            }
            if !grade.gamma.iter().all(|&v| v.is_finite() && v > 0.0) {
                return Err(("grade.gamma", "must be positive and finite"));
            }
            color(grade.gain, "grade.gain")?;
        }
        if let Some(bloom) = &self.bloom {
            non_negative(bloom.threshold, "bloom.threshold")?;
            within(bloom.knee, 0.0, 1.0, "bloom.knee", "must be within 0..=1")?;
            non_negative(bloom.intensity, "bloom.intensity")?;
            within(bloom.scatter, 0.0, 1.0, "bloom.scatter", "must be within 0..=1")?;
        }
        if let Some(vignette) = &self.vignette {
            within(vignette.intensity, 0.0, 1.0, "vignette.intensity", "must be within 0..=1")?;
            within(vignette.start, 0.0, 0.99, "vignette.start", "must be within 0..=0.99")?;
            color(vignette.color, "vignette.color")?;
        }
        if let Some(discs) = &self.discs {
            disc(
                &discs.sun,
                ("discs.sun.color", "discs.sun.intensity", "discs.sun.glow", "discs.sun.glow_size"),
            )?;
            disc(
                &discs.moon,
                ("discs.moon.color", "discs.moon.intensity", "discs.moon.glow", "discs.moon.glow_size"),
            )?;
        }
        if let Some(stars) = self.stars {
            non_negative(stars, "stars")?;
        }
        if let Some(nebula) = &self.nebula {
            color(nebula.clouds, "nebula.clouds")?;
            color(nebula.wisps, "nebula.wisps")?;
            color(nebula.accents, "nebula.accents")?;
        }
        if let Some(clouds) = &self.clouds {
            color(clouds.lit, "clouds.lit")?;
            color(clouds.shade, "clouds.shade")?;
            color(clouds.rim, "clouds.rim")?;
            within(clouds.coverage, 0.0, 1.0, "clouds.coverage", "must be within 0..=1")?;
            within(clouds.cirrus, 0.0, 1.0, "clouds.cirrus", "must be within 0..=1")?;
        }
        if let Some(diffusion) = &self.diffusion {
            non_negative(diffusion.intensity, "diffusion.intensity")?;
            non_negative(diffusion.threshold, "diffusion.threshold")?;
            color(diffusion.color, "diffusion.color")?;
        }
        if let Some(character) = &self.character_bloom {
            non_negative(character.threshold, "character_bloom.threshold")?;
            non_negative(character.strength, "character_bloom.strength")?;
        }
        if let Some(gradients) = &self.gradients {
            if gradients.len() > MAX_GRADIENT_LAYERS {
                return Err(("gradients", "can have at most 4 layers"));
            }
            for layer in gradients {
                gradient(layer)?;
            }
        }
        if let Some(flare) = &self.lens_flare {
            non_negative(flare.intensity, "lens_flare.intensity")?;
            color(flare.color, "lens_flare.color")?;
        }
        if let Some(emission) = self.emission {
            non_negative(emission, "emission")?;
        }
        Ok(())
    }
}

type Check = Result<(), (&'static str, &'static str)>;

fn celestial(
    key: &CelestialKey,
    (direction, light, intensity): (&'static str, &'static str, &'static str),
) -> Check {
    let length_squared: f32 = key.direction.iter().map(|v| v * v).sum();
    if !(length_squared.is_finite() && length_squared > 1e-12) {
        return Err((direction, "must be a finite, non-zero vector"));
    }
    color(key.color, light)?;
    non_negative(key.intensity, intensity)
}

fn disc(
    key: &DiscKey,
    (light, intensity, glow, glow_size): (&'static str, &'static str, &'static str, &'static str),
) -> Check {
    color(key.color, light)?;
    non_negative(key.intensity, intensity)?;
    color(key.glow, glow)?;
    within(key.glow_size, 0.0, 180.0, glow_size, "must be within 0..=180 degrees")
}

fn gradient(layer: &GradientKey) -> Check {
    match layer.shape {
        GradientShapeKey::Linear { edge } => finite(edge, "gradients.shape.edge")?,
        GradientShapeKey::Radial { center } => {
            if !center.iter().all(|v| v.is_finite()) {
                return Err(("gradients.shape.center", "must be finite"));
            }
        }
    }
    color(layer.color, "gradients.color")?;
    within(layer.opacity, 0.0, 1.0, "gradients.opacity", "must be within 0..=1")?;
    finite(layer.start, "gradients.start")?;
    finite(layer.end, "gradients.end")
}

fn color(rgb: Rgb, field: &'static str) -> Check {
    if rgb.iter().all(|&v| v.is_finite() && v >= 0.0) {
        Ok(())
    } else {
        Err((field, "must be finite and not negative"))
    }
}

fn finite(value: f32, field: &'static str) -> Check {
    if value.is_finite() { Ok(()) } else { Err((field, "must be finite")) }
}

fn non_negative(value: f32, field: &'static str) -> Check {
    if value.is_finite() && value >= 0.0 { Ok(()) } else { Err((field, "must be finite and not negative")) }
}

fn positive(value: f32, field: &'static str) -> Check {
    if value.is_finite() && value > 0.0 { Ok(()) } else { Err((field, "must be positive and finite")) }
}

fn within(value: f32, min: f32, max: f32, field: &'static str, problem: &'static str) -> Check {
    if (min..=max).contains(&value) { Ok(()) } else { Err((field, problem)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_name_what_a_key_sets() {
        let key = TodKey { exposure: Some(0.5), fog: Some(FogKey::default()), ..TodKey::default() };
        let groups = key.groups();
        assert!(groups.contains(TodGroup::Fog) && groups.contains(TodGroup::Exposure));
        assert_eq!(groups.iter().count(), 2);
        assert_eq!(groups.to_string(), "fog, exposure");
        assert_eq!(TodKey::default().groups(), TodGroups::NONE);
        assert_eq!(TodGroups::ALL.iter().count(), TodGroup::ALL.len());
        let keyed = TodKey { emission: Some(0.2), ..TodKey::default() };
        assert_eq!(keyed.groups().iter().collect::<Vec<_>>(), [TodGroup::Emission]);
        assert_eq!(keyed.groups().to_string(), "emission");
        assert!(!TodGroups::REQUIRED.contains(TodGroup::Emission), "the base timeline may leave it out");
        let optional = TodGroup::OPTIONAL.len();
        assert_eq!(TodGroups::REQUIRED.iter().count(), TodGroup::ALL.len() - optional);
        assert_eq!(TodGroups::ALL.first_missing_from(groups), Some(TodGroup::Sun));
        assert_eq!(groups.first_missing_from(TodGroups::ALL), None);
    }

    #[test]
    fn grade_bloom_and_vignette_keys_default_to_the_default_look() {
        assert_eq!(ColorGrade::from(GradeKey::default()), ColorGrade::NEUTRAL);
        assert_eq!(BloomParams::from(BloomKey::default()), BloomParams::default());
        assert_eq!(Vignette::from(VignetteKey::default()), Vignette::default());
        let grade = ColorGrade { saturation: 1.15, contrast: 1.05, ..ColorGrade::NEUTRAL };
        assert_eq!(ColorGrade::from(GradeKey::from(grade)), grade, "round trip");
    }

    #[test]
    fn out_of_range_values_name_their_field() {
        let check = |key: TodKey| key.validate().err().map(|(field, _)| field);
        let sun = CelestialKey { direction: [0.0, -1.0, 0.0], color: [1.0; 3], intensity: 1.0 };
        assert_eq!(check(TodKey { sun: Some(sun), ..TodKey::default() }), None);
        let flat = CelestialKey { direction: [0.0; 3], ..sun };
        assert_eq!(check(TodKey { sun: Some(flat), ..TodKey::default() }), Some("sun.direction"));
        let dark = CelestialKey { intensity: -1.0, ..sun };
        assert_eq!(check(TodKey { moon: Some(dark), ..TodKey::default() }), Some("moon.intensity"));
        assert_eq!(check(TodKey { day_night: Some(1.5), ..TodKey::default() }), Some("day_night"));
        assert_eq!(check(TodKey { exposure: Some(f32::NAN), ..TodKey::default() }), Some("exposure"));
        assert_eq!(
            check(TodKey { shadow_tint: Some([0.1, -0.1, 0.1]), ..TodKey::default() }),
            Some("shadow_tint")
        );
        let grade = GradeKey { gamma: [1.0, 0.0, 1.0], ..GradeKey::default() };
        assert_eq!(check(TodKey { grade: Some(grade), ..TodKey::default() }), Some("grade.gamma"));
        let fog =
            FogKey { far: FogLayerKey { density: -0.1, ..FogLayerKey::default() }, ..FogKey::default() };
        assert_eq!(check(TodKey { fog: Some(fog), ..TodKey::default() }), Some("fog.far.density"));
        let bloom = BloomKey { knee: 2.0, ..BloomKey::default() };
        assert_eq!(check(TodKey { bloom: Some(bloom), ..TodKey::default() }), Some("bloom.knee"));
        assert_eq!(check(TodKey { emission: Some(0.0), ..TodKey::default() }), None, "none shows");
        assert_eq!(check(TodKey { emission: Some(2.5), ..TodKey::default() }), None, "a night boost");
        for wrong in [-0.1, f32::NAN, f32::INFINITY] {
            assert_eq!(check(TodKey { emission: Some(wrong), ..TodKey::default() }), Some("emission"));
        }
    }
}
