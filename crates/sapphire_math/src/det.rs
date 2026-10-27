//! Deterministic transcendental functions for simulation code (spec §5.4).
//!
//! # Determinism scope
//!
//! Replays must be bit-identical **within a target architecture**: every aarch64 build (macOS,
//! iOS, Android, Linux on ARM) reproduces every other aarch64 run, and every x86_64 build
//! (Windows, Linux, Steam Deck) every x86_64 run. Replay checksums are recorded per
//! architecture. Bit-identical results *across* architectures are a non-goal until lockstep
//! netcode needs them.
//!
//! Two things break cross-architecture equality today:
//! - **glam's SIMD types.** `Vec3A`, `Vec4`, `Quat`, `Mat4` and `Affine3A` use NEON on aarch64
//!   and SSE2 on x86_64, and the two backends sum horizontal products in a different order
//!   (`Quat::dot` is `(x + y) + (z + w)` on NEON and `(x + z) + (y + w)` on SSE2), and the NEON
//!   quaternion product uses fused multiply-adds where SSE2 rounds twice. The last bit of a dot
//!   product, a normalization or a rotation then differs between architectures.
//! - **The platform C math library.** `f32::sin` and friends call glibc, Apple's libm, Bionic or
//!   the MSVC CRT, which round differently in the last bit or two, even on one architecture.
//!   One differing bit in a hit angle or a knockback curve diverges a replay within a few
//!   ticks.
//!
//! This module removes the second problem: it wraps the `libm` crate, a pure-Rust port of musl's
//! libm, which computes the same bits on every target and OS. Basic arithmetic and `sqrt` are
//! IEEE-exact, and Rust never fuses multiply-adds on its own, so scalar `f32` code built from
//! them and these functions is reproducible everywhere.
//!
//! glam's own trigonometry (`Quat::from_axis_angle`, `Quat::slerp`, `Mat3::from_rotation_y` and
//! so on) uses the platform functions, so simulation code builds rotations with
//! [`quat_from_axis_angle`]. Turning on glam's `libm` feature instead was rejected: Cargo unifies
//! features, so it would slow every glam call in the renderer too.
//!
//! # Guidance for simulation code
//!
//! - Use these functions, not `std`'s, for transcendental math in the fixed step (combat, AI,
//!   movement). Presentation code may use `std`.
//! - Simulation state that will one day need to match across architectures (anything lockstep
//!   netcode would exchange) should use scalar [`Vec3`] and `f32` with this module rather than
//!   `Vec3A`, `Quat` and `Affine3A` arithmetic. This is advice for now, not enforced.

use glam::{Quat, Vec3};

/// Sine of `x` radians.
#[inline]
#[must_use]
pub fn sin(x: f32) -> f32 {
    libm::sinf(x)
}

/// Cosine of `x` radians.
#[inline]
#[must_use]
pub fn cos(x: f32) -> f32 {
    libm::cosf(x)
}

/// Sine and cosine of `x` radians, computed together.
#[inline]
#[must_use]
pub fn sin_cos(x: f32) -> (f32, f32) {
    libm::sincosf(x)
}

/// Tangent of `x` radians.
#[inline]
#[must_use]
pub fn tan(x: f32) -> f32 {
    libm::tanf(x)
}

/// Arccosine of `x`, in radians within [0, π]. NaN outside [−1, 1].
#[inline]
#[must_use]
pub fn acos(x: f32) -> f32 {
    libm::acosf(x)
}

/// Four-quadrant arctangent of `y / x`, in radians within [−π, π].
#[inline]
#[must_use]
pub fn atan2(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}

/// e raised to `x`.
#[inline]
#[must_use]
pub fn exp(x: f32) -> f32 {
    libm::expf(x)
}

/// Natural logarithm of `x`.
#[inline]
#[must_use]
pub fn ln(x: f32) -> f32 {
    libm::logf(x)
}

/// `base` raised to `exponent`.
#[inline]
#[must_use]
pub fn pow(base: f32, exponent: f32) -> f32 {
    libm::powf(base, exponent)
}

/// A rotation of `angle` radians about the unit vector `axis`, computed deterministically
/// (unlike `Quat::from_axis_angle`).
#[inline]
#[must_use]
pub fn quat_from_axis_angle(axis: Vec3, angle: f32) -> Quat {
    debug_assert!(axis.is_normalized(), "axis {axis} is not unit length");
    let (s, c) = sin_cos(angle * 0.5);
    let v = axis * s;
    Quat::from_xyzw(v.x, v.y, v.z, c)
}

/// The shortest tangent (the sine of the angle) along which [`slerp_direction`] still turns
/// near-opposite directions onto each other: about 0.0002°, ten times what rounding leaves
/// exactly opposite directions.
const NEAR_OPPOSITE_TANGENT: f32 = 4e-6;

/// The unit vector `t` of the way from `from`'s direction to `to`'s along the great circle
/// between them, at constant angular speed (spherical linear interpolation), computed
/// deterministically. Neither input needs to be unit length; a zero vector counts as +Y.
///
/// Directions less than about 2° apart blend linearly and renormalize, where the circle is too
/// short to divide by. Directions less than about 2° from opposite, where dividing by the
/// angle's sine loses precision, turn in the plane of `from` and `to`'s part perpendicular to
/// it, still along the short arc and onto `to`. Only opposite directions (to within rounding)
/// have no unique circle: the blend turns about an axis perpendicular to `from`, chosen from
/// `from` alone, so the same inputs always take the same path.
#[must_use]
pub fn slerp_direction(from: Vec3, to: Vec3, t: f32) -> Vec3 {
    let a = from.try_normalize().unwrap_or(Vec3::Y);
    let b = to.try_normalize().unwrap_or(Vec3::Y);
    let cos = a.dot(b).clamp(-1.0, 1.0);
    if cos > 0.9995 {
        return a.lerp(b, t).try_normalize().unwrap_or(a);
    }
    if cos < -0.9995 {
        // `to`'s part perpendicular to `from`: the arc's tangent at `from`, with the sine of the
        // angle between them as its length. Rounding leaves exactly opposite directions at most
        // about 4e-7 of it, so anything well above that is a real arc (review FI-005).
        let tangent = b - a * cos;
        let sin_angle = tangent.length();
        if sin_angle > NEAR_OPPOSITE_TANGENT {
            let (sin_turn, cos_turn) = sin_cos(atan2(sin_angle, cos) * t);
            return (a * cos_turn + tangent * (sin_turn / sin_angle)).normalize();
        }
        // Any axis perpendicular to `from`: the cross product with whichever world axis it is
        // furthest from.
        let other = if a.x.abs() < 0.9 { Vec3::X } else { Vec3::Z };
        let axis = a.cross(other).normalize();
        let (sin_turn, cos_turn) = sin_cos(t * std::f32::consts::PI);
        return (a * cos_turn + axis.cross(a) * sin_turn).normalize();
    }
    let angle = acos(cos);
    let sin_angle = sin(angle);
    (a * (sin((1.0 - t) * angle) / sin_angle) + b * (sin(t * angle) / sin_angle)).normalize()
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    use super::*;

    #[test]
    fn exact_identities() {
        assert_eq!(sin(0.0), 0.0);
        assert_eq!(cos(0.0), 1.0);
        assert_eq!(tan(0.0), 0.0);
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(ln(1.0), 0.0);
        assert_eq!(pow(2.0, 10.0), 1024.0);
        assert_eq!(pow(9.0, 0.5), 3.0);
        assert_eq!(atan2(0.0, 1.0), 0.0);
        assert_eq!(atan2(1.0, 0.0), FRAC_PI_2);
        assert_eq!(atan2(0.0, -1.0), PI);
        assert_eq!(atan2(1.0, 1.0), FRAC_PI_4);
    }

    #[test]
    fn agrees_with_std_to_an_ulp_or_two() {
        let mut x = -10.0_f32;
        while x <= 10.0 {
            let close = |a: f32, b: f32| (a - b).abs() <= 2.0 * f32::EPSILON * a.abs().max(1.0);
            assert!(close(sin(x), x.sin()), "sin({x})");
            assert!(close(cos(x), x.cos()), "cos({x})");
            assert_eq!(sin_cos(x), (sin(x), cos(x)), "sin_cos({x})");
            assert!(close(exp(x * 0.5), (x * 0.5).exp()), "exp({x})");
            assert!(close(atan2(x, 3.0), x.atan2(3.0)), "atan2({x})");
            if x > 0.0 {
                assert!(close(ln(x), x.ln()), "ln({x})");
                assert!(close(pow(x, 1.7), x.powf(1.7)), "pow({x})");
            }
            x += 0.137;
        }
    }

    /// Golden bit patterns. `libm` is pure software, so these must hold on every target; a
    /// failure means a `libm` upgrade changed results and recorded replays need re-baselining.
    #[test]
    fn golden_bits() {
        let cases: [(&str, f32, u32); 7] = [
            ("sin(1)", sin(1.0), GOLDEN[0]),
            ("cos(1)", cos(1.0), GOLDEN[1]),
            ("tan(0.5)", tan(0.5), GOLDEN[2]),
            ("atan2(0.3, -0.7)", atan2(0.3, -0.7), GOLDEN[3]),
            ("exp(2.5)", exp(2.5), GOLDEN[4]),
            ("ln(10)", ln(10.0), GOLDEN[5]),
            ("pow(1.1, 7.3)", pow(1.1, 7.3), GOLDEN[6]),
        ];
        for (name, value, bits) in cases {
            assert_eq!(value.to_bits(), bits, "{name} = {value} ({:#010x})", value.to_bits());
        }
    }

    #[test]
    fn slerp_follows_the_great_circle_at_constant_speed() {
        let (from, to) = (Vec3::X, Vec3::Y);
        for step in 0..=8 {
            let t = step as f32 / 8.0;
            let v = slerp_direction(from * 2.0, to * 0.5, t);
            assert!(v.is_normalized());
            let expected = Vec3::new((t * FRAC_PI_2).cos(), (t * FRAC_PI_2).sin(), 0.0);
            assert!(v.abs_diff_eq(expected, 1e-6), "{t}: {v} vs {expected}");
        }
        // Nearly parallel and opposite directions still give unit vectors on a path.
        let near = slerp_direction(Vec3::X, Vec3::new(1.0, 1e-4, 0.0), 0.5);
        assert!(near.is_normalized() && near.y > 0.0);
        for from in [Vec3::X, Vec3::Y, Vec3::new(0.3, -0.7, 0.2)] {
            let half = slerp_direction(from, -from, 0.5);
            assert!(half.is_normalized() && half.dot(from).abs() < 1e-5, "a quarter turn from {from}");
            assert!(slerp_direction(from, -from, 1.0).abs_diff_eq(-from.normalize(), 1e-5));
        }
        assert_eq!(slerp_direction(Vec3::ZERO, Vec3::ZERO, 0.3), Vec3::Y);
    }

    /// Review FI-005: directions within about 2° of opposite, but not opposite, still have one
    /// shortest arc, and the blend takes it to `to`, not a half turn about an axis chosen from
    /// `from` alone.
    #[test]
    fn slerp_near_opposite_takes_the_short_arc() {
        let from = Vec3::Y;
        for (side, to) in [(-1.0, Vec3::new(-0.02, -1.0, 0.0)), (1.0, Vec3::new(0.02, -1.0, 0.0))] {
            let to = to.normalize();
            assert!(from.dot(to) < -0.9995, "inside the near-opposite band");
            let end = slerp_direction(from, to, 1.0);
            assert!(end.abs_diff_eq(to, 1e-5), "t = 1 lands on {to}, not {end}");
            // Halfway along the short arc is the direction between them, about ±X.
            let half = slerp_direction(from, to, 0.5);
            let between = (from + to).normalize();
            assert!(half.abs_diff_eq(between, 1e-4), "halfway is {between}, not {half}");
            assert!(half.x * side > 0.99, "{to} turns toward {side} X: {half}");
            assert!(slerp_direction(from, to, 0.0).abs_diff_eq(from, 1e-6));
        }
    }

    /// The angle between two directions, for checking a path's speed: `acos` loses precision
    /// near 0 and π, so it's measured with `atan2`.
    fn angle_between(a: Vec3, b: Vec3) -> f32 {
        atan2(a.cross(b).length(), a.dot(b))
    }

    /// Sweeps pairs across the near-opposite threshold (cos −0.9995, about 178.2°): every blend is
    /// a finite unit vector on the short arc at constant angular speed, and nothing jumps where
    /// the branches meet.
    #[test]
    fn slerp_is_continuous_across_the_near_opposite_threshold() {
        let from = Vec3::new(0.3, -0.7, 0.2).normalize();
        // A direction perpendicular to `from`, along which `to` swings away from the antipode.
        let side = from.cross(Vec3::X).normalize();
        let threshold = acos(-0.9995);
        let mut previous_half: Option<Vec3> = None;
        for step in 0..=400 {
            // Separations from 177.2° to π − 1e-4 rad, crossing the threshold.
            let separation = (threshold - 0.0175) + (PI - 1e-4 - threshold + 0.0175) * step as f32 / 400.0;
            let (s, c) = sin_cos(separation);
            let to = (from * c + side * s).normalize();
            let total = angle_between(from, to);
            for k in 0..=10 {
                let t = k as f32 / 10.0;
                let v = slerp_direction(from, to, t);
                assert!(v.is_finite() && v.is_normalized(), "{separation}, {t}: {v}");
                assert!(
                    (angle_between(from, v) - t * total).abs() < 2e-4,
                    "{separation}, {t}: {} from `from`, expected {}",
                    angle_between(from, v),
                    t * total
                );
                assert!(
                    (angle_between(v, to) - (1.0 - t) * total).abs() < 2e-4,
                    "{separation}, {t}: off the short arc"
                );
            }
            // The halfway direction moves smoothly as `to` does: no flip at the threshold.
            let half = slerp_direction(from, to, 0.5);
            if let Some(previous) = previous_half {
                assert!(angle_between(previous, half) < 1e-3, "{separation}: halfway jumped");
            }
            previous_half = Some(half);
        }
    }

    /// Exactly opposite directions, whatever their lengths, still take the axis chosen from
    /// `from` alone: rounding leaves them at most about 4e-7 of a tangent (measured over a million
    /// random pairs), well below the near-opposite turn's threshold.
    #[test]
    fn slerp_of_opposite_directions_of_any_length_is_deterministic() {
        let mut state = 0x1234_5678_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0
        };
        let bits = |v: Vec3| v.to_array().map(f32::to_bits);
        for _ in 0..100_000 {
            let v = Vec3::new(next(), next(), next()) * (next() * 40.0).exp2();
            let k = (next() * 10.0).exp2();
            if v.try_normalize().is_none() {
                continue;
            }
            for t in [0.25, 0.5, 1.0] {
                let opposite = slerp_direction(v, -v * k, t);
                assert_eq!(bits(opposite), bits(slerp_direction(v, -v, t)), "{v}, {k}, {t}");
            }
        }
    }

    /// The near-opposite fix leaves every pair outside its band bit-identical: the parallel and
    /// general branches are unchanged.
    #[test]
    fn slerp_outside_the_near_opposite_band_is_unchanged() {
        /// `slerp_direction` before review FI-005, for directions at least about 1.8° from
        /// opposite.
        fn before(from: Vec3, to: Vec3, t: f32) -> Vec3 {
            let a = from.try_normalize().unwrap_or(Vec3::Y);
            let b = to.try_normalize().unwrap_or(Vec3::Y);
            let cos = a.dot(b).clamp(-1.0, 1.0);
            if cos > 0.9995 {
                return a.lerp(b, t).try_normalize().unwrap_or(a);
            }
            let angle = acos(cos);
            let sin_angle = sin(angle);
            (a * (sin((1.0 - t) * angle) / sin_angle) + b * (sin(t * angle) / sin_angle)).normalize()
        }
        // A fixed xorshift sequence, so the pairs are the same on every run.
        let mut state = 0x9e37_79b9_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0
        };
        let mut checked = 0;
        while checked < 20_000 {
            let from = Vec3::new(next(), next(), next());
            let to = Vec3::new(next(), next(), next());
            let t = next() * 0.75 + 0.5;
            let (a, b) = (from.try_normalize().unwrap_or(Vec3::Y), to.try_normalize().unwrap_or(Vec3::Y));
            if a.dot(b).clamp(-1.0, 1.0) < -0.9995 {
                continue;
            }
            let (ours, theirs) = (slerp_direction(from, to, t), before(from, to, t));
            assert_eq!(
                ours.to_array().map(f32::to_bits),
                theirs.to_array().map(f32::to_bits),
                "{from}, {to}, {t}"
            );
            checked += 1;
        }
    }

    const GOLDEN: [u32; 7] =
        [0x3f57_6aa4, 0x3f0a_5140, 0x3f0b_da7b, 0x402f_261b, 0x4142_eb7f, 0x4013_5d8e, 0x4000_55e0];

    #[test]
    fn quaternion_matches_glam() {
        let axis = Vec3::new(1.0, 2.0, -0.5).normalize();
        for angle in [0.0, 0.3, -1.2, PI, 2.9] {
            let ours = quat_from_axis_angle(axis, angle);
            let glams = Quat::from_axis_angle(axis, angle);
            assert!(ours.abs_diff_eq(glams, 1e-6), "{angle}: {ours} vs {glams}");
            assert!(ours.is_normalized());
        }
    }
}
