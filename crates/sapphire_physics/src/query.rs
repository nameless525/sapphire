//! Ray, shape and overlap queries over the whole world.

use std::cell::Cell;

use glam::Vec3;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::Pose as ParryPose;
use parry3d::partitioning::{BvhLeafCost, BvhNode};
use parry3d::query::{self, Ray, RayCast};
use parry3d::shape::{Shape as ParryShape, TriMesh};
use sapphire_colliders::{CollisionGroups, GroupMask};

use crate::narrow::{self, Contact, Core, Sweep};
use crate::shape::{Convex, Pose, Shape};
use crate::stats::Counters;
use crate::world::{BodyEntry, BodyId, ColliderId, HitTarget, PhysicsWorld, StaticEntry, StaticGeometry};

/// What a query may hit: colliders and bodies in `groups`, except `exclude_body` and the run of IDs
/// in `exclude_bodies`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryFilter {
    pub groups: GroupMask,
    /// Normally the body asking.
    pub exclude_body: Option<BodyId>,
    /// The first and last IDs of a run of bodies, both included. An excluded body takes no test and
    /// no room in a result list, which filtering a list afterward can't promise.
    pub exclude_bodies: Option<(BodyId, BodyId)>,
}

impl QueryFilter {
    pub const fn new(groups: GroupMask) -> Self {
        Self { groups, exclude_body: None, exclude_bodies: None }
    }

    pub const fn excluding(self, body: BodyId) -> Self {
        Self { exclude_body: Some(body), ..self }
    }

    /// Excludes every body from `first` to `last`, as when one actor owns several hurtboxes.
    pub const fn excluding_bodies(self, first: BodyId, last: BodyId) -> Self {
        Self { exclude_bodies: Some((first, last)), ..self }
    }

    fn excludes(&self, id: BodyId) -> bool {
        self.exclude_body == Some(id)
            || self.exclude_bodies.is_some_and(|(first, last)| first <= id && id <= last)
    }
}

/// A ray's first hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub target: HitTarget,
    /// Metres along the ray. Zero if it starts inside the target.
    pub distance: f32,
    pub point: Vec3,
    /// Against the ray, or straight back along it if it starts inside.
    pub normal: Vec3,
}

/// A shape cast's hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeHit {
    pub target: HitTarget,
    /// Metres the shape travels before it touches. Zero if it starts overlapping.
    pub distance: f32,
    /// [`Self::distance`] as a share of the cast's length.
    pub fraction: f32,
    /// On the target's surface.
    pub point: Vec3,
    /// Away from the target. For a box that starts overlapping, back along the cast: with no core
    /// to measure from, a real normal would take EPA.
    pub normal: Vec3,
    pub started_overlapping: bool,
}

impl ShapeHit {
    /// A hit nothing can have: the buffers `cast_shape_all` fills start as this.
    pub const NONE: Self = Self {
        target: HitTarget::Static(ColliderId(u32::MAX)),
        distance: f32::MAX,
        fraction: f32::MAX,
        point: Vec3::ZERO,
        normal: Vec3::Y,
        started_overlapping: false,
    };
}

/// How many results a list-filling query found, and how many fit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Overlaps {
    pub found: usize,
    pub written: usize,
}

/// How deep a shape sits in something, and the way out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Penetration {
    pub target: HitTarget,
    /// Away from the target: moving along this by [`Self::depth`] separates them.
    pub normal: Vec3,
    /// Metres, always positive.
    pub depth: f32,
    /// On the target's surface, or as near as an overlap has one.
    pub point: Vec3,
}

/// Which colliders and bodies a walk considers.
#[derive(Clone, Copy)]
pub(crate) enum Selector {
    Query(QueryFilter),
    Mover { id: BodyId, groups: CollisionGroups },
}

impl Selector {
    fn takes_static(self, entry: &StaticEntry) -> bool {
        match self {
            Self::Query(filter) => filter.groups.intersects(entry.memberships),
            Self::Mover { groups, .. } => groups.filter.intersects(entry.memberships),
        }
    }

    fn takes_body(self, body: &BodyEntry) -> bool {
        match self {
            Self::Query(filter) => {
                filter.groups.intersects(body.groups.memberships) && !filter.excludes(body.id)
            }
            Self::Mover { id, groups } => id != body.id && groups.interacts_with(body.groups),
        }
    }
}

/// A sweep's hit in a tree's leaf, so `parry3d`'s best-first walk can rank it.
struct LeafSweep(Sweep);

impl BvhLeafCost for LeafSweep {
    fn cost(&self) -> f32 {
        self.0.distance
    }
}

/// The same for a ray.
struct LeafRay(f32, Vec3);

impl BvhLeafCost for LeafRay {
    fn cost(&self) -> f32 {
        self.0
    }
}

/// What to start a best-first walk's bound at for a query's range to include its end. A walk keeps only
/// what's strictly nearer than its bound, which is right for what it has already found, so a tie stays
/// with whatever it reached first, and a float past the end is the one bound that also takes in the end.
fn past(range: f32) -> f32 {
    range.next_up()
}

/// The time a ray from the origin along `direction` takes to reach `node` grown by a shape's box
/// (its Minkowski sum), or `f32::MAX`: a shape reaching the node can't happen sooner.
fn sweep_cost(node: &BvhNode, shape_box: &Aabb, direction: &Ray, best: f32) -> f32 {
    let shift = -shape_box.center();
    let margin = shape_box.half_extents();
    let sum = Aabb { mins: node.mins() + shift - margin, maxs: node.maxs() + shift + margin };
    sum.cast_local_ray(direction, best, true).unwrap_or(f32::MAX)
}

impl PhysicsWorld {
    /// The first thing a ray hits within `max_distance`, what it reaches at exactly that distance
    /// included, as in every query here. `direction` needn't be unit length; a zero one hits nothing.
    pub fn cast_ray(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        filter: &QueryFilter,
    ) -> Option<RayHit> {
        profiling::scope!("PhysicsWorld::cast_ray");
        Counters::add(&self.counters.ray_casts, 1);
        let direction = direction.try_normalize()?;
        let ray = Ray::new(origin, direction);
        let selector = Selector::Query(*filter);
        let tests = Cell::new(0u32);
        let limit = past(max_distance);

        let mut best = self
            .tree
            .find_best(
                limit,
                |node: &BvhNode, best| node.cast_ray(&ray, best),
                |leaf, best| {
                    let entry = &self.statics[leaf as usize];
                    if !selector.takes_static(entry) {
                        return None;
                    }
                    tests.set(tests.get() + 1);
                    let hit = entry.shape().cast_ray_and_get_normal(&entry.pose, &ray, best, true)?;
                    Some(LeafRay(hit.time_of_impact, hit.normal))
                },
            )
            .map(|(leaf, hit)| (HitTarget::Static(self.statics[leaf as usize].id), hit.0, hit.1));

        let mut reach = best.map_or(limit, |hit| hit.1);
        for body in &self.bodies {
            if !selector.takes_body(body) {
                continue;
            }
            tests.set(tests.get() + 1);
            if let Some(hit) =
                body.convex.shape().cast_ray_and_get_normal(&body.parry_pose, &ray, reach, true)
                && hit.time_of_impact < reach
            {
                reach = hit.time_of_impact;
                best = Some((HitTarget::Body(body.id), hit.time_of_impact, hit.normal));
            }
        }
        Counters::add(&self.counters.narrow_tests, tests.get());

        best.map(|(target, distance, normal)| RayHit {
            target,
            distance,
            point: origin + direction * distance,
            normal: normal.try_normalize().unwrap_or(-direction),
        })
    }

    /// The first thing `shape` hits, swept from `pose` along `direction` for `max_distance`. A
    /// shape starting in something hits it at distance zero, whichever way it's headed.
    /// `direction` needn't be unit length; a zero one hits nothing.
    pub fn cast_shape(
        &self,
        shape: &Shape,
        pose: Pose,
        direction: Vec3,
        max_distance: f32,
        filter: &QueryFilter,
    ) -> Option<ShapeHit> {
        profiling::scope!("PhysicsWorld::cast_shape");
        Counters::add(&self.counters.shape_casts, 1);
        let direction = direction.try_normalize()?;
        let convex = shape.to_convex();
        self.first_sweep(&convex, &pose.to_parry(), direction, max_distance, Selector::Query(*filter), false)
            .map(|(target, sweep)| shape_hit(target, sweep, max_distance))
    }

    /// Everything `shape` hits along a sweep, nearest first (ties by [`HitTarget`]), into `out`:
    /// each collider or body once. A mesh collider reports its first triangle hit. A negative range
    /// hits nothing, as with [`Self::cast_shape`], even in something a shape starts inside.
    pub fn cast_shape_all(
        &self,
        shape: &Shape,
        pose: Pose,
        direction: Vec3,
        max_distance: f32,
        filter: &QueryFilter,
        out: &mut [ShapeHit],
    ) -> Overlaps {
        profiling::scope!("PhysicsWorld::cast_shape_all");
        Counters::add(&self.counters.shape_casts, 1);
        // A sweep asks whether cores overlap before it looks at its range.
        if max_distance < 0.0 {
            return Overlaps::default();
        }
        let Some(direction) = direction.try_normalize() else {
            return Overlaps::default();
        };
        let convex = shape.to_convex();
        let start = pose.to_parry();
        let swept = swept_box(&convex, &start, direction, max_distance);
        let selector = Selector::Query(*filter);
        let mut tests = 0;
        let mut results = Overlaps::default();
        let mut keep = |hit: ShapeHit| {
            results.found += 1;
            insert_sorted(out, &mut results.written, hit, |a, b| {
                a.distance.total_cmp(&b.distance).then(a.target.cmp(&b.target))
            });
        };

        for leaf in self.tree.leaves(|node: &BvhNode| node.aabb().intersects(&swept)) {
            let entry = &self.statics[leaf as usize];
            if !selector.takes_static(entry) {
                continue;
            }
            tests += 1;
            let target = HitTarget::Static(entry.id);
            if let Some(sweep) = entry.sweep(&convex, &start, direction, max_distance, false, &mut tests) {
                keep(shape_hit(target, sweep, max_distance));
            }
        }
        for body in &self.bodies {
            if !selector.takes_body(body) || !body.aabb.intersects(&swept) {
                continue;
            }
            tests += 1;
            let target = body.convex.core();
            if let Some(sweep) = narrow::sweep(
                &target,
                &body.parry_pose,
                &convex.core(),
                &start,
                direction,
                max_distance,
                false,
            ) {
                keep(shape_hit(HitTarget::Body(body.id), sweep, max_distance));
            }
        }

        Counters::add(&self.counters.narrow_tests, tests);
        Counters::add(&self.counters.results_dropped, (results.found - results.written) as u32);
        results
    }

    /// Everything `shape` overlaps at `pose`, sorted by [`HitTarget`], into `out`.
    pub fn overlap(
        &self,
        shape: &Shape,
        pose: Pose,
        filter: &QueryFilter,
        out: &mut [HitTarget],
    ) -> Overlaps {
        profiling::scope!("PhysicsWorld::overlap");
        Counters::add(&self.counters.overlap_queries, 1);
        let convex = shape.to_convex();
        let at = pose.to_parry();
        let area = convex.aabb(&at);
        let selector = Selector::Query(*filter);
        let mut tests = 0;
        let mut results = Overlaps::default();
        let mut keep = |target: HitTarget| {
            results.found += 1;
            insert_sorted(out, &mut results.written, target, |a, b| a.cmp(b));
        };

        for leaf in self.tree.leaves(|node: &BvhNode| node.aabb().intersects(&area)) {
            let entry = &self.statics[leaf as usize];
            if !selector.takes_static(entry) {
                continue;
            }
            tests += 1;
            if overlaps(&at, convex.shape(), &entry.pose, entry.shape()) {
                keep(HitTarget::Static(entry.id));
            }
        }
        for body in &self.bodies {
            if !selector.takes_body(body) || !body.aabb.intersects(&area) {
                continue;
            }
            tests += 1;
            if overlaps(&at, convex.shape(), &body.parry_pose, body.convex.shape()) {
                keep(HitTarget::Body(body.id));
            }
        }

        Counters::add(&self.counters.narrow_tests, tests);
        Counters::add(&self.counters.results_dropped, (results.found - results.written) as u32);
        results
    }

    /// The deepest thing a sphere or capsule at `pose` overlaps, and the way out of it. `None` if
    /// it overlaps nothing, or for a box, whose depth would take EPA.
    pub fn penetration(&self, shape: &Shape, pose: Pose, filter: &QueryFilter) -> Option<Penetration> {
        profiling::scope!("PhysicsWorld::penetration");
        Counters::add(&self.counters.penetration_queries, 1);
        let convex = shape.to_convex();
        self.deepest_overlap(&convex, &pose.to_parry(), Selector::Query(*filter))
    }

    /// The nearest hit of a sweep over statics and bodies.
    pub(crate) fn first_sweep(
        &self,
        mover: &Convex,
        start: &ParryPose,
        direction: Vec3,
        max_distance: f32,
        selector: Selector,
        separating_passes: bool,
    ) -> Option<(HitTarget, Sweep)> {
        let tests = Cell::new(0u32);
        let box_at_start = mover.aabb(start);
        let limit = past(max_distance);
        // The tree's boxes are in world space: a ray from the origin measures how far `mover`
        // travels before its box reaches theirs, as `parry3d`'s own composite cast does.
        let ray = Ray::new(Vec3::ZERO, direction);
        let mut best = self
            .tree
            .find_best(
                limit,
                |node: &BvhNode, best| sweep_cost(node, &box_at_start, &ray, best),
                |leaf, best| {
                    let entry = &self.statics[leaf as usize];
                    if !selector.takes_static(entry) {
                        return None;
                    }
                    let mut count = tests.get() + 1;
                    let sweep = entry.sweep(mover, start, direction, best, separating_passes, &mut count);
                    tests.set(count);
                    sweep.map(LeafSweep)
                },
            )
            .map(|(leaf, hit)| (HitTarget::Static(self.statics[leaf as usize].id), hit.0));

        let mut reach = best.map_or(limit, |hit| hit.1.distance);
        let swept = swept_box(mover, start, direction, reach);
        for body in &self.bodies {
            if !selector.takes_body(body) || !body.aabb.intersects(&swept) {
                continue;
            }
            tests.set(tests.get() + 1);
            let target = body.convex.core();
            let core = mover.core();
            if let Some(sweep) =
                narrow::sweep(&target, &body.parry_pose, &core, start, direction, reach, separating_passes)
                && sweep.distance < reach
            {
                reach = sweep.distance;
                best = Some((HitTarget::Body(body.id), sweep));
            }
        }
        Counters::add(&self.counters.narrow_tests, tests.get());
        best
    }

    /// The deepest overlap of a sphere or capsule over statics and bodies. Ties go to whichever
    /// the walk reaches first, which a world's contents alone decide.
    pub(crate) fn deepest_overlap(
        &self,
        mover: &Convex,
        at: &ParryPose,
        selector: Selector,
    ) -> Option<Penetration> {
        if !mover.is_round() {
            return None;
        }
        let core = mover.core();
        let area = mover.aabb(at);
        let mut tests = 0;
        let mut deepest: Option<Penetration> = None;
        let mut consider = |target: HitTarget, contact: Option<Contact>| {
            if let Some(contact) = contact
                && contact.separation < 0.0
                && deepest.is_none_or(|known| -contact.separation > known.depth)
            {
                deepest = Some(Penetration {
                    target,
                    normal: contact.normal,
                    depth: -contact.separation,
                    point: contact.point,
                });
            }
        };

        for leaf in self.tree.leaves(|node: &BvhNode| node.aabb().intersects(&area)) {
            let entry = &self.statics[leaf as usize];
            if !selector.takes_static(entry) {
                continue;
            }
            let target = HitTarget::Static(entry.id);
            match &entry.geometry {
                StaticGeometry::Convex(convex) => {
                    tests += 1;
                    consider(target, narrow::contact(&convex.core(), &entry.pose, &core, at, 0.0));
                }
                StaticGeometry::Mesh(mesh) => {
                    let contact = deepest_in_mesh(mesh, &entry.pose, mover, &core, at, &mut tests);
                    consider(target, contact);
                }
            }
        }
        for body in &self.bodies {
            if !selector.takes_body(body) || !body.aabb.intersects(&area) {
                continue;
            }
            tests += 1;
            let contact = narrow::contact(&body.convex.core(), &body.parry_pose, &core, at, 0.0);
            consider(HitTarget::Body(body.id), contact);
        }
        Counters::add(&self.counters.narrow_tests, tests);
        deepest
    }
}

impl StaticEntry {
    fn shape(&self) -> &dyn ParryShape {
        match &self.geometry {
            StaticGeometry::Convex(convex) => convex.shape(),
            StaticGeometry::Mesh(mesh) => mesh.as_ref(),
        }
    }

    /// Sweeps `mover` against this collider, up to `reach`.
    fn sweep(
        &self,
        mover: &Convex,
        start: &ParryPose,
        direction: Vec3,
        reach: f32,
        separating_passes: bool,
        tests: &mut u32,
    ) -> Option<Sweep> {
        match &self.geometry {
            StaticGeometry::Convex(convex) => narrow::sweep(
                &convex.core(),
                &self.pose,
                &mover.core(),
                start,
                direction,
                reach,
                separating_passes,
            ),
            StaticGeometry::Mesh(mesh) => {
                sweep_mesh(mesh, &self.pose, mover, start, direction, reach, separating_passes, tests)
            }
        }
    }
}

/// A sweep over a mesh's triangles, best first. Everything's in the mesh's space until the end.
#[expect(clippy::too_many_arguments, reason = "the one sweep with a mesh's tree to walk")]
fn sweep_mesh(
    mesh: &TriMesh,
    mesh_pose: &ParryPose,
    mover: &Convex,
    start: &ParryPose,
    direction: Vec3,
    reach: f32,
    separating_passes: bool,
    tests: &mut u32,
) -> Option<Sweep> {
    let local_start = mesh_pose.inv_mul(start);
    let local_direction = mesh_pose.rotation.inverse() * direction;
    let box_at_start = mover.aabb(&local_start);
    let ray = Ray::new(Vec3::ZERO, local_direction);
    let core = mover.core();
    let count = Cell::new(*tests);
    let hit = mesh.bvh().find_best(
        past(reach),
        |node: &BvhNode, best| sweep_cost(node, &box_at_start, &ray, best),
        |triangle, best| {
            count.set(count.get() + 1);
            let triangle = Core::triangle(mesh.triangle(triangle));
            narrow::sweep(
                &triangle,
                &ParryPose::IDENTITY,
                &core,
                &local_start,
                local_direction,
                best,
                separating_passes,
            )
            .map(LeafSweep)
        },
    );
    *tests = count.get();
    let (_, LeafSweep(sweep)) = hit?;
    Some(Sweep {
        normal: mesh_pose.rotation * sweep.normal,
        point: mesh_pose.transform_point(sweep.point),
        ..sweep
    })
}

/// The deepest overlap among the triangles a sphere or capsule's box reaches.
fn deepest_in_mesh(
    mesh: &TriMesh,
    mesh_pose: &ParryPose,
    mover: &Convex,
    core: &Core,
    at: &ParryPose,
    tests: &mut u32,
) -> Option<Contact> {
    let local = mesh_pose.inv_mul(at);
    let area = mover.aabb(&local);
    let mut deepest: Option<Contact> = None;
    for triangle in mesh.bvh().leaves(|node: &BvhNode| node.aabb().intersects(&area)) {
        *tests += 1;
        let triangle = Core::triangle(mesh.triangle(triangle));
        if let Some(contact) = narrow::contact(&triangle, &ParryPose::IDENTITY, core, &local, 0.0)
            && deepest.is_none_or(|known| contact.separation < known.separation)
        {
            deepest = Some(contact);
        }
    }
    let deepest = deepest?;
    Some(Contact {
        normal: mesh_pose.rotation * deepest.normal,
        point: mesh_pose.transform_point(deepest.point),
        separation: deepest.separation,
    })
}

fn overlaps(pose1: &ParryPose, shape1: &dyn ParryShape, pose2: &ParryPose, shape2: &dyn ParryShape) -> bool {
    query::intersection_test(pose1, shape1, pose2, shape2).is_ok_and(|result| result.intersecting)
}

fn swept_box(shape: &Convex, start: &ParryPose, direction: Vec3, distance: f32) -> Aabb {
    let end = ParryPose::from_parts(start.translation + direction * distance, start.rotation);
    shape.aabb(start).merged(&shape.aabb(&end))
}

pub(crate) fn shape_hit(target: HitTarget, sweep: Sweep, max_distance: f32) -> ShapeHit {
    ShapeHit {
        target,
        distance: sweep.distance,
        fraction: if max_distance > 0.0 { sweep.distance / max_distance } else { 0.0 },
        point: sweep.point,
        normal: sweep.normal,
        started_overlapping: sweep.overlapping,
    }
}

/// Keeps `out` sorted while it fills, dropping whichever sorts last once it's full.
fn insert_sorted<T: Copy>(
    out: &mut [T],
    written: &mut usize,
    value: T,
    order: impl Fn(&T, &T) -> std::cmp::Ordering,
) {
    let place = out[..*written].partition_point(|known| order(known, &value).is_lt());
    if place == out.len() {
        return;
    }
    if *written < out.len() {
        *written += 1;
    }
    out[place..*written].rotate_right(1);
    out[place] = value;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_sorted_keeps_the_smallest() {
        let mut out = [0; 3];
        let mut written = 0;
        for value in [5, 1, 4, 3, 2] {
            insert_sorted(&mut out, &mut written, value, |a, b| a.cmp(b));
        }
        assert_eq!(written, 3);
        assert_eq!(out, [1, 2, 3]);
    }

    #[test]
    fn insert_sorted_fits_an_empty_buffer() {
        let mut out: [i32; 0] = [];
        let mut written = 0;
        insert_sorted(&mut out, &mut written, 1, |a, b| a.cmp(b));
        assert_eq!(written, 0);
    }
}
