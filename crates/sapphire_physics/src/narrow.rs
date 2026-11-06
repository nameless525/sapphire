//! One shape against one shape, by GJK's distance and nothing that allocates.
//!
//! Every shape here is a [`Core`] and a radius around it: a capsule's segment, a sphere's center, a
//! box or a triangle with no radius. GJK measures between the cores, where it's exact (they're
//! polytopes, so it ends on the true closest features) and well away from its tolerances (the
//! radius keeps them apart).
//!
//! Two of `parry3d`'s functions are deliberately not used, for what M2 D3 measured:
//!
//! - `query::cast_shapes` and `query::contact` run EPA as soon as two shapes truly overlap, and EPA
//!   builds its polytope in fresh `Vec`s on every call: 11 allocations a cast.
//! - `query::gjk::directional_distance`, the ray cast those use, can miss: a capsule 1 cm from a wall
//!   dead ahead reads nothing there (the test `parrys_ray_cast_can_miss_a_wall_in_front_of_it`).
//!
//! # Sweeps
//!
//! The gap between two convex shapes, as one translates along a line, is a convex function of the
//! distance travelled. So [`sweep`] runs Newton's method on it from the start of the path: a
//! tangent to a convex function lies under it, so each step lands at or before where the shapes
//! touch and can't step over them, however thin they are. A slope that has stopped closing means
//! the gap will never get smaller, so that's a miss, decided exactly. Flat faces take one step, and
//! round shapes a few.
//!
//! That holds for the true gap, and GJK's reads up to [`GJK_STOP`] of the distance long. A shape's
//! radius covers that error: its core stays on the near side of whatever it reaches. A shape too
//! small for the distance holds its step back by the error instead.

use glam::Vec3;
use parry3d::math::Pose as ParryPose;
use parry3d::query::gjk::{self, CsoPoint, GJKResult, VoronoiSimplex};
use parry3d::shape::{Ball, Cuboid, Segment, SupportMap, Triangle};

/// A surface a sweep meets at less than this cosine (about 0.06°) doesn't block a moving body: at
/// that angle the slope is mostly rounding, and a body stopped by one would stop for no visible
/// reason.
pub(crate) const GRAZE: f32 = 1e-3;

/// A gap under this counts as touching, in metres: it's about the rounding of `f32` coordinates a
/// hundred metres out, which is as large as a local frame gets.
const TOUCHING: f32 = 1e-4;

/// A sweep's distance is corrected by its last gap, along the approach. Below this cosine that
/// division would amplify rounding rather than correct anything.
const MIN_CORRECTED_APPROACH: f32 = 0.05;

/// `gjk::closest_points` stops once its bounds agree to this share of the distance (the square root
/// of ten of `f32`'s epsilons, rounded up), so a distance can read this much too long.
const GJK_STOP: f32 = 1.1e-3;

/// Newton's steps a sweep may take. Stopping early is safe: every step is short of the surface, so
/// the sweep just reports a stop a little before it. Only a near miss of a round shape comes close.
const MAX_SWEEP_STEPS: u32 = 32;

/// Two cores whose lines meet at less than this sine count as lying along one line: their cross product
/// is too small a difference of large numbers to point anywhere useful.
const PARALLEL_SINE: f32 = 1e-3;

/// A shape as GJK measures it: a thin convex core, and a radius around it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Core {
    Point { point: Ball, radius: f32 },
    Segment { segment: Segment, radius: f32 },
    Box { cuboid: Cuboid },
    Triangle { triangle: Triangle },
}

impl Core {
    pub(crate) fn point(radius: f32) -> Self {
        Self::Point { point: Ball::new(0.0), radius }
    }

    pub(crate) const fn segment(segment: Segment, radius: f32) -> Self {
        Self::Segment { segment, radius }
    }

    pub(crate) const fn cuboid(cuboid: Cuboid) -> Self {
        Self::Box { cuboid }
    }

    pub(crate) const fn triangle(triangle: Triangle) -> Self {
        Self::Triangle { triangle }
    }

    fn radius(&self) -> f32 {
        match *self {
            Self::Point { radius, .. } | Self::Segment { radius, .. } => radius,
            Self::Box { .. } | Self::Triangle { .. } => 0.0,
        }
    }

    fn support(&self) -> &dyn SupportMap {
        match self {
            Self::Point { point, .. } => point,
            Self::Segment { segment, .. } => segment,
            Self::Box { cuboid } => cuboid,
            Self::Triangle { triangle } => triangle,
        }
    }

    /// The two ends of a round shape's core in its own space, the same point twice for a sphere.
    /// A box or a triangle has no ends: nothing moves as a triangle, and a box's overlap has no
    /// answer here (`contact`).
    fn ends(&self) -> Option<[Vec3; 2]> {
        match self {
            Self::Point { .. } => Some([Vec3::ZERO; 2]),
            Self::Segment { segment, .. } => Some([segment.a, segment.b]),
            Self::Box { .. } | Self::Triangle { .. } => None,
        }
    }
}

/// One sweep's result, in world space.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sweep {
    pub(crate) distance: f32,
    /// Away from the target, toward the moving shape.
    pub(crate) normal: Vec3,
    /// On the target's surface.
    pub(crate) point: Vec3,
    /// The shapes overlap where the sweep starts.
    pub(crate) overlapping: bool,
}

/// How two shapes stand, in world space.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Contact {
    /// Away from the target, toward the mover.
    pub(crate) normal: Vec3,
    /// Their gap: negative when they overlap, by that depth.
    pub(crate) separation: f32,
    /// On the target's surface, or as near as an overlap has one.
    pub(crate) point: Vec3,
}

/// What GJK makes of two cores, in the target's space.
enum Measure {
    /// Further apart than was asked about.
    Far,
    /// The cores themselves overlap, which GJK can't measure.
    Inside,
    Closest {
        on_target: Vec3,
        /// From the target's core toward the mover's.
        normal: Vec3,
        /// Between the cores.
        distance: f32,
    },
}

fn measure(target: &Core, relative: &ParryPose, mover: &Core, max_distance: f32, start: Vec3) -> Measure {
    let mut simplex = VoronoiSimplex::new();
    simplex.reset(CsoPoint::from_shapes(relative, target.support(), mover.support(), start));
    match gjk::closest_points(relative, target.support(), mover.support(), max_distance, true, &mut simplex) {
        GJKResult::ClosestPoints(on_target, on_mover, normal) => {
            Measure::Closest { on_target, normal, distance: (on_mover - on_target).dot(normal) }
        }
        GJKResult::Intersection => Measure::Inside,
        GJKResult::NoIntersection(_) | GJKResult::Proximity(_) => Measure::Far,
    }
}

/// Sweeps `mover`, starting at `mover_pose` along the unit vector `direction`, against `target`, for
/// up to `max_distance`. `separating_passes` lets a body overlapping or touching something move away
/// from it; a query still reports what it starts inside, whichever way it's headed.
pub(crate) fn sweep(
    target: &Core,
    target_pose: &ParryPose,
    mover: &Core,
    mover_pose: &ParryPose,
    direction: Vec3,
    max_distance: f32,
    separating_passes: bool,
) -> Option<Sweep> {
    let local_direction = target_pose.rotation.inverse() * direction;
    let mut relative = target_pose.inv_mul(mover_pose);
    let radii = target.radius() + mover.radius();
    let mut travelled = 0.0;
    let mut start = relative.translation.try_normalize().unwrap_or(Vec3::X);
    let mut last_hit = None;

    for step in 0..MAX_SWEEP_STEPS {
        let reach = radii + (max_distance - travelled).max(0.0);
        match measure(target, &relative, mover, reach, start) {
            Measure::Far => return None,
            Measure::Inside => {
                if step > 0 {
                    // Rounding put the cores in touch: this is the surface, with the last normal.
                    break;
                }
                let contact = inside_contact(target, target_pose, mover, mover_pose, radii);
                let (normal, point) =
                    contact.map_or((-direction, mover_pose.translation), |c| (c.normal, c.point));
                if separating_passes && -normal.dot(direction) < GRAZE {
                    return None;
                }
                return Some(Sweep { distance: 0.0, normal, point, overlapping: true });
            }
            Measure::Closest { on_target, normal, distance } => {
                let gap = distance - radii;
                let approach = -normal.dot(local_direction);
                // Queries admit every positive closing slope. Only moving bodies apply the
                // angular anti-snag policy; a shallow query may still reach the face in range.
                let closing = approach > 0.0 && (!separating_passes || approach >= GRAZE);
                let on_surface = on_target + normal * target.radius();
                last_hit = Some((on_surface, normal));
                start = normal;
                if gap < TOUCHING {
                    let overlapping = step == 0 && gap < 0.0;
                    if !closing && (separating_passes || gap > 0.0) {
                        return None;
                    }
                    let corrected = if !overlapping && approach > MIN_CORRECTED_APPROACH {
                        (travelled + gap / approach).max(0.0)
                    } else {
                        travelled
                    };
                    if corrected > max_distance {
                        return None;
                    }
                    return Some(world_sweep(target_pose, corrected, on_surface, normal, overlapping));
                }
                // The gap can't shrink past here if it has stopped closing: it's convex.
                if !closing {
                    return None;
                }
                let error = distance * GJK_STOP;
                let advance = if error > radii { gap - error } else { gap } / approach;
                travelled += advance;
                if travelled > max_distance {
                    return None;
                }
                relative.translation += local_direction * advance;
            }
        }
    }

    // Stopped short, or met the surface by rounding: either way this is at or before the surface.
    let (point, normal) = last_hit?;
    Some(world_sweep(target_pose, travelled, point, normal, false))
}

fn world_sweep(
    target_pose: &ParryPose,
    distance: f32,
    point: Vec3,
    normal: Vec3,
    overlapping: bool,
) -> Sweep {
    Sweep {
        distance,
        normal: target_pose.rotation * normal,
        point: target_pose.transform_point(point),
        overlapping,
    }
}

/// Measures from `mover` to `target`, up to a gap of `reach`. `None` means further apart than that,
/// or a box overlapping its target: only a round shape's overlap has an answer without EPA.
pub(crate) fn contact(
    target: &Core,
    target_pose: &ParryPose,
    mover: &Core,
    mover_pose: &ParryPose,
    reach: f32,
) -> Option<Contact> {
    let relative = target_pose.inv_mul(mover_pose);
    let radii = target.radius() + mover.radius();
    let start = relative.translation.try_normalize().unwrap_or(Vec3::X);
    match measure(target, &relative, mover, radii + reach, start) {
        Measure::Far => None,
        Measure::Inside => inside_contact(target, target_pose, mover, mover_pose, radii),
        Measure::Closest { on_target, normal, distance } => {
            let contact = Contact {
                normal: target_pose.rotation * normal,
                separation: distance - radii,
                point: target_pose.transform_point(on_target + normal * target.radius()),
            };
            // The line between truly closest points has nothing more in the way than rounding. What GJK says
            // of cores that all but touch is itself rounding, and can lie along them, lengths and all.
            if distance < radii
                && let (Some(target_ends), Some(mover_ends)) = (target.ends(), mover.ends())
            {
                let ends = (target_ends, target_pose, mover_ends, mover_pose);
                if out_along(contact.normal, ends, radii) > TOUCHING - contact.separation {
                    return Some(crossing_contact(target_ends, target_pose, mover_ends, mover_pose, radii));
                }
            }
            Some(contact)
        }
    }
}

/// How far along `way` a mover has to go to clear a target by `radii`, of each one's two ends and its pose,
/// target first.
fn out_along(
    way: Vec3,
    (target, target_pose, mover, mover_pose): ([Vec3; 2], &ParryPose, [Vec3; 2], &ParryPose),
    radii: f32,
) -> f32 {
    let reach = |along: Vec3, ends: [Vec3; 2], pose: &ParryPose| {
        pose.transform_point(ends[0]).dot(along).max(pose.transform_point(ends[1]).dot(along))
    };
    reach(way, target, target_pose) + reach(-way, mover, mover_pose) + radii
}

/// Cores that overlap. Small enough to answer by hand for a round mover; `None` for a box.
fn inside_contact(
    target: &Core,
    target_pose: &ParryPose,
    mover: &Core,
    mover_pose: &ParryPose,
    radii: f32,
) -> Option<Contact> {
    let ends = mover.ends()?;
    Some(match target {
        Core::Box { cuboid } => inside_box_contact(cuboid, target_pose, ends, mover.radius(), mover_pose),
        Core::Triangle { triangle } => {
            through_triangle_contact(triangle, target_pose, ends, mover.radius(), mover_pose)
        }
        Core::Point { .. } | Core::Segment { .. } => {
            crossing_contact(target.ends()?, target_pose, ends, mover_pose, radii)
        }
    })
}

/// Two thin cores that touch: they cross, lie along one line or hold a center on the other. The shortest
/// way out is across both, where only their radii are in the way, and along either one a whole length can
/// be. Its depth is what clears the target's core by the radii on that side, rounding and all, which is
/// what makes moving by it separate them whichever way it goes.
fn crossing_contact(
    target: [Vec3; 2],
    target_pose: &ParryPose,
    mover: [Vec3; 2],
    mover_pose: &ParryPose,
    radii: f32,
) -> Contact {
    let between = mover_pose.translation - target_pose.translation;
    let length =
        |ends: [Vec3; 2], pose: &ParryPose| pose.transform_point(ends[1]) - pose.transform_point(ends[0]);
    let across = way_across(length(target, target_pose), length(mover, mover_pose), between);
    let ends = (target, target_pose, mover, mover_pose);
    let (forward, back) = (out_along(across, ends, radii), out_along(-across, ends, radii));
    let (normal, depth) = if back < forward { (-across, back) } else { (across, forward) };
    Contact { normal, separation: -depth, point: target_pose.translation }
}

/// A unit vector across two cores, from each one's length along itself and the line between their centers.
fn way_across(target: Vec3, mover: Vec3, between: Vec3) -> Vec3 {
    let (long, short) =
        if target.length_squared() < mover.length_squared() { (mover, target) } else { (target, mover) };
    let cross = long.cross(short);
    let limit = PARALLEL_SINE * PARALLEL_SINE * long.length_squared() * short.length_squared();
    if cross.length_squared() > limit
        && let Some(across) = cross.try_normalize()
    {
        return across;
    }
    let Some(along) = long.try_normalize() else {
        // Two points: nothing to go across, and only the line between them to tell them apart.
        return between.try_normalize().unwrap_or(Vec3::Y);
    };
    // One line: any way off it will do, so one that a line's numbers can't flip from frame to frame.
    let side = if along.x.abs() < 0.9 { Vec3::X } else { Vec3::Z };
    (side - along * side.dot(along)).try_normalize().unwrap_or(Vec3::Y)
}

/// A round shape's core inside a box: out through whichever face takes the shortest push for both
/// ends.
fn inside_box_contact(
    cuboid: &Cuboid,
    pose: &ParryPose,
    ends: [Vec3; 2],
    radius: f32,
    mover_pose: &ParryPose,
) -> Contact {
    let relative = pose.inv_mul(mover_pose);
    let [a, b] = ends.map(|end| relative.transform_point(end));
    let mut best_push = f32::MAX;
    let mut best_axis = Vec3::Y;
    for axis in 0..3 {
        let half = cuboid.half_extents[axis];
        let up = half - a[axis].min(b[axis]);
        let down = half + a[axis].max(b[axis]);
        let unit = Vec3::AXES[axis];
        if up < best_push {
            best_push = up;
            best_axis = unit;
        }
        if down < best_push {
            best_push = down;
            best_axis = -unit;
        }
    }
    let center = relative.translation;
    let on_face = center + best_axis * (cuboid.half_extents.dot(best_axis.abs()) - center.dot(best_axis));
    Contact {
        normal: pose.rotation * best_axis,
        separation: -(best_push + radius),
        point: pose.transform_point(on_face),
    }
}

/// A round shape's core through a triangle: out to the side its center is on, by how far its deepest
/// end is behind the triangle's plane.
fn through_triangle_contact(
    triangle: &Triangle,
    pose: &ParryPose,
    ends: [Vec3; 2],
    radius: f32,
    mover_pose: &ParryPose,
) -> Contact {
    let relative = pose.inv_mul(mover_pose);
    let face = (triangle.b - triangle.a).cross(triangle.c - triangle.a).normalize_or(Vec3::Y);
    let normal = if (relative.translation - triangle.a).dot(face) < 0.0 { -face } else { face };
    let [a, b] = ends.map(|end| relative.transform_point(end));
    let behind = (triangle.a - a).dot(normal).max((triangle.a - b).dot(normal)).max(0.0);
    let center = relative.translation;
    Contact {
        normal: pose.rotation * normal,
        separation: -(behind + radius),
        point: pose.transform_point(center - normal * (center - triangle.a).dot(normal)),
    }
}

#[cfg(test)]
mod tests {
    use glam::Quat;
    use parry3d::shape::Capsule;

    use super::*;
    use crate::shape::Shape;

    fn capsule() -> Core {
        Shape::Capsule { half_height: 0.5, radius: 0.25 }.to_convex().core()
    }

    fn cube(half_extents: Vec3) -> Core {
        Shape::Box { half_extents }.to_convex().core()
    }

    fn pose_at(x: f32, y: f32, z: f32) -> ParryPose {
        ParryPose::from_translation(Vec3::new(x, y, z))
    }

    #[track_caller]
    fn assert_near(a: Vec3, b: Vec3) {
        assert!((a - b).length() < 1e-4, "{a} is not {b}");
    }

    #[test]
    fn a_capsule_core_outside_a_box_measures_by_gjk() {
        // The capsule's lower cap center is 0.5 under its origin: at y = 1.75 it's 0.25 over the
        // box's top at 1, so its radius of 0.25 just touches. 0.1 lower overlaps by 0.1.
        let contact =
            contact(&cube(Vec3::ONE), &ParryPose::IDENTITY, &capsule(), &pose_at(0.0, 1.65, 0.0), 1.0)
                .unwrap();
        assert_near(contact.normal, Vec3::Y);
        assert!((contact.separation + 0.1).abs() < 1e-5, "{}", contact.separation);
        assert_near(contact.point, Vec3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn a_capsule_core_inside_a_box_leaves_by_the_nearest_face() {
        // Core from y = -0.5 to 0.5, 0.4 toward +z of the box's center: the nearest face is +z,
        // 0.1 away for the core, plus the capsule's radius.
        let target = cube(Vec3::new(2.0, 2.0, 0.5));
        let contact =
            contact(&target, &ParryPose::IDENTITY, &capsule(), &pose_at(0.0, 0.0, 0.4), 1.0).unwrap();
        assert_near(contact.normal, Vec3::Z);
        assert!((contact.separation + 0.35).abs() < 1e-5, "{}", contact.separation);
        assert_near(contact.point, Vec3::new(0.0, 0.0, 0.5));
    }

    #[test]
    fn a_turned_box_turns_its_answer() {
        // A half turn about Y: the box's +z face looks along world -z.
        let turned = ParryPose::from_parts(Vec3::ZERO, Quat::from_xyzw(0.0, 1.0, 0.0, 0.0));
        let target = cube(Vec3::new(2.0, 2.0, 0.5));
        let contact = contact(&target, &turned, &capsule(), &pose_at(0.0, 0.0, -0.4), 1.0).unwrap();
        assert_near(contact.normal, Vec3::NEG_Z);
        assert!((contact.separation + 0.35).abs() < 1e-5, "{}", contact.separation);
    }

    #[test]
    fn capsules_on_one_line_separate_off_it() {
        // Same place: nothing to tell them apart, so off to the side, by both radii. Straight up would
        // take both lengths as well.
        let contact =
            contact(&capsule(), &ParryPose::IDENTITY, &capsule(), &ParryPose::IDENTITY, 1.0).unwrap();
        assert_near(contact.normal, Vec3::X);
        assert!((contact.separation + 0.5).abs() < 1e-5, "{}", contact.separation);
    }

    #[test]
    fn crossing_capsules_separate_across_both() {
        // One upright through the middle of one lying along X, and a little in front of its center: out
        // along Z, away from the other, by both radii.
        let lying = ParryPose::from_parts(
            Vec3::new(0.1, 0.2, 0.0),
            Quat::from_xyzw(0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2),
        );
        let contact = contact(&capsule(), &lying, &capsule(), &ParryPose::IDENTITY, 1.0).unwrap();
        assert!(contact.normal.dot(Vec3::Z).abs() > 0.9999, "{}", contact.normal);
        assert!((contact.separation + 0.5).abs() < 1e-5, "{}", contact.separation);
    }

    /// The side a way across goes is the one with less to clear, so a mover a hair past a core's line
    /// doesn't go back through it.
    #[test]
    fn a_way_across_goes_to_the_near_side() {
        let thin = Core::segment(Segment::new(Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)), 0.1);
        for (x, side) in [(2e-7, Vec3::X), (-2e-7, Vec3::NEG_X)] {
            let target = thin.ends().expect("round");
            let contact = crossing_contact(target, &ParryPose::IDENTITY, target, &pose_at(x, 0.0, 0.0), 0.2);
            assert_near(contact.normal, side);
            assert!((contact.separation + 0.2 - x.abs()).abs() < 1e-6, "{}", contact.separation);
        }
    }

    #[test]
    fn a_way_across_never_lies_along_a_core() {
        for along in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(0.6, 0.8, 0.0), Vec3::new(0.95, 0.2, 0.2)] {
            for other in [along, along * 0.5, along + Vec3::splat(1e-5), Vec3::ZERO] {
                let across = way_across(along, other, Vec3::ZERO);
                assert!((across.length() - 1.0).abs() < 1e-5, "{across}");
                assert!(across.dot(along).abs() < 1e-3, "{across} along {along}");
            }
        }
        // Across two that truly cross, and by the line between them for two points.
        assert_near(way_across(Vec3::X, Vec3::Y, Vec3::ZERO), Vec3::Z);
        assert_near(way_across(Vec3::ZERO, Vec3::ZERO, Vec3::new(0.0, 0.0, 3.0)), Vec3::Z);
        assert_near(way_across(Vec3::ZERO, Vec3::ZERO, Vec3::ZERO), Vec3::Y);
    }

    #[test]
    fn a_core_through_a_triangle_leaves_toward_its_center() {
        let triangle = Core::triangle(Triangle::new(
            Vec3::new(-5.0, 0.0, -5.0),
            Vec3::new(-5.0, 0.0, 5.0),
            Vec3::new(5.0, 0.0, 0.0),
        ));
        // Center 0.2 above the plane, core down to -0.3: out upward by 0.3, plus the radius.
        let contact =
            contact(&triangle, &ParryPose::IDENTITY, &capsule(), &pose_at(0.0, 0.2, 0.0), 1.0).unwrap();
        assert_near(contact.normal, Vec3::Y);
        assert!((contact.separation + 0.55).abs() < 1e-5, "{}", contact.separation);
    }

    #[test]
    fn apart_past_the_reach_is_no_contact() {
        assert!(
            contact(&cube(Vec3::ONE), &ParryPose::IDENTITY, &capsule(), &pose_at(9.0, 0.0, 0.0), 1.0)
                .is_none()
        );
    }

    #[test]
    fn a_boxs_overlap_has_no_contact() {
        let mover = cube(Vec3::splat(0.5));
        assert!(
            contact(&cube(Vec3::ONE), &ParryPose::IDENTITY, &mover, &pose_at(0.0, 1.0, 0.0), 1.0).is_none()
        );
    }

    #[test]
    fn a_sweep_starting_in_overlap_reports_it_or_lets_go() {
        // Overlapping the box's top by 0.1.
        let target = cube(Vec3::ONE);
        let at = ParryPose::IDENTITY;
        let start = pose_at(0.0, 1.65, 0.0);
        let hit =
            sweep(&target, &at, &capsule(), &start, Vec3::NEG_Y, 5.0, true).expect("moving in is blocked");
        assert_eq!(hit.distance, 0.0);
        assert!(hit.overlapping);
        assert_near(hit.normal, Vec3::Y);
        // Moving out isn't, for a body; a query still reports what it started in.
        assert!(sweep(&target, &at, &capsule(), &start, Vec3::Y, 5.0, true).is_none());
        let hit = sweep(&target, &at, &capsule(), &start, Vec3::Y, 5.0, false).expect("a query reports it");
        assert!(hit.overlapping);
    }

    /// The trap the module's doc names: a capsule whose surface is 1 cm from a wall a hundred metres
    /// across, headed straight at it, gets no hit from `parry3d`'s ray cast. (2 cm and 1.5 cm do.) If
    /// this fails, an upgrade fixed the ray cast, though [`sweep`] has no reason to go back to it.
    #[test]
    fn parrys_ray_cast_can_miss_a_wall_in_front_of_it() {
        let wall = Cuboid::new(Vec3::new(0.01, 50.0, 50.0));
        let capsule = Capsule::new_y(1.0, 0.5);
        let relative = pose_at(3.0, 0.0, 0.0).inv_mul(&pose_at(2.48, 0.0, 0.0));
        let ray = gjk::directional_distance(&relative, &wall, &capsule, Vec3::X, &mut VoronoiSimplex::new());
        assert!(ray.is_none(), "{ray:?}");
    }

    /// The same wall by [`sweep`], from each of those starts and a few closer ones.
    #[test]
    fn a_sweep_near_a_huge_thin_wall_still_hits() {
        let wall = cube(Vec3::new(0.01, 50.0, 50.0));
        let at = pose_at(3.0, 0.0, 0.0);
        let mover = Shape::Capsule { half_height: 1.0, radius: 0.5 }.to_convex().core();
        for x in [0.0, 2.0, 2.4, 2.47, 2.48, 2.485, 2.489, 2.4899] {
            let hit = sweep(&wall, &at, &mover, &pose_at(x, 0.0, 0.0), Vec3::X, 3.0, true)
                .unwrap_or_else(|| panic!("a sweep from x = {x} must hit"));
            assert!((hit.distance - (2.49 - x)).abs() < 1e-4, "from {x}: {}", hit.distance);
            assert_near(hit.normal, Vec3::NEG_X);
        }
    }

    /// GJK stops once its bounds agree to 0.1%, so from far off it can read a gap that's too long by
    /// more than a tiny shape's radius and a thin wall's thickness. Starting nearly over the wall's
    /// diagonal, where it stops soonest, and aiming well away from it, where its tangent plane has
    /// fallen furthest behind the wall, a step of that length would land past the wall and see it as
    /// behind.
    #[test]
    fn a_tiny_shape_finds_a_thin_wall_from_far_away() {
        let mut state = 0x2545_f491_u32;
        let mut unit = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 8) as f32 / (1u32 << 24) as f32
        };
        let half_thickness = 0.002;
        let radius = 0.002;
        let wall = cube(Vec3::new(100.0, 100.0, half_thickness));
        let mover = Core::point(radius);
        for case in 0..4000 {
            #[allow(
                clippy::disallowed_methods,
                reason = "a test's random turn: its last bit matters to nothing"
            )]
            let rotation =
                Quat::from_xyzw(unit() - 0.5, unit() - 0.5, unit() - 0.5, unit() - 0.5).normalize();
            let (across, up, face) = (rotation * Vec3::X, rotation * Vec3::Y, rotation * Vec3::Z);
            let center = Vec3::new(unit() * 20.0, unit() * 20.0, unit() * 20.0);
            let height = 20.0 + unit() * 380.0;
            let start = center + face * height + across * (unit() - 0.5) * 4.0 + up * (unit() - 0.5) * 4.0;
            let aim = center + across * (unit() - 0.5) * 180.0 + up * (unit() - 0.5) * 180.0;
            let direction = (aim - start).normalize();
            let approach = -face.dot(direction);
            let exact = (height - half_thickness - radius) / approach;

            let at = ParryPose::from_parts(center, rotation);
            let hit =
                sweep(&wall, &at, &mover, &ParryPose::from_translation(start), direction, 1000.0, false)
                    .unwrap_or_else(|| {
                        panic!("case {case}: {height} m up, at a cosine of {approach}: no hit")
                    });
            assert!(
                hit.normal.dot(face) > 0.999 && (hit.distance - exact).abs() < 0.02,
                "case {case}: {height} m up: stopped at {}, not {exact}, facing {}",
                hit.distance,
                hit.normal.dot(face)
            );
        }
    }

    #[test]
    fn a_sweep_decides_a_near_miss_exactly() {
        // Spheres of radius 0.5 and 0.5 pass 1.001 apart: a miss. 0.999 apart: a hit, just.
        let target = Core::point(0.5);
        let mover = Core::point(0.5);
        let at = ParryPose::IDENTITY;
        assert!(sweep(&target, &at, &mover, &pose_at(-5.0, 1.001, 0.0), Vec3::X, 10.0, false).is_none());
        let hit =
            sweep(&target, &at, &mover, &pose_at(-5.0, 0.999, 0.0), Vec3::X, 10.0, false).expect("a hit");
        // √(1 − 0.999²) ≈ 0.0447 short of passing overhead. A stop this glancing counts as touching
        // up to `TOUCHING / 0.0447`, about 2 mm, early: never late.
        let exact = 5.0 - 0.0447;
        assert!(hit.distance <= exact + 1e-4 && hit.distance >= exact - 3e-3, "{}", hit.distance);
    }

    #[test]
    fn a_sweep_stops_at_its_range() {
        let target = cube(Vec3::ONE);
        let at = ParryPose::IDENTITY;
        let start = pose_at(5.0, 0.0, 0.0);
        let mover = Core::point(0.5);
        // The face is 4 away, the sphere's surface reaching it after 3.5.
        assert!(sweep(&target, &at, &mover, &start, Vec3::NEG_X, 3.0, false).is_none());
        let hit = sweep(&target, &at, &mover, &start, Vec3::NEG_X, 4.0, false).expect("a hit");
        assert!((hit.distance - 3.5).abs() < 1e-4, "{}", hit.distance);
    }
}
