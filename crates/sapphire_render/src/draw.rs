//! Recording a pass's mesh draws on the CPU-driven path (see the crate docs): each draw binds
//! its streams and its draw record by buffer offset, with `base_vertex` 0 and one instance.
//!
//! A static mesh's streams come from its slab at the mesh's first vertex, and its foliage stream
//! (M1 W6) from the pool's foliage buffer at the mesh's first element; a skinned or morphed
//! instance's positions and normals from its skinning outputs, and its surface stream from its
//! mesh's sources. Each pass binds only the streams it reads ([`MeshPass`]). Streams are rebound
//! only when the next draw's mesh (or deformed instance) differs, so a mesh's submeshes share
//! one binding. Passes that shade bind each draw's material (group 3) the same way: only when
//! it changes. Draws come in groups of one surface variant (the draw list's), each drawn
//! through its own pipeline; outline hulls are drawn for the draws that have one and are within
//! their material's LOD cutoff this frame (`outlines`), eyes through hair for the draws whose
//! material draws through (M1 C5), and the shadow passes and the character prepass draw only the
//! instances that cast into the map they draw, or are characters (`*_where`). Fog cards draw in
//! an order of the frame's own, back to front ([`MeshDraws::draw_ordered`]).
//!
//! The debug views' replays of the main pass (M1 F5, the `debug_view` module) draw through the
//! same calls: the overdraw view's draw each alpha-tested draw twice, through two pipelines in
//! turn (`*_through_where`), and the wireframe's draw its edges, with each draw's index range doubled
//! into the line indices it binds as the index buffer ([`MeshDraws::lines`]).

use std::ops::Range;

use sapphire_assets::material::MaterialFlags;
use sapphire_assets::mesh::{PositionFormat, VertexFoliage, VertexShading, VertexSurface};
use sapphire_rhi::wgpu;

use crate::buffer::GrowableBuffer;
use crate::gpu_types::DrawData;
use crate::materials::{MaterialHandle, MaterialRecord};
use crate::mesh_pool::MeshPool;
use crate::pipelines::{MeshPass, SurfaceVariant, format_index, slot};
use crate::scene::{DeformedDraw, DrawCommand, DrawList};
use crate::skinning::SkinnedOutputs;

const SHADING_STRIDE: u64 = size_of::<VertexShading>() as u64;
const SURFACE_STRIDE: u64 = size_of::<VertexSurface>() as u64;
const FOLIAGE_STRIDE: u64 = size_of::<VertexFoliage>() as u64;
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
    /// Per draw record: whether the frame draws its hull, if it has one (the LOD cutoff,
    /// `outlines::OutlineCull`).
    pub hulls: &'a [bool],
    /// Draws index a line list of each triangle's three edges instead of the pool's triangles:
    /// the debug view's line indices, two per triangle index at twice its position
    /// (`debug_view`, the wireframe's replay), bound in its place. Off for every other pass.
    pub lines: bool,
}

/// One of a variant's draws in the draw list: a static mesh's command, by format and position
/// in its list, or a skinned or morphed instance's, by position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrawRef {
    Static { format: PositionFormat, index: u32 },
    Deformed { index: u32 },
}

/// Which of a variant's draws a pass draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrawSet {
    /// Every surface.
    Surfaces,
    /// The outline hulls the frame draws: of the draws with one, those within their material's
    /// LOD cutoff.
    Outlines,
    /// The surfaces whose material draws eyes through the character's hair
    /// ([`MaterialFlags::THROUGH_HAIR`], M1 C5).
    ThroughHair,
}

impl MeshDraws<'_> {
    /// Whether `set` includes draw record `draw_id` of `material`, which has an outline hull or
    /// not.
    fn includes(&self, set: DrawSet, draw_id: u32, material: MaterialHandle, outline: bool) -> bool {
        match set {
            DrawSet::Surfaces => true,
            DrawSet::Outlines => outline && self.hulls.get(draw_id as usize).copied().unwrap_or(false),
            DrawSet::ThroughHair => self
                .materials
                .get(material.index())
                .is_some_and(|record| record.flags.contains(MaterialFlags::THROUGH_HAIR)),
        }
    }
}

impl MeshDraws<'_> {
    /// Whether there are static meshes in `format` of `variant` to draw.
    pub(crate) fn has_static(&self, format: PositionFormat, variant: SurfaceVariant) -> bool {
        !self.list.commands[format_index(format)][variant.index()].is_empty()
    }

    /// Whether there are skinned or morphed instances of `variant` to draw.
    pub(crate) fn has_deformed(&self, variant: SurfaceVariant) -> bool {
        !self.list.deformed[variant.index()].is_empty()
    }

    /// Whether there are draws of `variant` at all, static or deformed.
    pub(crate) fn has_any(&self, variant: SurfaceVariant) -> bool {
        PositionFormat::ALL.into_iter().any(|format| self.has_static(format, variant))
            || self.has_deformed(variant)
    }

    /// Draws `set` of the static meshes in `format` of `variant` with the streams `kind` reads,
    /// through the pipeline already set. The index buffer must be bound.
    pub(crate) fn draw_static(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        format: PositionFormat,
        variant: SurfaceVariant,
        set: DrawSet,
    ) {
        self.draw_static_where(pass, kind, format, variant, set, |_| true);
    }

    /// Draws `set` of the static meshes in `format` of `variant` whose instance slot `instance`
    /// takes, with the streams `kind` reads, through the pipeline already set. The index buffer
    /// must be bound.
    pub(crate) fn draw_static_where(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        format: PositionFormat,
        variant: SurfaceVariant,
        set: DrawSet,
        instance: impl Fn(u32) -> bool,
    ) {
        self.draw_static_through_where(pass, kind, format, variant, set, instance, &[]);
    }

    /// As [`Self::draw_static_where`], but each draw once through each of `pipelines` in turn,
    /// set before it (none: once, through the pipeline already set): a debug view's replay that
    /// counts a draw's fragments and then writes its depth (`debug_view`).
    #[expect(clippy::too_many_arguments, reason = "the `_where` draw's, and the pipelines")]
    pub(crate) fn draw_static_through_where(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        format: PositionFormat,
        variant: SurfaceVariant,
        set: DrawSet,
        instance: impl Fn(u32) -> bool,
        pipelines: &[&wgpu::RenderPipeline],
    ) {
        let mut bound = Bound::default();
        for command in &self.list.commands[format_index(format)][variant.index()] {
            if !self.includes(set, command.draw_id, command.material, command.outline)
                || !instance(command.slot)
            {
                continue;
            }
            if pipelines.is_empty() {
                self.draw_command(pass, kind, format, command, &mut bound);
            }
            for pipeline in pipelines {
                pass.set_pipeline(pipeline);
                self.draw_command(pass, kind, format, command, &mut bound);
            }
        }
    }

    /// Draws `command`, a static mesh's in `format`, binding its streams and material unless
    /// `bound` says they are bound already.
    fn draw_command(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        format: PositionFormat,
        command: &DrawCommand,
        bound: &mut Bound,
    ) {
        self.bind_material(pass, kind, command.material, &mut bound.material);
        let streams = Streams::Static { format, first_vertex: command.first_vertex };
        if bound.streams != Some(streams) {
            let slab = self.meshes.slab(format);
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
            if kind.reads_foliage() {
                // The mesh's foliage stream, numbered apart from the slab's vertices.
                let foliage = u64::from(command.foliage) * FOLIAGE_STRIDE;
                pass.set_vertex_buffer(slot::FOLIAGE, self.meshes.foliage_buffer().slice(foliage..));
            }
            bound.streams = Some(streams);
        }
        self.draw(pass, command.draw_id, command.indices.clone());
    }

    /// Draws `set` of the skinned or morphed instances of `variant` with the streams `kind`
    /// reads, through the f32 pipeline already set. The index buffer must be bound.
    pub(crate) fn draw_deformed(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        variant: SurfaceVariant,
        set: DrawSet,
    ) {
        self.draw_deformed_where(pass, kind, variant, set, |_| true);
    }

    /// Draws `set` of the skinned or morphed instances of `variant` whose instance slot
    /// `instance` takes, with the streams `kind` reads, through the f32 pipeline already set. The
    /// index buffer must be bound.
    pub(crate) fn draw_deformed_where(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        variant: SurfaceVariant,
        set: DrawSet,
        instance: impl Fn(u32) -> bool,
    ) {
        self.draw_deformed_through_where(pass, kind, variant, set, instance, &[]);
    }

    /// As [`Self::draw_deformed_where`], but each draw once through each of `pipelines` in turn,
    /// as [`Self::draw_static_through_where`] draws.
    pub(crate) fn draw_deformed_through_where(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        variant: SurfaceVariant,
        set: DrawSet,
        instance: impl Fn(u32) -> bool,
        pipelines: &[&wgpu::RenderPipeline],
    ) {
        debug_assert!(!kind.reads_foliage(), "foliage draws on static meshes alone");
        let mut bound = Bound::default();
        for command in &self.list.deformed[variant.index()] {
            if !self.includes(set, command.draw_id, command.material, command.outline)
                || !instance(command.slot)
            {
                continue;
            }
            if pipelines.is_empty() {
                self.draw_deformed_command(pass, kind, command, &mut bound);
            }
            for pipeline in pipelines {
                pass.set_pipeline(pipeline);
                self.draw_deformed_command(pass, kind, command, &mut bound);
            }
        }
    }

    /// Draws `command`, a skinned or morphed instance's, binding its streams and material unless
    /// `bound` says they are bound already.
    fn draw_deformed_command(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        command: &DeformedDraw,
        bound: &mut Bound,
    ) {
        let streams = Streams::Deformed { slot: command.slot };
        if bound.streams != Some(streams) {
            // An instance whose outputs didn't fit (`SkinningStats::unplaced`) doesn't draw.
            let Some(outputs) = self.skinned.get(command.slot) else { return };
            let outputs_buffer = self.skinned.buffer();
            pass.set_vertex_buffer(slot::POSITIONS, outputs_buffer.slice(outputs.positions..));
            if kind.reads_previous() {
                pass.set_vertex_buffer(slot::PREVIOUS_POSITIONS, outputs_buffer.slice(outputs.previous..));
            }
            if kind.reads_shading() {
                pass.set_vertex_buffer(slot::SHADING, outputs_buffer.slice(outputs.shading..));
            }
            if kind.reads_surface() {
                let surface = u64::from(command.surface) * 4;
                pass.set_vertex_buffer(slot::SURFACE, self.meshes.deformable_buffer().slice(surface..));
            }
            bound.streams = Some(streams);
        }
        self.bind_material(pass, kind, command.material, &mut bound.material);
        self.draw(pass, command.draw_id, command.indices.clone());
    }

    /// Draws `draws` of `variant`'s surfaces in their order, with the streams `kind` reads, each
    /// through `pipeline` of its position format (f32 for skinned and morphed instances), set
    /// only when the format changes. The index buffer must be bound.
    pub(crate) fn draw_ordered<'p>(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        variant: SurfaceVariant,
        draws: &[DrawRef],
        pipeline: impl Fn(PositionFormat) -> &'p wgpu::RenderPipeline,
    ) {
        let mut bound = Bound::default();
        let mut bound_format = None;
        for &draw in draws {
            let format = match draw {
                DrawRef::Static { format, .. } => format,
                DrawRef::Deformed { .. } => PositionFormat::Float32,
            };
            if bound_format != Some(format) {
                pass.set_pipeline(pipeline(format));
                bound_format = Some(format);
            }
            match draw {
                DrawRef::Static { format, index } => {
                    let commands = &self.list.commands[format_index(format)][variant.index()];
                    if let Some(command) = commands.get(index as usize) {
                        self.draw_command(pass, kind, format, command, &mut bound);
                    }
                }
                DrawRef::Deformed { index } => {
                    if let Some(command) = self.list.deformed[variant.index()].get(index as usize) {
                        self.draw_deformed_command(pass, kind, command, &mut bound);
                    }
                }
            }
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
        // A draw's whole triangles lie at a multiple of 3 (every mesh's index count is one), so
        // its edges' line indices at twice its range.
        let indices = if self.lines {
            debug_assert!(indices.start.is_multiple_of(3), "{indices:?} starts mid-triangle");
            2 * indices.start..2 * indices.end
        } else {
            indices
        };
        pass.draw_indexed(indices, 0, 0..1);
    }
}

/// The streams a run of draws has bound: a static mesh's from its first vertex in a format's
/// slab, or a deformed instance's outputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Streams {
    Static { format: PositionFormat, first_vertex: u32 },
    Deformed { slot: u32 },
}

/// What a run of draws has bound, so the next draw rebinds only what differs.
#[derive(Debug, Default)]
struct Bound {
    streams: Option<Streams>,
    material: Option<MaterialHandle>,
}
