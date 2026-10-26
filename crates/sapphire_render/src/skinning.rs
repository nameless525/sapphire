//! Compute skinning and morph targets (spec §5.12 "Skinning and morph targets", M1 R2).
//!
//! Every instance of a skinned or morphed mesh (a *deformed* instance: the engine's characters)
//! owns output streams in one shared buffer, which every pass draws it from instead of the
//! mesh's cooked streams: f32 positions (slot 1, drawn through the f32 pipelines with an
//! identity dequantization) and [`VertexShading`](sapphire_assets::mesh::VertexShading) normals
//! (slot 2). The surface stream (slot 3) stays the mesh's, shared by all its instances. A
//! static mesh never comes near any of this.
//!
//! # Poses
//!
//! - A packet's [`SkinnedInstance`](crate::SkinnedInstance)s are new poses. `prepare` keeps each
//!   instance's latest one, converted to the GPU layout (palette matrices as three rows of a 3×4
//!   matrix, weights dense per target), until a frame is recorded; several packets prepared
//!   before a record leave only the last pose.
//! - A frame skins only the instances posed since the last recorded frame, plus new ones, which
//!   start in their bind pose with every weight 0. An instance that holds still (hitstop,
//!   pause) keeps its outputs and costs nothing.
//! - Only the LOD prefix being drawn is skinned (`MeshRecord::lod_vertices`): LOD0 until LOD
//!   selection lands with culling, which then passes the instance's LOD.
//!
//! # Velocity
//!
//! Each instance's positions are double-buffered: a frame that skins it writes the half that
//! doesn't hold the last drawn pose, so the scene prepass reads both, this frame's and the last
//! drawn one, and the velocity of a deforming vertex costs one more 12-byte vertex fetch.
//! Re-skinning the previous pose from last frame's palette (the alternative) would read the
//! bind pose, influences, palette and every morph delta a second time, and would need last
//! frame's morph weights too; the second half costs 12 bytes a vertex of memory instead. A frame
//! that doesn't skin an instance draws it with no motion: previous and current are the same
//! half.
//!
//! # One dispatch
//!
//! All of a frame's instances are skinned by one dispatch rather than one each. The outputs are
//! one read-write storage buffer, and wgpu fences every dispatch that writes one against the
//! next, so per-instance dispatches would run one after another, each draining the GPU; small
//! instances (distant LODs, props) would leave it mostly idle. Each instance takes a run of whole
//! 64-thread workgroups instead, and the shader finds its instance's job by binary search per
//! workgroup. Per-frame data (jobs, palettes, weights) is one storage buffer written through the
//! staging belt, so the pass binds three storage buffers in all (budget: crate docs).
//!
//! # Outline normals (M1 C4)
//!
//! Meshes store the outline normal in the tangent frame of the shading normal, a frame derived
//! from the normal alone (`sapphire_assets::mesh::outline_frame`), and the outputs do the same
//! with the deformed normal, so every pass decodes static and deformed vertices alike. The frame
//! turns about the normal as the normal moves, so the pass carries the outline normal across
//! itself: decoded to object space, turned by the rotation that takes the bind normal to the
//! morphed one (it follows the morphed surface), transformed by the blended matrix with the
//! normal, and stored in the output normal's frame. Transforming the outline normal by the same
//! matrix, rather than rebuilding it from the deformed normal's frame, keeps the hull closed where
//! linear blending isn't a rotation (a bent joint): a hard edge's split vertices share one outline
//! normal and one matrix, so they get one result.
//!
//! # Later milestones
//!
//! - Joints with non-uniform scale skew normals slightly, since normals take the blended
//!   linear part (renormalized), as most engines do. Squash-and-stretch rigs that need exact
//!   normals would add a cofactor matrix per palette entry.
//! - A morph target that bends a hard edge turns each split vertex's outline normal with its own
//!   normal, so the hull can open there by the bend's angle × the outline's width. Character
//!   morphs (faces) move smooth surfaces, whose split vertices (UV seams) share a normal; if
//!   hard-edged morphs need it, the cooker can bake outline-normal deltas per target.
//! - Culling: skin only visible instances, and select their LOD prefix. Bounds of the posed
//!   instance (from its joints) then replace the bind-pose mesh bounds, which per-object light
//!   lists use today.
//! - GPU-driven draws: the surface stream lives with the mesh and positions with the instance,
//!   so multi-draw-indirect needs vertex pulling or a per-instance surface copy for these.

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use glam::Affine3A;
use sapphire_assets::mesh::PositionFormat;
use sapphire_rhi::wgpu;

use crate::buffer::{self, GrowableBuffer, RangeAllocator};
use crate::mesh_pool::{DeformSource, MeshPool, MeshRecord};
use crate::scene::InstanceSlots;
use crate::shaders::{self, Program, ShaderSources};
use crate::{FramePacket, MeshHandle};

/// Threads per workgroup (`SKINNING_WORKGROUP` in `shaders/skinning.wgsl`): one vertex each.
pub(crate) const WORKGROUP_SIZE: u32 = 64;

/// Words of output per vertex: two halves of f32 positions and the two shading words.
const OUTPUT_WORDS: u32 = 8;
const OUTPUT_VERTEX_BYTES: u64 = OUTPUT_WORDS as u64 * 4;

/// The LOD whose prefix is skinned and drawn: LOD0 until LOD selection (see the module docs).
const DRAWN_LOD: usize = 0;

/// Bits of [`Job::flags`] (`FLAG_*` in `shaders/skinning.wgsl`).
pub(crate) mod flags {
    pub const FLOAT_POSITIONS: u32 = 1;
    pub const SKIN: u32 = 2;
    pub const WIDE_INFLUENCES: u32 = 4;
    pub const MORPHS: u32 = 8;
}

/// One palette matrix as the shader reads it: the rows of a 3×4 affine matrix. 48 bytes.
type PaletteRows = [[f32; 4]; 3];

/// Skinning counters from the last prepared frame (rule R9). The skinning pass's GPU time is
/// the frame graph's, as `skinning` in [`GpuTimings`](crate::graph::GpuTimings).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkinningStats {
    /// Skinned or morphed instances in the scene.
    pub instances: u32,
    /// Instances the next recorded frame skins: those posed since the last one, and new ones.
    pub skinned: u32,
    /// Vertices those instances skin: each one's drawn LOD prefix.
    pub vertices: u32,
    /// Palette bytes those poses upload, 48 per matrix.
    pub palette_bytes: u32,
    /// Bytes of output streams in use, 32 per vertex: positions twice (for velocity) and
    /// normals.
    pub output_bytes: u64,
    /// Poses in the last packet that were ignored: for an instance without a skinned or morphed
    /// mesh, with a palette of the wrong length, or not finite.
    pub ignored_poses: u32,
    /// Skinned or morphed instances that don't draw because their outputs don't fit: the output
    /// buffer is bound whole as storage, so it is capped at a storage binding (128 MiB on the
    /// device floor, about 4 million vertices across all instances). They get outputs once room
    /// frees up.
    pub unplaced: u32,
}

/// One instance's work in the frame's dispatch (WGSL `Job` in `shaders/skinning.wgsl`, read as
/// five 16-byte rows). Offsets are in words, palettes in rows.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct Job {
    pub first_workgroup: u32,
    pub vertex_count: u32,
    pub flags: u32,
    pub palette: u32,
    pub positions: u32,
    pub shading: u32,
    pub influences: u32,
    pub morph_offsets: u32,
    pub morph_deltas: u32,
    pub weights: u32,
    pub out_positions: u32,
    pub out_shading: u32,
    /// xyz; w unused.
    pub position_offset: [f32; 4],
    pub position_scale: [f32; 4],
}

/// Rows of 16 bytes per [`Job`] (`JOB_ROWS`).
pub(crate) const JOB_ROWS: u32 = (size_of::<Job>() / 16) as u32;

/// Where a deformed instance's streams are this frame, in bytes into the output buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InstanceOutputs {
    pub positions: u64,
    /// The positions it was last drawn with: `positions` unless it was skinned this frame.
    pub previous: u64,
    pub shading: u64,
}

/// A deformed instance's state.
#[derive(Clone, Copy, Debug)]
struct Deformed {
    mesh: MeshHandle,
    /// Its outputs' first vertex in the output buffer, in [`OUTPUT_WORDS`] units.
    first: u32,
    /// Vertices its outputs hold: the whole mesh, so any LOD fits.
    vertex_count: u32,
    /// Which half of its positions holds the current pose.
    current: u32,
    /// Its outputs hold a pose a frame was drawn with.
    drawn: bool,
    /// Skinned in this recorded frame, after an earlier one drew it: its last drawn positions
    /// are in the other half.
    moved: bool,
    /// The skin the next recorded frame applies.
    pending: Option<Pending>,
}

impl Deformed {
    fn outputs(&self) -> InstanceOutputs {
        let base = u64::from(self.first) * OUTPUT_VERTEX_BYTES;
        let half = u64::from(self.vertex_count) * 12;
        let positions = base + u64::from(self.current) * half;
        let previous = if self.moved { base + u64::from(self.current ^ 1) * half } else { positions };
        InstanceOutputs { positions, previous, shading: base + 2 * half }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    /// The bind pose, with every weight 0: a new instance that hasn't been posed.
    Rest,
    /// A pose: its palette starts at `palette` of `Skinning::palettes`, its dense weights at
    /// `weights` of `Skinning::weights`, and `morphs` says whether any is non-zero.
    Pose { palette: u32, weights: u32, morphs: bool },
}

/// The deformed instances, their output buffer and the skinning pass.
#[derive(Debug)]
pub(crate) struct Skinning {
    /// Created with the first skinned or morphed mesh, so apps without any never compile it.
    pass: Option<SkinningPipeline>,
    /// Per instance slot.
    instances: Vec<Option<Deformed>>,
    /// Slots with a deformed instance.
    live: Vec<u32>,
    /// Slots that took a pending skin since the last record; may repeat or be stale.
    queued: Vec<u32>,
    /// Pending poses' palettes and weights, kept until the frame that uploads them has run.
    palettes: Vec<PaletteRows>,
    weights: Vec<f32>,
    /// The last record took the pending skins: the next prepare starts afresh.
    recorded: bool,
    /// The recorded frame's jobs, in dispatch order.
    jobs: Vec<Job>,
    workgroups: u32,
    outputs: GrowableBuffer,
    ranges: RangeAllocator,
    /// Slots of instances whose outputs didn't fit (see [`SkinningStats::unplaced`]).
    unplaced: Vec<u32>,
    /// Header, jobs, palettes and weights of the recorded frame.
    frame: GrowableBuffer,
    bind_group: Option<wgpu::BindGroup>,
    /// A buffer the bind group holds was replaced.
    stale: bool,
    stats: SkinningStats,
}

#[derive(Debug)]
struct SkinningPipeline {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: wgpu::ComputePipeline,
}

impl Skinning {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let limits = device.limits();
        let max_storage = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size);
        Self {
            pass: None,
            instances: Vec::new(),
            live: Vec::new(),
            queued: Vec::new(),
            palettes: Vec::new(),
            weights: Vec::new(),
            recorded: false,
            jobs: Vec::new(),
            workgroups: 0,
            // Written by the pass, drawn as vertex data, and bound whole as storage. Empty until
            // the first deformed instance.
            outputs: GrowableBuffer::new(
                device,
                "skinning outputs",
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
                0,
                max_storage,
            ),
            ranges: RangeAllocator::default(),
            unplaced: Vec::new(),
            frame: GrowableBuffer::new(device, "skinning frame", wgpu::BufferUsages::STORAGE, 0, max_storage),
            bind_group: None,
            stale: true,
            stats: SkinningStats::default(),
        }
    }

    /// Builds the pass's pipeline from `sources`, unless it exists: for the first skinned or
    /// morphed mesh.
    pub(crate) fn enable(
        &mut self,
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        sources: &ShaderSources,
    ) {
        if self.pass.is_none() {
            self.pass = Some(SkinningPipeline::new(device, cache, sources));
        }
    }

    /// The pass's pipeline built from `sources` instead, if it was built, for
    /// [`Self::set_pipeline`] (shader hot reload). Its bind group layout is the old one's, so
    /// the bind group stays.
    pub(crate) fn rebuilt_pipeline(
        &self,
        device: &wgpu::Device,
        cache: Option<&wgpu::PipelineCache>,
        sources: &ShaderSources,
    ) -> Option<wgpu::ComputePipeline> {
        let pass = self.pass.as_ref()?;
        Some(SkinningPipeline::pipeline(device, &pass.pipeline_layout, cache, sources))
    }

    pub(crate) fn set_pipeline(&mut self, pipeline: wgpu::ComputePipeline) {
        if let Some(pass) = &mut self.pass {
            pass.pipeline = pipeline;
        }
    }

    /// The mesh pool's deformable buffer was replaced.
    pub(crate) fn sources_moved(&mut self) {
        self.stale = true;
    }

    /// Follows the scene's instance changes (`slots` has the packet's deltas applied) and
    /// keeps the packet's poses for the next recorded frame.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        packet: &FramePacket,
        slots: &InstanceSlots,
        meshes: &[MeshRecord],
    ) {
        profiling::scope!("Skinning::prepare");
        if std::mem::take(&mut self.recorded) {
            self.palettes.clear();
            self.weights.clear();
        }
        for &slot in slots.freed() {
            self.remove(slot);
        }
        for &slot in slots.meshed() {
            self.remove(slot);
            let mesh = slots.mesh(slot).expect("a slot just given a mesh is live");
            if let Some(source) = meshes.get(mesh.index()).and_then(MeshRecord::deform) {
                self.add(device, queue, slot, mesh, source.vertex_count, false);
            }
        }
        // Instances that didn't fit try again: others may have freed room. Rare, so the list is
        // simply rebuilt.
        for slot in std::mem::take(&mut self.unplaced) {
            let mesh = slots.mesh(slot).expect("unplaced slots are live");
            let source = meshes[mesh.index()].deform().expect("unplaced instances have deformable meshes");
            self.add(device, queue, slot, mesh, source.vertex_count, true);
        }
        let mut ignored = 0;
        for skinned in &packet.skinned {
            if !self.pose(packet, skinned, slots, meshes) {
                ignored += 1;
            }
        }

        let (mut skinned, mut vertices, mut palette_bytes) = (0, 0, 0);
        for &slot in &self.live {
            let state = self.instances[slot as usize].as_ref().expect("live slots are deformed");
            let Some(pending) = state.pending else { continue };
            let record = &meshes[state.mesh.index()];
            skinned += 1;
            vertices += record.lod_vertices[DRAWN_LOD];
            if let Pending::Pose { .. } = pending {
                let source = record.deform().expect("deformed instances have deformable meshes");
                palette_bytes += source.palette_len() * size_of::<PaletteRows>() as u32;
            }
        }
        self.stats = SkinningStats {
            instances: self.live.len() as u32,
            skinned,
            vertices,
            palette_bytes,
            output_bytes: u64::from(self.ranges.end()) * OUTPUT_VERTEX_BYTES,
            ignored_poses: ignored,
            unplaced: self.unplaced.len() as u32,
        };
    }

    /// Gives the instance in `slot` output streams for `vertex_count` vertices, starting in its
    /// bind pose, if they fit; otherwise lists it as unplaced (warning unless it is a `retry`).
    fn add(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        slot: u32,
        mesh: MeshHandle,
        vertex_count: u32,
        retry: bool,
    ) {
        // Outputs written so far are kept: they hold other instances' last drawn poses.
        let used = u64::from(self.ranges.end()) * OUTPUT_VERTEX_BYTES;
        let first = self.ranges.allocate(vertex_count);
        let needed = u64::from(self.ranges.end()) * OUTPUT_VERTEX_BYTES;
        if needed > self.outputs.max_size() {
            self.ranges.free(first, vertex_count);
            self.unplaced.push(slot);
            if !retry {
                tracing::warn!(
                    target: crate::LOG_TARGET,
                    slot,
                    vertex_count,
                    limit = self.outputs.max_size(),
                    "skinning outputs are full: the instance won't draw until room frees up"
                );
            }
            return;
        }
        self.stale |= self.outputs.reserve(device, queue, needed, used);
        if self.instances.len() <= slot as usize {
            self.instances.resize(slot as usize + 1, None);
        }
        self.instances[slot as usize] = Some(Deformed {
            mesh,
            first,
            vertex_count,
            current: 0,
            drawn: false,
            moved: false,
            pending: Some(Pending::Rest),
        });
        self.live.push(slot);
        self.queued.push(slot);
    }

    /// Frees the outputs of the instance in `slot`, if it has any.
    fn remove(&mut self, slot: u32) {
        self.unplaced.retain(|&unplaced| unplaced != slot);
        let Some(state) = self.instances.get_mut(slot as usize).and_then(Option::take) else { return };
        self.ranges.free(state.first, state.vertex_count);
        let index = self.live.iter().position(|&live| live == slot).expect("deformed slots are live");
        self.live.swap_remove(index);
    }

    /// Keeps `skinned`'s pose for the next record. Returns `false` if it is ignored.
    fn pose(
        &mut self,
        packet: &FramePacket,
        skinned: &crate::SkinnedInstance,
        slots: &InstanceSlots,
        meshes: &[MeshRecord],
    ) -> bool {
        let Some(slot) = slots.slot(skinned.id) else { return false };
        let Some(state) = self.instances.get_mut(slot as usize).and_then(Option::as_mut) else {
            return false;
        };
        let source = meshes[state.mesh.index()].deform().expect("deformed instances have deformable meshes");
        let (Some(palette), Some(weights)) = (packet.palette(skinned), packet.morph_weights(skinned)) else {
            return false;
        };
        let targets = source.morph_targets() as usize;
        let weights = &weights[..weights.len().min(targets)];
        if palette.len() != source.palette_len() as usize
            || !palette.iter().all(Affine3A::is_finite)
            || !weights.iter().all(|weight| weight.is_finite())
        {
            return false;
        }
        let morphs = weights.iter().any(|&weight| weight != 0.0);
        // A pose already pending since the last record is overwritten in place: the mesh, and so
        // the sizes, are the same.
        let (palette_at, weights_at) = match state.pending {
            Some(Pending::Pose { palette, weights, .. }) => (palette as usize, weights as usize),
            _ => {
                let at = (self.palettes.len(), self.weights.len());
                self.palettes.resize(at.0 + palette.len(), [[0.0; 4]; 3]);
                self.weights.resize(at.1 + targets, 0.0);
                if state.pending.is_none() {
                    self.queued.push(slot);
                }
                at
            }
        };
        for (rows, matrix) in self.palettes[palette_at..][..palette.len()].iter_mut().zip(palette) {
            *rows = palette_rows(matrix);
        }
        let dense = &mut self.weights[weights_at..][..targets];
        dense[..weights.len()].copy_from_slice(weights);
        dense[weights.len()..].fill(0.0);
        state.pending =
            Some(Pending::Pose { palette: palette_at as u32, weights: weights_at as u32, morphs });
        true
    }

    /// Takes the pending skins into this frame's jobs and settles which half each instance
    /// draws from. Call once per recorded frame, before [`Self::frame`] and [`Self::outputs`].
    pub(crate) fn begin_record(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, meshes: &MeshPool) {
        profiling::scope!("Skinning::begin_record");
        self.recorded = true;
        self.jobs.clear();
        self.workgroups = 0;
        for &slot in &self.live {
            if let Some(state) = self.instances[slot as usize].as_mut() {
                state.moved = false;
            }
        }
        let records = meshes.records();
        for &slot in &self.queued {
            let Some(state) = self.instances.get_mut(slot as usize).and_then(Option::as_mut) else {
                continue;
            };
            let Some(pending) = state.pending.take() else { continue };
            let record = &records[state.mesh.index()];
            let source = record.deform().expect("deformed instances have deformable meshes");
            let vertex_count = record.lod_vertices[DRAWN_LOD];
            // Write the half the last drawn pose isn't in.
            state.current ^= 1;
            state.moved = state.drawn;
            state.drawn = true;
            let outputs = state.outputs();
            self.jobs.push(job(source, pending, vertex_count, self.workgroups, outputs));
            self.workgroups += vertex_count.div_ceil(WORKGROUP_SIZE);
        }
        self.queued.clear();
        if self.jobs.is_empty() {
            return;
        }
        // The palettes follow the header and jobs, and the weights the palettes, so the jobs'
        // offsets become absolute in the frame buffer.
        let palette_row = 1 + JOB_ROWS * self.jobs.len() as u32;
        let weight_word = 4 * (palette_row + 3 * self.palettes.len() as u32);
        for job in &mut self.jobs {
            job.palette += palette_row;
            job.weights += weight_word;
        }
        let bytes = u64::from(weight_word) * 4 + size_of_val(self.weights.as_slice()) as u64;
        // Rewritten whole every frame, so nothing needs keeping.
        self.stale |= self.frame.reserve(device, queue, bytes.next_multiple_of(16), 0);
        if std::mem::take(&mut self.stale) || self.bind_group.is_none() {
            let pass = self.pass.as_ref().expect("enabled with the first deformable mesh");
            self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("skinning"),
                layout: &pass.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: meshes.deformable_buffer().as_entire_binding(),
                    },
                    wgpu::BindGroupEntry { binding: 1, resource: self.frame.buffer().as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.outputs.buffer().as_entire_binding() },
                ],
            }));
        }
    }

    /// This recorded frame's skinning work, if it has any.
    pub(crate) fn frame(&self) -> Option<SkinningFrame<'_>> {
        if self.jobs.is_empty() {
            return None;
        }
        let pass = self.pass.as_ref().expect("enabled with the first deformable mesh");
        Some(SkinningFrame {
            pipeline: &pass.pipeline,
            bind_group: self.bind_group.as_ref().expect("built by begin_record"),
            buffer: self.frame.buffer(),
            header: [self.jobs.len() as u32, self.workgroups, 0, 0],
            jobs: &self.jobs,
            palettes: &self.palettes,
            weights: &self.weights,
            workgroups: self.workgroups,
        })
    }

    /// Where each deformed instance's streams are this frame.
    pub(crate) fn outputs(&self) -> SkinnedOutputs<'_> {
        SkinnedOutputs { buffer: self.outputs.buffer(), instances: &self.instances }
    }

    pub(crate) fn stats(&self) -> SkinningStats {
        self.stats
    }

    /// Times either buffer grew.
    pub(crate) fn growths(&self) -> u32 {
        self.outputs.growths() + self.frame.growths()
    }
}

/// A recorded frame's skinning work: its uploads and its dispatch.
#[derive(Clone, Copy)]
pub(crate) struct SkinningFrame<'a> {
    pipeline: &'a wgpu::ComputePipeline,
    bind_group: &'a wgpu::BindGroup,
    buffer: &'a wgpu::Buffer,
    header: [u32; 4],
    jobs: &'a [Job],
    palettes: &'a [PaletteRows],
    weights: &'a [f32],
    workgroups: u32,
}

impl SkinningFrame<'_> {
    /// Writes the header, jobs, palettes and weights through `belt`, in the frame's uploads.
    pub(crate) fn upload(&self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        let header = bytemuck::bytes_of(&self.header);
        buffer::stage(belt, encoder, self.buffer, 0, header);
        buffer::stage(belt, encoder, self.buffer, header.len() as u64, bytemuck::cast_slice(self.jobs));
        let palettes = (1 + u64::from(JOB_ROWS) * self.jobs.len() as u64) * 16;
        buffer::stage(belt, encoder, self.buffer, palettes, bytemuck::cast_slice(self.palettes));
        let weights = palettes + size_of_val(self.palettes) as u64;
        buffer::stage(belt, encoder, self.buffer, weights, bytemuck::cast_slice(self.weights));
    }

    /// Skins every job's instance. `timestamp_writes` times the pass for the frame graph's GPU
    /// timing.
    pub(crate) fn record(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        labels: bool,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: labels.then_some("skinning"),
            timestamp_writes,
        });
        pass.set_pipeline(self.pipeline);
        pass.set_bind_group(0, self.bind_group, &[]);
        let (x, y) = dispatch_size(self.workgroups);
        pass.dispatch_workgroups(x, y, 1);
    }
}

/// A grid of `workgroups` workgroups within the 65 535 per dimension every device allows, as
/// the shader linearizes it (row by row). The last row may run past; those workgroups return.
fn dispatch_size(workgroups: u32) -> (u32, u32) {
    const MAX_X: u32 = 65_535;
    let x = workgroups.clamp(1, MAX_X);
    (x, workgroups.div_ceil(x))
}

/// Where each deformed instance's streams are this frame, for its draws.
#[derive(Clone, Copy)]
pub(crate) struct SkinnedOutputs<'a> {
    buffer: &'a wgpu::Buffer,
    instances: &'a [Option<Deformed>],
}

impl SkinnedOutputs<'_> {
    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        self.buffer
    }

    /// The instance in `slot`'s streams, if it is deformed.
    pub(crate) fn get(&self, slot: u32) -> Option<InstanceOutputs> {
        self.instances.get(slot as usize)?.as_ref().map(Deformed::outputs)
    }
}

/// The job that skins an instance of `source` into `outputs`, starting at workgroup
/// `first_workgroup`, with palette and weights still relative to their arrays.
fn job(
    source: &DeformSource,
    pending: Pending,
    vertex_count: u32,
    first_workgroup: u32,
    outputs: InstanceOutputs,
) -> Job {
    let mut flags = match source.format {
        PositionFormat::Unorm16 => 0,
        PositionFormat::Float32 => flags::FLOAT_POSITIONS,
    };
    let (mut palette, mut weights) = (0, 0);
    if let Pending::Pose { palette: first_matrix, weights: first_weight, morphs } = pending {
        if let Some(skin) = source.skin {
            flags |= flags::SKIN;
            if skin.wide {
                flags |= flags::WIDE_INFLUENCES;
            }
        }
        if morphs {
            flags |= flags::MORPHS;
        }
        (palette, weights) = (3 * first_matrix, first_weight);
    }
    let word = |bytes: u64| u32::try_from(bytes / 4).expect("outputs fit a storage binding");
    let [x, y, z] = source.position_offset;
    let [sx, sy, sz] = source.position_scale;
    Job {
        first_workgroup,
        vertex_count,
        flags,
        palette,
        positions: source.positions,
        shading: source.shading,
        influences: source.skin.map_or(0, |skin| skin.influences),
        morph_offsets: source.morphs.map_or(0, |morphs| morphs.offsets),
        morph_deltas: source.morphs.map_or(0, |morphs| morphs.deltas),
        weights,
        out_positions: word(outputs.positions),
        out_shading: word(outputs.shading),
        position_offset: [x, y, z, 0.0],
        position_scale: [sx, sy, sz, 0.0],
    }
}

/// `matrix` as the shader reads it: its three rows.
fn palette_rows(matrix: &Affine3A) -> PaletteRows {
    let (axes, t) = (matrix.matrix3, matrix.translation);
    std::array::from_fn(|row| [axes.x_axis[row], axes.y_axis[row], axes.z_axis[row], t[row]])
}

impl SkinningPipeline {
    fn new(device: &wgpu::Device, cache: Option<&wgpu::PipelineCache>, sources: &ShaderSources) -> Self {
        let storage = |binding, read_only, min_size: u64| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(min_size),
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("skinning"),
            entries: &[storage(0, true, 4), storage(1, true, 16), storage(2, false, 4)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skinning"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = Self::pipeline(device, &pipeline_layout, cache, sources);
        Self { layout, pipeline_layout, pipeline }
    }

    /// The pass's pipeline on `layout`, from the skinning program in `sources`.
    fn pipeline(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        cache: Option<&wgpu::PipelineCache>,
        sources: &ShaderSources,
    ) -> wgpu::ComputePipeline {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("skinning"),
            layout: Some(layout),
            module: &shaders::module(device, Program::Skinning, sources),
            entry_point: Some("cs_skinning"),
            compilation_options: Default::default(),
            cache,
        })
    }
}

/// The CPU reference: what `shaders/skinning.wgsl` computes, step for step, for the tests.
#[cfg(test)]
pub(crate) mod reference {
    use glam::{Affine3A, Quat, Vec3};
    use sapphire_assets::mesh::{Mesh, Vertex, VertexShading, oct_decode, oct_encode};

    /// `mesh`'s vertices morphed by `weights` (dense, per target) and skinned by `palette` (empty
    /// for a mesh without a skin), as the skinning pass writes them: positions, and both normals
    /// after their snorm16 round trip, the outline normal through the tangent frame of the normal
    /// as written.
    pub(crate) fn deform(mesh: &Mesh, palette: &[Affine3A], weights: &[f32]) -> Vec<Vertex> {
        let mut vertices: Vec<Vertex> =
            (0..mesh.streams.len()).map(|v| mesh.streams.vertex(v).expect("in range")).collect();
        let mut normals: Vec<Vec3> = vertices.iter().map(|v| Vec3::from(v.normal)).collect();
        // Any non-zero weight morphs the whole instance, which re-encodes every normal.
        let morphs = weights.iter().any(|&w| w != 0.0);
        if morphs {
            for (target, morph) in mesh.morph_targets.iter().enumerate() {
                let first = morph.first_delta as usize;
                let weight = weights[target];
                if weight == 0.0 {
                    continue;
                }
                for delta in &mesh.morph_deltas[first..first + morph.delta_count as usize] {
                    let v = delta.vertex() as usize;
                    let p = Vec3::from(vertices[v].position) + weight * Vec3::from(delta.position());
                    vertices[v].position = p.to_array();
                    normals[v] += weight * Vec3::from(delta.normal());
                }
            }
        }
        let skinned = mesh.skin.is_some() && !palette.is_empty();
        for (v, vertex) in vertices.iter_mut().enumerate() {
            let mut normal = Vec3::from(vertex.normal);
            let mut outline = Vec3::from(vertex.outline_normal);
            if morphs {
                // The outline normal turns as the normal does.
                let morphed = normals[v].normalize_or(Vec3::Z);
                outline = Quat::from_rotation_arc(normal, morphed) * outline;
                normal = round_trip(morphed);
            }
            if let Some(skin) = mesh.skin.as_ref().filter(|_| skinned) {
                let influence = skin.influences.get(v).expect("one per vertex");
                let mut blended = Affine3A::ZERO;
                for (bind, weight) in influence.binds.iter().zip(influence.weights) {
                    let weight = f32::from(weight) / 255.0;
                    let m = palette[*bind as usize];
                    blended.matrix3 += m.matrix3 * weight;
                    blended.translation += m.translation * weight;
                }
                vertex.position = blended.transform_point3(Vec3::from(vertex.position)).to_array();
                normal = round_trip(blended.matrix3 * normal);
                outline = blended.matrix3 * outline;
            }
            if morphs || skinned {
                outline =
                    Vec3::from(VertexShading::new(normal.to_array(), outline.to_array()).outline_normal());
            }
            vertex.normal = normal.to_array();
            vertex.outline_normal = outline.to_array();
        }
        vertices
    }

    /// `n` normalized and through the shading stream's octahedral snorm16 encoding.
    fn round_trip(n: Vec3) -> Vec3 {
        Vec3::from(oct_decode(oct_encode(n.normalize_or(Vec3::Z).to_array())))
    }
}

#[cfg(test)]
mod gpu_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_are_five_rows_and_palettes_their_rows() {
        assert_eq!((size_of::<Job>(), JOB_ROWS), (80, 5));
        let matrix =
            Affine3A::from_cols_array(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0]);
        assert_eq!(
            palette_rows(&matrix),
            [[1.0, 4.0, 7.0, 10.0], [2.0, 5.0, 8.0, 11.0], [3.0, 6.0, 9.0, 12.0]]
        );
    }

    #[test]
    fn dispatches_cover_every_workgroup_within_the_limit() {
        assert_eq!(dispatch_size(1), (1, 1));
        assert_eq!(dispatch_size(65_535), (65_535, 1));
        assert_eq!(dispatch_size(65_536), (65_535, 2));
        let (x, y) = dispatch_size(300_000);
        assert!(x * y >= 300_000 && x * (y - 1) < 300_000);
    }

    #[test]
    fn instances_alternate_halves_and_draw_still_when_not_skinned() {
        let mut state = Deformed {
            mesh: MeshHandle::from_raw(0),
            first: 10,
            vertex_count: 100,
            current: 0,
            drawn: false,
            moved: false,
            pending: None,
        };
        let base = 10 * OUTPUT_VERTEX_BYTES;
        let skin = |state: &mut Deformed| {
            state.current ^= 1;
            state.moved = state.drawn;
            state.drawn = true;
        };
        skin(&mut state);
        let first = state.outputs();
        assert_eq!(
            first,
            InstanceOutputs { positions: base + 1200, previous: base + 1200, shading: base + 2400 }
        );
        skin(&mut state);
        assert_eq!(
            state.outputs(),
            InstanceOutputs { positions: base, previous: base + 1200, shading: base + 2400 }
        );
        state.moved = false;
        assert_eq!(state.outputs().previous, base, "a frame that doesn't skin it draws it still");
    }

    #[test]
    fn jobs_flag_what_their_pose_needs() {
        let source = DeformSource {
            vertex_count: 100,
            format: PositionFormat::Unorm16,
            positions: 4,
            shading: 204,
            surface: 404,
            skin: Some(crate::mesh_pool::SkinSource { influences: 604, wide: true, palette_len: 3 }),
            morphs: Some(crate::mesh_pool::MorphSource { offsets: 904, deltas: 1005, targets: 2 }),
            ..DeformSource::default()
        };
        let outputs = InstanceOutputs { positions: 64, previous: 64, shading: 2464 };
        let rest = job(&source, Pending::Rest, 60, 7, outputs);
        assert_eq!((rest.flags, rest.first_workgroup, rest.vertex_count), (0, 7, 60), "the bind pose");
        assert_eq!((rest.out_positions, rest.out_shading), (16, 616));
        let posed = job(&source, Pending::Pose { palette: 2, weights: 6, morphs: true }, 60, 7, outputs);
        assert_eq!(posed.flags, flags::SKIN | flags::WIDE_INFLUENCES | flags::MORPHS);
        assert_eq!((posed.palette, posed.weights, posed.influences, posed.morph_deltas), (6, 6, 604, 1005));
        let still_face =
            job(&source, Pending::Pose { palette: 0, weights: 0, morphs: false }, 60, 0, outputs);
        assert_eq!(still_face.flags & flags::MORPHS, 0, "all-zero weights skip the morph loop");
    }
}
