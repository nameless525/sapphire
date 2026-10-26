//! Clustered forward+ (spec §5.12 "Lighting", the desktop path): a compute pass bins the frame's
//! lights into a grid of view-space clusters, and each anime character pixel walks only its
//! cluster's list.
//!
//! # The grid
//!
//! - **Tiles:** 16 × 9 equal fractions of the target, so the grid keeps its shape at any size
//!   (120-pixel squares at 1080p). Tile rows count from the top, like pixel rows.
//! - **Slices:** 24 in view depth. Slice 0 is everything nearer than [`SLICE_START`]; slices 1
//!   to 23 split [`SLICE_START`] to [`SLICE_FAR`] exponentially, each about 31% deeper than the
//!   one before, so clusters keep their proportions at every depth; the last slice runs on to
//!   infinity, matching the infinite far plane. A pixel finds its slice from its reverse-Z depth `z`:
//!   view depth is `near / z` (see the crate docs' conventions).
//! - **Clusters** are numbered `(slice × 9 + tile y) × 16 + tile x`.
//!
//! # The cluster buffer
//!
//! One storage buffer holds every cluster's list at a fixed stride: a count, then 16-bit light
//! indices, two per word (low half first). Each cluster lists at most
//! `min(max_lights, 64)` lights ([`MAX_LIGHTS_PER_CLUSTER`]), so at the default light budgets no
//! cluster can overflow; with more lights allowed, a full cluster keeps the lights that come
//! first in the packet, and the rest of its lights are skipped for its pixels. The stride is
//! `1 + capacity / 2` words, so the buffer is 456 KB at 64 lights per cluster.
//!
//! # The pass
//!
//! One thread per cluster, 128 to a workgroup. Each workgroup loads the lights into workgroup
//! memory a batch at a time, in view space, and every thread tests the batch against its
//! cluster's box: the light's range sphere against the box, then, for spots narrower than 90°,
//! the cone against a sphere around the box. Lists come out in index order, so the GPU result
//! is deterministic and matches the CPU [`reference`] the tests compare it with.
//!
//! Buffers aren't frame graph resources (see the graph's later milestones), so the pass is a
//! side effect added after the uploads and before the main pass. It runs only on frames that
//! have lights.

use std::num::NonZeroU64;

use sapphire_rhi::wgpu;

use super::GpuLight;
use crate::gpu_types::FrameUniforms;
use crate::shaders::{self, Program, ShaderSources};

pub(crate) const TILES_X: u32 = 16;
pub(crate) const TILES_Y: u32 = 9;
pub(crate) const SLICES: u32 = 24;
pub(crate) const CLUSTER_COUNT: u32 = TILES_X * TILES_Y * SLICES;

/// View depth in meters where the exponential slices start. Nearer than this, at most an arm's
/// length from the camera, one slice is enough.
pub(crate) const SLICE_START: f32 = 1.0;
/// View depth where the exponential slices end and the last slice begins its run to infinity:
/// lights farther than this are a few pixels across.
pub(crate) const SLICE_FAR: f32 = 500.0;

/// The most lights a cluster lists.
pub(crate) const MAX_LIGHTS_PER_CLUSTER: u32 = 64;

/// Threads per workgroup (`CLUSTER_WORKGROUP` in `shaders/clusters.wgsl`): the device floor's
/// limit, and a divisor of [`CLUSTER_COUNT`].
pub(crate) const WORKGROUP_SIZE: u32 = 128;

/// Slices per doubling of view depth beyond [`SLICE_START`] (`frame.clusters.z`).
pub(crate) fn slices_per_octave() -> f32 {
    (SLICES - 1) as f32 / (SLICE_FAR / SLICE_START).log2()
}

/// Words per cluster list in the cluster buffer: the count and `capacity` 16-bit indices.
pub(crate) fn list_words(capacity: u32) -> u32 {
    1 + capacity.div_ceil(2)
}

/// `frame.clusters` for a projection whose near plane is `near`, with the drawn lights reaching
/// `farthest` meters deep: the last slice's boxes end there, or at [`SLICE_FAR`] if that is
/// deeper.
pub(crate) fn depth_uniforms(near: f32, farthest: f32) -> [f32; 4] {
    [near, SLICE_START, slices_per_octave(), farthest.max(SLICE_FAR)]
}

/// The cluster pass's pipeline and its bind group 0 (frame constants, lights and the cluster
/// buffer, which never move: the cluster buffer is sized once, and the light buffer for
/// `max_lights`). Group 1 is the renderer's view bind group.
#[derive(Debug)]
pub(crate) struct ClusterPass {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    /// Kept to build the pipeline again from new shader sources.
    layout: wgpu::PipelineLayout,
    cache: Option<wgpu::PipelineCache>,
}

impl ClusterPass {
    pub(crate) fn new(
        device: &wgpu::Device,
        view_layout: &wgpu::BindGroupLayout,
        frame_uniforms: &wgpu::Buffer,
        lights: &wgpu::Buffer,
        clusters: &wgpu::Buffer,
        cache: Option<&wgpu::PipelineCache>,
        sources: &ShaderSources,
    ) -> Self {
        let buffer = |binding, ty, min_size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(min_size as u64),
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("light clusters"),
            entries: &[
                buffer(0, wgpu::BufferBindingType::Uniform, size_of::<FrameUniforms>()),
                buffer(1, wgpu::BufferBindingType::Storage { read_only: true }, size_of::<GpuLight>()),
                buffer(2, wgpu::BufferBindingType::Storage { read_only: false }, size_of::<u32>()),
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("light clusters"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: frame_uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: lights.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: clusters.as_entire_binding() },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("light clusters"),
            bind_group_layouts: &[Some(&layout), Some(view_layout)],
            immediate_size: 0,
        });
        let pipeline = build(device, &layout, cache, sources);
        Self { pipeline, bind_group, layout, cache: cache.cloned() }
    }

    /// The pass's pipeline built from `sources` instead, for [`Self::set_pipeline`] (shader hot
    /// reload).
    pub(crate) fn rebuilt(&self, device: &wgpu::Device, sources: &ShaderSources) -> wgpu::ComputePipeline {
        build(device, &self.layout, self.cache.as_ref(), sources)
    }

    pub(crate) fn set_pipeline(&mut self, pipeline: wgpu::ComputePipeline) {
        self.pipeline = pipeline;
    }

    /// Bins the frame's lights, viewed through `view` (the main view's bind group).
    /// `timestamp_writes` times the pass for the frame graph's GPU timing.
    pub(crate) fn record(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::BindGroup,
        labels: bool,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: labels.then_some("light clusters"),
            timestamp_writes,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_bind_group(1, view, &[0]);
        pass.dispatch_workgroups(CLUSTER_COUNT.div_ceil(WORKGROUP_SIZE), 1, 1);
    }
}

/// The cluster pass's pipeline on `layout`, from the light-cluster program in `sources`.
fn build(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    cache: Option<&wgpu::PipelineCache>,
    sources: &ShaderSources,
) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("light clusters"),
        layout: Some(layout),
        module: &shaders::module(device, Program::Clusters, sources),
        entry_point: Some("cs_clusters"),
        compilation_options: Default::default(),
        cache,
    })
}

/// The CPU reference: the grid and culling math of `shaders/lights.wgsl` and
/// `shaders/clusters.wgsl`, step for step, for the tests.
#[cfg(test)]
pub(crate) mod reference {
    use glam::{Mat4, Vec2, Vec3, Vec4Swizzles};

    use super::*;

    /// `CLUSTER_CULL_PADDING` in `shaders/clusters.wgsl`.
    pub(crate) const CULL_PADDING: f32 = 1.001;

    /// `cluster_slice` in `shaders/lights.wgsl`.
    pub(crate) fn slice_of_depth(depth: f32) -> u32 {
        if depth < SLICE_START {
            return 0;
        }
        let slice = 1.0 + ((depth / SLICE_START).log2() * slices_per_octave()).floor();
        slice.min((SLICES - 1) as f32) as u32
    }

    /// View depth from a fragment's reverse-Z depth, as `anime_character.wgsl` computes it.
    pub(crate) fn view_depth(fragment_z: f32, near: f32) -> f32 {
        near / fragment_z.max(1e-30)
    }

    /// `slice_depths`: the view depth range of `slice`.
    pub(crate) fn slice_depths(slice: u32, near: f32, last_far: f32) -> (f32, f32) {
        let per_octave = slices_per_octave();
        let near = if slice > 0 { SLICE_START * ((slice - 1) as f32 / per_octave).exp2() } else { near };
        let far =
            if slice < SLICES - 1 { SLICE_START * (slice as f32 / per_octave).exp2() } else { last_far };
        (near, far)
    }

    /// The tile, slice and NDC rectangle (lower, upper) of `cluster`.
    pub(crate) fn cluster_ndc(cluster: u32) -> (u32, Vec2, Vec2) {
        let (x, y, slice) = (cluster % TILES_X, (cluster / TILES_X) % TILES_Y, cluster / (TILES_X * TILES_Y));
        let tiles = Vec2::new(TILES_X as f32, TILES_Y as f32);
        let lower = Vec2::new(x as f32 / tiles.x * 2.0 - 1.0, 1.0 - (y + 1) as f32 / tiles.y * 2.0);
        let upper = Vec2::new((x + 1) as f32 / tiles.x * 2.0 - 1.0, 1.0 - y as f32 / tiles.y * 2.0);
        (slice, lower, upper)
    }

    /// The view-space point at NDC `ndc` and view depth `depth`.
    pub(crate) fn unproject(view_from_clip: Mat4, ndc: Vec2, depth: f32, near: f32) -> Vec3 {
        let point = view_from_clip * ndc.extend(near / depth).extend(1.0);
        point.xyz() / point.w
    }

    /// `ClusterBox`: lower and upper corners.
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct ClusterBox {
        pub lower: Vec3,
        pub upper: Vec3,
    }

    pub(crate) fn cluster_box(cluster: u32, view_from_clip: Mat4, near: f32, last_far: f32) -> ClusterBox {
        let (slice, ndc_lower, ndc_upper) = cluster_ndc(cluster);
        let (near_depth, far_depth) = slice_depths(slice, near, last_far);
        let mut bounds = ClusterBox { lower: Vec3::splat(3.0e38), upper: Vec3::splat(-3.0e38) };
        for corner in 0..8 {
            let ndc = Vec2::select(glam::BVec2::new(corner & 1 != 0, corner & 2 != 0), ndc_upper, ndc_lower);
            let depth = if corner >= 4 { far_depth } else { near_depth };
            let p = unproject(view_from_clip, ndc, depth, near);
            bounds.lower = bounds.lower.min(p);
            bounds.upper = bounds.upper.max(p);
        }
        bounds
    }

    /// `cluster_light` with a chosen padding, then both culling tests.
    pub(crate) fn touches(
        light: &GpuLight,
        view_from_world: Mat4,
        bounds: &ClusterBox,
        padding: f32,
    ) -> bool {
        let center = (view_from_world * Vec3::from_slice(&light.position[..3]).extend(1.0)).xyz();
        let radius = light.position[3] * padding;
        let offset = center - center.clamp(bounds.lower, bounds.upper);
        if offset.dot(offset) > radius * radius {
            return false;
        }
        let cos_outer = light.spot[3];
        if cos_outer <= 0.0 {
            return true;
        }
        let axis = view_from_world.transform_vector3(Vec3::from_slice(&light.spot[..3])).normalize_or_zero();
        let sin_outer = (1.0 - cos_outer * cos_outer).max(0.0).sqrt();
        let box_center = (bounds.lower + bounds.upper) * 0.5;
        let box_radius = (bounds.upper - bounds.lower).length() * 0.5;
        let v = box_center - center;
        let along = v.dot(axis);
        let across = (v.dot(v) - along * along).max(0.0).sqrt();
        let outside_angle = cos_outer * across - along * sin_outer > box_radius;
        let beyond = along > box_radius + radius;
        let behind = along < -box_radius;
        !(outside_angle || beyond || behind)
    }

    /// Every cluster's list: the lights that touch it, in index order, at most `capacity`.
    pub(crate) fn bin(
        lights: &[GpuLight],
        view_from_world: Mat4,
        view_from_clip: Mat4,
        (near, last_far): (f32, f32),
        capacity: u32,
        padding: f32,
    ) -> Vec<Vec<u32>> {
        (0..CLUSTER_COUNT)
            .map(|cluster| {
                let bounds = cluster_box(cluster, view_from_clip, near, last_far);
                (0..lights.len() as u32)
                    .filter(|&index| touches(&lights[index as usize], view_from_world, &bounds, padding))
                    .take(capacity as usize)
                    .collect()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Vec2, Vec3, Vec3Swizzles};

    use super::reference::*;
    use super::*;
    use crate::lights::Rng;
    use crate::{CameraData, LightKind, LocalLight};

    #[test]
    fn slices_split_the_exponential_range_evenly() {
        let per_octave = slices_per_octave();
        assert!((SLICE_START * ((SLICES - 1) as f32 / per_octave).exp2() - SLICE_FAR).abs() < 1e-2);
        let (near, far) = slice_depths(5, 0.05, SLICE_FAR);
        assert!((far / near - (SLICE_FAR / SLICE_START).powf(1.0 / 23.0)).abs() < 1e-5);
        assert!((1.30..1.32).contains(&(far / near)), "about 31% deeper per slice: {}", far / near);
    }

    #[test]
    fn depth_and_slice_round_trip() {
        let near = 0.05;
        for slice in 0..SLICES {
            let (lower, upper) = slice_depths(slice, near, SLICE_FAR * 4.0);
            assert!(lower < upper, "slice {slice}");
            // Just inside both ends, and the middle (geometric for the exponential slices).
            for depth in [lower * (1.0 + 1e-4), (lower * upper).sqrt(), upper * (1.0 - 1e-4)] {
                assert_eq!(slice_of_depth(depth), slice, "depth {depth} in slice {slice}");
            }
            if slice > 0 {
                assert_eq!(
                    slice_depths(slice - 1, near, SLICE_FAR).1,
                    lower,
                    "slices {slice} and before touch"
                );
            }
        }
    }

    #[test]
    fn slices_near_the_camera_and_at_infinity() {
        let near = 0.05;
        // At the near plane (fragment depth 1) and anywhere short of the exponential range.
        assert_eq!(slice_of_depth(view_depth(1.0, near)), 0);
        assert_eq!(slice_of_depth(near), 0);
        assert_eq!(slice_of_depth(SLICE_START * 0.999), 0);
        assert_eq!(slice_of_depth(SLICE_START), 1);
        // Far beyond the exponential range, and at infinity (fragment depth 0, the cleared
        // depth): the last slice, never past it.
        for depth in [SLICE_FAR, SLICE_FAR * 10.0, 1e30, f32::MAX, f32::INFINITY] {
            assert_eq!(slice_of_depth(depth), SLICES - 1, "{depth}");
        }
        assert_eq!(slice_of_depth(view_depth(0.0, near)), SLICES - 1);
        assert_eq!(slice_of_depth(view_depth(1e-30, near)), SLICES - 1);
        // Monotonic in depth.
        let mut last = 0;
        for step in 0..10_000 {
            let slice = slice_of_depth(near * 1.0015f32.powi(step));
            assert!(slice >= last);
            last = slice;
        }
        assert_eq!(last, SLICES - 1);
    }

    #[test]
    fn reverse_z_depth_recovers_view_depth() {
        let camera = CameraData::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y, 1.0, 1.5, 0.1);
        for depth in [0.1f32, 0.5, 3.0, 40.0, 900.0] {
            let clip = camera.proj * Vec3::new(0.3, -0.2, -depth).extend(1.0);
            let recovered = view_depth(clip.z / clip.w, camera.proj.w_axis.z);
            assert!((recovered - depth).abs() < depth * 1e-5, "{depth}: {recovered}");
        }
    }

    fn camera() -> CameraData {
        CameraData::look_at(
            Vec3::new(1.0, 2.0, 6.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::Y,
            0.9,
            16.0 / 9.0,
            0.05,
        )
    }

    fn view_from_clip(camera: &CameraData) -> Mat4 {
        camera.view * (camera.proj * camera.view).inverse()
    }

    fn gpu_light(light: LocalLight) -> GpuLight {
        GpuLight::new(&light).expect("a valid light")
    }

    #[test]
    fn cluster_boxes_contain_their_frustum_pieces() {
        let camera = camera();
        let (near, last_far) = (0.05, 800.0);
        let inverse = view_from_clip(&camera);
        let mut rng = Rng(7);
        for cluster in (0..CLUSTER_COUNT).step_by(37) {
            let bounds = cluster_box(cluster, inverse, near, last_far);
            let (slice, lower, upper) = cluster_ndc(cluster);
            let (near_depth, far_depth) = slice_depths(slice, near, last_far);
            for _ in 0..20 {
                let ndc = lower + (upper - lower) * Vec2::new(rng.next(), rng.next());
                let depth = near_depth + (far_depth - near_depth) * rng.next();
                let p = unproject(inverse, ndc, depth, near);
                let slack = Vec3::splat(1e-3 * depth);
                assert!(p.cmpge(bounds.lower - slack).all() && p.cmple(bounds.upper + slack).all());
                assert!((-p.z - depth).abs() < depth * 1e-4, "view depth");
            }
        }
    }

    /// Whether `light` reaches `point` (view space) at all: inside its range and its cone.
    fn reaches(light: &GpuLight, view_from_world: Mat4, point: Vec3) -> bool {
        let world = view_from_world.inverse().transform_point3(point);
        let to_light = Vec3::from_slice(&light.position[..3]) - world;
        if to_light.length_squared() * light.radiance[3] >= 1.0 {
            return false;
        }
        let cos = (-to_light.normalize()).dot(Vec3::from_slice(&light.spot[..3]));
        (cos * light.cone[0] + light.cone[1]) > 0.0
    }

    #[test]
    fn binning_is_conservative() {
        let camera = camera();
        let inverse = view_from_clip(&camera);
        let mut rng = Rng(11);
        let lights: Vec<GpuLight> = (0..40)
            .map(|index| {
                let position = Vec3::new(rng.range(-4.0, 4.0), rng.range(-1.0, 4.0), rng.range(-6.0, 4.0));
                let kind = if index % 3 == 0 {
                    let outer = rng.range(0.2, 1.3);
                    LightKind::Spot {
                        direction: Vec3::new(
                            rng.range(-1.0, 1.0),
                            rng.range(-1.0, 1.0),
                            rng.range(-1.0, 1.0),
                        ),
                        inner_angle: outer * 0.7,
                        outer_angle: outer,
                    }
                } else {
                    LightKind::Point
                };
                gpu_light(LocalLight {
                    position,
                    color: Vec3::ONE,
                    intensity: 1.0,
                    range: rng.range(0.3, 3.0),
                    kind,
                })
            })
            .collect();
        let (near, last_far) = (0.05, SLICE_FAR);
        let lists = bin(&lights, camera.view, inverse, (near, last_far), 64, CULL_PADDING);
        let mut listed = 0;
        for (cluster, list) in lists.iter().enumerate() {
            listed += list.len();
            assert!(list.windows(2).all(|pair| pair[0] < pair[1]), "index order");
            let (slice, lower, upper) = cluster_ndc(cluster as u32);
            let (near_depth, far_depth) = slice_depths(slice, near, last_far);
            if slice > 16 {
                continue; // Far slices are vast; sampling them says little.
            }
            for _ in 0..12 {
                let ndc = lower + (upper - lower) * Vec2::new(rng.next(), rng.next());
                let depth = near_depth + (far_depth - near_depth) * rng.next();
                let point = unproject(inverse, ndc, depth, near);
                for (index, light) in lights.iter().enumerate() {
                    if reaches(light, camera.view, point) {
                        assert!(list.contains(&(index as u32)), "cluster {cluster} misses light {index}");
                    }
                }
            }
        }
        // Culling actually culls: far fewer pairs than every light in every cluster.
        assert!(listed > 0 && listed < lights.len() * CLUSTER_COUNT as usize / 20, "{listed} pairs");
    }

    #[test]
    fn cone_test_culls_clusters_behind_and_beside_a_spot() {
        let camera = camera();
        let inverse = view_from_clip(&camera);
        // A narrow spot at the camera's target, pointing straight away from the camera.
        let target = Vec3::new(0.0, 1.0, 0.0);
        let away = (target - camera.position).normalize();
        let light = LocalLight {
            position: target,
            color: Vec3::ONE,
            intensity: 1.0,
            range: 3.0,
            kind: LightKind::Spot { direction: away, inner_angle: 0.1, outer_angle: 0.2 },
        };
        let (spot, point) = (gpu_light(light), gpu_light(LocalLight { kind: LightKind::Point, ..light }));
        let lists = bin(&[spot, point], camera.view, inverse, (0.05, SLICE_FAR), 64, CULL_PADDING);
        let count = |light: u32| lists.iter().filter(|list| list.contains(&light)).count();
        assert!(count(0) > 0, "the spot lights something");
        assert!(count(0) * 3 < count(1), "spot {} vs point {} clusters", count(0), count(1));
        // In front of the spot's apex (toward the camera) the cone lights nothing.
        let apex_depth = -(camera.view.transform_point3(target)).z;
        for (cluster, list) in lists.iter().enumerate() {
            let (slice, ..) = cluster_ndc(cluster as u32);
            if slice_depths(slice, 0.05, SLICE_FAR).1 < apex_depth - 0.5 {
                assert!(!list.contains(&0), "cluster {cluster} is in front of the apex");
            }
        }
    }

    #[test]
    fn full_clusters_keep_the_earliest_lights() {
        let camera = camera();
        let lights: Vec<GpuLight> = (0..10)
            .map(|_| {
                gpu_light(LocalLight {
                    position: Vec3::new(0.0, 1.0, 0.0),
                    color: Vec3::ONE,
                    intensity: 1.0,
                    range: 2.0,
                    kind: LightKind::Point,
                })
            })
            .collect();
        let lists = bin(&lights, camera.view, view_from_clip(&camera), (0.05, SLICE_FAR), 4, CULL_PADDING);
        let full: Vec<_> = lists.iter().filter(|list| !list.is_empty()).collect();
        assert!(!full.is_empty());
        assert!(full.iter().all(|list| list.as_slice() == [0, 1, 2, 3]));
        assert_eq!(list_words(4), 3);
        assert_eq!(list_words(MAX_LIGHTS_PER_CLUSTER), 33);
        assert_eq!(list_words(7), 5);
    }

    #[test]
    fn grid_divides_into_workgroups() {
        assert_eq!(CLUSTER_COUNT, 3456);
        assert_eq!(CLUSTER_COUNT % WORKGROUP_SIZE, 0);
        // A cluster beside the screen center lies along the camera's axis, at its slice's depth.
        let camera = camera();
        let depth = 6.2;
        let slice = slice_of_depth(depth);
        let cluster = (slice * TILES_Y + TILES_Y / 2) * TILES_X + TILES_X / 2;
        let bounds = cluster_box(cluster, view_from_clip(&camera), 0.05, SLICE_FAR);
        assert!(bounds.lower.z < -depth * 0.8 && bounds.upper.z > -depth * 1.2);
        assert!(bounds.lower.x < 0.5 && bounds.upper.x > -0.5);
        let center = (bounds.lower + bounds.upper) * 0.5;
        assert!(center.xy().length() < depth, "{center}");
    }
}
