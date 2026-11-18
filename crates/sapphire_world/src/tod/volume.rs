//! Zone override volumes: where a zone's timeline takes over from the base time of day.

use std::sync::Arc;

use sapphire_math::{Ease, Quat, Vec3};
use sapphire_tod::TodTimeline;
use sapphire_zone::ZoneShape;

/// A zone's time-of-day override (WuWa gives every scene its own atmosphere and grade): inside
/// its shape the zone's timeline blends over the base at full `strength`, and the blend fades
/// out over `blend_distance` meters around it, by the distance of a probe point (the camera, or
/// the player) from the shape.
///
/// The zone's timeline is evaluated at the same time of day as the base. It may set every group
/// (a zone with a day of its own: a cave, an interior) or only some (a valley's fog and
/// ambient), which leaves the base's other groups as they are.
#[derive(Clone, Debug)]
pub struct TodVolume {
    pub shape: VolumeShape,
    /// Meters outside the shape over which the override fades out: 0 for a hard edge.
    pub blend_distance: f32,
    /// Volumes apply from the lowest priority to the highest, each over what the ones before it
    /// left; equal priorities apply in the order they were added.
    pub priority: i32,
    /// The override's weight inside the shape, 0 to 1: below 1, the zone tints the base rather
    /// than replacing it.
    pub strength: f32,
    pub timeline: Arc<TodTimeline>,
}

/// Where a volume of air is, in world space: a [`TodVolume`], or a weather region (M6 W8a). A zone places
/// one by its [`ZoneShape`] ([`Self::from_zone`]); a yaw turns a box or an ellipsoid about +Y, in radians,
/// counterclockwise seen from above (+X toward −Z).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VolumeShape {
    Sphere {
        center: Vec3,
        radius: f32,
    },
    Box {
        center: Vec3,
        half_extents: Vec3,
        yaw: f32,
    },
    /// Its half-axes along its own X, Y and Z.
    Ellipsoid {
        center: Vec3,
        radii: Vec3,
        yaw: f32,
    },
}

impl VolumeShape {
    /// `shape` about `position`, as a zone places it, its yaw from degrees to radians.
    #[must_use]
    pub fn from_zone(position: [f32; 3], shape: ZoneShape) -> Self {
        let center = Vec3::from(position);
        match shape {
            ZoneShape::Sphere { radius } => Self::Sphere { center, radius },
            ZoneShape::Box { half_extents, yaw } => {
                Self::Box { center, half_extents: half_extents.into(), yaw: yaw.to_radians() }
            }
            ZoneShape::Ellipsoid { radii, yaw } => {
                Self::Ellipsoid { center, radii: radii.into(), yaw: yaw.to_radians() }
            }
        }
    }

    /// Meters from `point` to the shape: 0 inside it, and infinitely far if the point or the
    /// shape isn't finite (`max` would otherwise turn NaN into "inside"). An ellipsoid's is
    /// close to the true distance, exact along its axes and never more than it.
    pub fn distance(&self, point: Vec3) -> f32 {
        // The point in a turned shape's own frame.
        let local = |center: Vec3, yaw: f32| Quat::from_rotation_y(-yaw) * (point - center);
        let (distance, finite) = match *self {
            Self::Sphere { center, radius } => (
                (point.distance(center) - radius.max(0.0)).max(0.0),
                center.is_finite() && radius.is_finite(),
            ),
            Self::Box { center, half_extents, yaw } => (
                (local(center, yaw).abs() - half_extents.max(Vec3::ZERO)).max(Vec3::ZERO).length(),
                center.is_finite() && half_extents.is_finite() && yaw.is_finite(),
            ),
            Self::Ellipsoid { center, radii, yaw } => (
                ellipsoid_distance(local(center, yaw), radii),
                center.is_finite() && radii.is_finite() && radii.min_element() > 0.0 && yaw.is_finite(),
            ),
        };
        if finite && point.is_finite() { distance } else { f32::INFINITY }
    }
}

/// How far `point` lies outside the ellipsoid of `radii` about the origin: 0 inside, and outside the
/// first-order estimate `k0 (k0 - 1) / k1`, `k0` the point's length in units of the radii and `k1` in
/// units of their squares, which is exact along the axes and short of the true distance elsewhere.
fn ellipsoid_distance(point: Vec3, radii: Vec3) -> f32 {
    let k0 = (point / radii).length();
    if k0 <= 1.0 {
        return 0.0;
    }
    let k1 = (point / (radii * radii)).length();
    (k0 * (k0 - 1.0) / k1).max(0.0)
}

impl TodVolume {
    /// A volume with a hard edge, at full strength and priority 0.
    pub fn new(shape: VolumeShape, timeline: Arc<TodTimeline>) -> Self {
        Self { shape, blend_distance: 0.0, priority: 0, strength: 1.0, timeline }
    }

    /// The override's weight at `point`: `strength` inside the shape, easing smoothly down to 0
    /// at `blend_distance` outside it.
    pub fn weight_at(&self, point: Vec3) -> f32 {
        let strength = if self.strength.is_finite() { self.strength.clamp(0.0, 1.0) } else { 0.0 };
        let distance = self.shape.distance(point);
        let fade = if distance <= 0.0 {
            1.0
        } else if self.blend_distance > 0.0 {
            1.0 - Ease::Smooth.apply(distance / self.blend_distance)
        } else {
            0.0
        };
        strength * fade
    }
}

/// Names a volume added to a [`TimeOfDay`](super::TimeOfDay), to remove it later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TodVolumeId(pub(crate) u32);

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(shape: VolumeShape, blend_distance: f32) -> TodVolume {
        TodVolume { blend_distance, ..TodVolume::new(shape, Arc::new(crate::tod::default_day())) }
    }

    #[test]
    fn distances_are_zero_inside() {
        let sphere = VolumeShape::Sphere { center: Vec3::new(1.0, 0.0, 0.0), radius: 2.0 };
        assert_eq!(sphere.distance(Vec3::new(2.0, 0.0, 0.0)), 0.0);
        assert_eq!(sphere.distance(Vec3::new(6.0, 0.0, 0.0)), 3.0);
        let cube = VolumeShape::Box { center: Vec3::ZERO, half_extents: Vec3::new(1.0, 2.0, 3.0), yaw: 0.0 };
        assert_eq!(cube.distance(Vec3::new(0.5, -1.5, 2.5)), 0.0);
        assert_eq!(cube.distance(Vec3::new(4.0, 0.0, 0.0)), 3.0);
        assert_eq!(cube.distance(Vec3::new(4.0, 6.0, 0.0)), 5.0, "to the corner region: a 3-4-5 triangle");
    }

    /// A zone's shapes, turned: a box a quarter turn round has swapped its X and Z, an ellipsoid measures
    /// along its own axes, and both are 0 inside.
    #[test]
    fn turned_shapes_measure_in_their_own_frames() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        let long = VolumeShape::from_zone(
            [10.0, 0.0, 0.0],
            ZoneShape::Box { half_extents: [4.0, 1.0, 1.0], yaw: 90.0 },
        );
        // A quarter turn counterclockwise from above takes its long +X side toward −Z.
        assert_eq!(long.distance(Vec3::new(10.0, 0.0, -3.5)), 0.0);
        assert!(
            close(long.distance(Vec3::new(13.0, 0.0, 0.0)), 2.0),
            "{}",
            long.distance(Vec3::new(13.0, 0.0, 0.0))
        );
        assert!(close(long.distance(Vec3::new(10.0, 0.0, 6.0)), 2.0));
        let egg = VolumeShape::from_zone(
            [0.0, 5.0, 0.0],
            ZoneShape::Ellipsoid { radii: [6.0, 2.0, 3.0], yaw: 0.0 },
        );
        assert_eq!(egg.distance(Vec3::new(5.0, 5.0, 0.0)), 0.0);
        assert!(close(egg.distance(Vec3::new(10.0, 5.0, 0.0)), 4.0), "exact along an axis");
        assert!(close(egg.distance(Vec3::new(0.0, 9.0, 0.0)), 2.0));
        assert!(close(egg.distance(Vec3::new(0.0, 5.0, -5.0)), 2.0));
        let off_axis = egg.distance(Vec3::new(6.0, 7.0, 3.0));
        assert!(off_axis > 0.0 && off_axis < Vec3::new(6.0, 2.0, 3.0).length(), "{off_axis}");
        let turned = VolumeShape::from_zone(
            [0.0, 5.0, 0.0],
            ZoneShape::Ellipsoid { radii: [6.0, 2.0, 3.0], yaw: 90.0 },
        );
        assert!(close(turned.distance(Vec3::new(0.0, 5.0, -10.0)), 4.0), "its long axis along −Z now");
        assert_eq!(
            VolumeShape::from_zone([0.0; 3], ZoneShape::Sphere { radius: 2.0 }).distance(Vec3::X),
            0.0
        );
        let broken = VolumeShape::Ellipsoid { center: Vec3::ZERO, radii: Vec3::new(1.0, 0.0, 1.0), yaw: 0.0 };
        assert_eq!(broken.distance(Vec3::ZERO), f32::INFINITY, "a flat ellipsoid holds nothing");
    }

    #[test]
    fn weight_eases_out_over_the_blend_distance() {
        let shape = VolumeShape::Sphere { center: Vec3::ZERO, radius: 10.0 };
        let soft = volume(shape, 4.0);
        let at = |x: f32| soft.weight_at(Vec3::new(x, 0.0, 0.0));
        assert_eq!(at(0.0), 1.0);
        assert_eq!(at(10.0), 1.0, "on the surface");
        assert_eq!(at(12.0), 0.5, "halfway out");
        assert!(at(11.0) > 0.8 && at(13.0) < 0.2, "smooth at both ends");
        assert_eq!(at(14.0), 0.0);
        assert_eq!(at(40.0), 0.0);
        let hard = volume(shape, 0.0);
        assert_eq!(hard.weight_at(Vec3::new(9.9, 0.0, 0.0)), 1.0);
        assert_eq!(hard.weight_at(Vec3::new(10.1, 0.0, 0.0)), 0.0);
        let tint = TodVolume { strength: 0.4, ..volume(shape, 4.0) };
        assert_eq!(tint.weight_at(Vec3::new(12.0, 0.0, 0.0)), 0.2);
        let broken = TodVolume { strength: f32::NAN, ..volume(shape, 4.0) };
        assert_eq!(broken.weight_at(Vec3::ZERO), 0.0);
        assert_eq!(soft.weight_at(Vec3::NAN), 0.0, "a broken probe is in no zone");
        let nowhere = volume(VolumeShape::Box { center: Vec3::NAN, half_extents: Vec3::ONE, yaw: 0.0 }, 4.0);
        assert_eq!(nowhere.weight_at(Vec3::ZERO), 0.0);
    }
}
