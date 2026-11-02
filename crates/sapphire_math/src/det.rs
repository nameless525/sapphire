//! Deterministic transcendental functions for simulation code (spec §5.4).
//!
//! # Determinism scope
//!
//! Replays must be bit-identical **within a target architecture**: every aarch64 build (macOS,
//! iOS, Android, Linux on ARM) reproduces every other aarch64 run, and every x86_64 build
//! (Windows, Linux, Steam Deck) every x86_64 run. Replay checksums are recorded per
//! architecture. Bit-identical results *across* architectures are a non-goal until lockstep
//! netcode needs them, except for what a bake writes (see "Guidance" below).
//!
//! Two things break cross-architecture equality today:
//! - **glam's SIMD types.** `Vec3A`, `Vec4`, `Quat`, `Mat4` and `Affine3A` use NEON on aarch64
//!   and SSE2 on x86_64, and the two backends sum horizontal products in a different order
//!   (`Quat::dot` is `(x + y) + (z + w)` on NEON and `(x + z) + (y + w)` on SSE2), and the NEON
//!   quaternion product uses fused multiply-adds where SSE2 rounds twice, and `normalize`
//!   multiplies by the length's reciprocal on NEON where SSE2 divides by the length. The last bit of
//!   a dot product, a normalization or a rotation then differs between architectures.
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
//! glam's own trigonometry uses the platform functions: with its default `std` math,
//! `Quat::from_axis_angle`, `Quat::from_rotation_*`, `Quat::to_axis_angle`, `Quat::from_euler`,
//! `Vec3::slerp`, `Vec3::rotate_y`, `Vec2::from_angle` and the matrix constructors all call them.
//! Simulation code uses this module's replacements instead, which do the same jobs in scalar
//! arithmetic with `libm`. Turning on glam's `libm` or `scalar-math` feature instead was rejected:
//! Cargo unifies features across a build, so it would slow every glam call in the renderer, or
//! change its layouts and its goldens.
//!
//! # Guidance for simulation code
//!
//! - Use these functions, not `std`'s, for transcendental math in the fixed step (combat, AI,
//!   movement). Presentation code may use `std`.
//! - **The simulation's crates are held to it:** their `clippy.toml` bans `std`'s functions and
//!   glam's helpers that reach them, with this module named as the way out, and `cargo xtask
//!   check-layers` holds those crates to that list (`[simulation]` in `xtask/layers.toml`).
//! - **Anything that bakes into cooked data must match across architectures,** because one replay
//!   file holds one hash of the move data for all of them: a cook uses scalar [`Vec3`] and `f32`
//!   with this module, never `Vec3A`, `Vec4` or `Quat` arithmetic. The simulation's `clippy.toml` bans
//!   the named methods that differ, and a test pins the test content's hash (M2 G2). It can't ban
//!   `Quat`'s operators, so `quat * vec` in a bake is still caught only by that test.
//! - Simulation state that will one day need to match across architectures (anything lockstep
//!   netcode would exchange) should do the same. This is advice for now, not enforced. Everything
//!   here works on scalars inside, so it agrees across architectures itself, and its [`Quat`]
//!   results are plain values whatever the backend.

use std::f32::consts::{PI, TAU};

use glam::{Quat, Vec2, Vec3};

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

/// Arcsine of `x`, in radians within [−π/2, π/2]. NaN outside [−1, 1].
#[inline]
#[must_use]
pub fn asin(x: f32) -> f32 {
    libm::asinf(x)
}

/// Arccosine of `x`, in radians within [0, π]. NaN outside [−1, 1].
#[inline]
#[must_use]
pub fn acos(x: f32) -> f32 {
    libm::acosf(x)
}

/// Arctangent of `x`, in radians within [−π/2, π/2].
#[inline]
#[must_use]
pub fn atan(x: f32) -> f32 {
    libm::atanf(x)
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

/// 2 raised to `x`.
#[inline]
#[must_use]
pub fn exp2(x: f32) -> f32 {
    libm::exp2f(x)
}

/// Natural logarithm of `x`.
#[inline]
#[must_use]
pub fn ln(x: f32) -> f32 {
    libm::logf(x)
}

/// Base-2 logarithm of `x`.
#[inline]
#[must_use]
pub fn log2(x: f32) -> f32 {
    libm::log2f(x)
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

/// A rotation of `angle` radians about +X (unlike `Quat::from_rotation_x`).
#[inline]
#[must_use]
pub fn quat_from_rotation_x(angle: f32) -> Quat {
    let (s, c) = sin_cos(angle * 0.5);
    Quat::from_xyzw(s, 0.0, 0.0, c)
}

/// A rotation of `angle` radians about +Y, up: a yaw (unlike `Quat::from_rotation_y`).
#[inline]
#[must_use]
pub fn quat_from_rotation_y(angle: f32) -> Quat {
    let (s, c) = sin_cos(angle * 0.5);
    Quat::from_xyzw(0.0, s, 0.0, c)
}

/// A rotation of `angle` radians about +Z (unlike `Quat::from_rotation_z`).
#[inline]
#[must_use]
pub fn quat_from_rotation_z(angle: f32) -> Quat {
    let (s, c) = sin_cos(angle * 0.5);
    Quat::from_xyzw(0.0, 0.0, s, c)
}

/// The unit axis and the angle in radians, within [−π, π], of the unit quaternion `q`, as
/// `Quat::to_axis_angle` gives them. A rotation too small to name an axis comes back as +X and 0.
#[must_use]
pub fn quat_to_axis_angle(q: Quat) -> (Vec3, f32) {
    let [x, y, z, w] = q.to_array();
    let length = (x * x + y * y + z * z).sqrt();
    if length < 1e-8 {
        return (Vec3::X, 0.0);
    }
    (Vec3::new(x / length, y / length, z / length), wrap_angle(2.0 * atan2(length, w)))
}

/// The dot product of two quaternions, summed in one order on every architecture (glam's differs
/// between NEON and SSE2).
#[inline]
fn quat_dot(a: [f32; 4], b: [f32; 4]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

/// The weighted sum of two quaternions, renormalized, in scalar arithmetic.
#[inline]
fn quat_blend(a: [f32; 4], a_weight: f32, b: [f32; 4], b_weight: f32) -> Quat {
    quat_unit(std::array::from_fn(|i| a[i] * a_weight + b[i] * b_weight))
}

/// Four components at unit length, in scalar arithmetic: the identity if they have no length.
#[inline]
fn quat_unit(q: [f32; 4]) -> Quat {
    let length = quat_dot(q, q).sqrt();
    if length == 0.0 {
        return Quat::IDENTITY;
    }
    Quat::from_array(q.map(|component| component / length))
}

/// `q` at unit length, with the same bits on every architecture, which [`Quat::normalize`] doesn't
/// have (see the module docs). A `q` with no length gives the identity.
#[must_use]
pub fn quat_normalize(q: Quat) -> Quat {
    quat_unit(q.to_array())
}

/// `from` and `to` as arrays, with `to` flipped into `from`'s hemisphere, so a blend takes the
/// shortest arc, and their dot product, which is then at least 0.
#[inline]
fn same_hemisphere(from: Quat, to: Quat) -> ([f32; 4], [f32; 4], f32) {
    let (a, b) = (from.to_array(), to.to_array());
    let dot = quat_dot(a, b);
    if dot < 0.0 { (a, b.map(|component| -component), -dot) } else { (a, b, dot) }
}

/// The unit rotation `t` of the way from `from` to `to` by a straight line and a renormalization,
/// along the shortest arc. Faster than [`quat_slerp`], and its speed drifts only for wide angles,
/// so it suits blending neighbouring samples of a table.
#[must_use]
pub fn quat_nlerp(from: Quat, to: Quat, t: f32) -> Quat {
    let (a, b, _) = same_hemisphere(from, to);
    quat_blend(a, 1.0 - t, b, t)
}

/// The unit rotation `t` of the way from `from` to `to` along the shortest arc, at constant angular
/// speed. Rotations less than about 3.6° apart blend as [`quat_nlerp`] does, where the arc is too
/// short to divide by.
#[must_use]
pub fn quat_slerp(from: Quat, to: Quat, t: f32) -> Quat {
    let (a, b, dot) = same_hemisphere(from, to);
    if dot > 0.9995 {
        return quat_blend(a, 1.0 - t, b, t);
    }
    let theta = acos(dot);
    let sin_theta = sin(theta);
    quat_blend(a, sin((1.0 - t) * theta) / sin_theta, b, sin(t * theta) / sin_theta)
}

/// The angle in radians, within [0, π], of the rotation between two unit quaternions. Measured
/// with [`atan2`], so it keeps its precision near 0, where `acos` loses it.
#[must_use]
pub fn quat_angle_between(from: Quat, to: Quat) -> f32 {
    let (_, _, dot) = same_hemisphere(from, to);
    let dot = dot.min(1.0);
    2.0 * atan2(((1.0 - dot) * (1.0 + dot)).sqrt(), dot)
}

/// `from` turned toward `to` by at most `max_angle` radians, along the shortest arc. A negative
/// `max_angle` turns away, by at most as far as `to` is. Lands on `to` when it is within reach.
#[must_use]
pub fn quat_rotate_towards(from: Quat, to: Quat, max_angle: f32) -> Quat {
    let angle = quat_angle_between(from, to);
    if angle <= 1e-4 {
        return to;
    }
    quat_slerp(from, to, (max_angle / angle).clamp(-1.0, 1.0))
}

/// `v` rotated by `angle` radians about +Y: the same as `Quat::from_rotation_y(angle) * v`, or
/// `v.rotate_y(angle)`.
#[inline]
#[must_use]
pub fn rotate_y(v: Vec3, angle: f32) -> Vec3 {
    let (s, c) = sin_cos(angle);
    Vec3::new(v.x * c + v.z * s, v.y, v.z * c - v.x * s)
}

/// The horizontal unit direction a character with this yaw faces: +Z at yaw 0, turning toward +X
/// (spec §5.4: every object faces +Z).
#[inline]
#[must_use]
pub fn direction_from_yaw(yaw: f32) -> Vec3 {
    let (s, c) = sin_cos(yaw);
    Vec3::new(s, 0.0, c)
}

/// The yaw of a direction, ignoring its height: the inverse of [`direction_from_yaw`]. A vertical
/// or zero direction has yaw 0.
#[inline]
#[must_use]
pub fn yaw_of_direction(direction: Vec3) -> f32 {
    atan2(direction.x, direction.z)
}

/// The yaw of what a rotation does to forward (+Z), for a rotation that isn't only a yaw.
#[must_use]
pub fn yaw_of_quat(q: Quat) -> f32 {
    let [x, y, z, w] = q.to_array();
    atan2(2.0 * (x * z + w * y), 1.0 - 2.0 * (x * x + y * y))
}

/// `angle` wrapped into [−π, π], by plain arithmetic.
#[inline]
#[must_use]
pub fn wrap_angle(angle: f32) -> f32 {
    angle - TAU * (angle / TAU).round()
}

/// `angle` turned toward `target` the short way round by at most `max_step` radians. For a turning
/// speed: the result isn't wrapped, so a yaw stays continuous, and it lands on `target` once
/// within reach, give or take whole turns.
#[inline]
#[must_use]
pub fn turn_towards(angle: f32, target: f32, max_step: f32) -> f32 {
    angle + wrap_angle(target - angle).clamp(-max_step, max_step)
}

/// The angle in radians, within [0, π], between two vectors, which needn't be unit length. Measured
/// with [`atan2`], so it keeps its precision near 0 and π; two zero vectors read 0.
#[inline]
#[must_use]
pub fn angle_between(a: Vec3, b: Vec3) -> f32 {
    atan2(a.cross(b).length(), a.dot(b))
}

/// The unit vector at `angle` radians from +X toward +Y (unlike `Vec2::from_angle`).
#[inline]
#[must_use]
pub fn vec2_from_angle(angle: f32) -> Vec2 {
    let (s, c) = sin_cos(angle);
    Vec2::new(c, s)
}

/// A vector's angle in radians from +X toward +Y, within [−π, π] (unlike `Vec2::to_angle`).
#[inline]
#[must_use]
pub fn vec2_to_angle(v: Vec2) -> f32 {
    atan2(v.y, v.x)
}

/// The signed angle in radians from `a` to `b`, within [−π, π], positive from +X toward +Y.
#[inline]
#[must_use]
pub fn vec2_angle_to(a: Vec2, b: Vec2) -> f32 {
    atan2(a.perp_dot(b), a.dot(b))
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
        let (sin_turn, cos_turn) = sin_cos(t * PI);
        return (a * cos_turn + axis.cross(a) * sin_turn).normalize();
    }
    let angle = acos(cos);
    let sin_angle = sin(angle);
    (a * (sin((1.0 - t) * angle) / sin_angle) + b * (sin(t * angle) / sin_angle)).normalize()
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

    use super::*;

    /// What glam 0.33's two backends compute for `Quat::normalize`, written out in scalars.
    fn neon_normalize(q: [f32; 4]) -> [f32; 4] {
        let [x, y, z, w] = q;
        let recip = 1.0 / ((x * x + y * y) + (z * z + w * w)).sqrt();
        q.map(|component| component * recip)
    }

    fn sse2_normalize(q: [f32; 4]) -> [f32; 4] {
        let [x, y, z, w] = q;
        let length = ((x * x + z * z) + (y * y + w * w)).sqrt();
        q.map(|component| component / length)
    }

    /// Rows the two backends disagree on, with what [`quat_normalize`] must give on every target. The
    /// first is a rotation from the test content, which differs by the reciprocal alone. The second
    /// also differs by the order of the sum, so all three answers are different.
    const NORMALIZED: [([f32; 4], [u32; 4]); 2] = [
        ([0.0, 0.3826834, 0.0, 0.9238795], [0, 0x3ec3_ef15, 0, 0x3f6c_835f]),
        ([0.9, 0.05, -0.3, 0.31], [0x3f66_8fbc, 0x3d4c_f18b, 0xbe99_b528, 0x3e9e_d4cb]),
    ];

    #[test]
    fn quat_normalize_has_one_answer_on_every_architecture() {
        let bits = |q: [f32; 4]| q.map(f32::to_bits);
        for (q, expected) in NORMALIZED {
            assert_eq!(bits(quat_normalize(Quat::from_array(q)).to_array()), expected, "{q:?}");
            assert_ne!(
                bits(neon_normalize(q)),
                bits(sse2_normalize(q)),
                "this row can't tell them apart: {q:?}"
            );
        }
        assert_eq!(quat_normalize(Quat::from_xyzw(0.0, 0.0, 0.0, 0.0)), Quat::IDENTITY);
    }

    /// The module docs' account of `Quat::normalize` holds for the glam this builds against, on
    /// whichever backend this runs. If an upgrade fails this, read its source again: the bans and the
    /// docs say why it can't reach a bake.
    #[test]
    fn glams_normalize_does_what_the_docs_say() {
        for (q, _) in NORMALIZED {
            let glam = Quat::from_array(q).normalize().to_array();
            if cfg!(target_arch = "aarch64") {
                assert_eq!(glam, neon_normalize(q), "{q:?}");
            } else if cfg!(target_arch = "x86_64") {
                assert_eq!(glam, sse2_normalize(q), "{q:?}");
            }
        }
    }

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

    /// The additions' bit patterns: `libm` and scalar arithmetic, so they hold on every target.
    #[test]
    fn golden_bits_of_the_simulations_helpers() {
        let q = quat_from_axis_angle(Vec3::new(0.48, 0.6, 0.64), 1.1);
        let r = quat_from_rotation_y(-0.7);
        let cases: [(&str, u32, u32); 12] = [
            ("asin(0.3)", asin(0.3).to_bits(), GOLDEN_HELPERS[0]),
            ("atan(0.7)", atan(0.7).to_bits(), GOLDEN_HELPERS[1]),
            ("exp2(1.5)", exp2(1.5).to_bits(), GOLDEN_HELPERS[2]),
            ("log2(10)", log2(10.0).to_bits(), GOLDEN_HELPERS[3]),
            ("slerp.x", quat_slerp(q, r, 0.3).x.to_bits(), GOLDEN_HELPERS[4]),
            ("slerp.w", quat_slerp(q, r, 0.3).w.to_bits(), GOLDEN_HELPERS[5]),
            ("nlerp.y", quat_nlerp(q, r, 0.3).y.to_bits(), GOLDEN_HELPERS[6]),
            ("angle", quat_angle_between(q, r).to_bits(), GOLDEN_HELPERS[7]),
            ("rotate_y.x", rotate_y(Vec3::new(1.0, 2.0, 3.0), 0.9).x.to_bits(), GOLDEN_HELPERS[8]),
            ("yaw", yaw_of_quat(q).to_bits(), GOLDEN_HELPERS[9]),
            ("wrap", wrap_angle(11.0).to_bits(), GOLDEN_HELPERS[10]),
            ("between", angle_between(Vec3::X, Vec3::new(1.0, 2.0, 2.0)).to_bits(), GOLDEN_HELPERS[11]),
        ];
        for (name, bits, golden) in cases {
            assert_eq!(bits, golden, "{name}: {bits:#010x}");
        }
    }

    #[test]
    fn inverse_functions_agree_with_std() {
        let close = |a: f32, b: f32| (a - b).abs() <= 2.0 * f32::EPSILON * a.abs().max(1.0);
        for step in -20..=20 {
            let x = step as f32 / 20.0;
            assert!(close(asin(x), x.asin()), "asin({x})");
            assert!(close(atan(x * 3.0), (x * 3.0).atan()), "atan({x})");
            assert!(close(exp2(x * 5.0), (x * 5.0).exp2()), "exp2({x})");
            if step > 0 {
                assert!(close(log2(x * 7.0), (x * 7.0).log2()), "log2({x})");
            }
        }
    }

    /// A spread of unit rotations, away from the near-parallel blend and the axis singularities.
    fn rotations() -> [Quat; 5] {
        [
            Quat::IDENTITY,
            Quat::from_axis_angle(Vec3::new(1.0, 2.0, -0.5).normalize(), 0.7),
            Quat::from_axis_angle(Vec3::new(-0.3, 0.2, 0.9).normalize(), 2.4),
            Quat::from_rotation_y(-1.9),
            Quat::from_euler(glam::EulerRot::YXZ, 0.8, -0.4, 1.3),
        ]
    }

    /// Whether two quaternions are the same rotation, either sign.
    fn same_rotation(a: Quat, b: Quat, tolerance: f32) -> bool {
        a.abs_diff_eq(b, tolerance) || a.abs_diff_eq(-b, tolerance)
    }

    #[test]
    fn rotations_match_glams() {
        for angle in [-3.0, -1.2, 0.0, 0.3, 1.5, FRAC_PI_2, 2.9, PI] {
            assert!(quat_from_rotation_x(angle).abs_diff_eq(Quat::from_rotation_x(angle), 1e-6), "x {angle}");
            assert!(quat_from_rotation_y(angle).abs_diff_eq(Quat::from_rotation_y(angle), 1e-6), "y {angle}");
            assert!(quat_from_rotation_z(angle).abs_diff_eq(Quat::from_rotation_z(angle), 1e-6), "z {angle}");
            let v = Vec3::new(0.4, -1.5, 2.2);
            assert!(rotate_y(v, angle).abs_diff_eq(v.rotate_y(angle), 1e-5), "rotate_y {angle}");
            assert!(
                rotate_y(v, angle).abs_diff_eq(Quat::from_rotation_y(angle) * v, 1e-5),
                "the quat {angle}"
            );
            assert!(vec2_from_angle(angle).abs_diff_eq(Vec2::from_angle(angle), 1e-6), "vec2 {angle}");
        }
    }

    #[test]
    fn slerp_matches_glams_along_the_short_arc() {
        for from in rotations() {
            for to in rotations() {
                for t in [0.0, 0.25, 0.5, 0.9, 1.0] {
                    let ours = quat_slerp(from, to, t);
                    assert!(ours.is_normalized(), "{from} {to} {t}");
                    assert!(same_rotation(ours, from.slerp(to, t), 1e-3), "{from} {to} {t}: {ours}");
                    let line = quat_nlerp(from, to, t);
                    assert!(line.is_normalized(), "{from} {to} {t}");
                    // A chord's halfway point is the arc's, and both ends meet it.
                    if t == 0.0 || t == 0.5 || t == 1.0 {
                        assert!(same_rotation(line, ours, 1e-3), "{from} {to} {t}: {line}");
                    }
                }
                // The opposite sign of one end is the same rotation, so the same path.
                assert!(same_rotation(quat_slerp(from, -to, 0.3), quat_slerp(from, to, 0.3), 1e-5));
            }
        }
    }

    #[test]
    fn slerp_of_nearly_equal_rotations_stays_finite() {
        let a = quat_from_rotation_y(0.5);
        for gap in [0.0, 1e-7, 1e-5, 1e-3, 0.03, 0.04] {
            let b = quat_from_rotation_y(0.5 + gap);
            let middle = quat_slerp(a, b, 0.5);
            assert!(middle.is_normalized());
            assert!(same_rotation(middle, quat_from_rotation_y(0.5 + gap / 2.0), 1e-4), "{gap}: {middle}");
        }
    }

    #[test]
    fn axis_angle_round_trips_and_matches_glams() {
        for q in rotations().into_iter().skip(1) {
            let (axis, angle) = quat_to_axis_angle(q);
            let (glams_axis, glams_angle) = q.to_axis_angle();
            assert!(axis.abs_diff_eq(glams_axis, 1e-5), "{q}");
            assert!((angle - glams_angle).abs() < 1e-5, "{q}");
            assert!(same_rotation(quat_from_axis_angle(axis, angle), q, 1e-5), "{q}");
        }
        assert_eq!(quat_to_axis_angle(Quat::IDENTITY), (Vec3::X, 0.0));
        // Past a half turn it reads as the shorter turn the other way.
        let far = Quat::from_xyzw(0.0, -0.8, 0.0, -0.6);
        let (axis, angle) = quat_to_axis_angle(far);
        assert!(same_rotation(quat_from_axis_angle(axis, angle), far, 1e-5));
        assert!(angle.abs() <= PI);
    }

    #[test]
    fn angles_between_rotations_and_vectors() {
        for from in rotations() {
            for to in rotations() {
                let ours = quat_angle_between(from, to);
                assert!((ours - from.angle_between(to)).abs() < 1e-3, "{from} {to}: {ours}");
                assert_eq!(ours.to_bits(), quat_angle_between(from, -to).to_bits(), "either sign");
            }
        }
        // `acos` would read 0 here; the atan2 form keeps the small angle.
        let small = quat_angle_between(Quat::IDENTITY, quat_from_rotation_y(1e-3));
        assert!((small - 1e-3).abs() < 1e-4, "{small}");

        assert_eq!(angle_between(Vec3::X, Vec3::X), 0.0);
        assert_eq!(angle_between(Vec3::X, Vec3::Y), FRAC_PI_2);
        assert_eq!(angle_between(Vec3::X, -Vec3::X), PI);
        assert_eq!(angle_between(Vec3::ZERO, Vec3::ZERO), 0.0);
        let (a, b) = (Vec3::new(1.0, 2.0, 3.0), Vec3::new(-2.0, 0.5, 4.0));
        assert!((angle_between(a * 3.0, b * 0.1) - a.angle_between(b)).abs() < 1e-5);
    }

    #[test]
    fn rotate_towards_stops_at_the_limit_or_on_the_target() {
        let (from, to) = (quat_from_rotation_y(0.0), quat_from_rotation_y(1.0));
        assert!(same_rotation(quat_rotate_towards(from, to, 0.25), quat_from_rotation_y(0.25), 1e-5));
        assert!(same_rotation(quat_rotate_towards(from, to, 2.0), to, 1e-5));
        assert!(same_rotation(quat_rotate_towards(from, to, -0.25), quat_from_rotation_y(-0.25), 1e-5));
        assert_eq!(quat_rotate_towards(from, from, 0.5), from);
    }

    #[test]
    fn yaw_conventions() {
        // The yaw's forward is what the rotation does to +Z (spec §5.4).
        for yaw in [-3.0, -1.0, 0.0, 0.4, FRAC_PI_2, 2.5] {
            let q = quat_from_rotation_y(yaw);
            assert!(direction_from_yaw(yaw).abs_diff_eq(q * Vec3::Z, 1e-6), "{yaw}");
            assert!((yaw_of_direction(direction_from_yaw(yaw)) - yaw).abs() < 1e-6, "{yaw}");
            assert!((yaw_of_quat(q) - yaw).abs() < 1e-5, "{yaw}");
            // A pitch about the character's own side doesn't move where forward points on the floor.
            let pitched = q * quat_from_rotation_x(0.2);
            assert!((yaw_of_quat(pitched) - yaw).abs() < 1e-5, "{yaw}");
        }
        assert_eq!(yaw_of_direction(Vec3::Y), 0.0);
        assert_eq!(yaw_of_direction(Vec3::ZERO), 0.0);
    }

    #[test]
    fn angles_wrap_and_turn_the_short_way() {
        let cases = [(0.0, 0.0), (1.0, 1.0), (TAU, 0.0), (-TAU, 0.0), (TAU + 1.0, 1.0), (-TAU - 1.0, -1.0)];
        for (angle, wrapped) in cases {
            assert!((wrap_angle(angle) - wrapped).abs() < 1e-5, "{angle}");
        }
        assert!(wrap_angle(3.5) < 0.0, "past a half turn is the short way back");
        assert_eq!(turn_towards(0.0, 1.0, 0.25), 0.25);
        assert_eq!(turn_towards(0.0, 1.0, 2.0), 1.0);
        // Across the seam: 3 to −3 is about 0.28 forward, not 6 back. The result stays continuous.
        let across = turn_towards(3.0, -3.0, 0.1);
        assert!((across - 3.1).abs() < 1e-5, "{across}");
    }

    #[test]
    fn vec2_angles() {
        assert_eq!(vec2_to_angle(Vec2::X), 0.0);
        assert_eq!(vec2_to_angle(Vec2::Y), FRAC_PI_2);
        assert_eq!(vec2_angle_to(Vec2::X, Vec2::Y), FRAC_PI_2);
        assert_eq!(vec2_angle_to(Vec2::Y, Vec2::X), -FRAC_PI_2);
        let (a, b) = (Vec2::new(0.3, -0.8), Vec2::new(-1.0, 0.1));
        assert!((vec2_angle_to(a, b) - a.angle_to(b)).abs() < 1e-5);
        assert!((vec2_to_angle(a) - a.to_angle()).abs() < 1e-6);
    }

    const GOLDEN_HELPERS: [u32; 12] = [
        0x3e9c_00ad,
        0x3f1c_5889,
        0x4035_04f3,
        0x4054_9a78,
        0x3e3e_5e52,
        0x3f71_7fee,
        0x3dff_f908,
        0x3fce_0332,
        0x403e_2e8b,
        0x3f4d_bb5f,
        0xbfc8_7ed8,
        0x3f9d_9014,
    ];

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
