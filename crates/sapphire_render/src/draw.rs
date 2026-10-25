//! Recording a pass's mesh draws on the CPU-driven path (see the crate docs): each draw binds
//! its streams and its draw record by buffer offset, with `base_vertex` 0 and one instance.
//!
//! A static mesh's streams come from its slab at the mesh's first vertex; a skinned or morphed
//! instance's positions and normals from its skinning outputs, and its surface stream from its
//! mesh's sources. Each pass binds only the streams it reads ([`MeshPass`]). Streams are rebound
//! only when the next draw's mesh (or deformed instance) differs, so a mesh's submeshes share
//! one binding. Passes that shade bind each draw's material (group 3) the same way: only when
//! it changes.

use std::ops::Range;

use sapphire_assets::mesh::{PositionFormat, VertexShading, VertexSurface};
use sapphire_rhi::wgpu;

use crate::buffer::GrowableBuffer;
use crate::gpu_types::DrawData;
use crate::materials::{MaterialHandle, MaterialRecord};
use crate::mesh_pool::MeshPool;
use crate::pipelines::{MeshPass, format_index, slot};
use crate::scene::DrawList;
use crate::skinning::SkinnedOutputs;

const SHADING_STRIDE: u64 = size_of::<VertexShading>() as u64;
const SURFACE_STRIDE: u64 = size_of::<VertexSurface>() as u64;
const DRAW_STRIDE: u64 = size_of::<DrawData>() as u64;

/// What a frame's mesh draws read, shared by every pass that draws meshes.
#[derive(Clone, Copy)]
pub(crate) struct MeshDraws<'a> {
    pub meshes: &'a MeshPool,
    pub list: &'a DrawList,
    /// The draw records.
    pub records: &'a GrowableBuffer,
    pub skinned: SkinnedOutputs<'a>,
    /// Every uploaded material, by handle: their group 3.
    pub materials: &'a [MaterialRecord],
}

impl MeshDraws<'_> {
    /// Whether there are static meshes in `format` to draw.
    pub(crate) fn has_static(&self, format: PositionFormat) -> bool {
        !self.list.commands[format_index(format)].is_empty()
    }

    /// Whether there are skinned or morphed instances to draw.
    pub(crate) fn has_deformed(&self) -> bool {
        !self.list.deformed.is_empty()
    }

    /// Draws every static mesh in `format` with the streams `kind` reads, through the pipeline
    /// already set. The index buffer must be bound.
    pub(crate) fn draw_static(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        format: PositionFormat,
    ) {
        let slab = self.meshes.slab(format);
        let mut bound_first_vertex = None;
        let mut bound_material = None;
        for command in &self.list.commands[format_index(format)] {
            self.bind_material(pass, kind, command.material, &mut bound_material);
            if bound_first_vertex != Some(command.first_vertex) {
                let vertex = u64::from(command.first_vertex);
                let positions = slab.positions().slice(vertex * slab.position_stride()..);
                pass.set_vertex_buffer(slot::POSITIONS, positions);
                if kind.reads_previous() {
                    // Static meshes move only by their instance transform.
                    pass.set_vertex_buffer(slot::PREVIOUS_POSITIONS, positions);
                }
                if kind.reads_shading() {
                    pass.set_vertex_buffer(slot::SHADING, slab.shading().slice(vertex * SHADING_STRIDE..));
                }
                if kind.reads_surface() {
                    pass.set_vertex_buffer(slot::SURFACE, slab.surface().slice(vertex * SURFACE_STRIDE..));
                }
                bound_first_vertex = Some(command.first_vertex);
            }
            self.draw(pass, command.draw_id, command.indices.clone());
        }
    }

    /// Draws every skinned or morphed instance with the streams `kind` reads, through the f32
    /// pipeline already set. The index buffer must be bound.
    pub(crate) fn draw_deformed(&self, pass: &mut wgpu::RenderPass<'_>, kind: MeshPass) {
        let outputs_buffer = self.skinned.buffer();
        let mut bound_slot = None;
        let mut bound_material = None;
        for command in &self.list.deformed {
            if bound_slot != Some(command.slot) {
                // An instance whose outputs didn't fit (`SkinningStats::unplaced`) doesn't draw.
                let Some(outputs) = self.skinned.get(command.slot) else { continue };
                pass.set_vertex_buffer(slot::POSITIONS, outputs_buffer.slice(outputs.positions..));
                if kind.reads_previous() {
                    pass.set_vertex_buffer(
                        slot::PREVIOUS_POSITIONS,
                        outputs_buffer.slice(outputs.previous..),
                    );
                }
                if kind.reads_shading() {
                    pass.set_vertex_buffer(slot::SHADING, outputs_buffer.slice(outputs.shading..));
                }
                if kind.reads_surface() {
                    let surface = u64::from(command.surface) * 4;
                    pass.set_vertex_buffer(slot::SURFACE, self.meshes.deformable_buffer().slice(surface..));
                }
                bound_slot = Some(command.slot);
            }
            self.bind_material(pass, kind, command.material, &mut bound_material);
            self.draw(pass, command.draw_id, command.indices.clone());
        }
    }

    /// Binds `material`'s group 3 if `kind` shades with materials and it isn't bound already.
    fn bind_material(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        material: MaterialHandle,
        bound: &mut Option<MaterialHandle>,
    ) {
        if kind.binds_materials() && *bound != Some(material) {
            pass.set_bind_group(3, &self.materials[material.index()].bind_group, &[]);
            *bound = Some(material);
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, draw_id: u32, indices: Range<u32>) {
        let record = u64::from(draw_id) * DRAW_STRIDE;
        pass.set_vertex_buffer(slot::DRAW, self.records.buffer().slice(record..record + DRAW_STRIDE));
        pass.draw_indexed(indices, 0, 0..1);
    }
}
