//! Per-frame draw list: turns `MeshInstance`s into instance data, draw records and draw
//! commands. The vectors are reused across frames, so steady state allocates nothing (R6).
//!
//! The commands are for the CPU-driven submission path, which is portable everywhere. The
//! GPU-driven path (later) writes equivalent multi-draw-indirect records on the GPU, with the
//! draw ID in `first_instance` (see the crate docs and `draw_ids`).
//!
//! M1 replaces the full per-frame rebuild with persistent instances updated from extract
//! deltas, and sorts opaque draws by pipeline, material and mesh (spec §5.12).

use std::ops::Range;

use crate::MeshInstance;
use crate::gpu_types::{DrawData, InstanceData};
use crate::mesh_pool::MeshRecord;

/// One `draw_indexed` call on the CPU-driven path: `base_vertex` 0 (indices are absolute) and
/// instances `0..1`, with the draw-ID vertex buffer bound at `draw_id`. The vertex shader
/// reads its draw record through that ID; see [`crate::draw_ids`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DrawCommand {
    pub indices: Range<u32>,
    pub draw_id: u32,
}

#[derive(Debug, Default)]
pub(crate) struct DrawList {
    pub instances: Vec<InstanceData>,
    pub draws: Vec<DrawData>,
    pub commands: Vec<DrawCommand>,
}

impl DrawList {
    pub(crate) fn build(&mut self, meshes: &[MeshRecord], instances: &[MeshInstance]) {
        profiling::scope!("DrawList::build");
        self.instances.clear();
        self.draws.clear();
        self.commands.clear();
        for instance in instances {
            let mesh = meshes.get(instance.mesh.index());
            debug_assert!(mesh.is_some(), "{:?} was not uploaded to this renderer", instance.mesh);
            let Some(mesh) = mesh else { continue };
            let instance_index = to_u32(self.instances.len());
            self.instances.push(InstanceData::new(instance.transform));
            for submesh in &mesh.submeshes {
                let draw_id = to_u32(self.draws.len());
                self.draws.push(DrawData { instance: instance_index, material: submesh.material });
                self.commands.push(DrawCommand {
                    indices: submesh.first_index..submesh.first_index + submesh.index_count,
                    draw_id,
                });
            }
        }
    }
}

fn to_u32(len: usize) -> u32 {
    u32::try_from(len).expect("more than u32::MAX draws in one frame")
}

#[cfg(test)]
mod tests {
    use glam::Mat4;

    use super::*;
    use crate::MeshHandle;
    use crate::mesh_pool::SubmeshDraw;

    #[test]
    fn expands_instances_into_one_draw_per_submesh() {
        let meshes = [
            MeshRecord { submeshes: vec![SubmeshDraw { first_index: 0, index_count: 6, material: 0 }] },
            MeshRecord {
                submeshes: vec![
                    SubmeshDraw { first_index: 6, index_count: 3, material: 1 },
                    SubmeshDraw { first_index: 9, index_count: 12, material: 2 },
                ],
            },
        ];
        let instances = [
            MeshInstance { mesh: MeshHandle::from_index(1), transform: Mat4::IDENTITY },
            MeshInstance {
                mesh: MeshHandle::from_index(0),
                transform: Mat4::from_translation(glam::Vec3::X),
            },
        ];
        let mut list = DrawList::default();
        list.build(&meshes, &instances);

        assert_eq!(list.instances.len(), 2);
        assert_eq!(
            list.draws,
            [
                DrawData { instance: 0, material: 1 },
                DrawData { instance: 0, material: 2 },
                DrawData { instance: 1, material: 0 },
            ]
        );
        assert_eq!(list.commands[1], DrawCommand { indices: 9..21, draw_id: 1 });
        assert_eq!(list.commands[2], DrawCommand { indices: 0..6, draw_id: 2 });

        // Rebuilding reuses the vectors instead of appending.
        list.build(&meshes, &instances[..1]);
        assert_eq!((list.instances.len(), list.draws.len(), list.commands.len()), (1, 2, 2));
    }
}
