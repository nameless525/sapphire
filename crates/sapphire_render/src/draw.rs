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
//! Every view's passes skip the draws of instances its culling left out (review RA-O01, M5 D3,
//! [`MeshDraws::view`]); the shadow passes draw from a copy without it, as their plan culls theirs.
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
//! **Each view walks its own list** (M5 D1, the [`lists`] module): with [`MeshDraws::lists`], a
//! group draws the positions its view's list holds for it, in their view's order, and tests nothing;
//! what the view left out was counted as its list was compacted, and its pass adds that once
//! ([`MeshDraws::count_left_out`]). Without, every group walks the whole draw list and tests each
//! draw as it goes: the filtered walk, kept for the test that compares the two
//! (`Renderer::walk_filtered`), which m5-plan D2 deletes. The debug views' replays walk the main
//! view's list and still test the instances they pick.

mod lists;

use std::ops::Range;

use sapphire_assets::material::MaterialFlags;
use sapphire_assets::mesh::{PositionFormat, VertexFoliage, VertexShading, VertexSurface};
use sapphire_rhi::wgpu;

pub(crate) use self::lists::{CasterTests, Orders, Source, ViewLists, ViewTests, Walk};
use crate::buffer::GrowableBuffer;
use crate::counts::{ChangeCounter, Counter, PassCounts, StateChanges};
use crate::gpu_types::DrawData;
use crate::materials::{MaterialHandle, MaterialRecord};
use crate::mesh_pool::MeshPool;
use crate::pipelines::{MeshPass, SurfaceVariant, format_index, slot};
use crate::scene::{DeformedDraw, DrawCommand, DrawList};
use crate::shadows::plan::SlotCaster;
use crate::skinning::SkinnedOutputs;
use crate::view_cull::{Culled, Verdict};

const SHADING_STRIDE: u64 = size_of::<VertexShading>() as u64;
const SURFACE_STRIDE: u64 = size_of::<VertexSurface>() as u64;
const FOLIAGE_STRIDE: u64 = size_of::<VertexFoliage>() as u64;
const DRAW_STRIDE: u64 = size_of::<DrawData>() as u64;

/// The level of detail every mesh draw draws, until levels are selected (m5-plan D4).
const DRAWN_LEVEL: u8 = 0;

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
    /// Per instance slot, whether the view it stands in draws it, or by what test not (review
    /// RA-O01, M5 D3, the `view_cull` module): for the main pass, both prepasses, the debug views
    /// and each inset's passes. `None` draws every one, as the shadow views do.
    pub view: Option<Culled<'a>>,
    /// A shadow cascade's casters and its index, on the filtered walk: what its pick left out by
    /// distance or size counts as such (M5 D3).
    pub casters: Option<(&'a [SlotCaster], usize)>,
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
    /// `None` walks the whole draw list, testing each draw (the filtered walk m5-plan D2 deletes).
    pub lists: Option<&'a ViewLists>,
    /// Which of [`Self::lists`] this pass walks.
    pub walk: Walk,
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
    /// The list this pass walks, if the frame walks lists.
    fn listed(&self) -> Option<&lists::ViewList> {
        self.lists.and_then(|lists| lists.list(self.walk))
    }

    /// Adds what the pass's view left out, counted as its list was compacted, to its counts: once a
    /// pass, by the pass. Nothing on the filtered walk, whose groups count it as they go.
    pub(crate) fn count_left_out(&self) {
        if let Some(lists) = self.lists {
            self.counter.add(&lists.left_out(self.walk));
        }
    }

    /// Sets `pipeline` for the draws after, counting it.
    pub(crate) fn set_pipeline(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline) {
        pass.set_pipeline(pipeline);
        self.changes.add(&StateChanges { pipelines: 1, ..StateChanges::default() });
    }

    /// Whether the instance in `slot` stands in the view this pass draws.
    fn stands_here(&self, slot: u32) -> bool {
        self.stands.get(slot as usize).copied().unwrap_or(0) == self.stand
    }

    /// Whether `set` holds draw record `draw_id` of `material`, which has an outline hull or not:
    /// [`Pick::Out`] if not, and [`Pick::Cutoff`] for a hull past its material's cutoff.
    fn includes(&self, set: DrawSet, draw_id: u32, material: MaterialHandle, outline: bool) -> Pick {
        let held = match set {
            DrawSet::Surfaces => true,
            DrawSet::Outlines if !outline => false,
            DrawSet::Outlines => {
                return if self.hulls.get(draw_id as usize).copied().unwrap_or(false) {
                    Pick::In
                } else {
                    Pick::Cutoff
                };
            }
            DrawSet::ThroughHair => self
                .materials
                .get(material.index())
                .is_some_and(|record| record.flags.contains(MaterialFlags::THROUGH_HAIR)),
        };
        if held { Pick::In } else { Pick::Out }
    }

    /// Whether a draw of the instance in `slot`, standing here, that its set holds as `pick`
    /// draws, counting it in `counts` as looked at and, if not drawn, by the test that left it out.
    fn passes(&self, counts: &mut PassCounts, pick: Pick, slot: u32, instance: impl Fn(u32) -> bool) -> bool {
        match pick {
            Pick::Out => return false,
            Pick::Cutoff => {
                counts.considered += 1;
                counts.culled_by_cutoff += 1;
                return false;
            }
            Pick::In => counts.considered += 1,
        }
        if !instance(slot) {
            // A cascade's pick: by its volume, or by distance or size (M5 D3).
            let left_out =
                self.casters.and_then(|(casters, cascade)| casters.get(slot as usize)?.left_out(cascade));
            match left_out {
                Some(verdict @ (Verdict::Distance | Verdict::Size)) => counts.count_culled(verdict),
                _ => counts.culled_by_pass += 1,
            }
            return false;
        }
        match self.verdict(slot) {
            Verdict::Drawn => true,
            verdict => {
                counts.count_culled(verdict);
                false
            }
        }
    }

    /// The verdict of the view the instance in `slot` stands in: drawn without culling.
    fn verdict(&self, slot: u32) -> Verdict {
        self.view.map_or(Verdict::Drawn, |view| view.verdict(slot))
    }
}

/// Whether a draw set holds a draw ([`MeshDraws::includes`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
    In,
    /// Not of the set: not looked at.
    Out,
    /// An outline hull past its material's distance cutoff.
    Cutoff,
}

impl MeshDraws<'_> {
    /// Whether there are static meshes in `format` of `variant` to draw: in the view's list, of any
    /// set, or on the filtered walk, in the draw list.
    pub(crate) fn has_static(&self, format: PositionFormat, variant: SurfaceVariant) -> bool {
        match self.listed() {
            Some(list) => list.has(Source::Static(format), variant),
            None => !self.list.commands[format_index(format)][variant.index()].is_empty(),
        }
    }

    /// Whether there are skinned or morphed instances of `variant` to draw, as [`Self::has_static`].
    pub(crate) fn has_deformed(&self, variant: SurfaceVariant) -> bool {
        match self.listed() {
            Some(list) => list.has(Source::Deformed, variant),
            None => !self.list.deformed[variant.index()].is_empty(),
        }
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
        let mut counts = PassCounts::default();
        let hull = set == DrawSet::Outlines;
        let commands = &self.list.commands[format_index(format)][variant.index()];
        let mut draw = |command: &DrawCommand, bound: &mut Bound, counts: &mut PassCounts| {
            let indices = command.indices.len() as u32;
            if pipelines.is_empty() {
                self.draw_command(pass, kind, format, command, bound);
                counts.mesh_draw(indices, DRAWN_LEVEL, hull);
            }
            for pipeline in pipelines {
                pass.set_pipeline(pipeline);
                bound.changes.pipelines += 1;
                self.draw_command(pass, kind, format, command, bound);
                counts.mesh_draw(indices, DRAWN_LEVEL, hull);
            }
        };
        if let Some(list) = self.listed() {
            // The view's own: listed, in order, and tested already, but for a debug replay's pick
            // of instances (the passes' take every one).
            for &index in list.group(Source::Static(format), variant, set) {
                let command = &commands[index as usize];
                counts.considered += 1;
                if instance(command.slot) {
                    draw(command, &mut bound, &mut counts);
                } else {
                    counts.culled_by_pass += 1;
                }
            }
        } else {
            for command in commands {
                if !self.stands_here(command.slot) {
                    continue;
                }
                let pick = self.includes(set, command.draw_id, command.material, command.outline);
                if self.passes(&mut counts, pick, command.slot, &instance) {
                    draw(command, &mut bound, &mut counts);
                }
            }
        }
        self.counter.add(&counts);
        self.changes.add(&bound.changes);
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
        let mut counts = PassCounts::default();
        let hull = set == DrawSet::Outlines;
        let deformed = &self.list.deformed[variant.index()];
        let mut draw = |command: &DeformedDraw, bound: &mut Bound, counts: &mut PassCounts| {
            let indices = command.indices.len() as u32;
            if pipelines.is_empty() && self.draw_deformed_command(pass, kind, command, bound) {
                counts.mesh_draw(indices, DRAWN_LEVEL, hull);
            }
            for pipeline in pipelines {
                pass.set_pipeline(pipeline);
                bound.changes.pipelines += 1;
                if self.draw_deformed_command(pass, kind, command, bound) {
                    counts.mesh_draw(indices, DRAWN_LEVEL, hull);
                }
            }
        };
        if let Some(list) = self.listed() {
            for &index in list.group(Source::Deformed, variant, set) {
                let command = &deformed[index as usize];
                counts.considered += 1;
                if instance(command.slot) {
                    draw(command, &mut bound, &mut counts);
                } else {
                    counts.culled_by_pass += 1;
                }
            }
        } else {
            for command in deformed {
                if !self.stands_here(command.slot) {
                    continue;
                }
                let pick = self.includes(set, command.draw_id, command.material, command.outline);
                if self.passes(&mut counts, pick, command.slot, &instance) {
                    draw(command, &mut bound, &mut counts);
                }
            }
        }
        self.counter.add(&counts);
        self.changes.add(&bound.changes);
    }

    /// Draws `command`, a skinned or morphed instance's, binding its streams and material unless
    /// `bound` says they are bound already. Whether it drew: an instance whose outputs didn't fit
    /// (`SkinningStats::unplaced`) doesn't.
    fn draw_deformed_command(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: MeshPass,
        command: &DeformedDraw,
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
        self.draw(pass, command.draw_id, command.indices.clone());
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
                    if let Some(command) = commands.get(index as usize).filter(|c| self.stands_here(c.slot)) {
                        self.draw_command(pass, kind, format, command, &mut bound);
                        counts.considered += 1;
                        counts.mesh_draw(command.indices.len() as u32, DRAWN_LEVEL, false);
                    }
                }
                DrawRef::Deformed { index } => {
                    let deformed = &self.list.deformed[variant.index()];
                    if let Some(command) = deformed.get(index as usize).filter(|c| self.stands_here(c.slot)) {
                        counts.considered += 1;
                        if self.draw_deformed_command(pass, kind, command, &mut bound) {
                            counts.mesh_draw(command.indices.len() as u32, DRAWN_LEVEL, false);
                        }
                    }
                }
            }
        }
        self.counter.add(&counts);
        self.changes.add(&bound.changes);
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
