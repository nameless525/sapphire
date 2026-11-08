//! Inertialization: a transition with no crossfade (spec §5.13).
//!
//! A crossfade plays two poses and hands over slowly, so a cancel waits for its new pose to fade in.
//! This plays only the new pose, at full weight, from the instant of the transition, and adds to it
//! what it takes for the character not to jump: the **offset** from the new pose to the pose drawn
//! there, and the **velocity** of that offset, each joint's own. Both then die away, on the owner's
//! clock, over a stated time. Nothing is sampled for the old pose after the transition, so a
//! transition costs one frame's extra sampling and a list to walk while it decays.
//!
//! # The curve
//!
//! Over a decay of length `T`, with `s = λt` and normally `λ = 6 / T`, what remains of an offset `x₀` with
//! velocity `v₀` at `t` after the transition is
//!
//! ```text
//! x(t) = ((1 + s) x₀ + t v₀) e^(−s) · w(t / T)        w(u) = 1 − u³ (6u² − 15u + 10)
//! ```
//!
//! The first factor is a critically damped spring's exact path: it never overshoots on its own
//! account, and it starts with exactly the velocity it was given. A spring reaches zero only at
//! infinity, so `w` is there to finish the job: it is flat at both ends (value, slope and curvature),
//! so it leaves `x(0)` and `x′(0)` alone and ends `x` at exactly zero with zero velocity and
//! acceleration, at exactly `T`. `λ = 6 / T` leaves the spring 1.7% of its way to go when `w` closes,
//! so `w` trims a tail and doesn't reshape the curve. From `T` on nothing is added, so the pose is the
//! target's bits.
//!
//! For a feel of the numbers: a decay of 5 ticks has closed 38% of the offset by the first tick, 79%
//! by the second and 96% by the third. So a cancel reads as the new move on its first frame or two,
//! with the old motion's momentum carried into it and no pop.
//!
//! # Rotations
//!
//! A joint's rotational offset lives in the tangent space around the new pose: the rotation vector
//! `r = log(old · new⁻¹)`, by the short way, and it's `r` that decays, in a straight line toward zero.
//! The velocity is `r`'s own slope, found by stepping both poses on by a tick, so it holds through the
//! exponential's curvature at any angle and not only for small offsets: `exp(r(t)) · new(t)` has the
//! old pose's value and velocity at the transition, because `old(t) = exp(r_true(t)) · new(t)` holds by
//! definition and `r` starts with `r_true`'s value and slope.
//!
//! # Time
//!
//! Decays run on whatever clock the caller reads, in that clock's ticks, so a frozen clock holds a
//! decay where it is, and a transition made on a frozen clock starts when the clock does.

use sapphire_math::{Quat, Transform, Vec3};

use crate::pose_math::PoseMath;

/// `λT`. See the [module docs](self).
const RATE: f32 = 6.0;

/// A support transfer keeps more of the pose offset until the free foot catches.
const SUPPORT_TRANSFER_RATE: f32 = 2.0;

/// Below this a joint's offset and velocity aren't worth a place on the list: a thousandth of a
/// millimetre or a millionth of a radian, per tick or not.
const NEGLIGIBLE: f32 = 1e-6;

/// How much of an offset remains and how much of its velocity counts, `elapsed` into a decay of
/// `length` ticks, both ticks: `(1 + s) e^(−s) w` and `t e^(−s) w`. Exactly `(1, 0)` at the start, and
/// exactly `(0, 0)` at `length` and after, whatever `length` is.
#[must_use]
pub fn decay_factors(elapsed: f32, length: f32) -> (f32, f32) {
    decay_factors_at_rate(elapsed, length, RATE, PoseMath::Legacy)
}

fn decay_factors_at_rate(elapsed: f32, length: f32, rate: f32, math: PoseMath) -> (f32, f32) {
    if elapsed >= length {
        return (0.0, 0.0);
    }
    if elapsed <= 0.0 {
        return (1.0, 0.0);
    }
    let u = elapsed / length;
    let window = (1.0 - u * u * u * (u * (6.0 * u - 15.0) + 10.0)).max(0.0);
    let s = rate * u;
    let spring = math.exp(-s) * window;
    ((1.0 + s) * spring, elapsed * spring)
}

/// The rotation vector of `q`, by the short way: its axis scaled by its angle, exact down to the
/// smallest turn a quaternion can hold, where `acos(w)` would be noise.
#[inline]
fn log(q: Quat, math: PoseMath) -> Vec3 {
    let q = if q.w < 0.0 { -q } else { q };
    let xyz = Vec3::new(q.x, q.y, q.z);
    let sine = xyz.length();
    if sine < 1e-5 {
        // sin(θ/2) ≈ θ/2 here, to within a part in 1e10.
        return xyz * (2.0 / q.w.max(0.5));
    }
    xyz * (2.0 * math.atan2(sine, q.w) / sine)
}

/// The rotation with rotation vector `r`.
#[inline]
fn exp(r: Vec3, math: PoseMath) -> Quat {
    let angle_squared = r.length_squared();
    if angle_squared < 1e-10 {
        // The series, to within a part in 1e10.
        let half = 0.5 - angle_squared / 48.0;
        return Quat::from_xyzw(r.x * half, r.y * half, r.z * half, 1.0 - angle_squared / 8.0);
    }
    let angle = angle_squared.sqrt();
    let (sine, cosine) = math.sin_cos(angle * 0.5);
    let axis = r * (sine / angle);
    Quat::from_xyzw(axis.x, axis.y, axis.z, cosine)
}

/// A pose and its morph weights.
#[derive(Clone, Copy, Debug)]
pub struct PoseRef<'a> {
    pub joints: &'a [Transform],
    pub weights: &'a [f32],
}

/// What a transition starts from: everything it needs of the poses around it.
#[derive(Clone, Copy, Debug)]
pub struct Transition<'a> {
    /// When it happens, in ticks on the owner's clock.
    pub at: f64,
    /// How long it takes to die away, in ticks. 0 cuts straight to the new pose.
    pub decay: f32,
    /// The pose drawn at `at`.
    pub drawn: PoseRef<'a>,
    /// The pose drawn `drawn_step` ticks before it. A pose with no past gives `drawn` again.
    pub earlier: PoseRef<'a>,
    pub drawn_step: f32,
    /// The new pose at `at`.
    pub target: PoseRef<'a>,
    /// The new pose `target_step` ticks after `at`.
    pub later: PoseRef<'a>,
    pub target_step: f32,
}

#[derive(Clone, Copy, Debug)]
struct JointDecay {
    joint: u16,
    translation: Vec3,
    translation_velocity: Vec3,
    rotation: Vec3,
    rotation_velocity: Vec3,
    scale: Vec3,
    scale_velocity: Vec3,
}

#[derive(Clone, Copy, Debug)]
struct WeightDecay {
    target: u16,
    offset: f32,
    velocity: f32,
}

/// What's left of the transitions so far, for one character. A new one replaces it: the pose drawn
/// already has the old one's offset in it, so a transition inside a decay stays continuous with no
/// more said.
#[derive(Clone, Debug)]
pub struct Inertia {
    joints: Vec<JointDecay>,
    weights: Vec<WeightDecay>,
    started: f64,
    length: f32,
    support_transfer: bool,
    math: PoseMath,
}

impl Inertia {
    /// For a character with this many joints and morph targets. Allocates; nothing later does.
    #[must_use]
    pub fn new(joints: usize, weights: usize) -> Self {
        Self {
            joints: Vec::with_capacity(joints),
            weights: Vec::with_capacity(weights),
            started: 0.0,
            length: 0.0,
            support_transfer: false,
            math: PoseMath::Legacy,
        }
    }

    pub(crate) fn set_math(&mut self, math: PoseMath) {
        self.math = math;
    }

    pub(crate) fn write_canonical(
        &self,
        out: &mut crate::graph::canonical::Canonical<'_, impl FnMut(&[u8])>,
    ) {
        out.len(self.joints.len());
        for joint in &self.joints {
            out.u16(joint.joint);
            out.floats(&joint.translation.to_array());
            out.floats(&joint.translation_velocity.to_array());
            out.floats(&joint.rotation.to_array());
            out.floats(&joint.rotation_velocity.to_array());
            out.floats(&joint.scale.to_array());
            out.floats(&joint.scale_velocity.to_array());
        }
        out.len(self.weights.len());
        for weight in &self.weights {
            out.u16(weight.target);
            out.f32(weight.offset);
            out.f32(weight.velocity);
        }
        if !self.joints.is_empty() || !self.weights.is_empty() {
            out.f64(self.started);
            out.f32(self.length);
            out.flag(self.support_transfer);
        }
    }

    /// Whether anything is left to add `now`.
    #[must_use]
    pub fn is_running(&self, now: f64) -> bool {
        !(self.joints.is_empty() && self.weights.is_empty()) && now - self.started < f64::from(self.length)
    }

    /// When the last transition happened.
    #[must_use]
    pub fn started(&self) -> f64 {
        self.started
    }

    /// Nothing more to add.
    pub fn clear(&mut self) {
        self.joints.clear();
        self.weights.clear();
    }

    /// Starts a transition, replacing whatever was decaying. Allocates nothing.
    pub fn begin(&mut self, transition: &Transition<'_>) {
        self.begin_transition(transition, false);
    }

    /// Starts a grounded transition whose pose settles over the recovery step.
    pub(crate) fn begin_support_transfer(&mut self, transition: &Transition<'_>) {
        self.begin_transition(transition, true);
    }

    fn begin_transition(&mut self, transition: &Transition<'_>, support_transfer: bool) {
        self.clear();
        self.support_transfer = support_transfer;
        self.started = transition.at;
        self.length = transition.decay.max(0.0);
        if self.length <= 0.0 {
            return;
        }
        let Transition { drawn, earlier, drawn_step, target, later, target_step, .. } = *transition;
        let per_drawn = if drawn_step > 0.0 { drawn_step.recip() } else { 0.0 };
        let step = target_step.max(f32::MIN_POSITIVE);
        for (joint, ((&now, &before), (&new, &after))) in
            drawn.joints.iter().zip(earlier.joints).zip(target.joints.iter().zip(later.joints)).enumerate()
        {
            let translation = now.translation - new.translation;
            let translation_velocity = (now.translation - before.translation) * per_drawn
                - (after.translation - new.translation) / step;
            let scale = now.scale - new.scale;
            let scale_velocity = (now.scale - before.scale) * per_drawn - (after.scale - new.scale) / step;
            let difference = self.math.normalize(self.math.multiply(now.rotation, new.rotation.inverse()));
            let rotation = log(difference, self.math);
            // Both poses a tick on: the old by the turn it was making, the new by what it says. The
            // difference between them, on the same side of the sphere as the one now, is where `r`
            // is headed. See the module docs.
            let turning = log(
                self.math.normalize(self.math.multiply(now.rotation, before.rotation.inverse())),
                self.math,
            ) * per_drawn;
            let carried =
                self.math.normalize(self.math.multiply(exp(turning * step, self.math), now.rotation));
            let next = self.math.normalize(self.math.multiply(carried, after.rotation.inverse()));
            let next = if self.math.dot(next, difference) < 0.0 { -next } else { next };
            let rotation_velocity = (log_from(next, difference, self.math) - rotation) / step;
            let moved = translation.abs().max_element()
                + translation_velocity.abs().max_element()
                + scale.abs().max_element()
                + scale_velocity.abs().max_element()
                + rotation.abs().max_element()
                + rotation_velocity.abs().max_element();
            if moved >= NEGLIGIBLE {
                self.joints.push(JointDecay {
                    joint: joint as u16,
                    translation,
                    translation_velocity,
                    rotation,
                    rotation_velocity,
                    scale,
                    scale_velocity,
                });
            }
        }
        for (target_index, ((&now, &before), (&new, &after))) in drawn
            .weights
            .iter()
            .zip(earlier.weights)
            .zip(target.weights.iter().zip(later.weights))
            .enumerate()
        {
            let offset = now - new;
            let velocity = (now - before) * per_drawn - (after - new) / step;
            if offset.abs() + velocity.abs() >= NEGLIGIBLE {
                self.weights.push(WeightDecay { target: target_index as u16, offset, velocity });
            }
        }
    }

    /// Adds what's left at `now` to the new pose, in place. Past the decay's end it adds nothing, so
    /// the pose keeps the bits it came with; a joint a transition barely moved keeps them throughout.
    /// Allocates nothing.
    pub fn apply(&self, now: f64, joints: &mut [Transform], weights: &mut [f32]) {
        let elapsed = (now - self.started) as f32;
        let (mut remaining, carried) = decay_factors_at_rate(elapsed, self.length, RATE, self.math);
        if self.support_transfer {
            // Carry the old velocity as usual and let the pose offset settle with the step.
            remaining = decay_factors_at_rate(elapsed, self.length, SUPPORT_TRANSFER_RATE, self.math).0;
        }
        if remaining == 0.0 && carried == 0.0 {
            return;
        }
        for decay in &self.joints {
            if let Some(joint) = joints.get_mut(usize::from(decay.joint)) {
                joint.translation += decay.translation * remaining + decay.translation_velocity * carried;
                joint.scale += decay.scale * remaining + decay.scale_velocity * carried;
                let turn = decay.rotation * remaining + decay.rotation_velocity * carried;
                joint.rotation =
                    self.math.normalize(self.math.multiply(exp(turn, self.math), joint.rotation));
            }
        }
        for decay in &self.weights {
            if let Some(weight) = weights.get_mut(usize::from(decay.target)) {
                // A morph target has no meaning below nothing.
                *weight = (*weight + decay.offset * remaining + decay.velocity * carried).max(0.0);
            }
        }
    }
}

/// [`log`] of `q`, the rotation near `near`: past a half turn, `log` would take the other way round
/// and `r` would seem to leap, so this stays on the side `near` is on.
#[inline]
fn log_from(q: Quat, near: Quat, math: PoseMath) -> Vec3 {
    let r = log(q, math);
    let reference = log(near, math);
    let angle = r.length();
    if r.dot(reference) < 0.0 && angle > 2.5 {
        // Across the half turn: the same rotation, the long way round.
        r - r * (core::f32::consts::TAU / angle)
    } else {
        r
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, reason = "reference fixtures also exercise the legacy math policy")]
mod tests {
    use core::f32::consts::PI;

    use super::*;

    /// A joint at `t` on two made-up motions: smooth, moving, and different.
    fn old_motion(t: f32) -> Transform {
        Transform {
            translation: Vec3::new(0.3 * (0.17 * t).sin(), 1.0 + 0.1 * t, -0.2 * (0.05 * t).cos()),
            rotation: Quat::from_rotation_y(0.12 * t) * Quat::from_rotation_x(0.4 + 0.07 * t),
            scale: Vec3::ONE,
        }
    }

    fn new_motion(t: f32) -> Transform {
        Transform {
            translation: Vec3::new(-0.2 + 0.04 * t, 0.7 - 0.02 * t, 0.5 * (0.1 * t).sin()),
            rotation: Quat::from_rotation_z(1.2 - 0.09 * t) * Quat::from_rotation_y(-0.6 + 0.03 * t),
            scale: Vec3::new(1.0, 1.0 + 0.01 * t, 1.0),
        }
    }

    fn weight_old(t: f32) -> f32 {
        0.5 + 0.02 * t
    }

    fn weight_new(t: f32) -> f32 {
        0.2 - 0.01 * t
    }

    /// `inertia` set at time 0 from the old motion to the new, with this length of decay.
    fn transition(decay: f32) -> Inertia {
        let h = 0.25;
        let mut inertia = Inertia::new(1, 1);
        let drawn = [old_motion(0.0)];
        let earlier = [old_motion(-h)];
        let target = [new_motion(0.0)];
        let later = [new_motion(1.0)];
        inertia.begin(&Transition {
            at: 0.0,
            decay,
            drawn: PoseRef { joints: &drawn, weights: &[weight_old(0.0)] },
            earlier: PoseRef { joints: &earlier, weights: &[weight_old(-h)] },
            drawn_step: h,
            target: PoseRef { joints: &target, weights: &[weight_new(0.0)] },
            later: PoseRef { joints: &later, weights: &[weight_new(1.0)] },
            target_step: 1.0,
        });
        inertia
    }

    /// The new motion at `t` with `inertia` over it.
    fn output(inertia: &Inertia, t: f32) -> (Transform, f32) {
        let mut joints = [new_motion(t)];
        let mut weights = [weight_new(t)];
        inertia.apply(f64::from(t), &mut joints, &mut weights);
        (joints[0], weights[0])
    }

    #[test]
    fn the_curve_starts_and_ends_exactly() {
        for length in [1.0, 3.0, 5.0, 8.0, 12.0, 60.0, 120.0] {
            assert_eq!(decay_factors(0.0, length), (1.0, 0.0));
            assert_eq!(decay_factors(length, length), (0.0, 0.0));
            assert_eq!(decay_factors(length + 3.0, length), (0.0, 0.0));
        }
        assert_eq!(decay_factors(5.0, 0.0), (0.0, 0.0), "no decay is a cut");
    }

    #[test]
    fn a_support_transfer_settles_later_without_extending_the_transition() {
        let length = 11.0;
        let ordinary = transition(length);
        let mut transfer = ordinary.clone();
        transfer.support_transfer = true;
        assert_eq!(output(&transfer, 0.0), output(&ordinary, 0.0));
        let halfway = length * 0.5;
        let remaining = |inertia| (output(inertia, halfway).1 - weight_new(halfway)).abs();
        assert!(remaining(&transfer) > remaining(&ordinary) * 2.0);
        assert_eq!(output(&transfer, length), output(&ordinary, length));
    }

    /// The numbers the module docs quote, and that nothing rings or leaves the unit range.
    #[test]
    fn the_curve_falls_without_ringing() {
        let closed = |ticks: f32| 1.0 - decay_factors(ticks, 5.0).0;
        assert!((closed(1.0) - 0.38).abs() < 0.01, "{}", closed(1.0));
        assert!((closed(2.0) - 0.79).abs() < 0.01, "{}", closed(2.0));
        assert!((closed(3.0) - 0.96).abs() < 0.01, "{}", closed(3.0));
        let mut last = 1.0;
        for step in 1..=1000 {
            let (remaining, carried) = decay_factors(step as f32 * 0.01, 10.0);
            assert!(remaining <= last + 1e-7 && (0.0..=1.0).contains(&remaining), "step {step}: {remaining}");
            assert!(carried >= 0.0, "step {step}");
            last = remaining;
        }
    }

    /// `exp` and `log` undo each other down to the smallest turns and up to almost half a turn, where
    /// the plain `acos` formula would be noise.
    #[test]
    fn log_and_exp_are_exact_at_every_size() {
        for angle in [0.0, 1e-7, 1e-5, 1e-3, 0.01, 0.5, 2.0, 3.1, PI - 1e-3] {
            let r = Vec3::new(0.6, -0.48, 0.64) * angle;
            let q = exp(r, PoseMath::Legacy);
            assert!((q.length() - 1.0).abs() < 1e-6, "{angle}: {q}");
            let back = log(q, PoseMath::Legacy);
            let tolerance = 1e-6_f32.max(angle * 1e-4);
            assert!(back.abs_diff_eq(r, tolerance), "{angle}: {back} against {r}");
        }
        // Either way a quaternion is written.
        let q = exp(Vec3::new(0.0, 1.0, 0.0), PoseMath::Legacy);
        assert!(log(-q, PoseMath::Legacy).abs_diff_eq(Vec3::Y, 1e-5));
    }

    /// At the instant of the transition the pose is the old pose, in value.
    #[test]
    fn the_pose_is_continuous_across_a_transition() {
        let inertia = transition(8.0);
        let (joint, weight) = output(&inertia, 0.0);
        let old = old_motion(0.0);
        assert!(joint.translation.abs_diff_eq(old.translation, 1e-5), "{joint:?} against {old:?}");
        assert!(joint.rotation.dot(old.rotation).abs() > 1.0 - 1e-6, "{joint:?} against {old:?}");
        assert!(joint.scale.abs_diff_eq(old.scale, 1e-5));
        assert!((weight - weight_old(0.0)).abs() < 1e-6);
    }

    /// Stepping out of the transition by a small `h`, the pose moves as the old one would have, to
    /// first order, however different the new one's own motion is: the error shrinks with `h`.
    #[test]
    fn the_velocity_is_continuous_across_a_transition() {
        // The worst of the step's error, translation and rotation, for a step of `h`.
        let error = |h: f32| {
            let mut inertia = Inertia::new(1, 1);
            let (drawn, earlier) = ([old_motion(0.0)], [old_motion(-h)]);
            let (target, later) = ([new_motion(0.0)], [new_motion(h)]);
            inertia.begin(&Transition {
                at: 0.0,
                decay: 8.0,
                drawn: PoseRef { joints: &drawn, weights: &[weight_old(0.0)] },
                earlier: PoseRef { joints: &earlier, weights: &[weight_old(-h)] },
                drawn_step: h,
                target: PoseRef { joints: &target, weights: &[weight_new(0.0)] },
                later: PoseRef { joints: &later, weights: &[weight_new(h)] },
                target_step: h,
            });
            let (out, weight) = output(&inertia, h);
            let old = old_motion(h);
            let translation = out.translation.distance(old.translation) / h;
            let rotation = out.rotation.angle_between(old.rotation) / h;
            let weight = (weight - weight_old(h)).abs() / h;
            translation.max(rotation).max(weight)
        };
        // What's left at a finite step is the decay's acceleration times the step: it goes to nothing
        // with the step, so the velocity itself has no jump.
        let (coarse, fine) = (error(0.5), error(0.125 / 4.0));
        assert!(fine < 0.02, "{fine}");
        assert!(fine < coarse * 0.2, "{fine} against {coarse}");
    }

    /// A rotational offset near half a turn is where a tangent-space decay most easily goes wrong.
    #[test]
    fn a_turn_near_half_keeps_its_velocity() {
        let h = 0.125;
        // A hair under half a turn, and past it a step on.
        let old = |t: f32| Quat::from_rotation_y(PI - 0.002 + 0.2 * t);
        let joint = |rotation| Transform { rotation, ..Transform::IDENTITY };
        let (drawn, earlier) = ([joint(old(0.0))], [joint(old(-h))]);
        let (target, later) = ([Transform::IDENTITY], [Transform::IDENTITY]);
        let mut inertia = Inertia::new(1, 0);
        inertia.begin(&Transition {
            at: 0.0,
            decay: 8.0,
            drawn: PoseRef { joints: &drawn, weights: &[] },
            earlier: PoseRef { joints: &earlier, weights: &[] },
            drawn_step: h,
            target: PoseRef { joints: &target, weights: &[] },
            later: PoseRef { joints: &later, weights: &[] },
            target_step: h,
        });
        let mut out = [Transform::IDENTITY];
        inertia.apply(f64::from(h), &mut out, &mut []);
        let moved = out[0].rotation.angle_between(old(h));
        assert!(moved < 0.2 * h, "{moved}");
    }

    /// By the decay's end the pose is the target's, bit for bit; just before, it's nearly there.
    #[test]
    fn it_reaches_the_target_bit_for_bit() {
        let inertia = transition(8.0);
        for t in [8.0, 8.0001, 9.0, 100.0] {
            let (joint, weight) = output(&inertia, t);
            let target = new_motion(t);
            assert_eq!(
                joint.translation.to_array().map(f32::to_bits),
                target.translation.to_array().map(f32::to_bits)
            );
            assert_eq!(
                joint.rotation.to_array().map(f32::to_bits),
                target.rotation.to_array().map(f32::to_bits)
            );
            assert_eq!(joint.scale.to_array().map(f32::to_bits), target.scale.to_array().map(f32::to_bits));
            assert_eq!(weight.to_bits(), weight_new(t).to_bits());
        }
        assert!(!inertia.is_running(8.0));
        assert!(inertia.is_running(7.999));
        let (joint, _) = output(&inertia, 7.99);
        assert!(joint.translation.abs_diff_eq(new_motion(7.99).translation, 1e-4));
    }

    /// The pose velocity at the decay's end is the target's own: nothing kinks as it finishes.
    #[test]
    fn it_arrives_with_the_targets_velocity() {
        let inertia = transition(8.0);
        let h = 0.01;
        let (before, _) = output(&inertia, 8.0 - h);
        let (at, _) = output(&inertia, 8.0);
        let velocity = (at.translation - before.translation) / h;
        let target = (new_motion(8.0).translation - new_motion(8.0 - h).translation) / h;
        assert!(velocity.abs_diff_eq(target, 2e-3), "{velocity} against {target}");
    }

    /// A joint a transition doesn't move keeps its bits while the others decay.
    #[test]
    fn an_untouched_joint_keeps_its_bits() {
        let still = Transform {
            translation: Vec3::new(0.1, 0.2, 0.3),
            rotation: Quat::from_rotation_x(0.3),
            scale: Vec3::ONE,
        };
        let (drawn, earlier) = ([still, old_motion(0.0)], [still, old_motion(-1.0)]);
        let (target, later) = ([still, new_motion(0.0)], [still, new_motion(1.0)]);
        let mut inertia = Inertia::new(2, 0);
        inertia.begin(&Transition {
            at: 0.0,
            decay: 8.0,
            drawn: PoseRef { joints: &drawn, weights: &[] },
            earlier: PoseRef { joints: &earlier, weights: &[] },
            drawn_step: 1.0,
            target: PoseRef { joints: &target, weights: &[] },
            later: PoseRef { joints: &later, weights: &[] },
            target_step: 1.0,
        });
        let mut out = [still, new_motion(3.0)];
        inertia.apply(3.0, &mut out, &mut []);
        assert_eq!(out[0], still);
        assert_ne!(out[1], new_motion(3.0));
    }

    /// A decay of nothing is a cut: nothing to add, whatever came before.
    #[test]
    fn no_decay_cuts_to_the_new_pose() {
        let inertia = transition(0.0);
        assert!(!inertia.is_running(0.0));
        let (joint, _) = output(&inertia, 0.0);
        assert_eq!(joint, new_motion(0.0));
    }

    /// No past (a character's first frame): the drawn pose's own velocity counts as nothing, and the
    /// velocity it starts with undoes only the new pose's.
    #[test]
    fn a_pose_with_no_past_starts_at_rest() {
        let (drawn, target, later) = ([old_motion(0.0)], [new_motion(0.0)], [new_motion(1.0)]);
        let mut inertia = Inertia::new(1, 0);
        inertia.begin(&Transition {
            at: 0.0,
            decay: 8.0,
            drawn: PoseRef { joints: &drawn, weights: &[] },
            earlier: PoseRef { joints: &drawn, weights: &[] },
            drawn_step: 0.0,
            target: PoseRef { joints: &target, weights: &[] },
            later: PoseRef { joints: &later, weights: &[] },
            target_step: 1.0,
        });
        let h = 0.01;
        let mut out = [new_motion(h)];
        inertia.apply(f64::from(h), &mut out, &mut []);
        assert!(out[0].translation.abs_diff_eq(old_motion(0.0).translation, 1e-3), "{:?}", out[0]);
    }
}
