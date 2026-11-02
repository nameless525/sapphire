//! The rigs' geometry: where each looks, and how far back a framing rig must stand. Pure functions,
//! so a shot can be checked by hand.

use std::f32::consts::PI;

use sapphire_math::{Vec2, Vec3};

use crate::tuning::FramingTuning;

/// The rigs, bottom of the stack first. [`Self::Explore`] is always on; each rig above it blends in
/// over what's below with its own weight, so turning one on or off is never a jump. See the crate
/// docs for what each does with look input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rig {
    /// Follows the player.
    Explore,
    /// Frames the player and the target, with look input free inside what keeps the target in frame.
    Combat,
    /// Holds the line from player to target.
    LockOn,
    /// A sequencer's authored shot (M3), given as a pose: see
    /// [`CameraDirector::set_takeover`](crate::CameraDirector::set_takeover).
    Takeover,
}

impl Rig {
    /// How many there are.
    pub(crate) const COUNT: usize = 4;
}

/// Someone the camera frames, in the world's metres at the frame's interpolated time: the app reads
/// these from its transforms, so this crate has no edge to gameplay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Subject {
    /// Where their feet are.
    pub position: Vec3,
    /// How tall they stand.
    pub height: f32,
    /// How wide they are from their middle.
    pub radius: f32,
}

impl Subject {
    pub(crate) fn is_valid(&self) -> bool {
        self.position.is_finite() && self.height.is_finite() && self.radius.is_finite()
    }

    /// The point `share` of the way up them.
    pub(crate) fn at_height(&self, share: f32) -> Vec3 {
        self.position + Vec3::Y * (self.height * share)
    }
}

/// A view's axes, world space. Like every object, a camera faces its local +Z.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Axes {
    pub(crate) forward: Vec3,
    pub(crate) right: Vec3,
    pub(crate) up: Vec3,
}

impl Axes {
    /// Looking along `yaw` (from +Z toward +X), down by `pitch`.
    pub(crate) fn of(yaw: f32, pitch: f32) -> Self {
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let forward = Vec3::new(sin_yaw * cos_pitch, -sin_pitch, cos_yaw * cos_pitch);
        let right = Vec3::new(-cos_yaw, 0.0, sin_yaw);
        Self { forward, right, up: right.cross(forward) }
    }
}

/// Where a framing rig looks: from the player toward the target by the tuning's bias, led at most the
/// tuning's distances.
pub(crate) fn framed_pivot(tuning: &FramingTuning, player: Vec3, target: Vec3) -> Vec3 {
    let lead = target - player;
    let across = (Vec2::new(lead.x, lead.z) * tuning.pivot_bias).clamp_length_max(tuning.max_lead);
    let rise = (lead.y * tuning.pivot_bias).clamp(-tuning.max_rise, tuning.max_rise);
    player + Vec3::new(across.x, rise, across.y)
}

/// How far the view's yaw may stand off the line toward a target `reach` metres past the pivot, and still
/// fit it from `distance` behind the pivot, in a frame `tan_x` wide (the tangent of its half-angle) and
/// with `depth` metres' clearance, what its height takes at the frame's height. A target that fits from
/// anywhere has no limit: π.
///
/// With the target at angle `a` off the view, it's at `reach·sin a` across and `distance + reach·cos a` deep.
/// Keeping across within `tan_x` of deep, with `tan_x = tan b`, comes to `sin(a − b) ≤ sin b · distance / reach`.
/// So a far target leaves just the frame's own half-angle. Keeping deep past `depth` comes to
/// `cos a ≥ (depth − distance) / reach`.
pub(crate) fn yaw_limit(tan_x: f32, depth: f32, distance: f32, reach: f32) -> f32 {
    let half_angle = tan_x.atan();
    let reach = reach.max(1e-3);
    let side = half_angle.sin() * distance / reach;
    let across = if side >= 1.0 { PI } else { half_angle + side.asin() };
    let deep = ((depth - distance) / reach).clamp(-1.0, 1.0).acos();
    across.min(deep)
}

/// How far behind `pivot` a view along `axes` must stand to fit both subjects' feet and heads inside
/// its safe frame, each no nearer than `min_depth`: the largest of what each point asks.
///
/// For a point at `x`, `y`, `z` from the pivot, in the view's axes, standing `distance` back puts it at
/// depth `z + distance`, so fitting across takes `|x| ≤ tan_x · (z + distance)` and up and down likewise.
pub(crate) fn framed_distance(
    subjects: [&Subject; 2],
    pivot: Vec3,
    axes: &Axes,
    tan_x: f32,
    tan_y: f32,
    min_depth: f32,
) -> f32 {
    let mut distance = f32::MIN;
    for subject in subjects {
        for point in [subject.position, subject.position + Vec3::Y * subject.height] {
            let offset = point - pivot;
            let (x, y, z) = (offset.dot(axes.right), offset.dot(axes.up), offset.dot(axes.forward));
            let depth = ((x.abs() + subject.radius) / tan_x).max(y.abs() / tan_y).max(min_depth);
            distance = distance.max(depth - z);
        }
    }
    distance
}

/// 0 at or below `from`, 1 at or above `to`, smooth between.
pub(crate) fn smoothstep(from: f32, to: f32, value: f32) -> f32 {
    let t = ((value - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CameraTuning;

    fn project(point: Vec3, eye: Vec3, axes: &Axes) -> (f32, f32, f32) {
        let offset = point - eye;
        let depth = offset.dot(axes.forward);
        (offset.dot(axes.right) / depth, offset.dot(axes.up) / depth, depth)
    }

    #[test]
    fn axes_follow_the_engines_convention() {
        let level = Axes::of(0.0, 0.0);
        assert!(level.forward.abs_diff_eq(Vec3::Z, 1e-6));
        assert!(level.right.abs_diff_eq(Vec3::NEG_X, 1e-6));
        assert!(level.up.abs_diff_eq(Vec3::Y, 1e-6));

        let turned = Axes::of(std::f32::consts::FRAC_PI_2, 0.3);
        assert!(turned.forward.x > 0.9 && turned.forward.y < 0.0, "it looks along +X and down");
        assert!(turned.right.abs_diff_eq(Vec3::Z, 1e-6));
        assert!(turned.up.y > 0.9);
    }

    /// The distance asked for puts the tightest point exactly on the frame's edge.
    #[test]
    fn framed_distance_is_the_least_that_fits() {
        let player = Subject { position: Vec3::ZERO, height: 1.7, radius: 0.3 };
        let target = Subject { position: Vec3::new(5.0, 0.0, 4.0), height: 2.1, radius: 0.5 };
        let pivot = Vec3::new(1.5, 1.2, 1.5);
        let axes = Axes::of(0.4, 0.2);
        let (tan_x, tan_y) = (0.6, 0.35);

        let distance = framed_distance([&player, &target], pivot, &axes, tan_x, tan_y, 0.5);
        let eye = pivot - axes.forward * distance;
        let mut tightest = 0.0_f32;
        for subject in [&player, &target] {
            for point in [subject.position, subject.position + Vec3::Y * subject.height] {
                let (x, y, depth) = project(point, eye, &axes);
                let across = x.abs() + subject.radius / depth;
                assert!(across <= tan_x + 1e-4 && y.abs() <= tan_y + 1e-4, "{across}, {y} from {eye}");
                tightest = tightest.max(across / tan_x).max(y.abs() / tan_y);
            }
        }
        assert!((tightest - 1.0).abs() < 1e-3, "{tightest}: a closer camera would fit too");
    }

    /// At the limit, a far target is on the frame's edge, and a near one has none.
    #[test]
    fn the_yaw_limit_puts_a_far_target_on_the_frames_edge() {
        let (tan_x, distance) = (0.55, 5.5);
        for reach in [8.0_f32, 12.0, 20.0, 60.0] {
            let limit = yaw_limit(tan_x, 0.0, distance, reach);
            let across = reach * limit.sin();
            let deep = distance + reach * limit.cos();
            assert!((across - tan_x * deep).abs() < 1e-3, "{reach}: {across} vs {}", tan_x * deep);
        }
        assert_eq!(yaw_limit(tan_x, 0.0, distance, 1.0), PI, "close enough to fit from anywhere");
        let far = yaw_limit(tan_x, 0.0, distance, 1e6);
        assert!((far - tan_x.atan()).abs() < 1e-3, "{far}");
    }

    /// At the limit from depth, the target stands exactly that deep.
    #[test]
    fn the_yaw_limit_keeps_a_target_clear_of_the_camera() {
        let (depth, distance, reach) = (5.0, 6.0, 3.0);
        let limit = yaw_limit(10.0, depth, distance, reach);
        assert!((distance + reach * limit.cos() - depth).abs() < 1e-3, "{limit}");
        assert_eq!(yaw_limit(10.0, 2.0, distance, reach), PI, "it can't get that close");
        assert_eq!(yaw_limit(10.0, 20.0, distance, reach), 0.0, "it needs to be dead ahead");
    }

    #[test]
    fn the_pivot_leads_the_player_only_so_far() {
        let tuning = CameraTuning::default_tuning().combat.framing;
        let player = Vec3::new(1.0, 1.4, 1.0);
        let near = framed_pivot(&tuning, player, player + Vec3::Z * 2.0);
        assert!(near.abs_diff_eq(player + Vec3::Z * (2.0 * tuning.pivot_bias), 1e-5), "{near}");
        let far = framed_pivot(&tuning, player, player + Vec3::new(40.0, 40.0, 0.0));
        assert!((far.x - player.x - tuning.max_lead).abs() < 1e-4, "{far}");
        assert!((far.y - player.y - tuning.max_rise).abs() < 1e-4, "{far}");
    }
}
