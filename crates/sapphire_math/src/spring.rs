//! Critically damped springs for cameras and UI (spec §5.4).
//!
//! A critically damped spring reaches its target as fast as possible without overshooting
//! (when starting at rest), and it carries velocity, so a target that moves every frame is
//! followed smoothly instead of with the visible kinks of exponential smoothing. The update is
//! the exact closed-form solution, so it is stable for any `dt` and frame-rate independent:
//! one step of 1/30 s lands where two steps of 1/60 s do.
//!
//! These use `std` `exp`, not [`crate::det`]: they drive presentation, not simulation.

use std::ops::{Add, Mul, Sub};

use glam::{Vec2, Vec3, Vec3A, Vec4};

/// Rate constant `u` with `(1 + u)·e^(−u) = ½`: a spring starting at rest closes exactly half
/// the gap to a fixed target after one half-life.
const HALF_LIFE_RATE: f32 = 1.678_347;

/// Types a spring can animate: anything that is a vector space over `f32`.
pub trait SpringValue: Copy + Add<Output = Self> + Sub<Output = Self> + Mul<f32, Output = Self> {
    /// The zero value, used for an initial velocity of rest.
    const ZERO: Self;
}

impl SpringValue for f32 {
    const ZERO: Self = 0.0;
}

impl SpringValue for Vec2 {
    const ZERO: Self = Vec2::ZERO;
}

impl SpringValue for Vec3 {
    const ZERO: Self = Vec3::ZERO;
}

impl SpringValue for Vec3A {
    const ZERO: Self = Vec3A::ZERO;
}

impl SpringValue for Vec4 {
    const ZERO: Self = Vec4::ZERO;
}

/// Advances a critically damped spring from `current` toward `target` by `dt` seconds and
/// returns the new value, updating `velocity` (units per second) in place.
///
/// `half_life` is the time, in seconds, a spring at rest takes to close half the gap. A
/// `half_life` of zero or less snaps to the target; a `dt` of zero or less changes nothing.
#[must_use]
pub fn damp<T: SpringValue>(current: T, target: T, velocity: &mut T, half_life: f32, dt: f32) -> T {
    if half_life <= 0.0 {
        *velocity = T::ZERO;
        return target;
    }
    if dt <= 0.0 {
        return current;
    }
    let rate = HALF_LIFE_RATE / half_life;
    let offset = current - target;
    let j1 = *velocity + offset * rate;
    let decay = (-rate * dt).exp();
    *velocity = (*velocity - j1 * (rate * dt)) * decay;
    target + (offset + j1 * dt) * decay
}

/// A value that follows a moving target through a critically damped spring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringDamper<T> {
    /// The current value.
    pub value: T,
    /// The current velocity, in units per second.
    pub velocity: T,
    /// Seconds for a spring at rest to close half the gap; see [`damp`].
    pub half_life: f32,
}

impl<T: SpringValue> SpringDamper<T> {
    /// A spring at rest at `value`.
    #[must_use]
    pub fn new(value: T, half_life: f32) -> Self {
        Self { value, velocity: T::ZERO, half_life }
    }

    /// Advances toward `target` by `dt` seconds and returns the new value.
    pub fn update(&mut self, target: T, dt: f32) -> T {
        self.value = damp(self.value, target, &mut self.velocity, self.half_life, dt);
        self.value
    }

    /// Jumps to `value` and stops, for camera cuts and teleports.
    pub fn reset(&mut self, value: T) {
        self.value = value;
        self.velocity = T::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_constant_matches_its_definition() {
        let u = f64::from(HALF_LIFE_RATE);
        assert!(((1.0 + u) * (-u).exp() - 0.5).abs() < 1e-7);
    }

    #[test]
    fn closes_half_the_gap_in_one_half_life() {
        let mut spring = SpringDamper::new(0.0_f32, 0.25);
        let after = spring.update(10.0, 0.25);
        assert!((after - 5.0).abs() < 1e-4, "{after}");
    }

    #[test]
    fn approaches_monotonically_without_overshoot() {
        let mut spring = SpringDamper::new(Vec3::ZERO, 0.1);
        let target = Vec3::new(1.0, -2.0, 3.0);
        let mut previous_gap = target.length();
        for _ in 0..600 {
            spring.update(target, 1.0 / 60.0);
            let gap = (target - spring.value).length();
            assert!(gap <= previous_gap + 1e-6, "gap grew from {previous_gap} to {gap}");
            assert!((target - spring.value).dot(target) >= -1e-6, "overshot: {:?}", spring.value);
            previous_gap = gap;
        }
        assert!(previous_gap < 1e-4);
    }

    #[test]
    fn frame_rate_independent() {
        let run = |steps: u32, dt: f32| {
            let mut spring = SpringDamper::new(Vec2::new(5.0, -1.0), 0.2);
            spring.velocity = Vec2::new(3.0, 4.0);
            for _ in 0..steps {
                spring.update(Vec2::new(-2.0, 7.0), dt);
            }
            spring
        };
        let at_30 = run(15, 1.0 / 30.0);
        let at_60 = run(30, 1.0 / 60.0);
        let at_144 = run(72, 1.0 / 144.0);
        for other in [at_60, at_144] {
            assert!(at_30.value.abs_diff_eq(other.value, 1e-4), "{at_30:?} vs {other:?}");
            assert!(at_30.velocity.abs_diff_eq(other.velocity, 1e-3), "{at_30:?} vs {other:?}");
        }
    }

    #[test]
    fn huge_steps_settle_on_the_target() {
        let mut velocity = 100.0;
        let value = damp(0.0_f32, 1.0, &mut velocity, 0.1, 1.0e6);
        assert_eq!(value, 1.0);
        assert_eq!(velocity, 0.0);
    }

    #[test]
    fn degenerate_inputs() {
        let mut velocity = Vec3A::ONE;
        assert_eq!(damp(Vec3A::ZERO, Vec3A::X, &mut velocity, 0.0, 0.016), Vec3A::X);
        assert_eq!(velocity, Vec3A::ZERO);

        let mut velocity = Vec4::ONE;
        assert_eq!(damp(Vec4::ZERO, Vec4::X, &mut velocity, 0.1, 0.0), Vec4::ZERO);
        assert_eq!(velocity, Vec4::ONE);

        let mut spring = SpringDamper::new(1.0_f32, 0.1);
        spring.update(5.0, 0.05);
        spring.reset(-3.0);
        assert_eq!((spring.value, spring.velocity), (-3.0, 0.0));
    }
}
