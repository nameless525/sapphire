//! What a world holds, for tools that look at it, such as the sandbox's collider view (M2 T1).
//!
//! These only read, past the work counters as well as the state, so looking can't move a checksum or
//! a stat. Neither iterator allocates.

use glam::Vec3;
use parry3d::math::Pose as ParryPose;
use sapphire_colliders::GroupMask;

use crate::shape::{Convex, Pose, Shape};
use crate::world::{BodyDesc, ColliderId, PhysicsWorld, StaticEntry, StaticGeometry};

/// A static collider as the world holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StaticView<'a> {
    pub id: ColliderId,
    pub geometry: StaticGeometryView<'a>,
    pub pose: Pose,
    pub memberships: GroupMask,
}

/// A static collider's geometry, in its own space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StaticGeometryView<'a> {
    Convex(Shape),
    Mesh { vertices: &'a [Vec3], triangles: &'a [[u32; 3]] },
}

impl PhysicsWorld {
    /// Every static collider, in ID order.
    pub fn statics(&self) -> impl ExactSizeIterator<Item = StaticView<'_>> {
        self.statics.iter().map(static_view)
    }

    /// Every body as it stands now, in ID order.
    pub fn bodies(&self) -> impl ExactSizeIterator<Item = BodyDesc> + '_ {
        self.bodies.iter().map(|body| BodyDesc {
            id: body.id,
            shape: body.shape,
            pose: body.pose,
            groups: body.groups,
        })
    }
}

fn static_view(entry: &StaticEntry) -> StaticView<'_> {
    let geometry = match &entry.geometry {
        StaticGeometry::Convex(convex) => StaticGeometryView::Convex(shape_of(convex)),
        StaticGeometry::Mesh(mesh) => {
            StaticGeometryView::Mesh { vertices: mesh.vertices(), triangles: mesh.indices() }
        }
    };
    StaticView { id: entry.id, geometry, pose: pose_of(entry.pose), memberships: entry.memberships }
}

/// [`Shape::to_convex`] undone. A capsule's segment runs along +Y, as that made it.
fn shape_of(convex: &Convex) -> Shape {
    match convex {
        Convex::Box(cuboid) => Shape::Box { half_extents: cuboid.half_extents },
        Convex::Sphere(ball) => Shape::Sphere { radius: ball.radius },
        Convex::Capsule(capsule) => {
            Shape::Capsule { half_height: capsule.half_height(), radius: capsule.radius }
        }
    }
}

fn pose_of(pose: ParryPose) -> Pose {
    Pose::new(pose.translation, pose.rotation)
}
