//! The engine's shape and pose types, and their `parry3d` counterparts, which stay in this crate.

use glam::{Quat, Vec3};
use parry3d::bounding_volume::Aabb;
use parry3d::math::Pose as ParryPose;
use parry3d::shape::{Ball, Capsule, Cuboid, Shape as ParryShape};
use sapphire_colliders::document::MIN_EXTENT;

use crate::narrow::Core;

/// A position and rotation: no scale, which collision shapes don't take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: Vec3,
    pub rotation: Quat,
}

impl Pose {
    pub const IDENTITY: Self = Self { position: Vec3::ZERO, rotation: Quat::IDENTITY };

    /// Unrotated, at `position`.
    pub const fn at(position: Vec3) -> Self {
        Self { position, rotation: Quat::IDENTITY }
    }

    pub const fn new(position: Vec3, rotation: Quat) -> Self {
        Self { position, rotation }
    }

    pub(crate) fn to_parry(self) -> ParryPose {
        ParryPose::from_parts(self.position, self.rotation)
    }

    pub(crate) fn is_valid(self) -> bool {
        self.position.is_finite() && self.rotation.is_finite() && self.rotation.is_normalized()
    }
}

/// A convex shape for a body or a query, in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Box {
        half_extents: Vec3,
    },
    Sphere {
        radius: f32,
    },
    /// Along local +Y: its cylinder's half height, then the radius of its caps.
    Capsule {
        half_height: f32,
        radius: f32,
    },
}

impl Shape {
    /// Whether every extent is finite and at least `sapphire_colliders`' minimum.
    pub fn is_valid(&self) -> bool {
        let extent = |v: f32| v.is_finite() && v >= MIN_EXTENT;
        match *self {
            Self::Box { half_extents } => half_extents.to_array().into_iter().all(extent),
            Self::Sphere { radius } => extent(radius),
            Self::Capsule { half_height, radius } => extent(half_height) && extent(radius),
        }
    }

    pub(crate) fn to_convex(self) -> Convex {
        match self {
            Self::Box { half_extents } => Convex::Box(Cuboid::new(half_extents)),
            Self::Sphere { radius } => Convex::Sphere(Ball::new(radius)),
            Self::Capsule { half_height, radius } => Convex::Capsule(Capsule::new_y(half_height, radius)),
        }
    }
}

/// A [`Shape`] as `parry3d` knows it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Convex {
    Box(Cuboid),
    Sphere(Ball),
    Capsule(Capsule),
}

impl Convex {
    pub(crate) fn shape(&self) -> &dyn ParryShape {
        match self {
            Self::Box(shape) => shape,
            Self::Sphere(shape) => shape,
            Self::Capsule(shape) => shape,
        }
    }

    pub(crate) fn aabb(&self, pose: &ParryPose) -> Aabb {
        self.shape().compute_aabb(pose)
    }

    /// This shape as a thin core and a radius around it.
    pub(crate) fn core(&self) -> Core {
        match self {
            Self::Box(cuboid) => Core::cuboid(*cuboid),
            Self::Sphere(ball) => Core::point(ball.radius),
            Self::Capsule(capsule) => Core::segment(capsule.segment, capsule.radius),
        }
    }

    pub(crate) fn is_round(&self) -> bool {
        !matches!(self, Self::Box(_))
    }
}
