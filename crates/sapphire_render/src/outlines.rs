//! Outline hulls per frame (outlines v2, M1 C4; spec §5.12 "Outlines"): which draws' hulls the
//! frame draws, by their materials' LOD cutoff.
//!
//! An outline's width follows a curve over the *effective distance* (`outline.wgsl`): the view
//! depth × `tan(fov_y / 2) / tan(22.5°)`, where a model looks as large as through a 45° lens. Past
//! its material's cutoff (`OutlineCurve::cutoff`) a hull would be all but invisible and still cost
//! its vertices and its depth test, so the frame skips the draw. The test is per draw, on the
//! instance's bounding sphere (its mesh record's, transformed): a hull is dropped once the
//! sphere's nearest point is past the cutoff, so a large instance straddling it keeps its hull,
//! whose width the shader fades to nothing over the cutoff's last quarter anyway. Nothing pops.
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
    /// Per draw record (`DrawList::draws`): whether the frame draws its hull, if it has one.
    visible: Vec<bool>,
    counts: OutlineCounts,
}

impl OutlineCull {
    /// Decides, for `camera`, which of `list`'s hulls draw: those whose instance's bounding
    /// sphere (from `meshes`, the pool's records) comes within its material's cutoff, read from
    /// the anime character block in `params(material slot)`, times `scale`
    /// (`Renderer::set_outline_distance`). Allocates nothing once the draw list stops growing.
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
        for draw in &list.draws {
            let record = slots.mesh(draw.instance).and_then(|mesh| meshes.get(mesh.index()));
            let cutoff = params(draw.material).map_or(0.0, |params| params.outline_curve[3]) * scale;
            let visible = record.zip(slots.transform(draw.instance)).is_some_and(|(record, transform)| {
                let sphere = record.bounds.transformed(transform);
                let gap = (sphere.center.distance(camera.position) - sphere.radius).max(0.0);
                gap * effective < cutoff
            });
            self.visible.push(visible);
        }
        let hulls =
            list.commands.iter().flatten().flatten().filter(|command| command.outline).map(|c| c.draw_id);
        let deformed = list.deformed.iter().flatten().filter(|draw| draw.outline).map(|draw| draw.draw_id);
        let drawn = hulls.chain(deformed).filter(|&draw| self.visible[draw as usize]).count() as u32;
        self.counts = OutlineCounts { drawn, culled: list.outlines() as u32 - drawn };
    }

    /// Per draw record: whether the last prepared frame draws its hull.
    pub(crate) fn visible(&self) -> &[bool] {
        &self.visible
    }

    pub(crate) fn counts(&self) -> OutlineCounts {
        self.counts
    }
}
