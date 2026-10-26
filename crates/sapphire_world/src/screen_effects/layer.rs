//! [`EffectLayer`]: one effect of a preset, its envelope, delay and clock, as authored.

use sapphire_math::color::mix_oklab;
use sapphire_math::{Vec2, Vec3};
use sapphire_render::{
    ChromaticAberration, Duotone, EffectCoverage, Inversion, PostEffects, RadialBlur, SpeedLines,
};
use serde::{Deserialize, Serialize};

use super::envelope::{Envelope, MAX_ENVELOPE_SECONDS};
use crate::tod::Rgb;

/// One effect and its parameters at full weight: the renderer's effects
/// ([`PostEffects`]), as authored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScreenEffect {
    RadialBlur(RadialBlurEffect),
    ChromaticAberration(ChromaticAberrationEffect),
    Inversion(InversionEffect),
    Duotone(DuotoneEffect),
    SpeedLines(SpeedLinesEffect),
}

/// What [`ScreenEffect`] is, for stats and the overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EffectKind {
    RadialBlur,
    ChromaticAberration,
    Inversion,
    Duotone,
    SpeedLines,
}

impl EffectKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::RadialBlur => "radial blur",
            Self::ChromaticAberration => "chromatic aberration",
            Self::Inversion => "inversion",
            Self::Duotone => "duotone",
            Self::SpeedLines => "speed lines",
        }
    }
}

impl ScreenEffect {
    pub fn kind(&self) -> EffectKind {
        match self {
            Self::RadialBlur(_) => EffectKind::RadialBlur,
            Self::ChromaticAberration(_) => EffectKind::ChromaticAberration,
            Self::Inversion(_) => EffectKind::Inversion,
            Self::Duotone(_) => EffectKind::Duotone,
            Self::SpeedLines(_) => EffectKind::SpeedLines,
        }
    }

    /// The effect centered on `center` (texture coordinates), if it has a center: the radial
    /// blur, chromatic aberration and speed lines.
    pub fn centered(mut self, center: Vec2) -> Self {
        let center = center.to_array();
        match &mut self {
            Self::RadialBlur(blur) => blur.center = center,
            Self::ChromaticAberration(aberration) => aberration.center = center,
            Self::SpeedLines(lines) => lines.center = center,
            Self::Inversion(_) | Self::Duotone(_) => {}
        }
        self
    }

    fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        let unit = |value: f32| (0.0..=1.0).contains(&value);
        let center = |[x, y]: [f32; 2]| (-1.0..=2.0).contains(&x) && (-1.0..=2.0).contains(&y);
        let color = |rgb: Rgb| rgb.iter().all(|c| c.is_finite() && *c >= 0.0);
        match self {
            Self::RadialBlur(blur) => {
                if !center(blur.center) {
                    return Err(("radial_blur.center", "must be within -1..=2 on both axes"));
                }
                if !unit(blur.strength) {
                    return Err(("radial_blur.strength", "must be within 0..=1"));
                }
            }
            Self::ChromaticAberration(aberration) => {
                if !center(aberration.center) {
                    return Err(("chromatic_aberration.center", "must be within -1..=2 on both axes"));
                }
                if !(0.0..=0.25).contains(&aberration.strength) {
                    return Err(("chromatic_aberration.strength", "must be within 0..=0.25"));
                }
            }
            Self::Inversion(inversion) => {
                if !unit(inversion.amount) {
                    return Err(("inversion.amount", "must be within 0..=1"));
                }
            }
            Self::Duotone(duotone) => {
                if !unit(duotone.amount) {
                    return Err(("duotone.amount", "must be within 0..=1"));
                }
                if !(color(duotone.dark) && color(duotone.light)) {
                    return Err(("duotone", "colors must be finite and not negative"));
                }
                if !unit(duotone.threshold) {
                    return Err(("duotone.threshold", "must be within 0..=1"));
                }
                if !(0.001..=1.0).contains(&duotone.softness) {
                    return Err(("duotone.softness", "must be within 0.001..=1"));
                }
            }
            Self::SpeedLines(lines) => {
                if !unit(lines.opacity) {
                    return Err(("speed_lines.opacity", "must be within 0..=1"));
                }
                if !center(lines.center) {
                    return Err(("speed_lines.center", "must be within -1..=2 on both axes"));
                }
                if !(1..=1024).contains(&lines.count) {
                    return Err(("speed_lines.count", "must be within 1..=1024"));
                }
                if !unit(lines.width) {
                    return Err(("speed_lines.width", "must be within 0..=1"));
                }
                if !((0.0..=4.0).contains(&lines.inner_radius) && (0.0..=4.0).contains(&lines.spread)) {
                    return Err(("speed_lines", "`inner_radius` and `spread` must be within 0..=4"));
                }
                if !color(lines.color) {
                    return Err(("speed_lines.color", "must be finite and not negative"));
                }
                if !(0.0..=120.0).contains(&lines.rate) {
                    return Err(("speed_lines.rate", "must be within 0..=120 redraws a second"));
                }
            }
        }
        Ok(())
    }

    /// Blends this effect over the same kind of effect in `effects` (what the layers under it
    /// resolved to) by `weight`, its speed lines drawn by `seed`: its strength, amount or opacity
    /// mixes linearly from what was there; its other values mix by the two layers' shares of the
    /// result (weight times amount), so an effect fading in over none keeps its own center and
    /// colors throughout, and one at full weight replaces what was under it. Speed lines' count
    /// and seed are the larger share's.
    pub(crate) fn blend_over(&self, effects: &mut PostEffects, weight: f32, seed: u32) {
        let lerp2 = |a: Vec2, b: [f32; 2], t: f32| a.lerp(Vec2::from(b), t);
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let coverage = |a: EffectCoverage, b: EffectMask, t: f32| {
            let b = b.coverage();
            EffectCoverage {
                world: lerp(a.world, b.world, t),
                characters: lerp(a.characters, b.characters, t),
            }
        };
        match *self {
            Self::RadialBlur(blur) => {
                let base = effects.radial_blur;
                let (strength, t) = shares(base.strength, blur.strength, weight);
                effects.radial_blur = RadialBlur { center: lerp2(base.center, blur.center, t), strength };
            }
            Self::ChromaticAberration(aberration) => {
                let base = effects.chromatic_aberration;
                let (strength, t) = shares(base.strength, aberration.strength, weight);
                effects.chromatic_aberration =
                    ChromaticAberration { center: lerp2(base.center, aberration.center, t), strength };
            }
            Self::Inversion(inversion) => {
                let base = effects.inversion;
                let (amount, t) = shares(base.amount, inversion.amount, weight);
                effects.inversion =
                    Inversion { amount, coverage: coverage(base.coverage, inversion.mask, t) };
            }
            Self::Duotone(duotone) => {
                let base = effects.duotone;
                let (amount, t) = shares(base.amount, duotone.amount, weight);
                effects.duotone = Duotone {
                    amount,
                    dark: mix_oklab(base.dark, Vec3::from(duotone.dark), t),
                    light: mix_oklab(base.light, Vec3::from(duotone.light), t),
                    threshold: lerp(base.threshold, duotone.threshold, t),
                    softness: lerp(base.softness, duotone.softness, t),
                    coverage: coverage(base.coverage, duotone.mask, t),
                };
            }
            Self::SpeedLines(lines) => {
                let base = effects.speed_lines;
                let (opacity, t) = shares(base.opacity, lines.opacity, weight);
                let (count, seed) = if t >= 0.5 { (lines.count, seed) } else { (base.count, base.seed) };
                effects.speed_lines = SpeedLines {
                    opacity,
                    center: lerp2(base.center, lines.center, t),
                    count,
                    width: lerp(base.width, lines.width, t),
                    inner_radius: lerp(base.inner_radius, lines.inner_radius, t),
                    spread: lerp(base.spread, lines.spread, t),
                    color: mix_oklab(base.color, Vec3::from(lines.color), t),
                    seed,
                    coverage: coverage(base.coverage, lines.mask, t),
                };
            }
        }
    }
}

/// A layer of `amount` over a base of `base` by `weight`: the result's amount, and the layer's
/// share of it, by which its other values mix in (1 over nothing).
fn shares(base: f32, amount: f32, weight: f32) -> (f32, f32) {
    let (under, over) = (base * (1.0 - weight), amount * weight);
    let share = if under + over > 0.0 { over / (under + over) } else { 1.0 };
    (under + over, share)
}

/// Where on the frame an effect shows, by the character mask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectMask {
    /// On every pixel.
    #[default]
    Everywhere,
    /// On the world, characters left as they are: an impact frame that keeps characters
    /// colored, speed lines behind characters.
    World,
    /// On characters alone.
    Characters,
}

impl EffectMask {
    fn coverage(self) -> EffectCoverage {
        match self {
            Self::Everywhere => EffectCoverage::EVERYWHERE,
            Self::World => EffectCoverage::WORLD,
            Self::Characters => EffectCoverage::CHARACTERS,
        }
    }
}

/// A radial blur ([`RadialBlur`]): positions are texture coordinates, (0, 0) the top left.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RadialBlurEffect {
    pub center: [f32; 2],
    /// The share of each pixel's distance from the center its streak reaches, 0 to 1.
    pub strength: f32,
}

impl Default for RadialBlurEffect {
    fn default() -> Self {
        Self { center: [0.5, 0.5], strength: 0.1 }
    }
}

/// Chromatic aberration ([`ChromaticAberration`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChromaticAberrationEffect {
    pub center: [f32; 2],
    /// The split half a frame's diagonal out, 0 to 0.25.
    pub strength: f32,
}

impl Default for ChromaticAberrationEffect {
    fn default() -> Self {
        Self { center: [0.5, 0.5], strength: 0.04 }
    }
}

/// The frame inverted ([`Inversion`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InversionEffect {
    pub amount: f32,
    pub mask: EffectMask,
}

impl Default for InversionEffect {
    fn default() -> Self {
        Self { amount: 1.0, mask: EffectMask::Everywhere }
    }
}

/// A duotone ([`Duotone`]): colors are linear RGB.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DuotoneEffect {
    pub amount: f32,
    pub dark: Rgb,
    pub light: Rgb,
    /// The display luminance where the frame turns from dark to light, 0 to 1.
    pub threshold: f32,
    /// The turn's width, 0.001 (a hard cut) to 1.
    pub softness: f32,
    pub mask: EffectMask,
}

impl Default for DuotoneEffect {
    /// A black-and-white impact frame on everything.
    fn default() -> Self {
        Self {
            amount: 1.0,
            dark: [0.0; 3],
            light: [1.0; 3],
            threshold: 0.5,
            softness: 0.1,
            mask: EffectMask::Everywhere,
        }
    }
}

/// Speed lines ([`SpeedLines`]): sizes are in frame heights, the color linear RGB.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpeedLinesEffect {
    pub opacity: f32,
    pub center: [f32; 2],
    /// Lines around the center, 1 to 1024.
    pub count: u32,
    /// A line's width at its outer end, as a share of its slot.
    pub width: f32,
    pub inner_radius: f32,
    pub spread: f32,
    pub color: Rgb,
    /// How often the lines are redrawn, per second of the effect's time, as anime redraws them
    /// every few frames; 0 keeps one drawing.
    pub rate: f32,
    pub mask: EffectMask,
}

impl Default for SpeedLinesEffect {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            center: [0.5, 0.5],
            count: 96,
            width: 0.3,
            inner_radius: 0.35,
            spread: 0.3,
            color: [1.0; 3],
            rate: 12.0,
            mask: EffectMask::Everywhere,
        }
    }
}

/// Which clock an effect plays on (the screen effects module's docs, "Clocks").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectClock {
    /// The world's time: pause holds it and slow motion stretches it.
    #[default]
    World,
    /// Fixed steps at full speed while the world runs: slow motion doesn't stretch it, a stopped
    /// world holds it (as it holds hit-stop's countdown). For impact frames authored in frames.
    Real,
}

/// One effect of a preset: what it draws at full weight, its envelope, how long after the push
/// it starts, and its clock. Written with exactly one effect field:
///
/// ```ron
/// (radial_blur: (strength: 0.12), envelope: Pulse(attack: 0.02, hold: 0.08, release: 0.3))
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "LayerDocument", into = "LayerDocument")]
pub struct EffectLayer {
    pub effect: ScreenEffect,
    pub envelope: Envelope,
    /// Seconds after the push before the envelope starts, on the layer's clock.
    pub delay: f32,
    pub clock: EffectClock,
}

impl EffectLayer {
    /// When the layer ends, in seconds after its push (and its delay): infinity for a sustained
    /// one until it is released at `released`.
    pub fn end(&self, released: Option<f32>) -> f32 {
        self.envelope.end(released)
    }

    /// Checks the layer, naming the field that is wrong.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        self.effect.validate()?;
        self.envelope.validate()?;
        if !(0.0..=MAX_ENVELOPE_SECONDS).contains(&self.delay) {
            return Err(("delay", "must be within 0..=60 seconds"));
        }
        Ok(())
    }
}

/// An [`EffectLayer`] as written: one optional field per effect.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LayerDocument {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    radial_blur: Option<RadialBlurEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chromatic_aberration: Option<ChromaticAberrationEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inversion: Option<InversionEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duotone: Option<DuotoneEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    speed_lines: Option<SpeedLinesEffect>,
    envelope: Option<Envelope>,
    #[serde(default, skip_serializing_if = "is_zero")]
    delay: f32,
    #[serde(default, skip_serializing_if = "is_world")]
    clock: EffectClock,
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

fn is_world(clock: &EffectClock) -> bool {
    *clock == EffectClock::World
}

/// Why a written layer isn't one.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum LayerError {
    #[error(
        "a layer sets exactly one effect: radial_blur, chromatic_aberration, inversion, duotone or speed_lines"
    )]
    EffectCount,
    #[error("a layer needs an envelope")]
    NoEnvelope,
}

impl TryFrom<LayerDocument> for EffectLayer {
    type Error = LayerError;

    fn try_from(document: LayerDocument) -> Result<Self, LayerError> {
        let LayerDocument {
            radial_blur,
            chromatic_aberration,
            inversion,
            duotone,
            speed_lines,
            envelope,
            delay,
            clock,
        } = document;
        let effects = [
            radial_blur.map(ScreenEffect::RadialBlur),
            chromatic_aberration.map(ScreenEffect::ChromaticAberration),
            inversion.map(ScreenEffect::Inversion),
            duotone.map(ScreenEffect::Duotone),
            speed_lines.map(ScreenEffect::SpeedLines),
        ];
        let mut set = effects.into_iter().flatten();
        let (Some(effect), None) = (set.next(), set.next()) else { return Err(LayerError::EffectCount) };
        let envelope = envelope.ok_or(LayerError::NoEnvelope)?;
        Ok(Self { effect, envelope, delay, clock })
    }
}

impl From<EffectLayer> for LayerDocument {
    fn from(layer: EffectLayer) -> Self {
        let mut document = Self {
            envelope: Some(layer.envelope),
            delay: layer.delay,
            clock: layer.clock,
            ..Self::default()
        };
        match layer.effect {
            ScreenEffect::RadialBlur(effect) => document.radial_blur = Some(effect),
            ScreenEffect::ChromaticAberration(effect) => document.chromatic_aberration = Some(effect),
            ScreenEffect::Inversion(effect) => document.inversion = Some(effect),
            ScreenEffect::Duotone(effect) => document.duotone = Some(effect),
            ScreenEffect::SpeedLines(effect) => document.speed_lines = Some(effect),
        }
        document
    }
}
