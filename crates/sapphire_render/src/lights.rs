//! Local lights (spec §5.12 "Lighting", M1 R5): the packet's point and spot lights, culled to
//! the main view and capped on the CPU, then listed for the anime character shader in one of
//! two ways ([`LightPath`]):
//!
//! - **Clustered forward+ (desktop):** a compute pass bins the lights into a 16 × 9 × 24 grid of
//!   view-space clusters ([`clusters`]), and each pixel walks its cluster's list.
//! - **Per-object lists (mobile):** the CPU picks each instance's four most influential lights
//!   ([`objects`]), and the instance's pixels walk those.
//!
//! Both fill one list buffer (group 0, binding 2) in one layout, a count and then 16-bit light
//! indices, so the anime character shader reads either the same way, and both run on every
//! backend. The path is a [`RenderSettings`] choice made by the app, never a platform one
//! (rule R7).
//!
//! # Per frame
//!
//! 1. **Cull:** lights that can't light anything, or whose range sphere misses the main view's
//!    frustum, are dropped.
//! 2. **Cap:** of the rest, the `max_lights` nearest the camera are kept (spec §3.4 budgets 64
//!    on desktop and 8 on mobile), in packet order.
//! 3. **Pack** into the GPU layout (`LocalLight` in `shaders/lights.wgsl`). The light buffer is
//!    uploaded only when that changed.
//! 4. **List:** the cluster pass, or per-object lists chosen for the instances that changed and,
//!    when the lights changed, for every instance.
//!
//! A frame without lights runs neither: the anime character shader skips local lights on a
//! uniform branch, so it costs nothing beyond the frame constants.
//!
//! # Shading
//!
//! `shaders/anime_character.wgsl` lights a pixel by each listed light the way it lights it by
//! the key light: the same two-tone split of N·L (threshold, ILM bias, softness, one-pixel edge)
//! and the same authored shadow color, in the light's color and brightness, scaled by its smooth
//! range window and spot cone. Local lights therefore push the light/shadow split where they
//! reach and tint both tones, instead of adding a smooth Lambert falloff. Their sum is clamped
//! to a luminance ceiling so stacked lights tint a character rather than blow it out (the
//! reference games' `_FilterLight`): the character light rig's (M1 C2), per character, 2 for
//! anything else. They keep their own directions: the rig's limits on where a key light may come
//! from are for the key light alone.
//!
//! # Later milestones
//!
//! - **Light layers** keep VFX lights off faces and off the depth-offset rim, which every local
//!   light reaches through the rim's shared mask (M1 C5: a rim from only the strongest light
//!   would pop as lights reorder).
//! - **Ramp and band modes (C1):** each light indexes the ramp (or bands) with its own split, as
//!   the sun does.
//! - **Cluster occupancy on the GPU:** the light lists' debug view (M1 F5, the
//!   [`debug_view`](crate::debug_view) module) draws each pixel's list count from the cluster
//!   buffer, and [`LightStats`] counts lights and clusters; the fullest cluster and the lights a
//!   full one dropped would need the cluster pass to count them and a readback.
//! - **More lights:** beyond a few hundred, cull per tile column first (or z-bin), and
//!   scalarize the per-pixel loop across waves.
//! - **Local-light shadows** for a few hero lights, and environment shading (W4) through the
//!   same lists.

pub(crate) mod clusters;
mod objects;

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use sapphire_rhi::wgpu;
use serde::{Deserialize, Serialize};

use crate::buffer::{self, GrowableBuffer};
use crate::mesh_pool::MeshRecord;
use crate::scene::InstanceSlots;
use crate::shaders::ShaderSources;
use crate::{CameraData, FramePacket, LightKind, LocalLight, MeshHandle, RenderSettings};
use clusters::ClusterPass;
use objects::{LIGHTS_PER_OBJECT, LIST_BYTES, ObjectLists};

/// The most local lights any frame can draw, whatever
/// [`RenderSettings::max_lights`](crate::RenderSettings::max_lights) says: the cluster pass
/// tests every light against every cluster, and lists hold 16-bit indices.
pub const MAX_LIGHTS: u32 = 1024;

/// How local lights reach the shading (spec §5.12 "Lighting"). Both paths run on every
/// backend; each quality tier picks one ([`QualityTier::light_path`](crate::quality::QualityTier)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum LightPath {
    /// Clustered forward+, the desktop path: a compute pass bins the lights into a 16 × 9 × 24
    /// grid of view-space clusters, and each pixel walks its cluster's list.
    #[default]
    Clustered,
    /// Per-object lists, the mobile path: the CPU picks each instance's four most influential
    /// lights, and its pixels walk those. No compute pass.
    PerObject,
}

/// Local-light counters from the last prepared frame (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LightStats {
    /// Lights the frame draws with.
    pub drawn: u32,
    /// Lights culled: outside the main view, or unable to light anything.
    pub culled: u32,
    /// Visible lights beyond `max_lights`, dropped farthest first.
    pub dropped: u32,
    /// Clusters the cluster pass binned the lights into: 16 × 9 × 24, or 0 if it didn't run
    /// (no lights, or the per-object path).
    pub clusters: u32,
    /// The most lights one list holds: per cluster, or 4 per instance.
    pub list_capacity: u32,
    /// Per-object path: instances whose lists were chosen again.
    pub object_lists: u32,
}

/// One light as the GPU reads it (WGSL `LocalLight` in `shaders/lights.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuLight {
    /// xyz: world position; w: range.
    pub position: [f32; 4],
    /// rgb: color × intensity; w: 1 / range².
    pub radiance: [f32; 4],
    /// xyz: unit vector the spot points along, zero for point lights; w: cos(outer angle), −1
    /// for point lights.
    pub spot: [f32; 4],
    /// xy: the cone as `saturate(cos θ × x + y)` before its smoothstep; zw: 0.
    pub cone: [f32; 4],
}

/// Ranges below this light nothing worth a list entry, and would overflow 1 / range².
const MIN_RANGE: f32 = 1e-3;

impl GpuLight {
    /// The GPU form of `light`, or `None` if it can't light anything: a range below a
    /// millimeter or not finite, an intensity that isn't positive, a black or non-finite color,
    /// or a spot without a direction.
    pub(crate) fn new(light: &LocalLight) -> Option<Self> {
        let LocalLight { position, color, intensity, range, kind } = *light;
        let radiance = color.max(Vec3::ZERO) * intensity;
        let valid = position.is_finite()
            && color.is_finite()
            && (MIN_RANGE..f32::INFINITY).contains(&range)
            && intensity > 0.0
            && radiance.is_finite()
            && radiance != Vec3::ZERO;
        if !valid {
            return None;
        }
        let (spot, cone) = match kind {
            LightKind::Point => ([0.0, 0.0, 0.0, -1.0], [0.0, 1.0, 0.0, 0.0]),
            LightKind::Spot { direction, inner_angle, outer_angle } => {
                let direction = direction.try_normalize()?;
                if !(inner_angle.is_finite() && outer_angle.is_finite()) {
                    return None;
                }
                let outer = outer_angle.clamp(0.0, std::f32::consts::PI);
                let inner = inner_angle.clamp(0.0, outer);
                let (cos_outer, cos_inner) = (outer.cos(), inner.cos());
                // An inner angle at the outer one is a hard edge, kept just short of a division
                // by zero.
                let scale = 1.0 / (cos_inner - cos_outer).max(1e-4);
                (direction.extend(cos_outer).to_array(), [scale, -cos_outer * scale, 0.0, 0.0])
            }
        };
        Some(Self {
            position: position.extend(range).to_array(),
            radiance: radiance.extend((range * range).recip()).to_array(),
            spot,
            cone,
        })
    }
}

/// Rec. 709 luminance of linear RGB (`LUMINANCE` in `frame.wgsl`).
pub(crate) fn luminance(rgb: Vec3) -> f32 {
    rgb.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

/// What group 0 binding 2 holds (`LIGHT_LISTS_*` in `shaders/lights.wgsl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum ListMode {
    None = 0,
    Clusters = 1,
    Objects = 2,
}

/// The light constants in [`FrameUniforms`](crate::gpu_types::FrameUniforms) (`frame.lights`
/// and `frame.clusters`; see `shaders/lights.wgsl`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LightUniforms {
    pub lights: [u32; 4],
    pub clusters: [f32; 4],
}

/// A visible light, while culling and capping.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    /// From the camera to the light's range sphere.
    distance: f32,
    /// In the packet.
    index: u32,
    light: GpuLight,
}

/// The renderer's local lights: the frame's packed lights, the light and list buffers, and the
/// state of whichever path the settings chose.
#[derive(Debug)]
pub(crate) struct Lights {
    path: LightPath,
    max_lights: u32,
    /// The most lights one list holds, and its words in the list buffer.
    capacity: u32,
    stride: u32,
    /// The lights the last frame with a view draws, and the frame before's (kept to tell
    /// whether they changed).
    packed: Vec<GpuLight>,
    previous: Vec<GpuLight>,
    /// Scratch for culling and capping.
    candidates: Vec<Candidate>,
    /// `packed` differs from what the light buffer holds.
    unsent: bool,
    /// Holds `max_lights` lights (at least one, as bindings can't be empty).
    light_buffer: wgpu::Buffer,
    /// Cluster lists (fixed size) or per-object lists (one per instance slot, growing).
    lists: GrowableBuffer,
    objects: ObjectLists,
    /// The clustered path's compute pass, unless local lights are off.
    cluster_pass: Option<ClusterPass>,
    /// The cluster pass runs this frame.
    bin: bool,
    uniforms: LightUniforms,
    stats: LightStats,
}

impl Lights {
    /// `frame_uniforms` is the buffer the cluster pass reads the frame constants from;
    /// `view_layout` is group 1's.
    pub(crate) fn new(
        device: &wgpu::Device,
        settings: &RenderSettings,
        frame_uniforms: &wgpu::Buffer,
        view_layout: &wgpu::BindGroupLayout,
        cache: Option<&wgpu::PipelineCache>,
        sources: &ShaderSources,
    ) -> Self {
        let max_lights = settings.max_lights.min(MAX_LIGHTS);
        let limits = device.limits();
        let max_storage = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size);
        let light_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("local lights"),
            size: u64::from(max_lights.max(1)) * size_of::<GpuLight>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let storage = wgpu::BufferUsages::STORAGE;
        let (capacity, lists, cluster_pass) = match settings.light_path {
            LightPath::Clustered => {
                let capacity = max_lights.min(clusters::MAX_LIGHTS_PER_CLUSTER);
                let bytes = u64::from(clusters::CLUSTER_COUNT * clusters::list_words(capacity)) * 4;
                let lists = GrowableBuffer::new(device, "light clusters", storage, bytes, max_storage);
                let pass = (max_lights > 0).then(|| {
                    ClusterPass::new(
                        device,
                        view_layout,
                        frame_uniforms,
                        &light_buffer,
                        lists.buffer(),
                        cache,
                        sources,
                    )
                });
                (capacity, lists, pass)
            }
            LightPath::PerObject => {
                let lists =
                    GrowableBuffer::new(device, "object light lists", storage, 256 * LIST_BYTES, max_storage);
                (LIGHTS_PER_OBJECT.min(max_lights), lists, None)
            }
        };
        let stride = match settings.light_path {
            LightPath::Clustered => clusters::list_words(capacity),
            LightPath::PerObject => (LIST_BYTES / 4) as u32,
        };
        Self {
            path: settings.light_path,
            max_lights,
            capacity,
            stride,
            packed: Vec::new(),
            previous: Vec::new(),
            candidates: Vec::new(),
            unsent: false,
            light_buffer,
            lists,
            objects: ObjectLists::default(),
            cluster_pass,
            bin: false,
            uniforms: LightUniforms::default(),
            stats: LightStats::default(),
        }
    }

    /// The cluster pass's pipeline built from `sources` instead, if the clustered path runs, for
    /// [`Self::set_cluster_pipeline`] (shader hot reload).
    pub(crate) fn rebuilt_cluster_pipeline(
        &self,
        device: &wgpu::Device,
        sources: &ShaderSources,
    ) -> Option<wgpu::ComputePipeline> {
        self.cluster_pass.as_ref().map(|pass| pass.rebuilt(device, sources))
    }

    pub(crate) fn set_cluster_pipeline(&mut self, pipeline: wgpu::ComputePipeline) {
        if let Some(pass) = &mut self.cluster_pass {
            pass.set_pipeline(pipeline);
        }
    }

    /// A mesh was replaced in place, with new bounds: every per-object list is chosen again on
    /// the next frame (a rare event, so its instances aren't looked for).
    pub(crate) fn mesh_replaced(&mut self) {
        self.objects.invalidate();
    }

    /// Culls, caps and packs the packet's lights for its main view, and on the per-object
    /// path chooses the lists that need it (`slots` already has the packet's instance deltas
    /// applied), with the bounds of `meshes`. Returns `true` when the list buffer was replaced,
    /// so bind groups that use it must be rebuilt.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        packet: &FramePacket,
        slots: &InstanceSlots,
        meshes: &[MeshRecord],
    ) -> bool {
        profiling::scope!("Lights::prepare");
        self.bin = false;
        let Some(view) = packet.main_view() else {
            // Nothing draws, so nothing is listed; the next frame with a view lists everything.
            self.objects.invalidate();
            self.stats = LightStats { list_capacity: self.capacity, ..LightStats::default() };
            return false;
        };
        let camera = &view.camera;
        std::mem::swap(&mut self.packed, &mut self.previous);
        let Gathered { culled, dropped, farthest } =
            gather(&packet.lights, camera, self.max_lights, &mut self.candidates, &mut self.packed);
        let changed = self.packed != self.previous;
        self.unsent |= changed;

        let drawn = self.packed.len() as u32;
        let mut replaced = false;
        let mode = match self.path {
            _ if drawn == 0 => ListMode::None,
            LightPath::Clustered => {
                self.bin = true;
                ListMode::Clusters
            }
            LightPath::PerObject => {
                // The old buffer holds every list uploaded so far: a GPU copy keeps them.
                let uploaded = self.lists.buffer().size();
                replaced = self.lists.reserve(device, queue, slots.len() as u64 * LIST_BYTES, uploaded);
                let bounds = |mesh: MeshHandle| meshes.get(mesh.index()).map(|record| record.bounds);
                self.objects.update(&self.packed, changed, slots, &packet.instances.changed, bounds);
                ListMode::Objects
            }
        };
        self.uniforms = LightUniforms {
            lights: [drawn, mode as u32, self.stride, self.capacity],
            clusters: clusters::depth_uniforms(near_plane(camera), farthest),
        };
        self.stats = LightStats {
            drawn,
            culled,
            dropped,
            clusters: if self.bin { clusters::CLUSTER_COUNT } else { 0 },
            list_capacity: self.capacity,
            object_lists: if mode == ListMode::Objects { self.objects.chosen() } else { 0 },
        };
        replaced
    }

    /// `frame.lights` and `frame.clusters` for the last prepared frame.
    pub(crate) fn uniforms(&self) -> LightUniforms {
        self.uniforms
    }

    pub(crate) fn stats(&self) -> LightStats {
        self.stats
    }

    /// Group 0 binding 1.
    pub(crate) fn light_buffer(&self) -> &wgpu::Buffer {
        &self.light_buffer
    }

    /// Group 0 binding 2.
    pub(crate) fn list_buffer(&self) -> &wgpu::Buffer {
        self.lists.buffer()
    }

    /// Times the list buffer grew.
    pub(crate) fn growths(&self) -> u32 {
        self.lists.growths()
    }

    /// On the per-object path, the lights each instance slot's pixels walk in the last prepared
    /// frame, by slot (none on the clustered path, and 0 for every slot in a frame without
    /// lights): the light-list debug view's tags.
    pub(crate) fn object_counts(&self) -> impl Iterator<Item = u32> + '_ {
        let listed = self.uniforms.lights[1] == ListMode::Objects as u32;
        let capacity = self.capacity;
        let counts = (self.path == LightPath::PerObject).then(|| self.objects.counts()).into_iter().flatten();
        counts.map(move |count| if listed { count.min(capacity) } else { 0 })
    }

    /// The lights the last frame with a view draws, as packed for the GPU.
    #[cfg(test)]
    pub(crate) fn packed(&self) -> &[GpuLight] {
        &self.packed
    }

    /// Splits the lights between a frame's passes: the uploads pass writes what changed, and
    /// the cluster pass, if it runs this frame, bins.
    pub(crate) fn split_for_record(&mut self) -> (LightUpload<'_>, Option<&ClusterPass>) {
        let Self {
            ref packed,
            ref mut unsent,
            ref light_buffer,
            ref lists,
            ref mut objects,
            ref cluster_pass,
            bin,
            ..
        } = *self;
        let upload = LightUpload { packed, unsent, light_buffer, lists: lists.buffer(), objects };
        (upload, cluster_pass.as_ref().filter(|_| bin))
    }
}

/// The lights' share of a frame's uploads pass.
pub(crate) struct LightUpload<'a> {
    packed: &'a [GpuLight],
    unsent: &'a mut bool,
    light_buffer: &'a wgpu::Buffer,
    lists: &'a wgpu::Buffer,
    objects: &'a mut ObjectLists,
}

impl LightUpload<'_> {
    /// Writes the lights if they changed, and the per-object lists that did, through `belt`.
    pub(crate) fn write(self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        let Self { packed, unsent, light_buffer, lists, objects } = self;
        if std::mem::take(unsent) {
            buffer::stage(belt, encoder, light_buffer, 0, bytemuck::cast_slice(packed));
        }
        objects.flush(|first, written| {
            buffer::stage(belt, encoder, lists, u64::from(first) * LIST_BYTES, bytemuck::cast_slice(written));
        });
    }
}

/// What culling and capping a frame's lights found.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Gathered {
    /// Lights that can't light anything or miss the view.
    culled: u32,
    /// Visible lights beyond the cap.
    dropped: u32,
    /// The deepest view depth a kept light reaches.
    farthest: f32,
}

/// Culls `lights` to `camera`'s view, keeps the `max` nearest it (ties to the lower index),
/// and packs those into `packed`, in packet order. `candidates` is scratch.
fn gather(
    lights: &[LocalLight],
    camera: &CameraData,
    max: u32,
    candidates: &mut Vec<Candidate>,
    packed: &mut Vec<GpuLight>,
) -> Gathered {
    let planes = frustum_planes(camera.proj * camera.view);
    candidates.clear();
    let mut culled = 0;
    for (index, light) in lights.iter().enumerate() {
        let visible = GpuLight::new(light).filter(|_| sphere_visible(&planes, light.position, light.range));
        let Some(gpu) = visible else {
            culled += 1;
            continue;
        };
        let distance = (light.position.distance(camera.position) - light.range).max(0.0);
        candidates.push(Candidate { distance, index: index as u32, light: gpu });
    }
    let keep = max as usize;
    let dropped = candidates.len().saturating_sub(keep);
    if dropped > 0 {
        if keep > 0 {
            let nearest =
                |a: &Candidate, b: &Candidate| a.distance.total_cmp(&b.distance).then(a.index.cmp(&b.index));
            candidates.select_nth_unstable_by(keep - 1, nearest);
        }
        candidates.truncate(keep);
        candidates.sort_unstable_by_key(|candidate| candidate.index);
    }
    packed.clear();
    let mut farthest = 0.0f32;
    for candidate in candidates.iter() {
        packed.push(candidate.light);
        let [x, y, z, range] = candidate.light.position;
        let depth = -camera.view.transform_point3(Vec3::new(x, y, z)).z;
        farthest = farthest.max(depth + range);
    }
    Gathered { culled, dropped: dropped as u32, farthest }
}

/// The near plane of a reverse-Z projection with an infinite far plane (crate docs,
/// "Conventions"): clip z is the near distance itself, so view depth is `near / z`.
fn near_plane(camera: &CameraData) -> f32 {
    debug_assert!(
        camera.proj.z_axis.z == 0.0 && camera.proj.z_axis.w == -1.0,
        "the camera must be reverse-Z with an infinite far plane"
    );
    camera.proj.w_axis.z
}

/// The view frustum's planes, facing inward, normalized: left, right, bottom, top and near.
/// The far plane is at infinity.
fn frustum_planes(clip_from_world: Mat4) -> [Vec4; 5] {
    let row = |index| clip_from_world.row(index);
    // Reverse-Z: the near plane is clip z = w.
    let planes = [row(3) + row(0), row(3) - row(0), row(3) + row(1), row(3) - row(1), row(3) - row(2)];
    planes.map(|plane| plane / plane.truncate().length().max(f32::MIN_POSITIVE))
}

fn sphere_visible(planes: &[Vec4; 5], center: Vec3, radius: f32) -> bool {
    planes.iter().all(|plane| plane.truncate().dot(center) + plane.w >= -radius)
}

/// A small deterministic generator (xorshift) for the tests.
#[cfg(test)]
pub(crate) struct Rng(pub u64);

#[cfg(test)]
impl Rng {
    /// Uniform in [0, 1).
    pub(crate) fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    pub(crate) fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> CameraData {
        CameraData::look_at(
            Vec3::new(0.0, 1.0, 5.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::Y,
            1.0,
            16.0 / 9.0,
            0.05,
        )
    }

    fn point(position: Vec3, range: f32) -> LocalLight {
        LocalLight { position, color: Vec3::ONE, intensity: 1.0, range, kind: LightKind::Point }
    }

    fn spot(inner_angle: f32, outer_angle: f32) -> LocalLight {
        LocalLight {
            kind: LightKind::Spot { direction: Vec3::new(0.0, -2.0, 0.0), inner_angle, outer_angle },
            ..point(Vec3::ZERO, 2.0)
        }
    }

    #[test]
    fn lights_that_light_nothing_are_rejected() {
        let good = point(Vec3::ZERO, 2.0);
        assert!(GpuLight::new(&good).is_some());
        let bad = [
            LocalLight { range: 0.0, ..good },
            LocalLight { range: -1.0, ..good },
            LocalLight { range: 1e-4, ..good },
            LocalLight { range: f32::NAN, ..good },
            LocalLight { range: f32::INFINITY, ..good },
            LocalLight { intensity: 0.0, ..good },
            LocalLight { intensity: -1.0, ..good },
            LocalLight { intensity: f32::NAN, ..good },
            LocalLight { color: Vec3::ZERO, ..good },
            LocalLight { color: Vec3::new(-1.0, -2.0, 0.0), ..good },
            LocalLight { color: Vec3::new(1.0, f32::NAN, 0.0), ..good },
            LocalLight { color: Vec3::splat(f32::MAX), intensity: 10.0, ..good },
            LocalLight { position: Vec3::new(0.0, f32::INFINITY, 0.0), ..good },
            LocalLight {
                kind: LightKind::Spot { direction: Vec3::ZERO, inner_angle: 0.1, outer_angle: 0.2 },
                ..good
            },
            LocalLight {
                kind: LightKind::Spot {
                    direction: Vec3::splat(f32::NAN),
                    inner_angle: 0.1,
                    outer_angle: 0.2,
                },
                ..good
            },
            spot(f32::NAN, 0.2),
            spot(0.1, f32::INFINITY),
        ];
        for light in bad {
            assert_eq!(GpuLight::new(&light), None, "{light:?}");
        }
        // Negative channels are clamped rather than subtracting light.
        let tinted = GpuLight::new(&LocalLight { color: Vec3::new(-1.0, 0.5, 2.0), ..good }).expect("valid");
        assert_eq!(tinted.radiance[..3], [0.0, 0.5, 2.0]);
    }

    /// The cone factor the shader computes for a direction `angle` off the spot's axis.
    fn cone(light: &GpuLight, angle: f32) -> f32 {
        let t = (angle.cos() * light.cone[0] + light.cone[1]).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    #[test]
    fn packs_points_and_spot_cones() {
        let packed = GpuLight::new(&point(Vec3::new(1.0, 2.0, 3.0), 4.0)).expect("valid");
        assert_eq!(packed.position, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(packed.radiance[3], 1.0 / 16.0);
        assert_eq!((packed.spot, packed.cone), ([0.0, 0.0, 0.0, -1.0], [0.0, 1.0, 0.0, 0.0]));
        assert_eq!(cone(&packed, 2.5), 1.0, "a point light has no cone");

        let packed = GpuLight::new(&spot(0.2, 0.4)).expect("valid");
        assert_eq!(packed.spot[..3], [0.0, -1.0, 0.0], "normalized");
        assert!((packed.spot[3] - 0.4f32.cos()).abs() < 1e-6);
        assert!(cone(&packed, 0.1) == 1.0 && cone(&packed, 0.2) > 0.999);
        // Smooth in the cosine between the angles.
        let between = cone(&packed, 0.3);
        assert!(between > 0.4 && between < 0.8 && cone(&packed, 0.35) < between, "{between}");
        assert!(cone(&packed, 0.4) < 1e-3 && cone(&packed, 0.5) == 0.0);

        // An inner angle past the outer one is a hard edge; angles past π are a full sphere.
        let hard = GpuLight::new(&spot(0.6, 0.4)).expect("valid");
        assert!(cone(&hard, 0.399) == 1.0 && cone(&hard, 0.401) == 0.0);
        let wide = GpuLight::new(&spot(4.0, 5.0)).expect("valid");
        assert_eq!(wide.spot[3], -1.0);
        assert!(cone(&wide, 1.0) == 1.0 && cone(&wide, 3.1) == 1.0);
    }

    fn run(lights: &[LocalLight], max: u32) -> (Gathered, Vec<GpuLight>) {
        let mut packed = Vec::new();
        let gathered = gather(lights, &camera(), max, &mut Vec::new(), &mut packed);
        (gathered, packed)
    }

    /// Which lights were packed, by their x position.
    fn kept(packed: &[GpuLight]) -> Vec<f32> {
        packed.iter().map(|light| light.position[0]).collect()
    }

    #[test]
    fn culls_lights_whose_reach_misses_the_view() {
        let lights = [
            point(Vec3::new(0.0, 1.0, 0.0), 1.0),
            // Behind the camera, out of reach of the near plane.
            point(Vec3::new(1.0, 1.0, 8.0), 1.0),
            // Behind it, but its range reaches into view.
            point(Vec3::new(2.0, 1.0, 6.0), 3.0),
            // Far off to the side, and far below.
            point(Vec3::new(503.0, 1.0, -100.0), 10.0),
            point(Vec3::new(4.0, -50.0, 0.0), 2.0),
            // Just outside the left edge, reaching in.
            point(Vec3::new(-5.0, 1.0, 0.0), 1.5),
            // Lights nothing.
            point(Vec3::new(6.0, 1.0, 0.0), 0.0),
        ];
        let (gathered, packed) = run(&lights, 64);
        assert_eq!(kept(&packed), [0.0, 2.0, -5.0]);
        assert_eq!((gathered.culled, gathered.dropped), (4, 0));
        // The deepest reach: the light at the left edge, 5 m deep, plus its range.
        assert!((gathered.farthest - 6.5).abs() < 1e-4, "{}", gathered.farthest);
    }

    #[test]
    fn keeps_the_nearest_in_packet_order() {
        // Visible lights at depths 1 to 10, shuffled, each reaching 0.5 m.
        let depths = [7.0, 2.0, 9.0, 1.0, 10.0, 4.0, 3.0, 8.0, 6.0, 5.0];
        let lights: Vec<LocalLight> =
            depths.iter().map(|&depth| point(Vec3::new(depth * 0.1, 1.0, 5.0 - depth), 0.5)).collect();
        let (gathered, packed) = run(&lights, 4);
        // Depths 2, 1, 4 and 3, in the order they came.
        let expected: Vec<f32> = [2.0f32, 1.0, 4.0, 3.0].iter().map(|depth| depth * 0.1).collect();
        assert_eq!(kept(&packed), expected);
        assert_eq!((gathered.culled, gathered.dropped), (0, 6));
        assert!((gathered.farthest - 4.5).abs() < 1e-4);

        // Equally near lights: the earlier ones win.
        let same: Vec<LocalLight> =
            (0..6).map(|i| point(Vec3::new(i as f32 * 1e-3, 1.0, 2.0), 0.5)).collect();
        let (_, packed) = run(&same, 3);
        assert_eq!(kept(&packed), [0.0, 1e-3, 2e-3]);

        // A cap of zero turns local lights off.
        let (gathered, packed) = run(&lights, 0);
        assert!(packed.is_empty());
        assert_eq!(gathered.dropped, 10);
        // More visible lights than ever fit are capped the same way.
        let many: Vec<LocalLight> = (0..MAX_LIGHTS + 10)
            .map(|i| point(Vec3::new((i % 50) as f32 * 0.02 - 0.5, 1.0, -(i as f32) * 0.01), 0.5))
            .collect();
        let (gathered, packed) = run(&many, MAX_LIGHTS);
        assert_eq!((packed.len() as u32, gathered.dropped), (MAX_LIGHTS, 10));
    }

    #[test]
    fn frustum_planes_face_inward() {
        let camera = camera();
        let planes = frustum_planes(camera.proj * camera.view);
        assert!(sphere_visible(&planes, Vec3::new(0.0, 1.0, 0.0), 0.0));
        assert!(sphere_visible(&planes, Vec3::new(0.0, 1.0, -1.0e6), 0.0), "no far plane");
        assert!(!sphere_visible(&planes, Vec3::new(0.0, 1.0, 5.1), 0.0), "behind the near plane");
        assert!(sphere_visible(&planes, Vec3::new(0.0, 1.0, 5.1), 0.2));
        for plane in planes {
            assert!((plane.truncate().length() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn near_plane_comes_from_the_projection() {
        assert!((near_plane(&camera()) - 0.05).abs() < 1e-7);
    }
}

/// Tests that read the cluster buffer back. They need a GPU: run them with
/// `cargo test -p sapphire_render --tests -- --ignored`.
#[cfg(test)]
mod gpu_tests {
    use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc};

    use super::clusters::{CLUSTER_COUNT, reference};
    use super::*;
    use crate::graph::{FrameGraph, GraphExecutor};
    use crate::{FramePacketRing, Renderer, ViewData};

    /// A random light near the camera's target: points and spots, some reaching far.
    fn random_light(rng: &mut Rng) -> LocalLight {
        let position = Vec3::new(rng.range(-4.0, 4.0), rng.range(-1.0, 4.0), rng.range(-8.0, 3.0));
        let kind = if rng.next() < 0.4 {
            let outer = rng.range(0.1, 1.4);
            let direction = Vec3::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
            LightKind::Spot { direction, inner_angle: outer * rng.next(), outer_angle: outer }
        } else {
            LightKind::Point
        };
        let range = if rng.next() < 0.1 { rng.range(4.0, 12.0) } else { rng.range(0.2, 2.5) };
        LocalLight { position, color: Vec3::ONE, intensity: 1.0, range, kind }
    }

    #[test]
    #[ignore = "needs a GPU"]
    fn gpu_clusters_match_the_cpu_reference() {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        // More lights than a cluster lists, so crowded clusters overflow.
        let settings = RenderSettings { max_lights: 400, ..RenderSettings::default() };
        let mut renderer =
            Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, settings).expect("renderer");
        let camera = CameraData::look_at(
            Vec3::new(1.0, 2.0, 6.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::Y,
            0.9,
            16.0 / 9.0,
            0.05,
        );
        let mut rng = Rng(5);
        let mut packets = FramePacketRing::new();
        let packet = packets.begin();
        packet.views.push(ViewData::new(camera));
        packet.lights.extend((0..500).map(|_| random_light(&mut rng)));
        renderer.prepare(&gpu, packets.current());

        let stats = renderer.stats().lights;
        assert!(stats.drawn > 200 && stats.culled > 0, "{stats:?}");
        assert_eq!((stats.clusters, stats.list_capacity), (CLUSTER_COUNT, 64));
        let lights = renderer.lights();
        let packed = lights.packed().to_vec();
        let LightUniforms { lights: [drawn, mode, stride, capacity], clusters: depths } = lights.uniforms();
        assert_eq!((drawn as usize, mode), (packed.len(), ListMode::Clusters as u32));
        let lists = lights.list_buffer().clone();
        let readback = gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: lists.size(),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 320, height: 180, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());

        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        renderer.record(&mut graph, target);
        let (lists, readback_buffer) = (&lists, &readback);
        graph.add_pass("read clusters").side_effect().record(move |ctx| {
            ctx.encoder.copy_buffer_to_buffer(lists, 0, readback_buffer, 0, lists.size());
        });
        let mut executor = GraphExecutor::new();
        executor.execute(&gpu, graph).expect("a valid graph");
        assert!(pollster::block_on(scope.pop()).is_none());
        assert_eq!(
            executor.stats().passes,
            16,
            "uploads, light clusters, sky cube, character prepass, main, 9 bloom, post, readback"
        );
        readback.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
        gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let words: Vec<u32> = bytemuck::cast_slice(&readback.get_mapped_range(..).expect("mapped")).to_vec();

        // The GPU and CPU round differently, so a light whose sphere grazes a cluster may go
        // either way: it must be listed if it touches with a slightly smaller radius, and may
        // be only if it touches with a slightly larger one.
        let view_from_clip = camera.view * (camera.proj * camera.view).inverse();
        let bin = |padding: f32| {
            reference::bin(&packed, camera.view, view_from_clip, (depths[0], depths[3]), u32::MAX, padding)
        };
        let (surely, maybe) =
            (bin(reference::CULL_PADDING * (1.0 - 2e-3)), bin(reference::CULL_PADDING * (1.0 + 2e-3)));
        let exact = bin(reference::CULL_PADDING);
        let (mut full, mut matching) = (0, 0);
        for cluster in 0..CLUSTER_COUNT as usize {
            let base = cluster * stride as usize;
            let count = words[base];
            assert!(count <= capacity, "cluster {cluster} lists {count}");
            let listed: Vec<u32> =
                (0..count as usize).map(|i| (words[base + 1 + i / 2] >> (16 * (i % 2))) & 0xffff).collect();
            assert!(listed.windows(2).all(|pair| pair[0] < pair[1]), "cluster {cluster}: {listed:?}");
            assert!(
                listed.iter().all(|index| maybe[cluster].contains(index)),
                "cluster {cluster}: {listed:?}"
            );
            // A full cluster keeps the earliest lights: every sure light before its last.
            let last = if count == capacity { listed.last().copied().unwrap_or(0) } else { u32::MAX };
            full += usize::from(count == capacity);
            for index in surely[cluster].iter().filter(|&&index| index <= last) {
                assert!(listed.contains(index), "cluster {cluster} misses light {index}: {listed:?}");
            }
            let expected: Vec<u32> = exact[cluster].iter().copied().take(capacity as usize).collect();
            matching += usize::from(listed == expected);
        }
        eprintln!(
            "{} lights: {matching} of {CLUSTER_COUNT} clusters match the reference exactly, {full} are full",
            packed.len()
        );
        assert!(full > 0, "the light set should overflow some clusters");
        assert!(matching * 100 >= CLUSTER_COUNT as usize * 99, "{matching} of {CLUSTER_COUNT} match exactly");
    }
}
