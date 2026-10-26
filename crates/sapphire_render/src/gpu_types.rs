//! GPU-visible structs. Each mirrors a WGSL struct in `shaders/frame.wgsl`; the layout tests
//! in `shaders.rs` compare sizes against naga's layout of the real shader source.

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec2, Vec3};

use crate::fog::GpuFog;
use crate::lights::LightUniforms;
use crate::{CameraData, FramePacket};

/// The period, in seconds, shaders see time wrapped to: an hour keeps f32 precision to a
/// fraction of a millisecond. Only periodic effects (star twinkle) read it; motion that must not
/// jump at the wrap (cloud drift) is reduced on the CPU in `f64` instead.
pub(crate) const SHADER_TIME_PERIOD: f64 = 3600.0;

/// Per-frame constants (WGSL `FrameUniforms`, group 0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct FrameUniforms {
    /// xyz: unit vector toward the world's key light (`FramePacket::sun`); characters take their
    /// light rigs' instead.
    pub sun_direction: [f32; 4],
    /// rgb: the world's key light's color × intensity.
    pub sun_radiance: [f32; 4],
    pub sky_zenith: [f32; 4],
    pub sky_horizon: [f32; 4],
    pub sky_ground: [f32; 4],
    /// x: presentation time in seconds, wrapped to [`SHADER_TIME_PERIOD`] (see
    /// `FramePacket::time_seconds`).
    pub time: [f32; 4],
    /// Light and list counts (see `LightUniforms`).
    pub lights: [u32; 4],
    /// The cluster grid's depth slices.
    pub clusters: [f32; 4],
    /// 0 by day, 1 at night (`FramePacket::day_night`).
    pub day_night: f32,
    /// The scale on materials' outline LOD cutoffs ([`crate::Renderer::set_outline_distance`],
    /// M1 C4).
    pub outline_distance: f32,
    /// Pads to the next vector, as WGSL aligns it.
    pub padding: [f32; 2],
    /// rgb: ambient light from above (`FramePacket::ambient`, sky × intensity).
    pub ambient_sky: [f32; 4],
    /// rgb: ambient light from below (ground × intensity).
    pub ambient_ground: [f32; 4],
    /// rgb: the share of the key light kept in shadow (`FramePacket::shadow_tint`).
    pub shadow_tint: [f32; 4],
    /// x: wetness; y: puddle coverage (`FramePacket::weather`).
    pub weather: [f32; 4],
    /// The fog over the world and the sky (`FramePacket::fog`, M1 W3).
    pub fog: GpuFog,
}

impl FrameUniforms {
    pub(crate) fn new(frame: &FramePacket, lights: LightUniforms, outline_distance: f32) -> Self {
        let toward_sun = (-frame.sun.direction).try_normalize().unwrap_or(Vec3::Y);
        Self {
            sun_direction: toward_sun.extend(0.0).to_array(),
            sun_radiance: (frame.sun.color * frame.sun.intensity).extend(0.0).to_array(),
            sky_zenith: frame.sky.zenith.extend(1.0).to_array(),
            sky_horizon: frame.sky.horizon.extend(1.0).to_array(),
            sky_ground: frame.sky.ground.extend(1.0).to_array(),
            // NaN clamps to 0, so a bad packet draws day rather than spreading NaN.
            time: [wrap_time(frame.time_seconds), 0.0, 0.0, 0.0],
            lights: lights.lights,
            clusters: lights.clusters,
            day_night: frame.day_night,
            outline_distance,
            padding: [0.0; 2],
            ambient_sky: (frame.ambient.sky * frame.ambient.intensity).extend(0.0).to_array(),
            ambient_ground: (frame.ambient.ground * frame.ambient.intensity).extend(0.0).to_array(),
            shadow_tint: frame.shadow_tint.extend(0.0).to_array(),
            weather: [frame.weather.wetness, frame.weather.puddles, 0.0, 0.0],
            fog: GpuFog::new(&frame.fog),
        }
    }
}

/// `seconds` wrapped into `0..SHADER_TIME_PERIOD`, as shaders see time; 0 for a time that isn't
/// finite.
pub(crate) fn wrap_time(seconds: f64) -> f32 {
    if seconds.is_finite() { seconds.rem_euclid(SHADER_TIME_PERIOD) as f32 } else { 0.0 }
}

/// One view: a camera rendering into a target of a given size (WGSL `ViewUniforms`, group 1,
/// bound with a dynamic offset).
///
/// Under temporal AA (M1 R7) the projection is jittered by a sub-pixel offset each frame, and
/// every matrix that maps to clip space carries it, so every pass rasterizes the same jittered
/// samples. Velocity must not move with the jitter, so the view also keeps its unjittered
/// `clip_from_world`, and `previous_clip_from_world` is unjittered too.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct ViewUniforms {
    pub view_from_world: [[f32; 4]; 4],
    /// Jittered under temporal AA.
    pub clip_from_view: [[f32; 4]; 4],
    /// Jittered under temporal AA.
    pub clip_from_world: [[f32; 4]; 4],
    /// The inverse of the jittered `clip_from_world`.
    pub world_from_clip: [[f32; 4]; 4],
    /// The unjittered `clip_from_world` in the last frame this view drew, for velocity.
    pub previous_clip_from_world: [[f32; 4]; 4],
    /// xyz: camera position.
    pub position: [f32; 4],
    /// xy: target size in pixels; zw: reciprocals.
    pub viewport: [f32; 4],
    /// `clip_from_world` without the jitter: what velocity compares with the last frame's.
    pub unjittered_clip_from_world: [[f32; 4]; 4],
    /// xy: this frame's jitter in pixels, x right and y down (the image moves by it); zw: the
    /// same in NDC. Zero without temporal AA.
    pub jitter: [f32; 4],
}

impl ViewUniforms {
    /// The view from `camera` into a target of `size` pixels, jittered by `jitter` pixels (x
    /// right, y down; zero without temporal AA), which drew the last frame with the unjittered
    /// `previous_clip_from_world` (its own `proj × view` on the first frame).
    pub(crate) fn new(
        camera: &CameraData,
        (width, height): (u32, u32),
        previous_clip_from_world: Mat4,
        jitter: [f32; 2],
    ) -> Self {
        let (width, height) = (width as f32, height as f32);
        let clip_from_view = jittered_projection(camera.proj, jitter, (width, height));
        let clip_from_world = clip_from_view * camera.view;
        let ndc = jitter_ndc(jitter, (width, height));
        Self {
            view_from_world: camera.view.to_cols_array_2d(),
            clip_from_view: clip_from_view.to_cols_array_2d(),
            clip_from_world: clip_from_world.to_cols_array_2d(),
            world_from_clip: clip_from_world.inverse().to_cols_array_2d(),
            previous_clip_from_world: previous_clip_from_world.to_cols_array_2d(),
            position: camera.position.extend(1.0).to_array(),
            viewport: [width, height, width.recip(), height.recip()],
            unjittered_clip_from_world: (camera.proj * camera.view).to_cols_array_2d(),
            jitter: [jitter[0], jitter[1], ndc.x, ndc.y],
        }
    }
}

/// `jitter` pixels (x right, y down) in NDC for a target of `size` pixels: NDC spans 2 units
/// across the target, and its y runs up.
fn jitter_ndc(jitter: [f32; 2], (width, height): (f32, f32)) -> Vec2 {
    Vec2::new(2.0 * jitter[0] / width, -2.0 * jitter[1] / height)
}

/// `proj` with its image moved by `jitter` pixels: a translation in clip space by the NDC offset
/// times w, which leaves depth and every other entry of a perspective projection alone (its
/// focal lengths, which outline widths read, included). Exactly `proj` for a zero jitter.
pub(crate) fn jittered_projection(proj: Mat4, jitter: [f32; 2], size: (f32, f32)) -> Mat4 {
    if jitter == [0.0, 0.0] {
        return proj;
    }
    Mat4::from_translation(jitter_ndc(jitter, size).extend(0.0)) * proj
}

/// One mesh instance (WGSL `InstanceData`, storage buffer indexed by instance). 192 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct InstanceData {
    pub world_from_object: [[f32; 4]; 4],
    /// Inverse-transpose of the upper 3×3, as WGSL `mat3x3<f32>` (columns padded to 16 bytes).
    pub normal_from_object: [[f32; 4]; 3],
    /// `world_from_object` in the last frame drawn, for velocity: equal to it unless the
    /// instance moved since.
    pub previous_world_from_object: [[f32; 4]; 4],
    /// The instance's entry in the frame's character buffer: 0, the world's key light, unless
    /// the frame lists it as a character (see the `characters` module).
    pub character: u32,
    /// How far its character has faded (M1 C7, 1 − [`CharacterInstance::opacity`]): 0 drawn
    /// whole. The prepasses' faded pipelines dither by it.
    ///
    /// [`CharacterInstance::opacity`]: crate::CharacterInstance::opacity
    pub fade: f32,
    /// Pads the struct to WGSL's 16-byte storage alignment.
    pub padding: [u32; 2],
}

impl InstanceData {
    /// A new instance at `transform`, with no motion yet.
    pub(crate) fn new(transform: Mat4) -> Self {
        let mut data = Self::zeroed();
        data.set_transform(transform);
        data.previous_world_from_object = data.world_from_object;
        data
    }

    /// Moves the instance to `transform`, keeping where it was last drawn.
    pub(crate) fn set_transform(&mut self, transform: Mat4) {
        let inverse = Mat3::from_mat4(transform).inverse();
        // A degenerate (zero-scale) transform has no inverse; its triangles have no area anyway.
        let normal = if inverse.is_finite() { inverse.transpose() } else { Mat3::IDENTITY };
        self.world_from_object = transform.to_cols_array_2d();
        self.normal_from_object = [
            normal.x_axis.extend(0.0).to_array(),
            normal.y_axis.extend(0.0).to_array(),
            normal.z_axis.extend(0.0).to_array(),
        ];
    }

    /// After a frame drew it: its next frame starts where this one ended. Returns whether it
    /// moved in this frame, in which case it must be uploaded once more to come to rest.
    pub(crate) fn settle(&mut self) -> bool {
        let moved = self.previous_world_from_object != self.world_from_object;
        self.previous_world_from_object = self.world_from_object;
        moved
    }
}

/// One draw record: which instance and which material a draw uses, and how to decode its
/// mesh's vertices. Read by the vertex shaders as instance-step vertex attributes (WGSL
/// `DrawRecord`, locations 5–9) from a buffer bound at the draw's record; see the crate docs on
/// submission paths. 48 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct DrawData {
    pub instance: u32,
    pub material: u32,
    pub dequantize: Dequantize,
}

/// A mesh's vertex dequantization: object position = `position_offset + position_scale × p`,
/// `uv = uv_transform.xy + uv_transform.zw × uv`, with `p` and `uv` as the vertex fetch reads
/// them (see `sapphire_assets::mesh::ArchivedVertexStreams::position_transform`). 40 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct Dequantize {
    pub position_offset: [f32; 3],
    pub position_scale: [f32; 3],
    pub uv_transform: [f32; 4],
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

    #[test]
    fn instances_keep_their_last_drawn_transform_until_they_settle() {
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        let mut data = InstanceData::new(at(1.0));
        assert_eq!(data.previous_world_from_object, data.world_from_object, "new instances don't move");
        data.set_transform(at(2.0));
        data.set_transform(at(3.0));
        assert_eq!(data.previous_world_from_object, at(1.0).to_cols_array_2d(), "drawn last at 1");
        assert!(data.settle(), "moved");
        assert_eq!(data.previous_world_from_object, at(3.0).to_cols_array_2d());
        assert!(!data.settle(), "at rest");
    }
}
