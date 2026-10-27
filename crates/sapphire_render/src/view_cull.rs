//! Main-view culling of static world geometry (spec §5.12 "Scene and draws", review RA-O01).
//!
//! Every recorded frame tests each static instance whose draws are all environment or foliage
//! surfaces (the draw list's [`CullCandidate`]s) against the main camera's frustum, by its
//! mesh's bounding sphere under its transform ([`Sphere::transformed`], which holds the mesh
//! under any affine transform, shear included). The main pass, the scene prepass and the debug
//! views' replays of the main pass skip the draws of those wholly outside, so every view of the
//! main pass agrees, and the frame's pixels are the same bit for bit: what is culled reaches no
//! sample. The test is conservative:
//!
//! - **Padding:** the frustum's sides move out by [`PAD_PIXELS`] and by the frame's temporal AA
//!   jitter, and the spheres grow by a share of their size for rounding (the cooked positions'
//!   quantization, the GPU's transforms), so a triangle that could cover a sample at the edge, or
//!   a line of the wireframe view, is never left out.
//! - **Foliage** grows by the frame's wind margin (`foliage::wind_margin`), as shadow casters do.
//! - **Not tested:** anime character surfaces (an instance with any: their outline hulls reach
//!   past the bounds), fog cards (camera-facing ones turn about their bounds), skinned and morphed
//!   instances (their bounds are the bind pose's), instances with a projective transform, and
//!   bounds that aren't finite.
//!
//! What else draws instances keeps its whole list: the shadow cascades and character maps (their
//! own culling, per view), the sky cube, the ambient probes and the character prepass. No LOD is
//! chosen here, and no posed bounds are made: those come with LOD selection (crate docs, "Later
//! milestones"). [`RenderSettings::view_culling`](crate::RenderSettings::view_culling) turns it
//! off, for comparisons and triage.

use glam::{Mat4, Vec4, Vec4Swizzles};

use crate::bounds::Sphere;
use crate::mesh_pool::MeshRecord;
use crate::scene::{DrawList, InstanceSlots};

/// Pixels the frustum's sides move out by, beyond temporal AA's jitter: whole pixels of margin
/// for the samples a triangle at the edge could cover under MSAA, and for the debug views' lines.
pub(crate) const PAD_PIXELS: f32 = 2.0;

/// The share of a sphere's radius it grows by for rounding: the cooked positions' quantization
/// (half a unorm16 step across the mesh's extent, under 2e-5 of the radius) and the GPU's
/// transforms.
const RADIUS_SLACK: f32 = 1e-4;

/// The share of a plane distance's terms it may be off by in f32 and still be rounding.
const DISTANCE_SLACK: f32 = 1e-5;

/// Main-view culling counters from the last recorded frame (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewCullingStats {
    /// Instances the main view tested: static ones whose draws are all environment or foliage
    /// surfaces, with a transform that isn't projective. 0 with culling off.
    pub tested: u32,
    /// Of those, the ones wholly outside the main view's frustum, whose draws the main pass, the
    /// scene prepass and the debug views skipped.
    pub culled: u32,
    /// Surface draws the main pass drew:
    /// [`RenderStats::draws`](crate::RenderStats::draws) less the culled instances'.
    pub drawn: u32,
}

/// A static instance whose draws are all environment or foliage surfaces: one the main view can
/// cull by its bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CullCandidate {
    pub slot: u32,
    pub mesh: crate::MeshHandle,
    /// Its surface draws.
    pub draws: u32,
}

/// The main view's frustum, as planes facing in: left, right, bottom, top and near. The far
/// plane is at infinity (`CameraData::proj`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frustum {
    /// xyz the unit normal, w the distance along it from the origin, negated: a point is inside
    /// a plane where `dot(xyz, p) + w >= 0`.
    planes: [Vec4; 5],
}

impl Frustum {
    /// The frustum of `clip_from_world` (unjittered) for a `size` image, its sides moved out by
    /// `pad` pixels.
    pub(crate) fn new(clip_from_world: Mat4, (width, height): (u32, u32), pad: f32) -> Self {
        let [x, y, z, w] = [0, 1, 2, 3].map(|row| clip_from_world.row(row));
        // A pixel is 2 / size of normalized device coordinates: the sides at ±(1 + 2 pad / size).
        let sx = 1.0 + 2.0 * pad / width.max(1) as f32;
        let sy = 1.0 + 2.0 * pad / height.max(1) as f32;
        let planes = [w * sx + x, w * sx - x, w * sy + y, w * sy - y, w - z].map(|plane| {
            let length = plane.xyz().length();
            // A degenerate camera's plane excludes nothing.
            if length > 0.0 && length.is_finite() { plane / length } else { Vec4::W }
        });
        Self { planes }
    }

    /// Whether `sphere` lies wholly outside: never when it isn't finite.
    pub(crate) fn excludes(&self, sphere: &Sphere) -> bool {
        let (center, radius) = (sphere.center, sphere.radius);
        let reach = radius * (1.0 + RADIUS_SLACK);
        self.planes.iter().any(|plane| {
            let along = plane.xyz().dot(center);
            let slack = DISTANCE_SLACK * (along.abs() + plane.w.abs());
            // False for NaN: a sphere that isn't finite stays.
            along + plane.w + reach + slack < 0.0
        })
    }
}

/// Which instance slots the main view draws, from each recorded frame's test.
#[derive(Debug, Default)]
pub(crate) struct ViewCull {
    /// Per instance slot, while culling is on: whether the main view draws its static draws.
    visible: Vec<bool>,
    on: bool,
    stats: ViewCullingStats,
}

impl ViewCull {
    /// Tests `list`'s candidates against `frustum` (`None`: culling is off) with `slots`'
    /// transforms and `meshes`' bounds, foliage grown by `margin`. `draws` is the frame's surface
    /// draws, for the stats.
    pub(crate) fn update(
        &mut self,
        frustum: Option<&Frustum>,
        list: &DrawList,
        slots: &InstanceSlots,
        meshes: &[MeshRecord],
        margin: impl Fn(&MeshRecord) -> f32,
        draws: u32,
    ) {
        profiling::scope!("ViewCull::update");
        self.visible.clear();
        self.on = frustum.is_some();
        let Some(frustum) = frustum else {
            self.stats = ViewCullingStats { drawn: draws, ..ViewCullingStats::default() };
            return;
        };
        self.visible.resize(slots.len(), true);
        let (mut tested, mut culled, mut culled_draws) = (0, 0, 0);
        for candidate in &list.cullable {
            let Some(transform) = slots.transform(candidate.slot) else { continue };
            let Some(record) = meshes.get(candidate.mesh.index()) else { continue };
            // The vertex stages transform by the whole matrix: a projective one would move what
            // the sphere holds.
            if transform.row(3) != Vec4::W {
                continue;
            }
            tested += 1;
            let mut sphere = record.bounds.transformed(transform);
            if record.foliage.is_some() {
                sphere.radius += margin(record);
            }
            if frustum.excludes(&sphere) {
                self.visible[candidate.slot as usize] = false;
                culled += 1;
                culled_draws += candidate.draws;
            }
        }
        self.stats = ViewCullingStats { tested, culled, drawn: draws.saturating_sub(culled_draws) };
    }

    /// Per instance slot, whether the main view draws it, or `None` with culling off.
    pub(crate) fn mask(&self) -> Option<&[bool]> {
        self.on.then_some(self.visible.as_slice())
    }

    pub(crate) fn stats(&self) -> ViewCullingStats {
        self.stats
    }
}

/// Whether `point` lands on a `size` image or within `pixels` of its edge, in front of the
/// camera, as `clip_from_world` projects it: the tests' reference.
#[cfg(test)]
fn on_screen(clip_from_world: Mat4, size: (u32, u32), pixels: f32, point: glam::Vec3) -> bool {
    let clip = clip_from_world * point.extend(1.0);
    let ndc = clip.xy() / clip.w;
    let limit = glam::Vec2::new(1.0 + 2.0 * pixels / size.0 as f32, 1.0 + 2.0 * pixels / size.1 as f32);
    clip.w > 0.0 && ndc.abs().cmple(limit).all()
}

#[cfg(test)]
mod tests {
    use glam::{Affine3A, Vec3};
    use sapphire_assets::material::{MaterialFamily, MaterialFlags};
    use sapphire_assets::mesh::PositionFormat;

    use super::*;
    use crate::gpu_types::Dequantize;
    use crate::materials::MaterialHandle;
    use crate::mesh_pool::{DeformSource, MeshStreams, Placement, SubmeshDraw, SubmeshMaterial};
    use crate::scene::DrawLimits;
    use crate::{CameraData, InstanceDeltas, InstanceId, MeshHandle, MeshInstance};

    const SIZE: (u32, u32) = (640, 360);
    const NEAR: f32 = 0.05;

    /// A camera at the origin looking down −z, 60° high.
    fn camera() -> CameraData {
        CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, 60f32.to_radians(), 640.0 / 360.0, NEAR)
    }

    fn frustum(pad: f32) -> Frustum {
        let camera = camera();
        Frustum::new(camera.proj * camera.view, SIZE, pad)
    }

    /// The world point `depth` meters ahead that lands at normalized device coordinates `ndc`:
    /// reverse-Z with an infinite far plane puts it at depth `NEAR / depth`.
    fn at(ndc: [f32; 2], depth: f32) -> Vec3 {
        let camera = camera();
        (camera.proj * camera.view).inverse().project_point3(Vec3::new(ndc[0], ndc[1], NEAR / depth))
    }

    #[test]
    fn spheres_inside_straddling_and_outside() {
        let frustum = frustum(0.0);
        let sphere = |center, radius| Sphere { center, radius };
        assert!(!frustum.excludes(&sphere(Vec3::new(0.0, 0.0, -10.0), 1.0)), "ahead");
        assert!(!frustum.excludes(&sphere(at([1.0, 0.0], 10.0), 0.01)), "on the right edge");
        assert!(!frustum.excludes(&sphere(at([1.2, 0.0], 10.0), 5.0)), "past it, but reaching in");
        assert!(frustum.excludes(&sphere(at([1.2, 0.0], 10.0), 0.5)), "past it");
        assert!(frustum.excludes(&sphere(at([-1.2, 0.3], 10.0), 0.5)), "past the left edge");
        assert!(frustum.excludes(&sphere(at([0.2, 1.3], 10.0), 0.5)), "above");
        assert!(frustum.excludes(&sphere(at([0.2, -1.3], 10.0), 0.5)), "below");
        assert!(frustum.excludes(&sphere(Vec3::new(0.0, 0.0, 10.0), 1.0)), "behind");
        assert!(!frustum.excludes(&sphere(Vec3::new(0.0, 0.0, 0.5), 1.0)), "around the camera");
        // The near plane, 5 cm ahead.
        assert!(frustum.excludes(&sphere(Vec3::new(0.0, 0.0, -0.02), 0.02)), "short of the near plane");
        assert!(!frustum.excludes(&sphere(Vec3::new(0.0, 0.0, -0.02), 0.04)), "reaching past it");
        // Far away, as the mountains are.
        assert!(!frustum.excludes(&sphere(Vec3::new(0.0, 100.0, -3000.0), 200.0)));
        assert!(frustum.excludes(&sphere(Vec3::new(3000.0, 0.0, 0.0), 200.0)), "beside, far");
        // Not finite: kept.
        for center in [Vec3::NAN, Vec3::new(f32::INFINITY, 0.0, -1.0)] {
            assert!(!frustum.excludes(&sphere(center, 1.0)), "{center}");
        }
        assert!(!frustum.excludes(&sphere(Vec3::new(0.0, 0.0, 10.0), f32::NAN)));
    }

    #[test]
    fn padding_keeps_spheres_within_its_pixels_of_the_edge() {
        let padded = frustum(PAD_PIXELS);
        let unpadded = frustum(0.0);
        // A pixel is 2 / 640 of x, 2 / 360 of y. Tiny spheres a pixel and three pixels past each
        // side.
        let tiny = |ndc: [f32; 2]| Sphere { center: at(ndc, 20.0), radius: 1e-4 };
        let (px, py) = (2.0 / 640.0, 2.0 / 360.0);
        for (x, y) in [(1.0, 0.0), (-1.0, 0.3), (0.5, 1.0), (-0.5, -1.0)] {
            let step = [x * px, y * py];
            let one = tiny([x + step[0], y + step[1]]);
            let three = tiny([x + 3.0 * step[0], y + 3.0 * step[1]]);
            assert!(!padded.excludes(&one) && unpadded.excludes(&one), "a pixel past ({x}, {y})");
            assert!(padded.excludes(&three), "three pixels past ({x}, {y})");
        }
    }

    #[test]
    fn excludes_matches_a_projection_of_the_center_for_points() {
        // Seeded points: excluded exactly when their projection lands outside the padded image
        // or behind the near plane, up to the slack.
        let camera = camera();
        let clip_from_world = camera.proj * camera.view;
        let frustum = frustum(PAD_PIXELS);
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = |low: f32, high: f32| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            low + (high - low) * ((state >> 40) as f32 / (1u64 << 24) as f32)
        };
        let mut culled = 0;
        for _ in 0..20_000 {
            let center = Vec3::new(next(-40.0, 40.0), next(-25.0, 25.0), next(-60.0, 5.0));
            let excluded = frustum.excludes(&Sphere { center, radius: 0.0 });
            let shown = on_screen(clip_from_world, SIZE, PAD_PIXELS, center) && -center.z >= NEAR;
            // Within a hair of an edge either answer is right; elsewhere they agree.
            let near_edge = on_screen(clip_from_world, SIZE, PAD_PIXELS + 0.01, center)
                != on_screen(clip_from_world, SIZE, PAD_PIXELS - 0.01, center);
            if !near_edge {
                assert_eq!(excluded, !shown, "{center}");
            }
            culled += u32::from(excluded);
        }
        assert!(culled > 5_000 && culled < 19_000, "{culled}");
    }

    /// A static mesh record of one environment draw, bounded by a unit sphere, with foliage data
    /// where `foliage`.
    fn record(foliage: bool, deformed: bool) -> MeshRecord {
        let streams = if deformed {
            MeshStreams::Deformable(DeformSource::default())
        } else {
            MeshStreams::Slab { format: PositionFormat::Unorm16, first_vertex: 0 }
        };
        MeshRecord {
            streams,
            dequantize: Dequantize { position_scale: [1.0; 3], ..Dequantize::default() },
            lods: vec![vec![SubmeshDraw {
                first_index: 0,
                index_count: 3,
                material: SubmeshMaterial { handle: MaterialHandle(0), slot: 0 },
            }]],
            lod_vertices: vec![3],
            placement: Placement::default(),
            slots: Vec::new(),
            bounds: Sphere { center: Vec3::ZERO, radius: 1.0 },
            foliage: foliage.then_some(0),
        }
    }

    #[test]
    fn candidates_are_tested_and_culled_by_their_transforms() {
        // Meshes 0 to 3: environment, foliage, deformed environment and an anime character's, by
        // their materials' handles.
        let mut meshes =
            [record(false, false), record(true, false), record(false, true), record(false, false)];
        meshes[1].lods[0][0].material.handle = MaterialHandle(1);
        meshes[3].lods[0][0].material.handle = MaterialHandle(2);
        let kind = |handle: MaterialHandle| match handle.0 {
            0 => (MaterialFamily::Environment, MaterialFlags::empty()),
            1 => (MaterialFamily::Foliage, MaterialFlags::empty()),
            _ => (MaterialFamily::AnimeCharacter, MaterialFlags::OUTLINE),
        };
        let place = |id: u64, mesh: u32, transform: Mat4| MeshInstance {
            id: InstanceId(id),
            mesh: MeshHandle::from_raw(mesh),
            transform,
        };
        let (ahead, behind) =
            (Mat4::from_translation(Vec3::NEG_Z * 10.0), Mat4::from_translation(Vec3::Z * 10.0));
        // 2.6 m right of the right edge, 10 m ahead: a unit sphere 0.8 m past the padded plane.
        let right = Mat4::from_translation(at([1.0, 0.0], 10.0) + Vec3::X * 2.6);
        let projective =
            Mat4::from_cols(Vec4::X, Vec4::Y, Vec4::new(0.0, 0.0, 1.0, 0.5), Vec4::new(0.0, 0.0, 10.0, 1.0));
        let instances = [
            place(0, 0, ahead),
            place(1, 0, behind),
            place(2, 1, behind),
            place(3, 1, right),
            place(4, 2, behind),
            place(5, 3, behind),
            place(6, 0, projective),
            // Three times the size, where the foliage is: it reaches into the view.
            place(7, 0, right * Mat4::from(Affine3A::from_scale(Vec3::splat(3.0)))),
        ];
        let mut slots = InstanceSlots::default();
        slots.apply(&InstanceDeltas { changed: instances.to_vec(), removed: Vec::new() });
        let mut list = DrawList::default();
        list.build(&meshes, kind, |_| 0, &slots, DrawLimits::NONE);
        // The static environment and foliage instances are candidates; the deformed one and the
        // anime character aren't.
        let candidates: Vec<u32> = list.cullable.iter().map(|candidate| candidate.slot).collect();
        assert_eq!(candidates, [0, 1, 2, 3, 6, 7]);

        let mut cull = ViewCull::default();
        let frustum = frustum(PAD_PIXELS);
        // Calm, the foliage beside the view is culled; with wind that carries it 2 m, it isn't.
        // The projective transform isn't tested.
        for (margin, culled) in [(0.0, vec![1, 2, 3]), (2.0, vec![1, 2])] {
            cull.update(Some(&frustum), &list, &slots, &meshes, |_| margin, 8);
            let mask = cull.mask().expect("on");
            let found: Vec<u32> = (0..8).filter(|&slot| !mask[slot as usize]).collect();
            assert_eq!(found, culled, "wind margin {margin}");
            let count = culled.len() as u32;
            assert_eq!(cull.stats(), ViewCullingStats { tested: 5, culled: count, drawn: 8 - count });
        }
        // Off: no mask, every draw drawn.
        cull.update(None, &list, &slots, &meshes, |_| 0.0, 8);
        assert_eq!((cull.mask(), cull.stats()), (None, ViewCullingStats { tested: 0, culled: 0, drawn: 8 }));
    }
}
