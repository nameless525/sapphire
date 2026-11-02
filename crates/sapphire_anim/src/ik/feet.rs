//! Foot placement: both feet on the ground under them, with the hips dropped so the lower one reaches.
//!
//! A clip's feet stand on a flat floor at the model's feet. The ground under each one is somewhere
//! else: a slope falls away, a step rises, and the character controller's round foot hovers over any
//! slope by `radius × (1/cos θ − 1)`, 4.6 cm at 30° (M2 G1). So:
//!
//! - **The ground comes in as data.** The caller casts where [`FootProbe`]s say and hands over what
//!   it hit, in the world, so this crate has no physics edge and runs in `evaluate_all`'s jobs.
//!   Under each foot it chooses between what's under the ankle and what's under the ball of the foot
//!   ([`FootGround`]): whichever plane the other's point isn't above, so a foot facing a step stands
//!   on it, and one overhanging a step's edge stays on top.
//! - **That plane is smoothed, in the world.** A plane holds still however a foot crosses it, so a
//!   swinging foot over a constant slope doesn't lag it, and a planted foot stays put when the root
//!   pops up a step. Only a change of ground eases in.
//! - **Each ankle goes where the clip had it over a flat floor, moved with the ground.** What the clip
//!   lifted a foot by, it keeps. A planted sole tilts to the ground about the ankle, which then has to
//!   stand `ankle / cos θ` above the plane, not `ankle`; a foot lifting off lets the tilt go by
//!   [`FootSettings::plant_height`].
//! - **The hips drop by the lower foot's move,** and rise only as far as [`FootSettings::max_raise`]
//!   when both feet want up, and drop further if a leg still can't reach. Their height eases in the
//!   model's space, but a root that jumps ([`FootSettings::pop_speed`]) is taken back out of it, so
//!   a step-up shows on screen as the feet alone. Then two-bone solves
//!   ([`two_bone`]) keep each knee's plane from the clip.
//! - **A weight fades it all,** [`FeetInput::weight`]: a state or move says 0 in the air or while it
//!   wants its feet as animated, and it eases off over [`FootSettings::fade`]. With no ground under a
//!   foot, that foot alone eases back to its animation.
//!
//! The model's +Y is taken to be the world's up, with the character upright and scaled uniformly.

use sapphire_math::{Affine3A, Quat, Vec3};

use crate::ik::look::approach;
use crate::ik::solve::{SolvePose, TwoBone, TwoBoneGoal, rotation_of, two_bone, weighted};

/// How foot placement behaves. Lengths in metres, times in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FootSettings {
    /// How far above and below an ankle the caller should look for ground.
    pub reach_up: f32,
    pub reach_down: f32,
    /// Ground steeper than this isn't ground: its normal's Y is at least this.
    pub min_normal_y: f32,
    /// How far a sole tilts to the ground, radians.
    pub max_tilt: f32,
    /// How far above its place on the floor a clip's ankle has lifted when the sole stops following
    /// the ground's tilt.
    pub plant_height: f32,
    pub max_drop: f32,
    pub max_raise: f32,
    /// How quickly a foot takes up a change of ground, and the hips a change of height.
    pub ground_half_life: f32,
    pub hips_half_life: f32,
    /// How long a weight takes to go from 0 to 1.
    pub fade: f32,
    /// A root rising or falling faster than this, metres a second, has jumped rather than moved, as
    /// the controller's step-up does, and the hips stay where they were on screen. Over what a run up
    /// the steepest slope can make.
    pub pop_speed: f32,
    /// See [`TwoBoneGoal::soften`].
    pub soften: f32,
}

impl Default for FootSettings {
    fn default() -> Self {
        Self {
            reach_up: 0.5,
            reach_down: 0.4,
            min_normal_y: 0.5,
            max_tilt: 40f32.to_radians(),
            plant_height: 0.12,
            max_drop: 0.3,
            max_raise: 0.08,
            ground_half_life: 0.04,
            hips_half_life: 0.06,
            fade: 0.12,
            pop_speed: 5.5,
            soften: 0.985,
        }
    }
}

/// A leg's joints.
#[derive(Clone, Debug, PartialEq)]
pub struct LegBones {
    pub chain: TwoBone,
    pub toes: Option<u16>,
    /// The upper leg's place down to the foot, every joint between, parents first.
    pub path: Vec<u16>,
    /// The foot's rest rotation in the model's space, which its forward turns from.
    pub rest_foot: Quat,
    /// How far the ankle stands above the floor at rest.
    pub ankle_height: f32,
}

/// Where to look for ground under a foot, in the world: from each of these, look from
/// [`FootSettings::reach_up`] above to [`FootSettings::reach_down`] below.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FootProbe {
    pub ankle: Vec3,
    /// The ball of the foot, for a rig with toes.
    pub ball: Option<Vec3>,
}

/// A place on the ground, in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundHit {
    pub point: Vec3,
    /// Up out of the ground, unit length.
    pub normal: Vec3,
}

/// What a caller found under one foot's [`FootProbe`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FootGround {
    pub ankle: Option<GroundHit>,
    pub ball: Option<GroundHit>,
}

/// What foot placement takes each frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FeetInput {
    /// 0 lets the feet go as animated, 1 grounds them. See the `feet` module's docs.
    pub weight: f32,
    /// Left, then right.
    pub ground: [FootGround; 2],
}

/// A plane of the world: where `normal · p = offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Plane {
    normal: Vec3,
    offset: f32,
}

impl Plane {
    fn of(hit: GroundHit) -> Self {
        Self { normal: hit.normal, offset: hit.normal.dot(hit.point) }
    }

    /// The plane's height over `at`'s place on the ground.
    fn height_at(self, at: Vec3) -> f32 {
        (self.offset - self.normal.x * at.x - self.normal.z * at.z) / self.normal.y
    }
}

/// The plane a foot stands on: see the [module docs](self).
fn supporting(ground: &FootGround, probe: &FootProbe, min_normal_y: f32) -> Option<Plane> {
    let usable = |hit: Option<GroundHit>| hit.filter(|hit| hit.normal.y >= min_normal_y).map(Plane::of);
    match (usable(ground.ankle), usable(ground.ball), probe.ball) {
        (Some(ankle), Some(ball), Some(place)) => {
            // A hair of slack, so one plane seen twice is never told apart by rounding.
            if ankle.height_at(place) >= ball.height_at(place) - 1e-3 { Some(ankle) } else { Some(ball) }
        }
        (Some(ankle), _, _) => Some(ankle),
        (None, ball, _) => ball,
    }
}

/// One foot's state.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FootState {
    plane: Plane,
    /// How much of the ground this foot follows: it eases out where nothing is under it.
    presence: f32,
    /// What it's closing on, while there is ground.
    goal: Option<Plane>,
}

const FLAT: Plane = Plane { normal: Vec3::Y, offset: 0.0 };

/// One character's foot placement. See the `feet` module's docs.
#[derive(Clone, Debug, PartialEq)]
pub struct FootPlacement {
    legs: [LegBones; 2],
    hips: u16,
    /// The floor a clip's feet stand on, in the model's space.
    floor: f32,
    settings: FootSettings,
    feet: [FootState; 2],
    /// What's changed the hips' height, in the model's space.
    drop: f32,
    drop_goal: f32,
    /// The root's height last frame, in the world, to tell a root that jumped.
    root_height: f32,
    fade: f32,
    fade_goal: f32,
    /// Whether anything has been seen since a [`Self::reset`]: the first ground is taken at once.
    started: bool,
}

impl FootPlacement {
    #[must_use]
    pub fn new(legs: [LegBones; 2], hips: u16, floor: f32, settings: FootSettings) -> Self {
        let foot = FootState { plane: FLAT, presence: 0.0, goal: None };
        Self {
            legs,
            hips,
            floor,
            settings,
            feet: [foot; 2],
            drop: 0.0,
            drop_goal: 0.0,
            root_height: 0.0,
            fade: 0.0,
            fade_goal: 0.0,
            started: false,
        }
    }

    /// The settings it runs by, for whoever casts for it.
    #[must_use]
    pub fn settings(&self) -> &FootSettings {
        &self.settings
    }

    /// The share of its placement the character is showing, 0 to 1.
    #[must_use]
    pub fn fade(&self) -> f32 {
        self.fade
    }

    /// Whether another step would move nothing, for the same ground.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.fade == self.fade_goal
            && if self.fade == 0.0 {
                self.drop == 0.0
            } else {
                self.drop == self.drop_goal
                    && self.feet.iter().all(|foot| match foot.goal {
                        Some(goal) => foot.presence == 1.0 && foot.plane == goal,
                        None => foot.presence == 0.0,
                    })
            }
    }

    /// Forgets the ground, for a cut or a teleport: what's under the feet next is taken at once.
    pub fn reset(&mut self) {
        self.started = false;
    }

    /// Where a caller should look for ground under each foot. `pose` must be up to date for the feet
    /// and toes.
    #[must_use]
    pub fn probes(&self, pose: &SolvePose<'_>, world_from_model: &Affine3A) -> [FootProbe; 2] {
        let probe = |leg: &LegBones| FootProbe {
            ankle: world_from_model.transform_point3(pose.position(leg.chain.end)),
            ball: leg.toes.map(|toes| world_from_model.transform_point3(pose.position(usize::from(toes)))),
        };
        [probe(&self.legs[0]), probe(&self.legs[1])]
    }

    /// Grounds the feet, by `dt` seconds of the owner's clock. `pose` must be up to date for the hips
    /// and legs; `reads` is every joint the solvers read, parents first, which it refreshes if the hips
    /// move. Allocates nothing.
    pub fn solve(
        &mut self,
        pose: &mut SolvePose<'_>,
        reads: &[u16],
        input: &FeetInput,
        world_from_model: &Affine3A,
        dt: f32,
    ) {
        self.fade_goal = input.weight.clamp(0.0, 1.0);
        // The hips ease more slowly than the fade goes, so they outlast it: off means they're home too.
        if self.fade == 0.0 && self.fade_goal == 0.0 {
            if !self.started {
                self.drop = 0.0;
            }
            if self.drop == 0.0 {
                return;
            }
        }
        // A character just started stands on the ground at once. One fading back in has been off, so
        // it takes what's there now rather than easing from wherever it left the ground, while the
        // fade itself still eases.
        let fresh = !self.started;
        let snap = fresh || (self.fade == 0.0 && self.fade_goal > 0.0);
        self.started = true;
        self.fade = if fresh {
            self.fade_goal
        } else {
            approach(self.fade, self.fade_goal, dt / self.settings.fade.max(1e-3))
        };
        let probes = self.probes(pose, world_from_model);
        let scale = world_from_model.matrix3.y_axis.length().max(1e-6);
        let model_turn = rotation_of(world_from_model).inverse();
        let ease = 1.0 - 0.5f32.powf(dt / self.settings.ground_half_life.max(1e-4));

        // Where each foot wants its ankle, as a move up or down from where the clip has it, and how much
        // its sole tilts.
        let mut moves = [0.0f32; 2];
        let mut tilts = [Quat::IDENTITY; 2];
        for (side, leg) in self.legs.iter().enumerate() {
            let foot = &mut self.feet[side];
            foot.goal = supporting(&input.ground[side], &probes[side], self.settings.min_normal_y);
            if let Some(goal) = foot.goal {
                foot.plane =
                    if snap || foot.presence == 0.0 { goal } else { ease_plane(foot.plane, goal, ease) };
                foot.presence = if snap { 1.0 } else { ease_to(foot.presence, 1.0, ease) };
            } else {
                foot.presence = if snap { 0.0 } else { ease_to(foot.presence, 0.0, ease) };
            }
            if foot.presence == 0.0 || self.fade == 0.0 {
                continue;
            }
            let ankle = pose.position(leg.chain.end);
            let floor_height = world_from_model.translation.y + self.floor * scale;
            let ground = (foot.plane.height_at(probes[side].ankle) - floor_height) / scale + self.floor;
            let lift = ankle.y - self.floor - leg.ankle_height;
            let planted = 1.0 - smoothstep(lift / self.settings.plant_height.max(1e-3));
            let cosine = foot.plane.normal.y.max(self.settings.max_tilt.cos()).max(1e-3);
            let wanted = ground + leg.ankle_height + lift + leg.ankle_height * (1.0 / cosine - 1.0) * planted;
            let share = foot.presence * self.fade;
            moves[side] = (wanted - ankle.y) * share;
            tilts[side] =
                weighted(tilt_to(model_turn * foot.plane.normal, self.settings.max_tilt), planted * share);
        }

        // The hips go down with the lower foot, easing, and at once as far as a leg can't otherwise
        // reach: a foot short of the ground shows more than a hip that moved a little quicker.
        let lowest = -self.settings.max_drop;
        let mut reachable = f32::MAX;
        for (side, leg) in self.legs.iter().enumerate() {
            let hip = pose.position(leg.chain.root);
            let target = pose.position(leg.chain.end) + Vec3::Y * moves[side];
            let length = pose.position(leg.chain.mid).distance(hip)
                + pose.position(leg.chain.end).distance(pose.position(leg.chain.mid));
            let across = (hip.x - target.x).hypot(hip.z - target.z);
            let room = (length * length - across * across).max(0.0).sqrt();
            // A hair of slack: a leg the clip has straight, on flat ground, is no reason to move.
            reachable = reachable.min(room + target.y - hip.y + 1e-4);
        }
        self.drop_goal = moves[0]
            .min(moves[1])
            .clamp(-self.settings.max_drop, self.settings.max_raise)
            .min(reachable)
            .max(lowest);
        // A root that jumped, as the controller's step-up does, moves nothing on screen.
        let root_height = world_from_model.translation.y;
        let rise = root_height - self.root_height;
        self.root_height = root_height;
        if !snap && dt > 0.0 && rise.abs() > self.settings.pop_speed * dt {
            self.drop = (self.drop - rise / scale).clamp(-self.settings.max_drop, self.settings.max_raise);
        }
        self.drop = if fresh || (snap && self.drop == 0.0) {
            self.drop_goal
        } else {
            let ease = 1.0 - 0.5f32.powf(dt / self.settings.hips_half_life.max(1e-4));
            let eased = self.drop + (self.drop_goal - self.drop) * ease;
            let eased = if (self.drop_goal - eased).abs() < 1e-4 { self.drop_goal } else { eased };
            eased.min(reachable).max(lowest)
        };
        if self.drop != 0.0 {
            pose.translate(usize::from(self.hips), Vec3::Y * self.drop);
            pose.refresh(reads);
        }

        for (side, leg) in self.legs.iter().enumerate() {
            let left = moves[side] - self.drop;
            let tilted = tilts[side] != Quat::IDENTITY;
            if left.abs() < 1e-5 && !tilted {
                continue;
            }
            let rotation = pose.rotation(leg.chain.end);
            let goal = TwoBoneGoal {
                target: pose.position(leg.chain.end) + Vec3::Y * left,
                pole: None,
                hint: rotation * leg.rest_foot.inverse() * Vec3::Z,
                soften: self.settings.soften,
                weight: 1.0,
            };
            two_bone(pose, leg.chain, &goal);
            pose.refresh(&leg.path);
            pose.set_rotation(leg.chain.end, tilts[side] * rotation);
        }
    }
}

/// `value` eased toward `goal`, arriving at a hair's breadth so a settled state is exactly settled.
fn ease_to(value: f32, goal: f32, ease: f32) -> f32 {
    let eased = value + (goal - value) * ease;
    if (goal - eased).abs() < 1e-3 { goal } else { eased }
}

fn ease_plane(plane: Plane, goal: Plane, ease: f32) -> Plane {
    if plane.normal.abs_diff_eq(goal.normal, 1e-4) && (plane.offset - goal.offset).abs() < 1e-4 {
        return goal;
    }
    Plane {
        normal: plane.normal.lerp(goal.normal, ease).normalize_or(goal.normal),
        offset: plane.offset + (goal.offset - plane.offset) * ease,
    }
}

/// The turn from up to `normal`, no more than `max` radians of it.
fn tilt_to(normal: Vec3, max: f32) -> Quat {
    let turn = Quat::from_rotation_arc(Vec3::Y, normal);
    let (axis, angle) = turn.to_axis_angle();
    if angle < 1e-5 { Quat::IDENTITY } else { Quat::from_axis_angle(axis, angle.min(max)) }
}

/// 0 at or under 0, 1 at or over 1, smoothly between.
fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(point: Vec3, normal: Vec3) -> Option<GroundHit> {
        Some(GroundHit { point, normal: normal.normalize() })
    }

    #[test]
    fn a_planes_height_is_its_height() {
        let slope = Plane::of(GroundHit {
            point: Vec3::new(1.0, 2.0, 3.0),
            normal: Vec3::new(-0.5, 1.0, 0.0).normalize(),
        });
        assert!((slope.height_at(Vec3::new(1.0, 99.0, -4.0)) - 2.0).abs() < 1e-5);
        assert!((slope.height_at(Vec3::new(2.0, 0.0, 3.0)) - 2.5).abs() < 1e-5);
    }

    /// A foot facing a step stands on it, a foot overhanging an edge stays on top, and a slope seen
    /// from both ends of a foot is one slope.
    #[test]
    fn a_foot_stands_on_the_plane_it_cant_go_through() {
        let probe = FootProbe { ankle: Vec3::ZERO, ball: Some(Vec3::new(0.0, 0.0, 0.2)) };
        let low = hit(Vec3::ZERO, Vec3::Y);
        let high = hit(Vec3::new(0.0, 0.15, 0.2), Vec3::Y);
        let up_a_step = supporting(&FootGround { ankle: low, ball: high }, &probe, 0.5).expect("ground");
        assert!((up_a_step.height_at(probe.ankle) - 0.15).abs() < 1e-5, "{up_a_step:?}");
        let off_a_step = supporting(
            &FootGround {
                ankle: hit(Vec3::new(0.0, 0.15, 0.0), Vec3::Y),
                ball: hit(Vec3::new(0.0, 0.0, 0.2), Vec3::Y),
            },
            &probe,
            0.5,
        )
        .expect("ground");
        assert!((off_a_step.height_at(probe.ankle) - 0.15).abs() < 1e-5, "{off_a_step:?}");
        let normal = Vec3::new(0.0, 1.0, -0.5).normalize();
        let slope = |z: f32| Vec3::new(0.0, 0.5 * z, z);
        let on_a_slope = supporting(
            &FootGround { ankle: hit(slope(0.0), normal), ball: hit(slope(0.2), normal) },
            &probe,
            0.5,
        )
        .expect("ground");
        assert!(on_a_slope.height_at(probe.ankle).abs() < 1e-5, "{on_a_slope:?}");
    }

    #[test]
    fn nothing_steep_counts_as_ground() {
        let probe = FootProbe { ankle: Vec3::ZERO, ball: None };
        let wall = hit(Vec3::ZERO, Vec3::new(1.0, 0.2, 0.0));
        assert_eq!(supporting(&FootGround { ankle: wall, ball: None }, &probe, 0.5), None);
        assert_eq!(supporting(&FootGround::default(), &probe, 0.5), None);
    }
}
