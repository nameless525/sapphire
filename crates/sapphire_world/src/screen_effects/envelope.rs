//! [`Envelope`]: an effect's weight over its own time.

use sapphire_math::ease::smoothstep;
use serde::{Deserialize, Serialize};

/// The most keys an [`Envelope::Keys`] holds: enough for a double flash with a tail, and a
/// fixed size, so a layer copies into the effect stack without allocating.
pub const MAX_ENVELOPE_KEYS: usize = 8;

/// The longest an envelope's timed part may run, in seconds: screen effects accent moments.
pub const MAX_ENVELOPE_SECONDS: f32 = 60.0;

/// How an effect's weight runs over its own time, in seconds from when it starts (after its
/// layer's delay). The weight scales the effect's strength, amount or opacity; at 0 it shows
/// nothing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Envelope {
    /// Rises over `attack`, holds full for `hold`, falls over `release`, then ends. Rises and
    /// falls are smoothstepped.
    Pulse {
        #[serde(default)]
        attack: f32,
        #[serde(default)]
        hold: f32,
        #[serde(default)]
        release: f32,
    },
    /// Rises over `attack` and holds full until released
    /// ([`EffectStack::release`](super::EffectStack::release)), then falls over `release` from
    /// wherever it was, and ends: a perfect dodge's slow-motion look, an ultimate's lines.
    Sustain {
        #[serde(default)]
        attack: f32,
        #[serde(default)]
        release: f32,
    },
    /// Weights at times, linear between keys: before the first key, its weight; the effect ends
    /// at the last key. Two keys at one time cut from the first's weight to the second's, so
    /// impact frames can flash on and off on exact frames.
    Keys(EnvelopeKeys),
}

impl Envelope {
    /// The weight `time` seconds in (negative before the effect starts), 0 to 1, for an effect
    /// released at `released` (a [`Self::Sustain`]'s; the others ignore it).
    pub fn weight(&self, time: f32, released: Option<f32>) -> f32 {
        // NaN is outside too.
        if time.is_nan() || time < 0.0 || time >= self.end(released) {
            return 0.0;
        }
        match *self {
            Self::Pulse { attack, hold, release } => {
                if time < attack {
                    smoothstep(0.0, attack, time)
                } else if time < attack + hold {
                    1.0
                } else {
                    1.0 - smoothstep(attack + hold, attack + hold + release, time)
                }
            }
            Self::Sustain { attack, release } => {
                let rise = |time: f32| if time < attack { smoothstep(0.0, attack, time) } else { 1.0 };
                match released {
                    Some(at) if time >= at => rise(at) * (1.0 - smoothstep(at, at + release, time)),
                    _ => rise(time),
                }
            }
            Self::Keys(keys) => keys.weight(time),
        }
    }

    /// When the effect ends, in its own seconds: infinity for a sustained one not yet released.
    pub fn end(&self, released: Option<f32>) -> f32 {
        match *self {
            Self::Pulse { attack, hold, release } => attack + hold + release,
            Self::Sustain { release, .. } => released.map_or(f32::INFINITY, |at| at.max(0.0) + release),
            Self::Keys(keys) => keys.end(),
        }
    }

    /// Checks the envelope, naming the field that is wrong.
    pub(crate) fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        let seconds = |value: f32| (0.0..=MAX_ENVELOPE_SECONDS).contains(&value);
        match *self {
            Self::Pulse { attack, hold, release } => {
                if !(seconds(attack) && seconds(hold) && seconds(release)) {
                    return Err(("envelope", "a pulse's times must be within 0..=60 seconds"));
                }
                if attack + hold + release <= 0.0 {
                    return Err(("envelope", "a pulse must last some time"));
                }
            }
            Self::Sustain { attack, release } => {
                if !(seconds(attack) && seconds(release)) {
                    return Err(("envelope", "a sustain's times must be within 0..=60 seconds"));
                }
            }
            Self::Keys(_) => {}
        }
        Ok(())
    }
}

/// An [`Envelope::Keys`]'s keys: `(seconds, weight)`, at most [`MAX_ENVELOPE_KEYS`], in a fixed
/// array. Written as a list; checked however it is made: at least one key, times from 0 to
/// [`MAX_ENVELOPE_SECONDS`] that never decrease and end after 0, weights from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<(f32, f32)>", into = "Vec<(f32, f32)>")]
pub struct EnvelopeKeys {
    keys: [(f32, f32); MAX_ENVELOPE_KEYS],
    len: u8,
}

/// Why an [`EnvelopeKeys`] can't be made.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum EnvelopeError {
    #[error("an envelope needs between 1 and {MAX_ENVELOPE_KEYS} keys, not {0}")]
    KeyCount(usize),
    #[error("key {0}: its time must be within 0..=60 seconds and not before the key before it")]
    Time(usize),
    #[error("key {0}: its weight must be within 0..=1")]
    Weight(usize),
    #[error("the last key must come after 0 seconds")]
    Instant,
}

impl EnvelopeKeys {
    /// Keys `(seconds, weight)`, checked.
    pub fn new(keys: &[(f32, f32)]) -> Result<Self, EnvelopeError> {
        if keys.is_empty() || keys.len() > MAX_ENVELOPE_KEYS {
            return Err(EnvelopeError::KeyCount(keys.len()));
        }
        let mut previous = 0.0;
        for (index, &(time, weight)) in keys.iter().enumerate() {
            if !(time >= previous && time <= MAX_ENVELOPE_SECONDS) {
                return Err(EnvelopeError::Time(index));
            }
            if !(0.0..=1.0).contains(&weight) {
                return Err(EnvelopeError::Weight(index));
            }
            previous = time;
        }
        if previous <= 0.0 {
            return Err(EnvelopeError::Instant);
        }
        let mut array = [(0.0, 0.0); MAX_ENVELOPE_KEYS];
        array[..keys.len()].copy_from_slice(keys);
        Ok(Self { keys: array, len: keys.len() as u8 })
    }

    pub fn keys(&self) -> &[(f32, f32)] {
        &self.keys[..usize::from(self.len)]
    }

    fn end(&self) -> f32 {
        self.keys()[self.keys().len() - 1].0
    }

    /// The weight at `time`, which lies before the end.
    fn weight(&self, time: f32) -> f32 {
        let keys = self.keys();
        let after = keys.partition_point(|&(at, _)| at <= time);
        match after {
            0 => keys[0].1,
            // `time` is before the last key's, so a key follows, strictly later.
            _ => {
                let ((from, a), (to, b)) = (keys[after - 1], keys[after]);
                a + (b - a) * (time - from) / (to - from)
            }
        }
    }
}

impl TryFrom<Vec<(f32, f32)>> for EnvelopeKeys {
    type Error = EnvelopeError;

    fn try_from(keys: Vec<(f32, f32)>) -> Result<Self, EnvelopeError> {
        Self::new(&keys)
    }
}

impl From<EnvelopeKeys> for Vec<(f32, f32)> {
    fn from(keys: EnvelopeKeys) -> Self {
        keys.keys().to_vec()
    }
}
