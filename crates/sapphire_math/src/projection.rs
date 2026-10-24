//! View and projection matrices for the engine's conventions (see the crate docs).
//!
//! glam 0.34 already provides the right matrices in `glam::camera`, whose "DirectX" projection
//! convention (z in [0, 1], +Y up) is also wgpu's. This module names the ones the engine uses,
//! so call sites cannot pick a Vulkan (Y-down) or OpenGL (z in [−1, 1]) variant, or forget that
//! cameras face +Z, by accident.

use glam::Mat4;

use crate::Transform;

/// The view matrix of a camera whose world transform is `camera`.
///
/// The camera faces its local +Z ([`crate::FORWARD`]) like every other object; the view matrix
/// maps that direction to view-space −Z, its local up to +Y and its local right to +X. Built
/// with glam's right-handed `look_to`, so the result is exactly affine (bottom row `0 0 0 1`)
/// even far from the origin. Scale is ignored. `camera.rotation` must be normalized.
#[must_use]
pub fn view_from_transform(camera: &Transform) -> Mat4 {
    debug_assert!(camera.rotation.is_normalized(), "camera rotation {} is not normalized", camera.rotation);
    glam::camera::rh::view::look_to_mat4(camera.translation, camera.forward(), camera.up())
}

/// The main-camera projection: right-handed, reverse-Z, with the far plane at infinity.
///
/// `fov_y` is the full vertical field of view in radians, `aspect` is width / height, and
/// `near` is the near-plane distance in meters. View-space points at distance `near` in front of
/// the camera (z = −near) map to depth 1, and depth falls toward 0 as distance grows. The
/// resulting [`crate::Frustum`] has five planes, since there is no far plane.
///
/// For a finite reverse-Z projection (shadow cascades, reflection probes), call
/// `glam::camera::rh::proj::directx::perspective(fov_y, aspect, far, near)` with near and far
/// swapped.
#[must_use]
pub fn perspective_reverse_z_infinite(fov_y: f32, aspect: f32, near: f32) -> Mat4 {
    debug_assert!(fov_y > 0.0 && fov_y < std::f32::consts::PI, "fov_y {fov_y} outside (0, π)");
    debug_assert!(aspect > 0.0, "aspect {aspect} must be positive");
    debug_assert!(near > 0.0, "near {near} must be positive");
    glam::camera::rh::proj::directx::perspective_infinite_reverse(fov_y, aspect, near)
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use glam::{Quat, Vec3, Vec4};

    use super::*;
    use crate::{FORWARD, RIGHT, UP};

    fn ndc(proj: &Mat4, view_point: Vec3) -> Vec3 {
        let clip = *proj * Vec4::from((view_point, 1.0));
        clip.truncate() / clip.w
    }

    #[test]
    fn near_maps_to_one_and_infinity_to_zero() {
        let proj = perspective_reverse_z_infinite(FRAC_PI_2, 16.0 / 9.0, 0.1);
        assert!((ndc(&proj, Vec3::new(0.0, 0.0, -0.1)).z - 1.0).abs() < 1e-6);
        assert!((ndc(&proj, Vec3::new(0.0, 0.0, -1.0)).z - 0.1).abs() < 1e-6);
        assert!(ndc(&proj, Vec3::new(0.0, 0.0, -1.0e6)).z < 1e-6);
        assert!(ndc(&proj, Vec3::new(0.0, 0.0, -1.0e6)).z > 0.0);
    }

    #[test]
    fn y_is_up_and_fov_is_vertical() {
        let proj = perspective_reverse_z_infinite(FRAC_PI_2, 2.0, 0.1);
        // With a 90° vertical FOV, the top edge at distance d is at y = d.
        let top = ndc(&proj, Vec3::new(0.0, 5.0, -5.0));
        assert!((top.y - 1.0).abs() < 1e-6, "{top}");
        // Aspect 2 doubles the horizontal extent.
        let right = ndc(&proj, Vec3::new(10.0, 0.0, -5.0));
        assert!((right.x - 1.0).abs() < 1e-6, "{right}");
    }

    #[test]
    fn camera_forward_maps_to_view_negative_z() {
        let camera = Transform::from_translation_rotation(
            Vec3::new(3.0, 1.5, -2.0),
            Quat::from_euler(glam::EulerRot::YXZ, 0.8, -0.3, 0.1),
        );
        let view = view_from_transform(&camera);
        let eye = camera.translation;
        assert!(view.transform_point3(eye).abs_diff_eq(Vec3::ZERO, 1e-5));
        assert!(view.transform_vector3(camera.forward()).abs_diff_eq(Vec3::NEG_Z, 1e-5));
        assert!(view.transform_vector3(camera.up()).abs_diff_eq(Vec3::Y, 1e-5));
        assert!(view.transform_vector3(camera.right()).abs_diff_eq(Vec3::X, 1e-5));
        // A point ahead of the camera lands in front of it in view space.
        assert!(view.transform_point3(camera.transform_point(FORWARD * 10.0)).z < 0.0);
        assert_eq!(view.row(3), Vec4::W, "exactly affine");
    }

    #[test]
    fn identity_camera_is_a_half_turn() {
        let view = view_from_transform(&Transform::IDENTITY);
        assert!(view.transform_vector3(FORWARD).abs_diff_eq(Vec3::NEG_Z, 1e-6));
        assert!(view.transform_vector3(RIGHT).abs_diff_eq(Vec3::X, 1e-6));
        assert!(view.transform_vector3(UP).abs_diff_eq(Vec3::Y, 1e-6));
    }
}
