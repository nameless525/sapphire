//! Plain-data frame input (rule R4). Everything here is `Copy` data with no references into
//! the simulation; the app (and in M1 the extract systems) fills it once per frame.

use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3};

use crate::MeshHandle;

/// Everything the renderer needs to draw one frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameView<'a> {
    pub camera: CameraData,
    pub sun: DirectionalLight,
    pub instances: &'a [MeshInstance],
    pub sky: SkyGradient,
    /// Presentation time in seconds, read from the owning clock (rule R2), for shader animation.
    pub time_seconds: f32,
}

/// The main camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraData {
    /// World to view space: right-handed, the camera looks down −Z.
    pub view: Mat4,
    /// View to clip space. Must be reverse-Z with an infinite far plane and Y-up NDC, as built
    /// by `glam::camera::rh::proj::directx::perspective_infinite_reverse` (WebGPU shares
    /// DirectX's clip space).
    pub proj: Mat4,
    /// World-space eye position.
    pub position: Vec3,
}

impl CameraData {
    /// A perspective camera at `eye` looking at `target`.
    pub fn look_at(eye: Vec3, target: Vec3, up: Vec3, fov_y_radians: f32, aspect: f32, z_near: f32) -> Self {
        Self {
            view: view::look_at_mat4(eye, target, up),
            proj: directx::perspective_infinite_reverse(fov_y_radians, aspect, z_near),
            position: eye,
        }
    }
}

/// The key light. For characters this is the artist-curved virtual key light (spec §5.12).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DirectionalLight {
    /// Direction the light travels, from the sun into the scene. Need not be normalized.
    pub direction: Vec3,
    /// Linear RGB.
    pub color: Vec3,
    pub intensity: f32,
}

/// One drawn mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshInstance {
    pub mesh: MeshHandle,
    /// Object to world. Mirroring (negative determinant) is not supported in M0: it flips
    /// winding, so the toon and outline passes would cull the wrong faces.
    pub transform: Mat4,
}

/// A three-stop sky gradient in linear RGB. It also drives the toon shader's ambient term.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyGradient {
    pub zenith: Vec3,
    pub horizon: Vec3,
    pub ground: Vec3,
}

impl Default for SkyGradient {
    /// A clear midday sky.
    fn default() -> Self {
        Self {
            zenith: Vec3::new(0.10, 0.30, 0.80),
            horizon: Vec3::new(0.62, 0.78, 0.95),
            ground: Vec3::new(0.36, 0.34, 0.42),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_is_reverse_z_with_infinite_far() {
        let camera = CameraData::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y, 1.0, 1.5, 0.1);
        let depth = |z: f32| {
            let clip = camera.proj * camera.view * Vec3::new(0.0, 0.0, z).extend(1.0);
            clip.z / clip.w
        };
        assert!((depth(4.9) - 1.0).abs() < 1e-5, "near plane maps to 1");
        assert!(depth(0.0) > depth(-100.0), "nearer is greater");
        assert!(depth(-1.0e7) > 0.0 && depth(-1.0e7) < 1.0e-6, "far away approaches 0");
    }
}
