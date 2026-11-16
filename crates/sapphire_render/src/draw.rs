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
//! Every pass walks its own view's list ([`MeshDraws::lists`], M5 D1), which holds only the
//! static draws of the instances that view's culling kept (review RA-O01).
//! Every pass draws one view's instances alone (M3 U6, [`MeshDraws::stand`]): the world's, or an
//! inset's, whose instances draw nowhere else.
//!
//! The debug views' replays of the main pass (M1 F5, the `debug_view` module) draw through the
//! same calls: the overdraw view's draw each alpha-tested draw twice, through two pipelines in
//! turn (`*_through_where`), and the wireframe's draw its edges, with each draw's index range doubled
//! into the line indices it binds as the index buffer ([`MeshDraws::lines`]).
//!
//! Each group of draws counts what it submitted and what it left out, by which test, on its stack,
//! and adds that to its pass's counter once ([`MeshDraws::counter`], the `counts` module). A draw
//! of another view, or outside the group's set (a surface without a hull in an outline group, one
//! not drawn through hair in that group), isn't counted as looked at. The pipelines, materials and
//! streams the draws set count too ([`StateChanges`]).
//!
//! **Each view walks its own list** (M5 D1, the [`lists`] module, [`MeshDraws::lists`]): a group
//! draws the positions its view's list holds for it, in their view's order, and tests nothing; what
//! the view left out was counted as its list was compacted, and its pass adds that once
//! ([`MeshDraws::count_left_out`]). The debug views' replays walk the main view's list and still
//! test the instances they pick.
//!
//! **A draw draws its level of detail's indices** (M5 D4, the `levels` module): its list entry's
//! level, the index range its mesh's record holds for its submesh at that level, from the same
//! vertex streams (each level reads a prefix of them). A skinned or morphed instance draws its
//! entry's level too (M5 D5), from outputs skinned only as far as the finest level any view drew
//! it at; the ordered draws (an eye's layers, the transparent ones) draw it at the main view's
//! family's ([`MeshDraws::levels`]). Fog cards draw level 0.
//!
//! **Runs draw as one call** (M5 D2, the [`runs`] module): where a view's list formed runs of a
//! static depth-writing group, each run binds its mesh's streams and material once, its records at
//! slot 0 from their first, and draws its instances `0..n` ([`MeshDraws::runs`]). The debug views'
//! replays draw a call a draw.

mod lists;
mod runs;

use std::ops::Range;

use sapphire_assets::mesh::{PositionFormat, VertexFoliage, VertexShading, VertexSurface};
use sapphire_rhi::wgpu;

pub(crate) use self::lists::{
    CasterTests, Entry, Orders, Source, ViewList, ViewLists, ViewTests, Walk, has_level,
};
pub(crate) use self::runs::RunRecords;
pub use self::runs::{DrawRuns, RunStats};
use crate::buffer::GrowableBuffer;
use crate::counts::{ChangeCounter, Counter, PassCounts, StateChanges};
use crate::gpu_types::DrawData;
use crate::materials::{MaterialHandle, MaterialRecord};
use crate::mesh_pool::{MeshHandle, MeshPool};
use crate::pipelines::{MeshPass, SurfaceVariant, format_index, slot};
use crate::scene::{DeformedDraw, DrawCommand, DrawList};
use crate::skinning::SkinnedOutputs;
use crate::view_cull::{Culled, Verdict};

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
    /// Draws index a line list of each triangle's three edges instead of the pool's triangles:
    /// the debug view's line indices, two per triangle index at twice its position
    /// (`debug_view`, the wireframe's replay), bound in its place. Off for every other pass.
    pub lines: bool,
    /// Per instance slot, the view it stands in (M3 U6, `inset`): 0 for the world's, `n` for the
    /// frame's inset `n - 1`. Slots past its end stand in the world.
    pub stands: &'a [u16],
    /// The view this pass draws: only the instances that stand in it draw.
    pub stand: u16,
    /// The pass's counts, which its draws add to.
    pub counter: &'a Counter,
    /// The frame's state changes, which every mesh pass's draws add to.
    pub changes: &'a ChangeCounter,
    /// Each view's list this frame (M5 D1): the pass walks [`Self::walk`]'s and tests nothing.
    pub lists: &'a ViewLists,
    /// Which of [`Self::lists`] this pass walks.
    pub walk: Walk,
    /// Per instance slot, whether the view it stands in draws it, or by what test not (review
    /// RA-O01, M5 D3, the `view_cull` module): what the ordered draws ([`Self::draw_ordered`]), which
    /// no list holds, are tested by. `None` draws every one, as the shadow views do.
    pub view: Option<Culled<'a>>,
    /// The frame's run records (M5 D2, [`RunRecords`]): a run of more than one draw binds its
    /// records here.
    pub run_records: &'a wgpu::Buffer,
    /// Whether the pass draws its list's runs as one call each: every pass but the debug views'
    /// replays, which draw a call a draw.
    pub runs: bool,
    /// Per instance slot, the level the main view's family draws it at (M5 D4, D5): what the
    /// ordered draws of a skinned or morphed instance draw at, since only that much of it is
    /// skinned. `None` draws level 0.
    pub levels: Option<Culled<'a>>,
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
    /// ([`MaterialFlags::THROUGH_HAIR`](sapphire_assets::material::MaterialFlags::THROUGH_HAIR), M1 C5).
    ThroughHair,
}

/// One draw of a view's list, as a pass that draws some of them picks it (`*_where`): its
/// instance's slot, its material, and the level of detail its list draws it at. The debug views'
/// replays pick by all three (the per-object light lists by slot, M5 X1's scale views by level
/// and material too); every other pass takes every draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pick {
    pub slot: u32,
    pub material: MaterialHandle,
    pub level: u8,
}

impl Pick {
    fn of(command: &DrawCommand, level: u8) -> Self {
        Self { slot: command.slot, material: command.material, level }
    }
}

impl MeshDraws<'_> {
    /// The list this pass walks: none for the character maps' pass as a whole, which draws each
    /// map's.
    fn listed(&self) -> Option<&ViewList> {
        self.lists.list(self.walk)
    }

    /// Adds what the pass's view left out, counted as its list was compacted, to its counts: once a
    /// pass, by the pass.
    pub(crate) fn count_left_out(&self) {
        self.counter.add(&self.lists.left_out(self.walk));
    }

    /// Sets `pipeline` for the draws after, counting it.
    pub(crate) fn set_pipeline(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline) {
        pass.set_pipeline(pipeline);
        self.count_changes(&StateChanges { pipelines: 1, ..StateChanges::default() });
    }

    /// Adds `changes` to the frame's, and its pipelines to this pass's own.
    fn count_changes(&self, changes: &StateChanges) {
        self.changes.add(changes);
        self.counter.add_pipelines(changes.pipelines);
    }

    /// Whether the instance in `slot` stands in the view this pass draws.
    fn stands_here(&self, slot: u32) -> bool {
        self.stands.get(slot as usize).copied().unwrap_or(0) == self.stand
    }

    /// The verdict of the view the instance in `slot` stands in: drawn without culling.
    fn verdict(&self, slot: u32) -> Verdict {
        self.view.map_or(Verdict::Drawn, |view| view.verdict(slot))
    }

    /// The indices `command` draws at `level`: its submesh's range in its mesh's record at that
    /// level (M5 D4), its own at level 0.
    fn indices(&self, command: &DrawCommand, level: u8) -> Range<u32> {
        self.level_indices(command.mesh, command.submesh, &command.indices, level)
    }

    /// The indices `command`, a skinned or morphed instance's, draws at `level`, as
    /// [`Self::indices`] (M5 D5).
    fn deformed_indices(&self, command: &DeformedDraw, level: u8) -> Range<u32> {
        self.level_indices(command.mesh, command.submesh, &command.indices, level)
    }

    /// `submesh` of `mesh`'s range at `level`: `own`, its level 0's, at level 0.
    fn level_indices(&self, mesh: MeshHandle, submesh: u16, own: &Range<u32>, level: u8) -> Range<u32> {
        if level == 0 {
            return own.clone();
        }
        let records = self.meshes.records();
        let submesh = records
            .get(mesh.index())
            .and_then(|record| record.lods.get(usize::from(level))?.get(usize::from(submesh)));
        submesh.map_or_else(
            || own.clone(),
            |submesh| submesh.first_index..submesh.first_index + submesh.index_count,
        )
    }

    /// The level the ordered draws draw the instance in `slot` at: the main view's family's.
    fn ordered_level(&self, slot: u32) -> u8 {
        self.levels.map_or(0, |levels| levels.level(slot))
    }
}

impl MeshDraws<'_> {
    /// Whether there are static meshes in `format` of `variant` to draw: in the view's list, of any
    /// set.
    pub(crate) fn has_static(&self, format: PositionFormat, variant: SurfaceVariant) -> bool {
        self.listed().is_some_and(|list| list.has(Source::Static(format), variant))
    }

    /// Whether there are skinned or morphed instances of `variant` to draw, as [`Self::has_static`].
    pub(crate) fn has_deformed(&self, variant: SurfaceVariant) -> bool {
        self.listed().is_some_and(|list| list.has(Source::Deformed, variant))
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

    /// Draws `set` of the static meshes in `format` of `variant` that `instance` picks, with the
    /// streams `kind` reads, through the pipeline already set. The index buffer must be bound.
    pub(crate) fn draw_static_where(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        format: PositionFormat,
        variant: SurfaceVariant,
        set: DrawSet,
        instance: impl Fn(Pick) -> bool,
    ) {
        self.draw_static_through_where(pass, kind, format, variant, set, instance, &[]);
    }

    /// Draws `set` of the static meshes in `format` of `variant`, as [`Self::draw_static`], but in the
    /// reverse of their order, a call a draw: the main pass's walk where it tests `Equal` against the
    /// scene prepass's depth (M5 D10, [`MainDepth::FromPrepass`](crate::MainDepth)), where the last of
    /// the draws at a pixel's depth keeps it, as the first does where each tests `Greater`.
    pub(crate) fn draw_static_reversed(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        format: PositionFormat,
        variant: SurfaceVariant,
        set: DrawSet,
    ) {
        let mut bound = Bound::default();
        let mut counts = PassCounts::default();
        let hull = set == DrawSet::Outlines;
        let commands = &self.list.commands[format_index(format)][variant.index()];
        let Some(list) = self.listed() else { return };
        for &Entry { at, level } in list.group(Source::Static(format), variant, set).iter().rev() {
            let command = &commands[at as usize];
            let indices = self.indices(command, level);
            let count = indices.len() as u32;
            self.draw_command(pass, kind, format, command, indices, &mut bound);
            counts.considered += 1;
            counts.mesh_call(1, count, level, hull);
        }
        self.counter.add(&counts);
        self.count_changes(&bound.changes);
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
        instance: impl Fn(Pick) -> bool,
        pipelines: &[&wgpu::RenderPipeline],
    ) {
        let mut bound = Bound::default();
        let mut counts = PassCounts::default();
        let hull = set == DrawSet::Outlines;
        let commands = &self.list.commands[format_index(format)][variant.index()];
        let Some(list) = self.listed() else { return };
        let listed = list.group(Source::Static(format), variant, set);
        let runs = if self.runs && pipelines.is_empty() && set == DrawSet::Surfaces {
            list.runs(format, variant)
        } else {
            &[]
        };
        if !runs.is_empty() {
            // The view's runs, which take every draw of its group in order, each run of one level:
            // a pass's list is tested already, and a replay that picks instances draws no runs.
            for run in runs {
                let entries = &listed[run.first as usize..][..run.count as usize];
                let Entry { at, level } = entries[0];
                let command = &commands[at as usize];
                debug_assert!(
                    entries.iter().all(|entry| entry.level == level
                        && instance(Pick::of(&commands[entry.at as usize], level))),
                    "a run's draws are of one level, and its pass takes every one"
                );
                let indices = self.indices(command, level);
                let count = indices.len() as u32;
                self.bind_static(pass, kind, format, command, &mut bound);
                let records = if run.count == 1 { self.records.buffer() } else { self.run_records };
                self.draw_from(pass, records, run.record, run.count, indices);
                counts.considered += run.count;
                counts.mesh_call(run.count, count, level, hull);
            }
            self.counter.add(&counts);
            self.count_changes(&bound.changes);
            return;
        }
        let mut draw = |command: &DrawCommand, level: u8, bound: &mut Bound, counts: &mut PassCounts| {
            let indices = self.indices(command, level);
            let count = indices.len() as u32;
            if pipelines.is_empty() {
                self.draw_command(pass, kind, format, command, indices.clone(), bound);
                counts.mesh_call(1, count, level, hull);
            }
            for pipeline in pipelines {
                pass.set_pipeline(pipeline);
                bound.changes.pipelines += 1;
                self.draw_command(pass, kind, format, command, indices.clone(), bound);
                counts.mesh_call(1, count, level, hull);
            }
        };
        // The view's own: listed, in order, and tested already, but for a debug replay's pick of
        // instances (the passes' take every one).
        for &Entry { at, level } in listed {
            let command = &commands[at as usize];
            counts.considered += 1;
            if instance(Pick::of(command, level)) {
                draw(command, level, &mut bound, &mut counts);
            } else {
                counts.culled_by_pass += 1;
            }
        }
        self.counter.add(&counts);
        self.count_changes(&bound.changes);
    }

    /// Draws `indices` of `command`, a static mesh's in `format` (its level's, [`Self::indices`]),
    /// binding its streams and material unless `bound` says they are bound already.
    fn draw_command(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        format: PositionFormat,
        command: &DrawCommand,
        indices: Range<u32>,
        bound: &mut Bound,
    ) {
        self.bind_static(pass, kind, format, command, bound);
        self.draw_from(pass, self.records.buffer(), command.draw_id, 1, indices);
    }

    /// Binds the streams and material of `command`, a static mesh's in `format`, unless `bound`
    /// says they are bound already.
    fn bind_static(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        format: PositionFormat,
        command: &DrawCommand,
        bound: &mut Bound,
    ) {
        self.bind_material(pass, kind, command.material, bound);
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
            bound.changes.vertex_buffers += streams_bound(kind);
            bound.streams = Some(streams);
        }
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

    /// Draws `set` of the skinned or morphed instances of `variant` that `instance` picks, with
    /// the streams `kind` reads, through the f32 pipeline already set. The index buffer must be
    /// bound.
    pub(crate) fn draw_deformed_where(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        variant: SurfaceVariant,
        set: DrawSet,
        instance: impl Fn(Pick) -> bool,
    ) {
        self.draw_deformed_through_where(pass, kind, variant, set, instance, &[]);
    }

    /// Draws `set` of the skinned or morphed instances of `variant` in the reverse of their order, as
    /// [`Self::draw_static_reversed`].
    pub(crate) fn draw_deformed_reversed(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        variant: SurfaceVariant,
        set: DrawSet,
    ) {
        let mut bound = Bound::default();
        let mut counts = PassCounts::default();
        let hull = set == DrawSet::Outlines;
        let deformed = &self.list.deformed[variant.index()];
        let Some(list) = self.listed() else { return };
        // Each at its entry's level, as the forward draw's (M5 D5).
        for &Entry { at, level } in list.group(Source::Deformed, variant, set).iter().rev() {
            let command = &deformed[at as usize];
            let indices = self.deformed_indices(command, level);
            let count = indices.len() as u32;
            counts.considered += 1;
            if self.draw_deformed_command(pass, kind, command, indices, &mut bound) {
                counts.mesh_call(1, count, level, hull);
            }
        }
        self.counter.add(&counts);
        self.count_changes(&bound.changes);
    }

    /// As [`Self::draw_deformed_where`], but each draw once through each of `pipelines` in turn,
    /// as [`Self::draw_static_through_where`] draws.
    pub(crate) fn draw_deformed_through_where(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        variant: SurfaceVariant,
        set: DrawSet,
        instance: impl Fn(Pick) -> bool,
        pipelines: &[&wgpu::RenderPipeline],
    ) {
        debug_assert!(!kind.reads_foliage(), "foliage draws on static meshes alone");
        let mut bound = Bound::default();
        let mut counts = PassCounts::default();
        let hull = set == DrawSet::Outlines;
        let deformed = &self.list.deformed[variant.index()];
        let mut draw = |command: &DeformedDraw, level: u8, bound: &mut Bound, counts: &mut PassCounts| {
            let indices = self.deformed_indices(command, level);
            let count = indices.len() as u32;
            if pipelines.is_empty() && self.draw_deformed_command(pass, kind, command, indices.clone(), bound)
            {
                counts.mesh_call(1, count, level, hull);
            }
            for pipeline in pipelines {
                pass.set_pipeline(pipeline);
                bound.changes.pipelines += 1;
                if self.draw_deformed_command(pass, kind, command, indices.clone(), bound) {
                    counts.mesh_call(1, count, level, hull);
                }
            }
        };
        let Some(list) = self.listed() else { return };
        // Each at its entry's level (M5 D5): 0 for a character that stays whole.
        for &Entry { at, level } in list.group(Source::Deformed, variant, set) {
            let command = &deformed[at as usize];
            counts.considered += 1;
            if instance(Pick { slot: command.slot, material: command.material, level }) {
                draw(command, level, &mut bound, &mut counts);
            } else {
                counts.culled_by_pass += 1;
            }
        }
        self.counter.add(&counts);
        self.count_changes(&bound.changes);
    }

    /// Draws `indices` of `command`, a skinned or morphed instance's (its level's,
    /// [`Self::deformed_indices`]), binding its streams and material unless `bound` says they are
    /// bound already. Whether it drew: an instance whose outputs didn't fit
    /// (`SkinningStats::unplaced`) doesn't.
    fn draw_deformed_command(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        command: &DeformedDraw,
        indices: Range<u32>,
        bound: &mut Bound,
    ) -> bool {
        let streams = Streams::Deformed { slot: command.slot };
        if bound.streams != Some(streams) {
            let Some(outputs) = self.skinned.get(command.slot) else { return false };
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
            bound.changes.vertex_buffers += streams_bound(kind);
            bound.streams = Some(streams);
        }
        self.bind_material(pass, kind, command.material, bound);
        self.draw_from(pass, self.records.buffer(), command.draw_id, 1, indices);
        true
    }

    /// Draws `draws` of `variant`'s surfaces in their order, with the streams `kind` reads, each
    /// through `pipeline` of its position format (f32 for skinned and morphed instances), set
    /// only when the format changes, but those its view's culling left out (fog cards, M5 D3),
    /// which it counts by their test. The index buffer must be bound.
    pub(crate) fn draw_ordered<'p>(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        variant: SurfaceVariant,
        draws: &[DrawRef],
        pipeline: impl Fn(PositionFormat) -> &'p wgpu::RenderPipeline,
    ) {
        let mut bound = Bound::default();
        let mut counts = PassCounts::default();
        let mut bound_format = None;
        for &draw in draws {
            let (format, slot) = match draw {
                DrawRef::Static { format, index } => {
                    let commands = &self.list.commands[format_index(format)][variant.index()];
                    (format, commands.get(index as usize).map(|command| command.slot))
                }
                DrawRef::Deformed { index } => {
                    let deformed = &self.list.deformed[variant.index()];
                    (PositionFormat::Float32, deformed.get(index as usize).map(|draw| draw.slot))
                }
            };
            let Some(slot) = slot.filter(|&slot| self.stands_here(slot)) else { continue };
            let verdict = self.verdict(slot);
            if verdict != Verdict::Drawn {
                counts.considered += 1;
                counts.count_culled(verdict);
                continue;
            }
            if bound_format != Some(format) {
                pass.set_pipeline(pipeline(format));
                bound.changes.pipelines += 1;
                bound_format = Some(format);
            }
            match draw {
                DrawRef::Static { format, index } => {
                    let commands = &self.list.commands[format_index(format)][variant.index()];
                    // Fog cards draw level 0 alone (M5 D4).
                    if let Some(command) = commands.get(index as usize).filter(|c| self.stands_here(c.slot)) {
                        self.draw_command(pass, kind, format, command, command.indices.clone(), &mut bound);
                        counts.considered += 1;
                        counts.mesh_call(1, command.indices.len() as u32, 0, false);
                    }
                }
                DrawRef::Deformed { index } => {
                    let deformed = &self.list.deformed[variant.index()];
                    if let Some(command) = deformed.get(index as usize).filter(|c| self.stands_here(c.slot)) {
                        counts.considered += 1;
                        // At the main view's family's level (M5 D5); a submesh simplified away
                        // there isn't drawn, as a list leaves it out.
                        let level = self.ordered_level(command.slot);
                        if !has_level(command.levels, level) {
                            continue;
                        }
                        let indices = self.deformed_indices(command, level);
                        let count = indices.len() as u32;
                        if self.draw_deformed_command(pass, kind, command, indices, &mut bound) {
                            counts.mesh_call(1, count, level, false);
                        }
                    }
                }
            }
        }
        self.counter.add(&counts);
        self.count_changes(&bound.changes);
    }

    /// Binds `material`'s group 3 if `kind` shades with materials and it isn't bound already.
    fn bind_material(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        material: MaterialHandle,
        bound: &mut Bound,
    ) {
        if kind.binds_materials() && bound.material != Some(material) {
            pass.set_bind_group(3, self.materials[material.index()].bind_group(), &[]);
            bound.material = Some(material);
            bound.changes.materials += 1;
        }
    }

    /// Draws `instances` instances of `indices`, whose records lie side by side in `records` from
    /// record `first`, bound there at slot 0 (no first instance: ADR 0002).
    fn draw_from(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        records: &wgpu::Buffer,
        first: u32,
        instances: u32,
        indices: Range<u32>,
    ) {
        let start = u64::from(first) * DRAW_STRIDE;
        let end = start + u64::from(instances) * DRAW_STRIDE;
        pass.set_vertex_buffer(slot::DRAW, records.slice(start..end));
        // A draw's whole triangles lie at a multiple of 3 (every mesh's index count is one), so
        // its edges' line indices at twice its range.
        let indices = if self.lines {
            debug_assert!(indices.start.is_multiple_of(3), "{indices:?} starts mid-triangle");
            2 * indices.start..2 * indices.end
        } else {
            indices
        };
        pass.draw_indexed(indices, 0, 0..instances);
    }
}

/// The streams a run of draws has bound: a static mesh's from its first vertex in a format's
/// slab, or a deformed instance's outputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Streams {
    Static { format: PositionFormat, first_vertex: u32 },
    Deformed { slot: u32 },
}

/// What a run of draws has bound, so the next draw rebinds only what differs, and what it changed.
#[derive(Debug, Default)]
struct Bound {
    streams: Option<Streams>,
    material: Option<MaterialHandle>,
    changes: StateChanges,
}

/// The vertex buffers a mesh's streams take in `kind`: positions, and each other stream it reads.
fn streams_bound(kind: MeshPass) -> u32 {
    let read = [kind.reads_previous(), kind.reads_shading(), kind.reads_surface(), kind.reads_foliage()];
    1 + read.into_iter().map(u32::from).sum::<u32>()
}
