//! Values over an age: a [`Curve`] of keys, linear between them.

use bevy_reflect::Reflect;
use glam::Vec3;
use sapphire_reflect::Transparent;
use serde::{Deserialize, Serialize};

/// The most keys a curve holds: enough to shape a flash, and few enough that sampling one stays a
/// short scan for every particle every frame.
pub const MAX_KEYS: usize = 8;

/// A value over a particle's or a trail point's life, `(t, value)` keys with `t` from 0 (born) to
/// 1 (gone), linear between them and level beyond the first and the last. A document writes it as
/// a list: `size: [(0.0, 0.05), (0.2, 0.3), (1.0, 0.0)]`, and one key is a constant.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(@Transparent)]
#[serde(transparent)]
pub struct Curve<T>(pub Vec<(f32, T)>);

/// What a curve can hold.
pub trait CurveValue: Copy {
    /// `a` to `b` by `t`.
    fn lerp(a: Self, b: Self, t: f32) -> Self;
    /// Whether every component is finite.
    fn is_finite(&self) -> bool;
}

impl CurveValue for f32 {
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        a + (b - a) * t
    }

    fn is_finite(&self) -> bool {
        f32::is_finite(*self)
    }
}

impl CurveValue for [f32; 3] {
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        [f32::lerp(a[0], b[0], t), f32::lerp(a[1], b[1], t), f32::lerp(a[2], b[2], t)]
    }

    fn is_finite(&self) -> bool {
        self.iter().all(|c| c.is_finite())
    }
}

impl<T: CurveValue> Curve<T> {
    /// The value at `t`.
    pub fn sample(&self, t: f32) -> T {
        let keys = &self.0;
        // Validation holds at least one key, and `t` is clamped by the first and last.
        let mut previous = keys[0];
        if t <= previous.0 {
            return previous.1;
        }
        for &key in &keys[1..] {
            if t <= key.0 {
                let span = key.0 - previous.0;
                let share = if span > 0.0 { (t - previous.0) / span } else { 1.0 };
                return T::lerp(previous.1, key.1, share);
            }
            previous = key;
        }
        previous.1
    }

    /// Why this curve isn't valid, if it isn't: at most [`MAX_KEYS`], at least one, finite, with
    /// their times rising from 0 to 1.
    pub(crate) fn problem(&self) -> Option<&'static str> {
        if self.0.is_empty() {
            return Some("needs at least one key");
        }
        if self.0.len() > MAX_KEYS {
            return Some("has too many keys");
        }
        if self.0.iter().any(|(t, value)| !t.is_finite() || !value.is_finite()) {
            return Some("has a value that isn't finite");
        }
        if self.0.iter().any(|(t, _)| !(0.0..=1.0).contains(t)) {
            return Some("has a key's time outside 0 to 1");
        }
        if self.0.windows(2).any(|keys| keys[0].0 > keys[1].0) {
            return Some("has keys out of order");
        }
        None
    }
}

impl Curve<f32> {
    /// Whether every key's value is at least 0.
    pub(crate) fn never_negative(&self) -> bool {
        self.0.iter().all(|(_, value)| *value >= 0.0)
    }

    /// Whether every key's value is at most 1.
    pub(crate) fn at_most_one(&self) -> bool {
        self.0.iter().all(|(_, value)| *value <= 1.0)
    }
}

impl Curve<[f32; 3]> {
    /// The value at `t`, as a vector.
    pub(crate) fn sample_vec3(&self, t: f32) -> Vec3 {
        Vec3::from(self.sample(t))
    }

    /// Whether every key's every component is at least 0.
    pub(crate) fn never_negative(&self) -> bool {
        self.0.iter().all(|(_, value)| value.iter().all(|c| *c >= 0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_between_and_beyond_its_keys() {
        let curve = Curve(vec![(0.25, 1.0), (0.75, 3.0)]);
        assert_eq!(curve.sample(0.0), 1.0);
        assert_eq!(curve.sample(0.25), 1.0);
        assert_eq!(curve.sample(0.5), 2.0);
        assert_eq!(curve.sample(0.75), 3.0);
        assert_eq!(curve.sample(1.0), 3.0);
    }

    #[test]
    fn a_step_takes_the_later_key() {
        let curve = Curve(vec![(0.0, 1.0), (0.5, 1.0), (0.5, 4.0), (1.0, 4.0)]);
        assert_eq!(curve.sample(0.5), 1.0, "at the step itself, the first key it reaches");
        assert_eq!(curve.sample(0.5001), 4.0);
    }

    #[test]
    fn colors_blend_per_channel() {
        let curve = Curve(vec![(0.0, [0.0, 1.0, 2.0]), (1.0, [2.0, 1.0, 0.0])]);
        assert_eq!(curve.sample_vec3(0.5), Vec3::new(1.0, 1.0, 1.0));
    }

    #[test]
    fn problems_are_named() {
        assert!(Curve::<f32>(vec![]).problem().is_some_and(|p| p.contains("at least one")));
        assert!(Curve(vec![(0.0, f32::NAN)]).problem().is_some_and(|p| p.contains("finite")));
        assert!(Curve(vec![(1.5, 1.0)]).problem().is_some_and(|p| p.contains("outside")));
        assert!(Curve(vec![(0.5, 1.0), (0.25, 1.0)]).problem().is_some_and(|p| p.contains("order")));
        assert!(Curve(vec![(0.0, 1.0); MAX_KEYS + 1]).problem().is_some_and(|p| p.contains("too many")));
        assert_eq!(Curve(vec![(0.0, 1.0), (1.0, 0.0)]).problem(), None);
    }
}
