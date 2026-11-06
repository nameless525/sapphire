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

    /// Advances a spring for positive `half_life` and `dt`, returning its value and velocity.
    /// The default uses wide coefficients with generic vector-space arithmetic: differences,
    /// scaled terms and the result must be representable. Built-in scalar/vector values retain
    /// ordinary f32 arithmetic and use componentwise f64 arithmetic at numeric extremes.
    fn damped(self, target: Self, velocity: Self, half_life: f32, dt: f32) -> (Self, Self) {
        let (rate, u, decay) = coefficients(half_life, dt);
        if decay == 0.0 {
            return (target, Self::ZERO);
        }
        let offset = self - target;
        (
            target + scale(offset, (1.0 + u) * decay) + scale(velocity, f64::from(dt) * decay),
            scale(velocity, (1.0 - u) * decay) - scale(offset, rate * u * decay),
        )
    }
}

impl SpringValue for f32 {
    const ZERO: Self = 0.0;

    #[inline]
    fn damped(self, target: Self, velocity: Self, half_life: f32, dt: f32) -> (Self, Self) {
        let result = damp_f32(self, target, velocity, half_life, dt);
        if result.0.is_finite() && result.1.is_finite() {
            return result;
        }
        let (rate, u, decay) = coefficients(half_life, dt);
        if decay == 0.0 {
            return (target, 0.0);
        }
        damp_component(self, target, velocity, rate, u, decay)
    }
}

// Keep the original vector arithmetic on ordinary updates, and use the same scalar fallback
// per component only at extremes. Opposite near-f32::MAX endpoints then subtract in f64.
macro_rules! spring_vector {
    ($ty:ty, $components:literal) => {
        impl SpringValue for $ty {
            const ZERO: Self = Self::ZERO;

            #[inline]
            fn damped(self, target: Self, velocity: Self, half_life: f32, dt: f32) -> (Self, Self) {
                let result = damp_f32(self, target, velocity, half_life, dt);
                if result.0.is_finite() && result.1.is_finite() {
                    return result;
                }
                let (rate, u, decay) = coefficients(half_life, dt);
                if decay == 0.0 {
                    return (target, Self::ZERO);
                }
                let (current, target, velocity) = (self.to_array(), target.to_array(), velocity.to_array());
                let result: [(f32, f32); $components] = std::array::from_fn(|i| {
                    damp_component(current[i], target[i], velocity[i], rate, u, decay)
                });
                (Self::from_array(result.map(|pair| pair.0)), Self::from_array(result.map(|pair| pair.1)))
            }
        }
    };
}

spring_vector!(Vec2, 2);
spring_vector!(Vec3, 3);
spring_vector!(Vec3A, 3);
spring_vector!(Vec4, 4);

/// Advances a critically damped spring from `current` toward `target` by `dt` seconds and
/// returns the new value, updating `velocity` (units per second) in place.
///
/// `half_life` is the time, in seconds, a spring at rest takes to close half the gap. A
/// `half_life` of zero or less snaps to the target; a `dt` of zero or less changes nothing.
/// Finite positive times, including subnormal values, are supported. Results whose true
/// magnitude exceeds the value type's range can still overflow.
#[must_use]
pub fn damp<T: SpringValue>(current: T, target: T, velocity: &mut T, half_life: f32, dt: f32) -> T {
    if half_life <= 0.0 {
        *velocity = T::ZERO;
        return target;
    }
    if dt <= 0.0 {
        return current;
    }
    let (value, next_velocity) = current.damped(target, *velocity, half_life, dt);
    *velocity = next_velocity;
    value
}

/// Original operation order for bit-identical ordinary presentation updates. Overflowing
/// candidates are local and never committed by the built-in implementations.
fn damp_f32<T: SpringValue>(current: T, target: T, velocity: T, half_life: f32, dt: f32) -> (T, T) {
    let rate = HALF_LIFE_RATE / half_life;
    let offset = current - target;
    let j1 = velocity + offset * rate;
    let decay = (-rate * dt).exp();
    (target + (offset + j1 * dt) * decay, (velocity - j1 * (rate * dt)) * decay)
}

fn coefficients(half_life: f32, dt: f32) -> (f64, f64, f64) {
    // Form the dimensionless time ratio in f64: an overflowing f32 rate alone does not
    // imply settling when dt is comparably tiny.
    let rate = f64::from(HALF_LIFE_RATE) / f64::from(half_life);
    let u = rate * f64::from(dt);
    (rate, u, (-u).exp())
}

fn damp_component(current: f32, target: f32, velocity: f32, rate: f64, u: f64, decay: f64) -> (f32, f32) {
    let offset = f64::from(current) - f64::from(target);
    let velocity = f64::from(velocity);
    let value = f64::from(target) + offset * ((1.0 + u) * decay) + velocity * (u / rate * decay);
    let velocity = velocity * ((1.0 - u) * decay) - offset * (rate * u * decay);
    (value as f32, velocity as f32)
}

/// Applies a wide coefficient without first rounding it to zero or infinity.
fn scale<T: SpringValue>(value: T, coefficient: f64) -> T {
    if coefficient == 0.0 {
        T::ZERO
    } else if coefficient.abs() > f64::from(f32::MAX) {
        (value * f32::MAX) * (coefficient / f64::from(f32::MAX)) as f32
    } else if coefficient.abs() < f64::from(f32::MIN_POSITIVE) {
        (value * f32::MIN_POSITIVE) * (coefficient / f64::from(f32::MIN_POSITIVE)) as f32
    } else {
        value * coefficient as f32
    }
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
    fn ordinary_updates_preserve_the_original_f32_solution() {
        for index in 1..=2048 {
            let current = (index % 113) as f32 - 51.0;
            let target = (index % 79) as f32 - 35.0;
            let initial_velocity = (index % 67) as f32 - 25.0;
            let half_life = 0.001 + (index % 100) as f32 * 0.025;
            let dt = 0.001 + (index % 32) as f32 * 0.001;
            let rate = HALF_LIFE_RATE / half_life;
            let offset = current - target;
            let j1 = initial_velocity + offset * rate;
            let decay = (-rate * dt).exp();
            let expected_velocity = (initial_velocity - j1 * (rate * dt)) * decay;
            let expected_value = target + (offset + j1 * dt) * decay;
            let mut velocity = initial_velocity;
            let value = damp(current, target, &mut velocity, half_life, dt);
            assert_eq!(value.to_bits(), expected_value.to_bits());
            assert_eq!(velocity.to_bits(), expected_velocity.to_bits());
        }
    }

    #[test]
    fn extreme_finite_time_scales_match_a_wide_reference() {
        for (half_life, dt, offset, initial_velocity) in [
            (1e-40_f32, 1.0 / 60.0, 1.0_f32, 2.0_f32),
            (0.1, f32::MAX, 1.0, 2.0),
            (1e-40, 1e-40, 1e-10, 1e20),
            (f32::from_bits(1), f32::from_bits(1), 1e-20, 0.0),
            (0.25, 0.01, 1.0, 2.0),
        ] {
            let rate = f64::from(HALF_LIFE_RATE) / f64::from(half_life);
            let u = rate * f64::from(dt);
            let e = (-u).exp();
            // Independent scalar closed form, evaluated entirely in f64.
            let j = f64::from(initial_velocity) + f64::from(offset) * rate;
            let expected_value = (f64::from(offset) + j * f64::from(dt)) * e;
            let expected_velocity = (f64::from(initial_velocity) - j * u) * e;
            let mut velocity = initial_velocity;
            let value = damp(offset, 0.0, &mut velocity, half_life, dt);
            let close = |a: f32, b: f64| {
                assert!(a.is_finite(), "{a}, half-life={half_life}, dt={dt}");
                assert!((f64::from(a) - b).abs() <= b.abs() * 2e-6 + 1e-35, "{a} vs {b}");
            };
            close(value, expected_value);
            close(velocity, expected_velocity);
            let mut vector_velocity = Vec3::splat(initial_velocity);
            let vector = damp(Vec3::splat(offset), Vec3::ZERO, &mut vector_velocity, half_life, dt);
            assert_eq!(vector, Vec3::splat(value));
            assert_eq!(vector_velocity, Vec3::splat(velocity));
        }
    }

    #[test]
    fn finite_endpoints_do_not_need_a_representable_f32_difference() {
        let mut velocity = 0.0;
        let value = damp(f32::MAX, -f32::MAX, &mut velocity, 100.0, 100.0);
        let u = f64::from(HALF_LIFE_RATE);
        let gap = 2.0 * f64::from(f32::MAX);
        let expected = -f64::from(f32::MAX) + gap * (1.0 + u) * (-u).exp();
        let expected_velocity = -gap * (u / 100.0) * u * (-u).exp();
        assert!(value.is_finite() && velocity.is_finite());
        assert!((f64::from(value) - expected).abs() <= f64::from(f32::MAX) * 1e-7);
        assert!((f64::from(velocity) - expected_velocity).abs() <= expected_velocity.abs() * 1e-6);
        let mut vector_velocity = Vec4::ZERO;
        let vector = damp(Vec4::splat(f32::MAX), Vec4::splat(-f32::MAX), &mut vector_velocity, 100.0, 100.0);
        assert_eq!(vector, Vec4::splat(value));
        assert_eq!(vector_velocity, Vec4::splat(velocity));
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
