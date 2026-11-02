//! Shakes: a small additive layer on the finished pose, authored as data and fired as cues.
//!
//! **E3's interface.** A cue table resolves a preset's name once, at load and after a reload, with
//! [`CameraTuning::shake_id`](crate::CameraTuning::shake_id), then fires it in the frame after the
//! cue's tick with [`CameraDirector::fire_shake`](crate::CameraDirector::fire_shake)`(id, strength)`.
//! `strength` is that cue's own scale (a hit's weight, say), and
//! [`CameraDirector::set_shake_setting`](crate::CameraDirector::set_shake_setting) is the player's, 0
//! to 1. Firing allocates nothing, can't fail, and never moves the simulation: nothing reads a shake
//! back.
//!
//! **Deterministic given a seed.** The director's seed gives one sequence of noise phases, drawn as
//! shakes fire, and the noise is a function of a shake's age alone, through `sapphire_math::det`'s
//! pinned functions: the same seed and the same fires at the same times give the same offsets, at any
//! frame rate and on every OS. A setting of 0 still draws the phases, so turning shakes off moves
//! nothing else.
//!
//! **On the camera's clock:** a shake plays through hitstop and slow motion, and holds in a pause.

use sapphire_math::rng::Pcg32;
use sapphire_math::{Vec3, det};
use serde::{Deserialize, Serialize};

use crate::tuning::ShakeTuning;

/// The most shakes playing at once. A shake fired past this replaces the one nearest its end.
pub const MAX_SHAKES: usize = 8;

/// The ratio of a channel's two frequencies, so its noise doesn't repeat within a shake's life.
const SECOND_VOICE: f32 = 1.618;
/// The two voices' shares, adding to 1, so a channel's peak is the authored amplitude.
const VOICES: [f32; 2] = [0.65, 0.35];
/// Six channels of two voices each.
const PHASES: usize = 12;

/// A preset by its place in a tuning's list ([`CameraTuning::shake_id`](crate::CameraTuning::shake_id)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShakeId(pub u16);

/// One shake, as authored.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shake {
    /// Seconds from firing to nothing.
    pub duration: f32,
    /// Seconds to reach its peak, so firing doesn't jump.
    pub attack: f32,
    /// The fall from peak to nothing: 1 is a straight line, more falls away sooner.
    pub falloff: f32,
    /// Its main frequency, in hertz.
    pub frequency: f32,
    /// Peak radians about the view's local x, y and z: pitch, yaw and roll.
    pub rotation: [f32; 3],
    /// Peak metres along the view's right, up and forward.
    pub translation: [f32; 3],
}

impl Shake {
    /// The weight at `age` seconds: up over the attack, down to exactly 0 at the end.
    fn envelope(&self, age: f32) -> f32 {
        if age >= self.duration {
            return 0.0;
        }
        if age < self.attack {
            let rise = age / self.attack;
            return rise * rise * (3.0 - 2.0 * rise);
        }
        let left = 1.0 - (age - self.attack) / (self.duration - self.attack).max(1e-3);
        det::pow(left.clamp(0.0, 1.0), self.falloff)
    }
}

/// A preset and the name a tuning knows it by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedShake {
    pub name: String,
    pub shake: Shake,
}

/// A shake playing. It holds its own copy, so a reload mid-shake doesn't change it.
#[derive(Clone, Copy, Debug)]
struct Layer {
    shake: Shake,
    strength: f32,
    age: f32,
    phases: [f32; PHASES],
}

impl Layer {
    const EMPTY: Self = Self {
        shake: Shake {
            duration: 1.0,
            attack: 1.0,
            falloff: 1.0,
            frequency: 1.0,
            rotation: [0.0; 3],
            translation: [0.0; 3],
        },
        strength: 0.0,
        age: 0.0,
        phases: [0.0; PHASES],
    };

    /// Each channel's value at this age: the rotation's three, then the translation's.
    fn add_to(&self, rotation: &mut Vec3, translation: &mut Vec3) {
        let weight = self.shake.envelope(self.age) * self.strength;
        if weight <= 0.0 {
            return;
        }
        let turn = std::f32::consts::TAU * self.shake.frequency * self.age;
        let noise = |channel: usize| {
            VOICES[0] * det::sin(turn + self.phases[channel * 2])
                + VOICES[1] * det::sin(turn * SECOND_VOICE + self.phases[channel * 2 + 1])
        };
        for axis in 0..3 {
            rotation[axis] += noise(axis) * self.shake.rotation[axis] * weight;
            translation[axis] += noise(axis + 3) * self.shake.translation[axis] * weight;
        }
    }
}

/// The shakes playing now. Fixed size, so firing and playing allocate nothing.
#[derive(Clone, Debug)]
pub(crate) struct ShakeStack {
    layers: [Layer; MAX_SHAKES],
    count: usize,
    rng: Pcg32,
    setting: f32,
}

impl ShakeStack {
    /// The stream a director's shakes draw from, so they don't share a sequence with anything else
    /// seeded from one number.
    const STREAM: u64 = 0x63616d657261; // "camera"

    pub(crate) fn new(seed: u64) -> Self {
        Self {
            layers: [Layer::EMPTY; MAX_SHAKES],
            count: 0,
            rng: Pcg32::new(seed, Self::STREAM),
            setting: 1.0,
        }
    }

    pub(crate) fn set_setting(&mut self, setting: f32) {
        self.setting = if setting.is_finite() { setting.clamp(0.0, 1.0) } else { 0.0 };
    }

    #[cfg(test)]
    fn playing(&self) -> usize {
        self.count
    }

    /// Starts a shake, and says if another had to make way for it.
    pub(crate) fn fire(&mut self, shake: Shake, strength: f32) -> bool {
        let mut phases = [0.0; PHASES];
        for phase in &mut phases {
            *phase = self.rng.unit_f32() * std::f32::consts::TAU;
        }
        let strength = if strength.is_finite() { strength.max(0.0) } else { 0.0 };
        let layer = Layer { shake, strength, age: 0.0, phases };
        if self.count < MAX_SHAKES {
            self.layers[self.count] = layer;
            self.count += 1;
            return false;
        }
        let mut soonest = 0;
        let mut left = f32::MAX;
        for (index, playing) in self.layers.iter().enumerate() {
            let its_left = playing.shake.duration - playing.age;
            if its_left < left {
                (soonest, left) = (index, its_left);
            }
        }
        self.layers[soonest] = layer;
        true
    }

    /// Ages every shake, and drops those that ended.
    pub(crate) fn advance(&mut self, dt: f32) {
        let mut kept = 0;
        for index in 0..self.count {
            let mut layer = self.layers[index];
            layer.age += dt;
            if layer.age < layer.shake.duration {
                self.layers[kept] = layer;
                kept += 1;
            }
        }
        self.count = kept;
    }

    /// The sum of everything playing, scaled by the player's setting and held to the tuning's limits:
    /// radians about local x, y and z, then metres along right, up and forward.
    pub(crate) fn sample(&self, tuning: &ShakeTuning) -> (Vec3, Vec3) {
        if self.count == 0 || self.setting <= 0.0 {
            return (Vec3::ZERO, Vec3::ZERO);
        }
        let (mut rotation, mut translation) = (Vec3::ZERO, Vec3::ZERO);
        for layer in &self.layers[..self.count] {
            layer.add_to(&mut rotation, &mut translation);
        }
        let limit =
            |value: Vec3, limit: f32| (value * self.setting).clamp(Vec3::splat(-limit), Vec3::splat(limit));
        (limit(rotation, tuning.rotation_limit), limit(translation, tuning.translation_limit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THUMP: Shake = Shake {
        duration: 0.4,
        attack: 0.02,
        falloff: 2.0,
        frequency: 24.0,
        rotation: [0.01, 0.01, 0.015],
        translation: [0.01, 0.01, 0.0],
    };

    fn tuning() -> ShakeTuning {
        ShakeTuning { rotation_limit: 0.05, translation_limit: 0.05, presets: Vec::new() }
    }

    /// Everything a stack plays over `frames` steps of `dt`, firing `shake` at the start.
    fn run(seed: u64, frames: u32, dt: f32) -> Vec<(Vec3, Vec3)> {
        let mut stack = ShakeStack::new(seed);
        stack.fire(THUMP, 1.0);
        (0..frames)
            .map(|_| {
                stack.advance(dt);
                stack.sample(&tuning())
            })
            .collect()
    }

    #[test]
    fn the_same_seed_gives_the_same_shake_and_another_differs() {
        let first = run(7, 20, 1.0 / 60.0);
        assert_eq!(first, run(7, 20, 1.0 / 60.0));
        assert_ne!(first, run(8, 20, 1.0 / 60.0));
    }

    #[test]
    fn its_path_doesnt_depend_on_the_frame_rate() {
        let at_60 = run(7, 20, 1.0 / 60.0);
        let at_120 = run(7, 40, 1.0 / 120.0);
        for (frame, sample) in at_60.iter().enumerate() {
            let other = at_120[frame * 2 + 1];
            assert!(sample.0.abs_diff_eq(other.0, 1e-4), "{frame}: {sample:?} vs {other:?}");
            assert!(sample.1.abs_diff_eq(other.1, 1e-4), "{frame}: {sample:?} vs {other:?}");
        }
    }

    #[test]
    fn it_rises_from_nothing_and_ends_at_nothing() {
        assert_eq!(THUMP.envelope(0.0), 0.0);
        assert!((THUMP.envelope(THUMP.attack) - 1.0).abs() < 1e-6);
        assert_eq!(THUMP.envelope(THUMP.duration), 0.0);
        let mut stack = ShakeStack::new(1);
        stack.fire(THUMP, 1.0);
        stack.advance(0.2);
        assert_ne!(stack.sample(&tuning()), (Vec3::ZERO, Vec3::ZERO));
        assert_eq!(stack.playing(), 1);
        stack.advance(0.21);
        assert_eq!(stack.sample(&tuning()), (Vec3::ZERO, Vec3::ZERO));
        assert_eq!(stack.playing(), 0);
    }

    #[test]
    fn the_players_setting_scales_it_and_zero_is_none() {
        let sample = |setting: f32| {
            let mut stack = ShakeStack::new(3);
            stack.set_setting(setting);
            stack.fire(THUMP, 1.0);
            stack.advance(0.05);
            stack.sample(&tuning())
        };
        let (full, half) = (sample(1.0), sample(0.5));
        assert!(half.0.abs_diff_eq(full.0 * 0.5, 1e-6));
        assert!(half.1.abs_diff_eq(full.1 * 0.5, 1e-6));
        assert_eq!(sample(0.0), (Vec3::ZERO, Vec3::ZERO));
        assert_eq!(sample(f32::NAN), (Vec3::ZERO, Vec3::ZERO));
    }

    /// Off still draws its phases: a player's setting can't move what later shakes do.
    #[test]
    fn a_setting_of_zero_doesnt_change_the_sequence() {
        let later = |setting: f32| {
            let mut stack = ShakeStack::new(3);
            stack.set_setting(setting);
            stack.fire(THUMP, 1.0);
            stack.advance(1.0);
            stack.set_setting(1.0);
            stack.fire(THUMP, 1.0);
            stack.advance(0.05);
            stack.sample(&tuning())
        };
        assert_eq!(later(0.0), later(1.0));
    }

    #[test]
    fn a_full_stack_makes_way_for_the_newest_and_stays_within_its_limits() {
        let mut stack = ShakeStack::new(5);
        let long = Shake { duration: 2.0, ..THUMP };
        for index in 0..MAX_SHAKES {
            assert!(!stack.fire(if index == 3 { THUMP } else { long }, 40.0));
        }
        assert_eq!(stack.playing(), MAX_SHAKES);
        stack.advance(0.1);
        assert!(stack.fire(long, 40.0), "the eighth place is taken");
        assert_eq!(stack.playing(), MAX_SHAKES);
        assert_eq!(stack.layers[3].shake.duration, 2.0, "the one nearest its end made way");
        let (rotation, translation) = {
            stack.advance(0.02);
            stack.sample(&tuning())
        };
        assert!(rotation.abs().max_element() <= 0.05, "{rotation}");
        assert!(translation.abs().max_element() <= 0.05, "{translation}");
    }
}
