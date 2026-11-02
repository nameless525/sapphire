//! Head and eye look-at: a chain down the neck turning toward a target, then the eyes the rest of
//! the way, each within limits and at a limited speed.
//!
//! **What moves.** The rig's own data says: VRM's `lookAt` is either eye joints turned by degrees
//! ([`EyeDrive::Bones`], the shipped character's) or the `lookUp`, `lookDown`, `lookLeft` and
//! `lookRight` expressions by weight ([`EyeDrive::Expressions`]), through the same range maps from
//! gaze angle to response. A rig with neither just turns its head.
//!
//! **How a gaze is held.** As a swing from straight ahead in the frame of the joint under the chain,
//! so a character's own turn carries the gaze round with it and the limits stay the body's. Within
//! its limits the swing is the target's. Past them it holds the limit, and past [`LookSettings::hold`]
//! it falls away to straight ahead, reaching it with the target dead behind. So the map from target
//! to gaze is continuous everywhere on the sphere: a target crossing behind the character passes
//! through straight ahead rather than jumping from one shoulder to the other, and one hovering
//! exactly behind doesn't make the head wobble between them. The state then follows that at a limited
//! speed, so nothing a target does can make the head snap.
//!
//! **Time** is the owner's: `dt` 0 moves nothing and a frozen character keeps its gaze, however its
//! target moves, and takes it up at the same limited speed when its clock thaws.

use std::f32::consts::PI;

use sapphire_math::{Quat, Vec2, Vec3};

use crate::ik::solve::{SolvePose, weighted};

/// How a character's look-at behaves. Angles in radians, times in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookSettings {
    /// How far the head turns from the body, either way.
    pub yaw_limit: f32,
    pub up_limit: f32,
    pub down_limit: f32,
    /// How far round a target may get before the gaze starts falling away from its limit, reaching
    /// straight ahead with the target dead behind.
    pub hold: f32,
    /// The head's top speed, radians a second.
    pub rate: f32,
    /// How quickly the head closes on its gaze short of that speed.
    pub half_life: f32,
    /// The eyes' top speed and half-life: much quicker than the head.
    pub eye_rate: f32,
    pub eye_half_life: f32,
    /// How long a weight takes to go from 0 to 1.
    pub fade: f32,
}

impl Default for LookSettings {
    fn default() -> Self {
        Self {
            yaw_limit: 70f32.to_radians(),
            up_limit: 30f32.to_radians(),
            down_limit: 40f32.to_radians(),
            hold: 110f32.to_radians(),
            rate: 4.5,
            half_life: 0.07,
            eye_rate: 14.0,
            eye_half_life: 0.02,
            fade: 0.25,
        }
    }
}

/// A linear map from a gaze angle to the eyes' response (VRM's range maps).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeMap {
    /// The gaze angle at which the response is full, radians. Larger angles hold it.
    pub input_max: f32,
    /// The full response: radians of eye rotation, or an expression weight.
    pub output: f32,
}

impl RangeMap {
    fn map(self, angle: f32) -> f32 {
        if self.input_max <= 0.0 { 0.0 } else { (angle / self.input_max).min(1.0) * self.output }
    }
}

/// What the rig moves for its eyes. See the `look` module's docs.
#[derive(Clone, Debug, PartialEq)]
pub enum EyeDrive {
    /// Eye joints, each with its parent's rest rotation in the model's space: the eye's turns are in
    /// rest model space.
    Bones { left: Option<(u16, Quat)>, right: Option<(u16, Quat)> },
    /// Morph targets and their weights at full look.
    Expressions { up: Vec<(u32, f32)>, down: Vec<(u32, f32)>, left: Vec<(u32, f32)>, right: Vec<(u32, f32)> },
}

/// The eyes of a rig's look-at data.
#[derive(Clone, Debug, PartialEq)]
pub struct EyeRig {
    /// Toward the nose (each eye's inner side), away from it, and down and up.
    pub inner: RangeMap,
    pub outer: RangeMap,
    pub down: RangeMap,
    pub up: RangeMap,
    pub drive: EyeDrive,
}

/// The joints a look-at turns.
#[derive(Clone, Debug, PartialEq)]
pub struct LookBones {
    /// The joints that turn, top down, each with its share of the turn and its place in `path`.
    pub(crate) chain: Vec<(u16, f32, u16)>,
    /// The first of `chain` down to the head, every joint between, parents first.
    pub(crate) path: Vec<u16>,
    /// The joint under the chain: the body the limits are the limits of.
    pub(crate) body: u16,
    pub(crate) rest_body: Quat,
    pub(crate) head: u16,
    pub(crate) rest_head: Quat,
    /// Where the eyes look from, in the head joint's space.
    pub(crate) origin: Vec3,
    pub(crate) eyes: Option<EyeRig>,
}

/// One character's look-at state. See the `look` module's docs.
#[derive(Clone, Debug, PartialEq)]
pub struct LookAt {
    bones: LookBones,
    settings: LookSettings,
    /// Where the head looks, as a swing from straight ahead in the body's frame.
    gaze: Vec2,
    /// What it's closing on.
    desired: Vec2,
    /// The eyes' yaw and pitch from the head's forward.
    eyes: Vec2,
    eyes_desired: Vec2,
    fade: f32,
    fade_goal: f32,
}

impl LookAt {
    #[must_use]
    pub fn new(bones: LookBones, settings: LookSettings) -> Self {
        Self {
            bones,
            settings,
            gaze: Vec2::ZERO,
            desired: Vec2::ZERO,
            eyes: Vec2::ZERO,
            eyes_desired: Vec2::ZERO,
            fade: 0.0,
            fade_goal: 0.0,
        }
    }

    /// The share of its look the character is showing, 0 to 1.
    #[must_use]
    pub fn fade(&self) -> f32 {
        self.fade
    }

    /// Whether another step would move nothing, for the same target.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.fade == self.fade_goal
            && (self.fade == 0.0 || (self.gaze == self.desired && self.eyes == self.eyes_desired))
    }

    /// Forgets the gaze, for a cut or a teleport: the next target is looked at straight away.
    pub fn reset(&mut self) {
        self.fade = 0.0;
        self.fade_goal = 0.0;
    }

    /// Turns the pose toward `target`, in the model's space, by `dt` seconds of the owner's clock.
    /// `pose` must be up to date for the joints in the chain's path and above it. `None`, or a weight
    /// of 0, fades out. Keeps the chain's path up to date. Allocates nothing.
    pub fn solve(
        &mut self,
        pose: &mut SolvePose<'_>,
        morphs: &mut [f32],
        target: Option<Vec3>,
        weight: f32,
        dt: f32,
    ) {
        self.fade_goal = if target.is_some() { weight.clamp(0.0, 1.0) } else { 0.0 };
        let fresh = self.fade == 0.0;
        if fresh && self.fade_goal == 0.0 {
            return;
        }
        self.fade = approach(self.fade, self.fade_goal, dt / self.settings.fade.max(1e-3));

        let (body, head) = (usize::from(self.bones.body), usize::from(self.bones.head));
        let body_turn = pose.rotation(body) * self.bones.rest_body.inverse();
        if let Some(target) = target {
            let origin = pose.transform(head).transform_point3(self.bones.origin);
            let toward = (body_turn.inverse() * (target - origin)).normalize_or_zero();
            self.desired = self.settings.limited(swing_of(toward));
            self.gaze = if fresh {
                self.desired
            } else {
                follow(self.gaze, self.desired, dt, self.settings.half_life, self.settings.rate)
            };
        }
        if self.fade == 0.0 {
            return;
        }

        let look = Quat::from_scaled_axis(Vec3::new(self.gaze.x, self.gaze.y, 0.0)) * Vec3::Z;
        let forward = body_turn.inverse() * pose.rotation(head) * self.bones.rest_head.inverse() * Vec3::Z;
        let turn = body_turn * level_turn(forward, look) * body_turn.inverse();
        let (axis, angle) = weighted(turn, self.fade).to_axis_angle();
        for &(joint, share, place) in &self.bones.chain {
            pose.turn(usize::from(joint), Quat::from_axis_angle(axis, angle * share));
            pose.refresh(&self.bones.path[usize::from(place) + 1..]);
        }
        self.solve_eyes(pose, morphs, target, dt, fresh);
    }

    /// The eyes follow on from where the head ended up.
    fn solve_eyes(
        &mut self,
        pose: &mut SolvePose<'_>,
        morphs: &mut [f32],
        target: Option<Vec3>,
        dt: f32,
        fresh: bool,
    ) {
        let Some(eyes) = &self.bones.eyes else { return };
        let head = usize::from(self.bones.head);
        if let Some(target) = target {
            let head_turn = pose.rotation(head) * self.bones.rest_head.inverse();
            let origin = pose.transform(head).transform_point3(self.bones.origin);
            let toward = (head_turn.inverse() * (target - origin)).normalize_or_zero();
            self.eyes_desired = eye_angles(toward, self.settings.hold);
            self.eyes = if fresh {
                self.eyes_desired
            } else {
                follow(self.eyes, self.eyes_desired, dt, self.settings.eye_half_life, self.settings.eye_rate)
            };
        }
        let (yaw, pitch) = (self.eyes.x, self.eyes.y);
        // A positive yaw is toward the character's left, which is outward for its left eye.
        let (left_yaw, right_yaw) = if yaw >= 0.0 {
            (eyes.outer.map(yaw), eyes.inner.map(yaw))
        } else {
            (-eyes.inner.map(-yaw), -eyes.outer.map(-yaw))
        };
        let lift = if pitch >= 0.0 { eyes.up.map(pitch) } else { -eyes.down.map(-pitch) };
        match &eyes.drive {
            EyeDrive::Bones { left, right } => {
                for (eye, yaw) in [(left, left_yaw), (right, right_yaw)] {
                    if let Some((joint, parent_rest)) = eye {
                        let turn = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-lift);
                        pose.turn_at_rest(usize::from(*joint), *parent_rest, weighted(turn, self.fade));
                    }
                }
            }
            EyeDrive::Expressions { up, down, left, right } => {
                let mut add = |targets: &[(u32, f32)], amount: f32| {
                    for &(target, weight) in targets {
                        if let Some(morph) = morphs.get_mut(target as usize) {
                            *morph += amount * self.fade * weight;
                        }
                    }
                };
                add(up, lift.max(0.0));
                add(down, (-lift).max(0.0));
                add(left, left_yaw.max(0.0));
                add(right, (-left_yaw).max(0.0));
            }
        }
    }
}

impl LookSettings {
    /// `swing` held within the limits, then falling away toward straight ahead. See the
    /// [module docs](self).
    fn limited(&self, swing: Vec2) -> Vec2 {
        let angle = swing.length();
        if angle < 1e-6 {
            return swing;
        }
        // A positive turn about +X looks down.
        let vertical = if swing.x > 0.0 { self.down_limit } else { self.up_limit };
        let reach = ((swing.x / vertical).powi(2) + (swing.y / self.yaw_limit).powi(2)).sqrt();
        let held = swing / reach.max(1.0);
        held * falloff(angle, self.hold.max(held.length()))
    }
}

/// 1 up to `start` and 0 at a half-turn from straight ahead, by a smooth step.
fn falloff(angle: f32, start: f32) -> f32 {
    if angle <= start {
        return 1.0;
    }
    let left = ((PI - angle) / (PI - start).max(1e-3)).clamp(0.0, 1.0);
    left * left * (3.0 - 2.0 * left)
}

/// The turn from straight ahead (+Z) to the unit `direction`, as a rotation vector, which lies in the
/// XY plane. Dead behind has no direction to turn in, and reads as none.
fn swing_of(direction: Vec3) -> Vec2 {
    let sine = Vec2::new(direction.x, direction.y).length();
    if sine < 1e-6 {
        return Vec2::ZERO;
    }
    Vec2::new(-direction.y, direction.x) * (sine.atan2(direction.z) / sine)
}

/// The turn from unit `from` to unit `to` as a neck makes it, with no lean of its own: back along
/// `from`'s pitch and yaw, then out along `to`'s. The shortest arc between them would tilt the head
/// on a diagonal, by half the product of the yaw and the pitch, as nothing looking at something does.
fn level_turn(from: Vec3, to: Vec3) -> Quat {
    let angles = |direction: Vec3| {
        (direction.x.atan2(direction.z), direction.y.atan2(Vec2::new(direction.x, direction.z).length()))
    };
    let (from_yaw, from_pitch) = angles(from);
    let (to_yaw, to_pitch) = angles(to);
    (Quat::from_rotation_y(to_yaw)
        * Quat::from_rotation_x(from_pitch - to_pitch)
        * Quat::from_rotation_y(-from_yaw))
    .normalize()
}

/// An eye's yaw (positive toward +X) and pitch (positive up) for the unit `direction` in the head's
/// frame, falling away past `hold` as the head's gaze does.
fn eye_angles(direction: Vec3, hold: f32) -> Vec2 {
    let level = Vec2::new(direction.x, direction.z).length();
    let angle = Vec2::new(direction.x, direction.y).length().atan2(direction.z);
    Vec2::new(direction.x.atan2(direction.z), direction.y.atan2(level)) * falloff(angle, hold)
}

/// `value` moved toward `goal` by at most `step`.
pub(crate) fn approach(value: f32, goal: f32, step: f32) -> f32 {
    if value < goal { (value + step).min(goal) } else { (value - step).max(goal) }
}

/// `current` closing on `desired` over `dt`, by half the distance each `half_life` and never faster
/// than `rate`. A hair's breadth away it arrives, so a settled gaze is exactly settled.
fn follow(current: Vec2, desired: Vec2, dt: f32, half_life: f32, rate: f32) -> Vec2 {
    if dt <= 0.0 {
        return current;
    }
    let left = desired - current;
    if left.length() < 1e-4 {
        return desired;
    }
    let step = left * (1.0 - 0.5f32.powf(dt / half_life.max(1e-4)));
    current + step.clamp_length_max(rate * dt)
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

    use super::*;

    fn settings() -> LookSettings {
        LookSettings {
            yaw_limit: FRAC_PI_2 * 0.75,
            up_limit: 0.5,
            down_limit: 0.7,
            hold: 2.0,
            ..LookSettings::default()
        }
    }

    fn swing_toward(yaw: f32, pitch: f32) -> Vec2 {
        let direction = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-pitch) * Vec3::Z;
        swing_of(direction)
    }

    #[test]
    fn a_swing_is_the_turn_that_gets_there() {
        for (yaw, pitch) in [(0.0, 0.0), (0.5, 0.0), (-1.2, 0.3), (0.2, -0.6), (2.8, 0.4)] {
            let direction = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-pitch) * Vec3::Z;
            let swing = swing_of(direction);
            let back = Quat::from_scaled_axis(Vec3::new(swing.x, swing.y, 0.0)) * Vec3::Z;
            assert!(back.abs_diff_eq(direction, 1e-5), "{yaw} {pitch}: {back} against {direction}");
        }
        assert_eq!(swing_of(Vec3::Z), Vec2::ZERO);
        assert_eq!(swing_of(Vec3::NEG_Z), Vec2::ZERO);
        assert!(swing_toward(FRAC_PI_4, 0.0).y > 0.0, "a positive yaw swings about +Y");
        assert!(swing_toward(0.0, 0.3).x < 0.0, "up swings against +X");
    }

    #[test]
    fn a_level_turn_gets_there_without_leaning() {
        let direction =
            |yaw: f32, pitch: f32| Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-pitch) * Vec3::Z;
        for (from, to) in [((0.0, 0.0), (0.9, 0.3)), ((0.2, -0.1), (-0.7, 0.4)), ((-0.5, 0.2), (-0.5, 0.2))] {
            let (start, end) = (direction(from.0, from.1), direction(to.0, to.1));
            let turn = level_turn(start, end);
            assert!((turn * start).abs_diff_eq(end, 1e-5), "{from:?} to {to:?}");
            // What had no lean, level across, has none after.
            let across = Quat::from_rotation_y(from.0) * Vec3::X;
            assert!((turn * across).y.abs() < 1e-5, "{from:?} to {to:?}: {}", turn * across);
        }
        assert!(level_turn(Vec3::Z, Vec3::Z).abs_diff_eq(Quat::IDENTITY, 1e-6));
    }

    #[test]
    fn limits_hold_each_way() {
        let limits = settings();
        // Inside, nothing changes.
        let within = swing_toward(0.4, 0.1);
        assert!(limits.limited(within).abs_diff_eq(within, 1e-6));
        // Past each limit, it holds there.
        let side = limits.limited(swing_toward(1.8, 0.0));
        assert!((side.y - limits.yaw_limit).abs() < 1e-4 && side.x.abs() < 1e-4, "{side}");
        let side = limits.limited(swing_toward(-1.8, 0.0));
        assert!((side.y + limits.yaw_limit).abs() < 1e-4, "{side}");
        let up = limits.limited(swing_toward(0.0, 1.2));
        assert!((up.x + limits.up_limit).abs() < 1e-4, "{up}");
        let down = limits.limited(swing_toward(0.0, -1.2));
        assert!((down.x - limits.down_limit).abs() < 1e-4, "{down}");
        // Between the axes the limit is an ellipse.
        let both = limits.limited(swing_toward(1.2, 1.2));
        let reach = (both.x / limits.up_limit).powi(2) + (both.y / limits.yaw_limit).powi(2);
        assert!((reach - 1.0).abs() < 1e-4, "{both}: {reach}");
    }

    /// The map from target to gaze has no jump anywhere on the sphere, the point of it: a clamp alone
    /// would jump from one shoulder to the other at dead behind.
    #[test]
    fn the_gaze_is_continuous_all_the_way_round_and_over() {
        let limits = settings();
        let steps = 720;
        for ring in -6..=6 {
            let pitch = ring as f32 * 0.25;
            let at = |yaw: f32| limits.limited(swing_toward(yaw, pitch));
            let mut last = at(-PI);
            for step in 1..=steps {
                let now = at(-PI + step as f32 / steps as f32 * 2.0 * PI);
                assert!(now.distance(last) < 0.05, "pitch {pitch}, step {step}: {last} to {now}");
                last = now;
            }
        }
        assert!(limits.limited(swing_toward(PI - 1e-3, 0.0)).length() < 1e-3, "dead behind: straight on");
    }

    #[test]
    fn following_is_limited_exact_when_settled_and_stands_still_with_no_time() {
        let from = Vec2::new(0.0, -1.0);
        let to = Vec2::new(0.0, 1.0);
        assert_eq!(follow(from, to, 0.0, 0.07, 4.0), from, "no time");
        let step = follow(from, to, 1.0 / 60.0, 0.07, 4.0);
        assert!(step.distance(from) <= 4.0 / 60.0 + 1e-5, "{step}");
        let mut at = from;
        for _ in 0..600 {
            at = follow(at, to, 1.0 / 60.0, 0.07, 4.0);
        }
        assert_eq!(at, to, "it arrives");
        // However finely it's stepped, the same time covers about the same ground.
        let mut fine = from;
        for _ in 0..4 {
            fine = follow(fine, to, 1.0 / 240.0, 0.07, 4.0);
        }
        let coarse = follow(from, to, 1.0 / 60.0, 0.07, 4.0);
        assert!(fine.distance(coarse) < 1e-3, "{fine} against {coarse}");
    }

    #[test]
    fn range_maps_are_vrms() {
        let map = RangeMap { input_max: FRAC_PI_2, output: 10f32.to_radians() };
        assert_eq!(map.map(0.0), 0.0);
        assert!((map.map(FRAC_PI_4) - 5f32.to_radians()).abs() < 1e-6);
        assert!((map.map(3.0) - 10f32.to_radians()).abs() < 1e-6, "larger angles hold it");
        assert_eq!(RangeMap { input_max: 0.0, output: 1.0 }.map(1.0), 0.0);
    }

    #[test]
    fn an_eyes_angles_are_its_yaw_and_pitch() {
        let at = eye_angles(Quat::from_rotation_y(0.3) * Quat::from_rotation_x(-0.2) * Vec3::Z, 2.0);
        assert!(at.abs_diff_eq(Vec2::new(0.3, 0.2), 1e-5), "{at}");
        assert_eq!(eye_angles(Vec3::NEG_Z, 2.0), Vec2::ZERO, "dead behind: straight on");
    }
}
