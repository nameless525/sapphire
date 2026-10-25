//! Authored keys: the look at one time, group by group, as documents spell it.

use bevy_reflect::Reflect;
use sapphire_math::Ease;
use sapphire_render::{BloomParams, ColorGrade, Vignette};
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

/// Hemispherical ambient light at a key: `color × intensity` from above and below.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(deny_unknown_fields)]
pub struct AmbientKey {
    pub sky: Rgb,
    pub ground: Rgb,
    pub intensity: f32,
}

/// Fog at a key ([`sapphire_render::Fog`] says how W3 reads each part). A layer left out has
/// no fog.
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
    /// Light scattered toward the camera around the sun, linear RGB (color × strength).
    #[serde(default)]
    pub sun_inscatter: Rgb,
}

/// One distance layer of fog.
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
/// apart from the world's.
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
}

impl Default for VignetteKey {
    fn default() -> Self {
        let Vignette { intensity, start } = Vignette::default();
        Self { intensity, start }
    }
}

impl From<VignetteKey> for Vignette {
    fn from(key: VignetteKey) -> Self {
        Self { intensity: key.intensity, start: key.start }
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
}

impl TodGroup {
    pub const ALL: [Self; 12] = [
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
        }
    }

    fn bit(self) -> u16 {
        1 << self as u16
    }
}

impl std::fmt::Display for TodGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// A set of [`TodGroup`]s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TodGroups(u16);

impl TodGroups {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self((1 << TodGroup::ALL.len()) - 1);

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
    }
}
