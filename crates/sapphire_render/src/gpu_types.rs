//! GPU-visible structs. Each mirrors a WGSL struct in `shaders/frame.wgsl`; the layout tests
//! in `shaders.rs` compare sizes against naga's layout of the real shader source.

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec3};

use crate::lights::LightUniforms;
use crate::{CameraData, FramePacket};

/// Per-frame constants (WGSL `FrameUniforms`, group 0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct FrameUniforms {
    /// xyz: unit vector toward the sun.
    pub sun_direction: [f32; 4],
    /// rgb: sun color × intensity.
    pub sun_radiance: [f32; 4],
    pub sky_zenith: [f32; 4],
    pub sky_horizon: [f32; 4],
    pub sky_ground: [f32; 4],
    /// x: time in seconds (wrapped by the app, see `FramePacket::time_seconds`).
    pub time: [f32; 4],
    /// Light and list counts (see `LightUniforms`).
    pub lights: [u32; 4],
    /// The cluster grid's depth slices.
    pub clusters: [f32; 4],
}

impl FrameUniforms {
    pub(crate) fn new(frame: &FramePacket, lights: LightUniforms) -> Self {
        let toward_sun = (-frame.sun.direction).try_normalize().unwrap_or(Vec3::Y);
        Self {
            sun_direction: toward_sun.extend(0.0).to_array(),
            sun_radiance: (frame.sun.color * frame.sun.intensity).extend(0.0).to_array(),
            sky_zenith: frame.sky.zenith.extend(1.0).to_array(),
            sky_horizon: frame.sky.horizon.extend(1.0).to_array(),
            sky_ground: frame.sky.ground.extend(1.0).to_array(),
            time: [frame.time_seconds, 0.0, 0.0, 0.0],
            lights: lights.lights,
            clusters: lights.clusters,
        }
    }
}

/// One view: a camera rendering into a target of a given size (WGSL `ViewUniforms`, group 1,
/// bound with a dynamic offset).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct ViewUniforms {
    pub view_from_world: [[f32; 4]; 4],
    pub clip_from_view: [[f32; 4]; 4],
    pub clip_from_world: [[f32; 4]; 4],
    pub world_from_clip: [[f32; 4]; 4],
    /// xyz: camera position.
    pub position: [f32; 4],
    /// xy: target size in pixels; zw: reciprocals.
    pub viewport: [f32; 4],
}

impl ViewUniforms {
    pub(crate) fn new(camera: &CameraData, (width, height): (u32, u32)) -> Self {
        let clip_from_world = camera.proj * camera.view;
        let (width, height) = (width as f32, height as f32);
        Self {
            view_from_world: camera.view.to_cols_array_2d(),
            clip_from_view: camera.proj.to_cols_array_2d(),
            clip_from_world: clip_from_world.to_cols_array_2d(),
            world_from_clip: clip_from_world.inverse().to_cols_array_2d(),
            position: camera.position.extend(1.0).to_array(),
            viewport: [width, height, width.recip(), height.recip()],
        }
    }
}

/// One mesh instance (WGSL `InstanceData`, storage buffer indexed by instance).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct InstanceData {
    pub world_from_object: [[f32; 4]; 4],
    /// Inverse-transpose of the upper 3×3, as WGSL `mat3x3<f32>` (columns padded to 16 bytes).
    pub normal_from_object: [[f32; 4]; 3],
}

impl InstanceData {
    pub(crate) fn new(transform: Mat4) -> Self {
        let inverse = Mat3::from_mat4(transform).inverse();
        // A degenerate (zero-scale) transform has no inverse; its triangles have no area anyway.
        let normal = if inverse.is_finite() { inverse.transpose() } else { Mat3::IDENTITY };
        Self {
            world_from_object: transform.to_cols_array_2d(),
            normal_from_object: [
                normal.x_axis.extend(0.0).to_array(),
                normal.y_axis.extend(0.0).to_array(),
                normal.z_axis.extend(0.0).to_array(),
            ],
        }
    }
}

/// One draw record: which instance and which material a draw uses. Read by the vertex
/// shaders as instance-step vertex attributes (WGSL `DrawRecord`, locations 5 and 6) from a
/// buffer bound at the draw's record; see the crate docs on submission paths.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub(crate) struct DrawData {
    pub instance: u32,
    pub material: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_matrix_handles_non_uniform_and_degenerate_scale() {
        let data = InstanceData::new(Mat4::from_scale(Vec3::new(2.0, 1.0, 1.0)));
        assert_eq!(data.normal_from_object[0], [0.5, 0.0, 0.0, 0.0]);
        let flat = InstanceData::new(Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0)));
        assert_eq!(flat.normal_from_object[1], [0.0, 1.0, 0.0, 0.0]);
    }
}
