//! Math for Sapphire (spec §5.4): glam re-exports plus the engine's transforms, bounds, culling
//! volumes, projection, springs and deterministic scalar functions.
//!
//! # Coordinate convention
//!
//! These are engine-wide decisions; every crate, tool and shader follows them.
//!
//! - **Right-handed and Y-up. Every object faces +Z in its local space, cameras included,** so
//!   [`FORWARD`] is +Z, [`UP`] is +Y and [`RIGHT`] is −X. This is glTF 2.0's convention ("the
//!   front of a glTF asset faces +Z", §3.4) and VRM 1.0's, so imported characters, props and
//!   levels need no axis conversion, in the cooker or anywhere else.
//! - **View space is glam's and wgpu's**: the camera looks down −Z with +X right. A camera's view
//!   matrix therefore turns its local +Z forward onto view-space −Z (a half turn about Y), as in
//!   Unity. Build it with [`projection::view_from_transform`], never by inverting the camera's
//!   world matrix directly.
//! - **Units are meters** (and radians, and seconds). World space is `f32`; each zone stays
//!   within about 4 km of its own origin (spec §5.4).
//! - **Clip space is wgpu's**: x and y in [−1, 1] with +Y up, and z in [0, 1].
//! - **Depth is reverse-Z**: the near plane maps to depth 1 and the far plane (usually at
//!   infinity) to 0. Depth buffers clear to `0.0` and test with `GreaterEqual`. This spreads
//!   `f32` depth precision evenly over distance. See [`projection`] and [`Frustum`].
//! - Matrices are column-major and multiply column vectors (`clip = proj * view * world * p`),
//!   as in glam and WGSL.
//!
//! # Hot paths
//!
//! Bounds and planes store [`Vec3A`] (16-byte aligned, SIMD friendly); [`Transform`] stores
//! [`Vec3`] because it is mostly authored data. Use [`Affine3A`] for transforms applied in bulk.
//!
//! # Not here yet
//!
//! `Ray`, easing and piecewise curves, and the seeded simulation RNG (spec §5.4) arrive with
//! their first consumers in M1–M2.

pub mod bounds;
pub mod det;
pub mod frustum;
pub mod projection;
pub mod spring;
pub mod transform;

pub use glam;
pub use glam::{Affine2, Affine3A, EulerRot, Mat2, Mat3, Mat3A, Mat4, Quat, Vec2, Vec3, Vec3A, Vec4};

pub use bounds::{Aabb, Sphere};
pub use frustum::{Frustum, Plane};
pub use spring::{SpringDamper, damp};
pub use transform::Transform;

/// World up: +Y.
pub const UP: Vec3 = Vec3::Y;

/// Local forward of every object at identity rotation, cameras included: +Z (glTF 2.0 §3.4).
pub const FORWARD: Vec3 = Vec3::Z;

/// Local right: −X, which makes (right, up, forward) right-handed with forward +Z.
pub const RIGHT: Vec3 = Vec3::NEG_X;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_are_right_handed() {
        // Right-handed: forward × up = right, and up × right = forward.
        assert_eq!(FORWARD.cross(UP), RIGHT);
        assert_eq!(UP.cross(RIGHT), FORWARD);
        assert_eq!(FORWARD, Vec3::Z, "glTF 2.0 §3.4: the front of an asset faces +Z");
    }
}
