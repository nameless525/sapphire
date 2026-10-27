//! Zone override volumes: where a zone's timeline takes over from the base time of day.

use std::sync::Arc;

use sapphire_math::{Ease, Vec3};
use sapphire_tod::TodTimeline;

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

/// Where a [`TodVolume`] is, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VolumeShape {
    Sphere {
        center: Vec3,
        radius: f32,
    },
    /// An axis-aligned box.
    Box {
        center: Vec3,
        half_extents: Vec3,
    },
}

impl VolumeShape {
    /// Meters from `point` to the shape: 0 inside it, and infinitely far if the point or the
    /// shape isn't finite (`max` would otherwise turn NaN into "inside").
    pub fn distance(&self, point: Vec3) -> f32 {
        let (distance, finite) = match *self {
            Self::Sphere { center, radius } => (
                (point.distance(center) - radius.max(0.0)).max(0.0),
                center.is_finite() && radius.is_finite(),
            ),
            Self::Box { center, half_extents } => (
                ((point - center).abs() - half_extents.max(Vec3::ZERO)).max(Vec3::ZERO).length(),
                center.is_finite() && half_extents.is_finite(),
            ),
        };
        if finite && point.is_finite() { distance } else { f32::INFINITY }
    }
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
        let cube = VolumeShape::Box { center: Vec3::ZERO, half_extents: Vec3::new(1.0, 2.0, 3.0) };
        assert_eq!(cube.distance(Vec3::new(0.5, -1.5, 2.5)), 0.0);
        assert_eq!(cube.distance(Vec3::new(4.0, 0.0, 0.0)), 3.0);
        assert_eq!(cube.distance(Vec3::new(4.0, 6.0, 0.0)), 5.0, "to the corner region: a 3-4-5 triangle");
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
        let nowhere = volume(VolumeShape::Box { center: Vec3::NAN, half_extents: Vec3::ONE }, 4.0);
        assert_eq!(nowhere.weight_at(Vec3::ZERO), 0.0);
    }
}
