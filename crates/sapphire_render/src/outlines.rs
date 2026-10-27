//! Outline hulls per frame (outlines v2, M1 C4; spec §5.12 "Outlines"): which draws' hulls the
//! frame draws, by their materials' LOD cutoff.
//!
//! An outline's width follows a curve over the *effective distance* (`outline.wgsl`): the view
//! depth × `tan(fov_y / 2) / tan(22.5°)`, where a model looks as large as through a 45° lens. Past
//! its material's cutoff (`OutlineCurve::cutoff`) a hull would be all but invisible and still cost
//! its vertices and its depth test, so the frame skips the draw. The test is per draw, on the
//! instance's bounding sphere (its mesh record's, transformed): a hull is dropped once the
//! sphere's nearest point is past the cutoff in view depth, the distance the shader fades by, so
//! a large instance straddling it keeps its hull, whose width the shader fades to nothing over
//! the cutoff's last quarter anyway. Nothing pops.
//!
//! The camera's distance would not do: off the view axis it runs longer than the depth (at 45°,
//! 1.24× at the side of a 16:9 frame and 1.31× in its corners), so it dropped hulls whose lines
//! the shader still drew (review RA-002). A hull behind the camera is in reach by its depth and
//! left to clipping, as its surface is: nothing culls by view yet.
//!
//! A drawn hull draws twice under temporal AA: in the main pass, and in the scene prepass, where
//! it writes its line's velocity (the silhouette's motion, M1 P1) and the outline mask that makes
//! lines fully reactive (`outline.wgsl`'s header; the `temporal` module). A dropped one leaves
//! neither, which is nothing to lose: its width had faded out.
//!
//! # Later milestones
//!
//! - With LOD selection (culling), the cutoff becomes the LOD past which hulls stop, and skinned
//!   instances' bounds come from their joints rather than the bind pose.

use sapphire_assets::material::AnimeCharacterParams;

use crate::CameraData;
use crate::mesh_pool::MeshRecord;
use crate::scene::{DrawList, InstanceSlots};

/// tan(22.5°): effective distances are normalized to a 45° vertical field of view
/// (`OUTLINE_REFERENCE_TAN_HALF_FOV` in `outline.wgsl`).
pub(crate) const REFERENCE_TAN_HALF_FOV: f32 = 0.414_213_57;

/// Outline counters from the last prepared frame (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OutlineCounts {
    /// Hulls the frame draws.
    pub drawn: u32,
    /// Hulls past their material's LOD cutoff.
    pub culled: u32,
}

/// Which draws' hulls a frame draws.
#[derive(Debug, Default)]
pub(crate) struct OutlineCull {
    /// Per draw record (`DrawList::draws`): whether the frame draws its hull; false for draws
    /// without one.
    visible: Vec<bool>,
    counts: OutlineCounts,
}

impl OutlineCull {
    /// Decides, for `camera`, which of `list`'s hulls draw: those whose instance's bounding
    /// sphere (from `meshes`, the pool's records) comes within its material's cutoff, read from
    /// the anime character block in `params(material slot)`, times `scale`
    /// (`Renderer::set_outline_distance`). Only draws with a hull are visited, their blocks
    /// read and their bounds placed (review RA-O03): most draws are the world's, without one,
    /// their material slots another family's. Allocates nothing once the draw list stops
    /// growing.
    pub(crate) fn prepare(
        &mut self,
        camera: &CameraData,
        scale: f32,
        list: &DrawList,
        slots: &InstanceSlots,
        meshes: &[MeshRecord],
        params: impl Fn(u32) -> Option<AnimeCharacterParams>,
    ) {
        profiling::scope!("OutlineCull::prepare");
        // tan(fov_y / 2) is 1 / the projection's y focal length.
        let effective = 1.0 / (camera.proj.y_axis.y.abs().max(1e-6) * REFERENCE_TAN_HALF_FOV);
        self.visible.clear();
        self.visible.resize(list.draws.len(), false);
        let hulls =
            list.commands.iter().flatten().flatten().filter(|command| command.outline).map(|c| c.draw_id);
        let deformed = list.deformed.iter().flatten().filter(|draw| draw.outline).map(|draw| draw.draw_id);
        let mut counts = OutlineCounts::default();
        for draw_id in hulls.chain(deformed) {
            let draw = &list.draws[draw_id as usize];
            let cutoff = params(draw.material).map_or(0.0, |params| params.outline_curve[3]) * scale;
            // A cutoff of 0 (or one that isn't a number) draws nothing, wherever the hull is.
            let visible = cutoff > 0.0
                && slots
                    .mesh(draw.instance)
                    .and_then(|mesh| meshes.get(mesh.index()))
                    .zip(slots.transform(draw.instance))
                    .is_some_and(|(record, transform)| {
                        let sphere = record.bounds.transformed(transform);
                        // The view is rigid, so no point of the sphere is nearer in depth than this.
                        let depth = -camera.view.transform_point3(sphere.center).z;
                        let gap = (depth - sphere.radius).max(0.0);
                        gap * effective < cutoff
                    });
            self.visible[draw_id as usize] = visible;
            counts.drawn += u32::from(visible);
            counts.culled += u32::from(!visible);
        }
        debug_assert_eq!((counts.drawn + counts.culled) as usize, list.outlines(), "every hull counted once");
        self.counts = counts;
    }

    /// Per draw record: whether the last prepared frame draws its hull (false for draws without
    /// one).
    pub(crate) fn visible(&self) -> &[bool] {
        &self.visible
    }

    pub(crate) fn counts(&self) -> OutlineCounts {
        self.counts
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use bytemuck::Zeroable;
    use glam::{Mat4, Quat, Vec3};
    use sapphire_assets::mesh::PositionFormat;

    use super::*;
    use crate::bounds::Sphere;
    use crate::gpu_types::{Dequantize, DrawData};
    use crate::mesh_pool::{MeshStreams, Placement};
    use crate::scene::{DeformedDraw, DrawCommand};
    use crate::{InstanceDeltas, InstanceId, MaterialHandle, MeshHandle, MeshInstance};

    /// A mesh record around `bounds`: outline culling reads nothing else of it.
    fn record(bounds: Sphere) -> MeshRecord {
        MeshRecord {
            streams: MeshStreams::Slab { format: PositionFormat::Unorm16, first_vertex: 0 },
            dequantize: Dequantize::default(),
            lods: Vec::new(),
            lod_vertices: Vec::new(),
            placement: Placement::default(),
            slots: Vec::new(),
            bounds,
            foliage: None,
        }
    }

    /// An anime character block whose LOD cutoff is `cutoff` effective meters.
    fn cutoff(cutoff: f32) -> AnimeCharacterParams {
        let mut params = AnimeCharacterParams::zeroed();
        params.outline_curve[3] = cutoff;
        params
    }

    /// One draw for each `(mesh, transform, material slot, hull)`, an instance of its own in
    /// slot order; its hull static (`Some(false)`), deformed (`Some(true)`) or absent (`None`).
    fn scene(draws: &[(u32, Mat4, u32, Option<bool>)]) -> (DrawList, InstanceSlots) {
        let mut slots = InstanceSlots::default();
        let changed = draws
            .iter()
            .enumerate()
            .map(|(id, &(mesh, transform, ..))| MeshInstance {
                id: InstanceId(id as u64),
                mesh: MeshHandle::from_raw(mesh),
                transform,
            })
            .collect();
        slots.apply(&InstanceDeltas { changed, removed: Vec::new() });
        let mut list = DrawList::default();
        for (draw_id, &(_, _, material, hull)) in draws.iter().enumerate() {
            let draw_id = draw_id as u32;
            list.draws.push(DrawData { instance: draw_id, material, dequantize: Dequantize::default() });
            let handle = MaterialHandle::from_raw(material);
            let (indices, slot) = (0..3, draw_id);
            match hull {
                Some(true) => list.deformed[0].push(DeformedDraw {
                    indices,
                    slot,
                    surface: 0,
                    draw_id,
                    material: handle,
                    outline: true,
                }),
                outline => list.commands[0][0].push(DrawCommand {
                    indices,
                    first_vertex: 0,
                    slot,
                    draw_id,
                    material: handle,
                    outline: outline.is_some(),
                    foliage: 0,
                }),
            }
        }
        (list, slots)
    }

    /// Points spread over the unit sphere.
    fn directions(count: u32) -> impl Iterator<Item = Vec3> {
        (0..count).map(move |i| {
            let y = 1.0 - 2.0 * (i as f32 + 0.5) / count as f32;
            let ring = (1.0 - y * y).sqrt();
            let angle = i as f32 * 2.399_963;
            Vec3::new(ring * angle.cos(), y, ring * angle.sin())
        })
    }

    #[test]
    fn hulls_stay_wherever_their_shader_draws_a_line() {
        // `outline.wgsl` fades a vertex's line by its view depth (clip w) in effective meters, to
        // a width of 0 at the cutoff. Spheres at the frustum's edges and corners and in its
        // middle, through several fields of view and aspects, from inside the cutoff to past
        // it: a hull goes only where no point of its bounds in front of the camera would draw a
        // line.
        let limit = 10.0;
        let bounds = Sphere { center: Vec3::ZERO, radius: 1.0 };
        let meshes = [record(bounds)];
        for (fov, aspect) in
            [(45.0f32, 16.0 / 9.0), (20.0, 21.0 / 9.0), (70.0, 4.0 / 3.0), (60.0, 9.0 / 16.0)]
        {
            let (eye, target) = (Vec3::new(3.0, 1.5, -2.0), Vec3::new(-4.0, 1.0, -12.0));
            let camera = CameraData::look_at(eye, target, Vec3::Y, fov.to_radians(), aspect, 0.05);
            let tan_half = (fov.to_radians() * 0.5).tan();
            let effective = tan_half / REFERENCE_TAN_HALF_FOV;
            let world_from_view = camera.view.inverse();
            let mut placed = Vec::new();
            let edges = [
                (0.0, 0.0),
                (0.97, 0.0),
                (-0.97, 0.0),
                (0.0, 0.97),
                (0.0, -0.97),
                (0.95, 0.95),
                (-0.95, 0.9),
            ];
            for (x, y) in edges {
                // The nearest point's depth, as a share of the cutoff.
                for nearest in [0.8, 0.95, 0.99, 1.01, 1.05, 1.3] {
                    for (radius, rotation) in [(0.2, Quat::IDENTITY), (1.5, Quat::from_rotation_y(0.6))] {
                        let depth = nearest * limit / effective + radius;
                        let view = Vec3::new(x * depth * tan_half * aspect, y * depth * tan_half, -depth);
                        let center = world_from_view.transform_point3(view);
                        let transform =
                            Mat4::from_scale_rotation_translation(Vec3::splat(radius), rotation, center);
                        placed.push((0, transform, 0, Some(placed.len() % 3 == 0)));
                    }
                }
            }
            let (list, slots) = scene(&placed);
            let mut cull = OutlineCull::default();
            cull.prepare(&camera, 1.0, &list, &slots, &meshes, |_| Some(cutoff(limit)));
            for (draw, &(_, transform, ..)) in placed.iter().enumerate() {
                let sphere = bounds.transformed(transform);
                let depth = |point: Vec3| -camera.view.transform_point3(point).z;
                let draws_a_line = directions(2000).any(|direction| {
                    let depth = depth(sphere.center + direction * sphere.radius);
                    depth > 0.0 && depth * effective < limit
                });
                let nearest = (depth(sphere.center) - sphere.radius) * effective;
                let visible = cull.visible()[draw];
                assert!(
                    !draws_a_line || visible,
                    "{fov}° at {aspect}: hull {draw} culled at {nearest} effective m, inside its cutoff"
                );
                if nearest > limit * 1.001 {
                    assert!(!visible, "{fov}° at {aspect}: hull {draw} wholly past its cutoff, at {nearest}");
                }
            }
        }
    }

    #[test]
    fn the_reviews_off_axis_sphere_keeps_its_hull() {
        // Review RA-002: at the side of a 16:9 frame, a sphere whose surface lies 8.4–8.8 m deep
        // lost its hull to its Euclidean distance (10.1 m past its bounds' radius) under a
        // cutoff of 10.
        let camera =
            CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, 45f32.to_radians(), 16.0 / 9.0, 0.05);
        let meshes = [record(Sphere::around([[-1.0; 3], [1.0; 3]]))];
        let place = Mat4::from_scale_rotation_translation(
            Vec3::splat(0.2),
            Quat::IDENTITY,
            Vec3::new(5.9, 0.0, -8.6),
        );
        let (list, slots) = scene(&[(0, place, 0, Some(false))]);
        let mut cull = OutlineCull::default();
        cull.prepare(&camera, 1.0, &list, &slots, &meshes, |_| Some(cutoff(10.0)));
        assert_eq!((cull.visible(), cull.counts()), (&[true][..], OutlineCounts { drawn: 1, culled: 0 }));
    }

    #[test]
    fn only_hulls_are_tested_and_counted() {
        let camera =
            CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, 45f32.to_radians(), 16.0 / 9.0, 0.05);
        let meshes = [record(Sphere { center: Vec3::ZERO, radius: 1.0 })];
        let at = |z: f32| Mat4::from_translation(Vec3::new(0.0, 0.0, -z));
        // Material slots 0 and 1 reach 20 m, 2 has no hull (a cutoff of 0) and 3 no anime block.
        let blocks = [Some(cutoff(20.0)), Some(cutoff(20.0)), Some(cutoff(0.0)), None];
        let draws = [
            (0, at(5.0), 0, None),         // environment, near: no hull to test
            (0, at(5.0), 0, Some(false)),  // static hull, near
            (0, at(50.0), 1, Some(false)), // static hull, past its cutoff
            (0, at(5.0), 1, Some(true)),   // deformed hull, near
            (0, at(50.0), 0, Some(true)),  // deformed hull, past its cutoff
            (0, at(5.0), 2, Some(false)),  // a cutoff of 0: never drawn
            (0, at(5.0), 3, Some(false)),  // no block: never drawn
            (7, at(5.0), 0, Some(false)),  // no mesh record: never drawn
            (0, at(50.0), 1, None),        // environment, far
            (0, at(5.0), 1, None),         // environment, near
        ];
        let (list, slots) = scene(&draws);
        let looked_up = RefCell::new(Vec::new());
        let params = |slot: u32| {
            looked_up.borrow_mut().push(slot);
            blocks[slot as usize]
        };
        let mut cull = OutlineCull::default();
        for _ in 0..2 {
            looked_up.borrow_mut().clear();
            cull.prepare(&camera, 1.0, &list, &slots, &meshes, params);
            let visible = [false, true, false, true, false, false, false, false, false, false];
            assert_eq!(cull.visible(), visible, "draws without a hull stay false");
            assert_eq!(cull.counts(), OutlineCounts { drawn: 2, culled: 5 });
            assert_eq!(cull.counts().drawn + cull.counts().culled, list.outlines() as u32);
            // Each hull's block is read once, and no other draw's.
            let mut read = looked_up.borrow().clone();
            read.sort_unstable();
            assert_eq!(read, [0, 0, 0, 1, 1, 2, 3]);
        }
        // The renderer's scale reaches every cutoff: at 0 nothing draws.
        cull.prepare(&camera, 0.0, &list, &slots, &meshes, params);
        assert_eq!(cull.counts(), OutlineCounts { drawn: 0, culled: 7 });
        assert!(cull.visible().iter().all(|&visible| !visible));
    }
}
