//! GPU-visible structs. Each mirrors a WGSL struct in `shaders/frame.wgsl`; the layout tests
//! in `shaders.rs` compare sizes against naga's layout of the real shader source.

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec3};

use crate::FrameView;

/// Per-frame constants (WGSL `FrameUniforms`, a uniform buffer).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct FrameUniforms {
    pub view: [[f32; 4]; 4],
    pub proj: [[f32; 4]; 4],
    pub view_proj: [[f32; 4]; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    /// xyz: camera position; w: time in seconds.
    pub camera_position: [f32; 4],
    /// xyz: unit vector toward the sun.
    pub sun_direction: [f32; 4],
    /// rgb: sun color × intensity.
    pub sun_radiance: [f32; 4],
    pub sky_zenith: [f32; 4],
    pub sky_horizon: [f32; 4],
    pub sky_ground: [f32; 4],
    /// xy: target size in pixels; zw: reciprocals.
    pub viewport: [f32; 4],
}

impl FrameUniforms {
    pub(crate) fn new(frame: &FrameView<'_>, (width, height): (u32, u32)) -> Self {
        let camera = &frame.camera;
        let view_proj = camera.proj * camera.view;
        let toward_sun = (-frame.sun.direction).try_normalize().unwrap_or(Vec3::Y);
        let (width, height) = (width as f32, height as f32);
        Self {
            view: camera.view.to_cols_array_2d(),
            proj: camera.proj.to_cols_array_2d(),
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            camera_position: camera.position.extend(frame.time_seconds).to_array(),
            sun_direction: toward_sun.extend(0.0).to_array(),
            sun_radiance: (frame.sun.color * frame.sun.intensity).extend(0.0).to_array(),
            sky_zenith: frame.sky.zenith.extend(1.0).to_array(),
            sky_horizon: frame.sky.horizon.extend(1.0).to_array(),
            sky_ground: frame.sky.ground.extend(1.0).to_array(),
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

/// One draw record (WGSL `DrawData`): which instance and which material a draw uses.
///
/// Indexed by the draw ID the vertex shader receives as an attribute (see `draw_ids`). This
/// is also the layout GPU-driven culling will write for multi-draw indirect later, and it
/// lets one instance use several materials.
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
