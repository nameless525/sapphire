//! The mesh pool (spec §5.12): every mesh's vertex streams live in shared stream buffers and
//! its indices in one shared index buffer. Its material slots name uploaded materials
//! (`materials`), which live in their family's parameter buffer.
//!
//! Uploads copy the cooked streams byte for byte: the renderer reads the quantized encodings
//! of `sapphire_assets::mesh` directly, and its vertex shaders decode them with the per-mesh
//! dequantization each draw record carries. The streams are kept apart (positions, shading,
//! surface), so a pass binds only the ones it reads.
//!
//! Static meshes are grouped into one slab per position format, each a set of stream buffers
//! with its own vertex numbering. Within a slab every stream has one element per vertex, which
//! is what the GPU-driven path (later) needs: one `base_vertex` addresses all of a draw's
//! streams.
//!
//! Indices stay mesh-relative, exactly as cooked. A draw locates its mesh's vertices by
//! binding the stream buffers at the mesh's first vertex (the CPU-driven path, which needs no
//! base-vertex support and so runs on the iOS Simulator) or, on the future GPU-driven path,
//! through `base_vertex`. Relative indices also let compute skinning write each instance's
//! vertices to buffers of its own, and the pool compact, without rewriting indices.
//!
//! Every LOD's indices are uploaded with the mesh and its index ranges recorded; draws use
//! LOD0 until LOD selection lands with culling.
//!
//! Foliage meshes' foliage stream (M1 W6, [`VertexFoliage`]) goes to a buffer of its own, with
//! its own numbering: other meshes spend nothing on it, at the cost of a second base vertex on
//! the GPU-driven path (the `foliage` module's later milestones). A draw binds it at the mesh's
//! first element, as it binds the slab's streams.
//!
//! Each buffer hands out ranges first fit ([`RangeAllocator`]), and what's given back goes to later
//! uploads: a mesh freed ([`MeshPool::free`], M3 W3) gives its ranges and its record, and a mesh
//! replaced in place (hot reload, live link: [`MeshPool::replace`]) its old ranges, under the same
//! handle. Either way the ranges wait until no frame can be reading them (the `retire` module). The
//! buffers themselves never shrink: what's freed is room to reuse, and [`MeshPool::capacity_bytes`]
//! says what they hold on to.
//!
//! # Skinned and morphed meshes (M1 R2)
//!
//! A mesh with a skin or morph targets is never drawn from its own positions and shading:
//! every instance draws from what the skinning pass wrote for it (`crate::skinning`). Its
//! streams therefore go to one *deformable* buffer instead of a slab, read as storage by the
//! skinning pass and, for the surface stream every instance shares, as vertex data by the draws.
//! A mesh's region holds, in 4-byte words:
//!
//! | Data | Words per element | Layout |
//! |---|---|---|
//! | positions | 2 or 3 per vertex | as cooked: unorm16 × 4 or f32 × 3 |
//! | shading | 2 per vertex | as cooked ([`VertexShading`]) |
//! | surface | 2 per vertex | as cooked ([`VertexSurface`]), drawn as slot 3 |
//! | influences | 2 or 3 per vertex, if skinned | as cooked ([`Influence8`](sapphire_assets::mesh::Influence8) or `Influence16`) |
//! | morph offsets | vertex count + 1, if morphed | vertex `v`'s deltas are `offsets[v]..offsets[v + 1]` |
//! | morph deltas | 4 per delta, if morphed | [`MorphDelta`]'s layout with the target index in place of the vertex |
//!
//! Morph deltas are cooked target by target (sparse per target, `sapphire_assets::mesh`). The
//! upload regroups them by vertex ([`morphs_by_vertex`]), targets in order within a vertex, so
//! the skinning pass applies them in the same dispatch that skins: each vertex walks its own
//! deltas, deterministically, with no atomics and no pass per target. A vertex no target moves
//! (all of a body, usually) costs one read of its two offsets.

use sapphire_assets::mesh::{
    ArchivedInfluences, ArchivedMesh, ArchivedSlotMaterial, MorphDelta, PositionFormat, VertexFoliage,
    VertexShading, VertexSurface,
};
use sapphire_rhi::wgpu;

use crate::RenderError;
use crate::bounds::{PaletteReach, Sphere, palette_reach};
use crate::buffer::{GrowableBuffer, RangeAllocator};
use crate::gpu_types::Dequantize;
use crate::materials::MaterialHandle;
use crate::pipelines::format_index;
use crate::retire::{Retiring, new_generation};

const SHADING_BYTES: u64 = size_of::<VertexShading>() as u64;
const SURFACE_BYTES: u64 = size_of::<VertexSurface>() as u64;
const FOLIAGE_BYTES: u64 = size_of::<VertexFoliage>() as u64;
const INDEX_BYTES: u64 = size_of::<u32>() as u64;

/// Vulkan devices without `fullDrawIndexUint32` (some Vulkan 1.1 phones) only guarantee
/// index values up to 2^24 − 1, which caps a single mesh (not the pool).
const MAX_MESH_VERTICES: usize = 1 << 24;

/// Initial room in the quantized slab, in vertices: 768 KiB over its three streams. f32 meshes
/// are the exception, so their slab starts empty and grows on first use.
const INITIAL_QUANTIZED_VERTICES: u64 = 1 << 15;

/// A mesh uploaded with [`crate::Renderer::upload_mesh`], until it's freed
/// ([`crate::Renderer::free_mesh`]). Only meaningful to the renderer that returned it: an instance
/// of a handle that renderer never returned, or has freed, draws nothing (the `retire` module).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshHandle {
    index: u32,
    generation: u64,
}

impl MeshHandle {
    /// What an instance draws once its handle turned out not to be current: no mesh at all.
    pub(crate) const NONE: Self = Self { index: u32::MAX, generation: 0 };

    /// A handle from its raw index, for code that builds packets without a renderer, such as
    /// extraction tests. No renderer ever calls it current, so it draws nothing on one.
    pub const fn from_raw(index: u32) -> Self {
        Self { index, generation: 0 }
    }

    pub(crate) fn index(self) -> usize {
        self.index as usize
    }
}

/// A submesh with its indices resolved to pool-global positions and its material slot to the
/// material uploaded for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SubmeshDraw {
    pub first_index: u32,
    pub index_count: u32,
    pub material: SubmeshMaterial,
}

/// The material a submesh draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SubmeshMaterial {
    /// Selects its group 3.
    pub handle: MaterialHandle,
    /// Its slot in the family's parameter buffer, which the draw record carries.
    pub slot: u32,
}

/// Where a mesh lives in the pool.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MeshRecord {
    pub streams: MeshStreams,
    /// How its draws decode the positions and UVs they read: the mesh's own for a static mesh;
    /// for a deformable one, identity positions (skinning writes f32 object space) and the
    /// mesh's UVs.
    pub dequantize: Dequantize,
    /// Per LOD, LOD0 first: its submeshes.
    pub lods: Vec<Vec<SubmeshDraw>>,
    /// Per LOD: the vertices it reads, a prefix of the streams.
    pub lod_vertices: Vec<u32>,
    /// The pool's ranges it occupies.
    pub placement: Placement,
    /// Per material slot: the material it draws with, and whether the mesh embeds it.
    pub slots: Vec<SlotRecord>,
    /// The sphere around its cooked bounds, in object space: per-object light lists and shadow
    /// casters' culling place it by each instance's transform. A skinned or morphed mesh's is
    /// its bind pose's; [`Self::palette_reach`] finds a pose's.
    pub bounds: Sphere,
    /// What each palette entry's vertices fill in the bind pose (the `bounds` module): empty without
    /// a skin.
    pub palette_reach: Vec<PaletteReach>,
    /// Maximum bind-space displacement per morph target, for conservative posed bounds.
    pub morph_reach: Vec<f32>,
    /// A static mesh's foliage stream (M1 W6): its first element in the pool's foliage buffer,
    /// if it has one.
    pub foliage: Option<u32>,
}

/// A mesh's material slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlotRecord {
    pub material: SubmeshMaterial,
    /// The mesh embeds the material (`SlotMaterial::Embedded`), so the material is the mesh's
    /// own; otherwise it is a material asset's.
    pub embedded: bool,
}

/// The ranges of the pool's buffers a mesh occupies, as `(start, length)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Placement {
    /// Vertices of its slab, or words of the deformable buffer.
    pub vertices: (u32, u32),
    /// Indices of the index buffer.
    pub indices: (u32, u32),
    /// Elements of the foliage buffer.
    pub foliage: (u32, u32),
}

/// Where a mesh's vertex streams are.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MeshStreams {
    /// A static mesh, in the slab of `format`: vertex 0 is the slab's `first_vertex`.
    Slab { format: PositionFormat, first_vertex: u32 },
    /// A skinned or morphed mesh, in the deformable buffer (see the module docs).
    Deformable(DeformSource),
}

/// A freed mesh's ranges, on their way back to the free lists.
#[derive(Clone, Copy, Debug)]
struct RetiredRanges {
    /// Its slab's format; `None` for the deformable buffer.
    slab: Option<PositionFormat>,
    placement: Placement,
}

impl RetiredRanges {
    fn of(record: &MeshRecord) -> Self {
        let slab = match record.streams {
            MeshStreams::Slab { format, .. } => Some(format),
            MeshStreams::Deformable(_) => None,
        };
        Self { slab, placement: record.placement }
    }
}

impl MeshRecord {
    /// What's left at a freed mesh's index: nothing to draw, for a handle nobody should be holding.
    fn gone() -> Self {
        Self {
            streams: MeshStreams::Slab { format: PositionFormat::Unorm16, first_vertex: 0 },
            dequantize: Dequantize {
                position_offset: [0.0; 3],
                position_scale: [1.0; 3],
                uv_transform: [0.0; 4],
            },
            lods: vec![Vec::new()],
            lod_vertices: vec![0],
            placement: Placement::default(),
            slots: Vec::new(),
            bounds: Sphere::around([[0.0; 3], [0.0; 3]]),
            palette_reach: Vec::new(),
            morph_reach: Vec::new(),
            foliage: None,
        }
    }

    /// The position format its draws read: the slab's, or f32 skinning outputs.
    pub(crate) fn draw_format(&self) -> PositionFormat {
        match self.streams {
            MeshStreams::Slab { format, .. } => format,
            MeshStreams::Deformable(_) => PositionFormat::Float32,
        }
    }

    pub(crate) fn deform(&self) -> Option<&DeformSource> {
        match &self.streams {
            MeshStreams::Slab { .. } => None,
            MeshStreams::Deformable(source) => Some(source),
        }
    }
}

/// A skinned or morphed mesh's region of the deformable buffer, as word offsets into the
/// buffer (see the module docs for the layout).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DeformSource {
    pub vertex_count: u32,
    /// How its positions are stored, and how to decode them: `offset + scale × p`.
    pub format: PositionFormat,
    pub position_offset: [f32; 3],
    pub position_scale: [f32; 3],
    pub positions: u32,
    pub shading: u32,
    pub surface: u32,
    pub skin: Option<SkinSource>,
    pub morphs: Option<MorphSource>,
}

impl DeformSource {
    /// Entries in its skinning palette: 0 without a skin.
    pub(crate) fn palette_len(&self) -> u32 {
        self.skin.map_or(0, |skin| skin.palette_len)
    }

    /// Its morph targets: 0 without any.
    pub(crate) fn morph_targets(&self) -> u32 {
        self.morphs.map_or(0, |morphs| morphs.targets)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SkinSource {
    pub influences: u32,
    /// u16 palette indices (3 words a vertex) rather than u8 (2 words).
    pub wide: bool,
    pub palette_len: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MorphSource {
    pub offsets: u32,
    pub deltas: u32,
    pub targets: u32,
}

/// The stream buffers of one position format.
#[derive(Debug)]
pub(crate) struct Slab {
    format: PositionFormat,
    positions: GrowableBuffer,
    shading: GrowableBuffer,
    surface: GrowableBuffer,
    /// Vertex ranges in use, shared by the three streams.
    vertices: RangeAllocator,
    /// Vertices in use.
    used: u32,
}

impl Slab {
    fn new(device: &wgpu::Device, format: PositionFormat, vertices: u64) -> Self {
        let max = device.limits().max_buffer_size;
        let stream = |label, stride: u64| {
            GrowableBuffer::new(device, label, wgpu::BufferUsages::VERTEX, vertices * stride, max)
        };
        let (positions, shading, surface) = match format {
            PositionFormat::Unorm16 => ("mesh positions (unorm16)", "mesh shading", "mesh surface"),
            PositionFormat::Float32 => {
                ("mesh positions (f32)", "mesh shading (f32 slab)", "mesh surface (f32 slab)")
            }
        };
        Self {
            format,
            positions: stream(positions, format.stride() as u64),
            shading: stream(shading, SHADING_BYTES),
            surface: stream(surface, SURFACE_BYTES),
            vertices: RangeAllocator::default(),
            used: 0,
        }
    }

    pub(crate) fn positions(&self) -> &wgpu::Buffer {
        self.positions.buffer()
    }

    pub(crate) fn shading(&self) -> &wgpu::Buffer {
        self.shading.buffer()
    }

    pub(crate) fn surface(&self) -> &wgpu::Buffer {
        self.surface.buffer()
    }

    /// Bytes per vertex of the position stream.
    pub(crate) fn position_stride(&self) -> u64 {
        self.format.stride() as u64
    }

    fn bytes_used(&self) -> u64 {
        u64::from(self.used) * (self.position_stride() + SHADING_BYTES + SURFACE_BYTES)
    }

    fn capacity_bytes(&self) -> u64 {
        self.positions.buffer().size() + self.shading.buffer().size() + self.surface.buffer().size()
    }

    fn growths(&self) -> u32 {
        self.positions.growths() + self.shading.growths() + self.surface.growths()
    }
}

/// Buffers an upload replaced, so bind groups that reference them must be rebuilt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PoolMoves {
    pub deformable: bool,
}

#[derive(Debug)]
pub(crate) struct MeshPool {
    /// Per [`PositionFormat::ALL`] entry.
    slabs: [Slab; 2],
    /// Skinned and morphed meshes' streams (see the module docs).
    deformable: GrowableBuffer,
    /// Word ranges of the deformable buffer in use, and the words they hold.
    deformable_words: RangeAllocator,
    deformable_used: u32,
    indices: GrowableBuffer,
    /// Index ranges in use, and the indices they hold.
    index_ranges: RangeAllocator,
    indices_used: u32,
    /// Static meshes' foliage streams (see the module docs).
    foliage: GrowableBuffer,
    /// Element ranges of the foliage buffer in use, and the elements they hold.
    foliage_ranges: RangeAllocator,
    foliage_used: u32,
    /// Per handle: freed ones hold [`MeshRecord::gone`].
    records: Vec<MeshRecord>,
    /// Each record's current generation: a handle of any other names nothing.
    generations: Vec<u64>,
    /// Where a new mesh's record starts counting from: this pool's own (the `retire` module).
    epoch: u64,
    /// Indices of [`Self::records`] that are free.
    free_records: Vec<u32>,
    /// Meshes asked to be freed that an instance may still draw.
    dying: Vec<MeshHandle>,
    /// Ranges of freed and replaced meshes, until no frame can be reading them.
    retired: Retiring<RetiredRanges>,
    /// Meshes given back since the pool was created.
    freed: u64,
}

impl MeshPool {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let limits = device.limits();
        let max_storage = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size);
        Self {
            slabs: PositionFormat::ALL.map(|format| {
                let vertices = if format == PositionFormat::Unorm16 { INITIAL_QUANTIZED_VERTICES } else { 0 };
                Slab::new(device, format, vertices)
            }),
            // Bound whole as storage by the skinning pass, so capped at a storage binding. Empty
            // until the first skinned or morphed mesh.
            deformable: GrowableBuffer::new(
                device,
                "deformable mesh sources",
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
                0,
                max_storage,
            ),
            deformable_words: RangeAllocator::default(),
            deformable_used: 0,
            indices: GrowableBuffer::new(
                device,
                "mesh pool indices",
                wgpu::BufferUsages::INDEX,
                1 << 19,
                limits.max_buffer_size,
            ),
            index_ranges: RangeAllocator::default(),
            indices_used: 0,
            // Empty until the first foliage mesh.
            foliage: GrowableBuffer::new(
                device,
                "mesh foliage",
                wgpu::BufferUsages::VERTEX,
                0,
                limits.max_buffer_size,
            ),
            foliage_ranges: RangeAllocator::default(),
            foliage_used: 0,
            records: Vec::new(),
            generations: Vec::new(),
            epoch: new_generation(),
            free_records: Vec::new(),
            dying: Vec::new(),
            retired: Retiring::default(),
            freed: 0,
        }
    }

    /// Adds a mesh whose material slots draw with `materials`, one per slot: a static mesh's
    /// streams to its format's slab, a skinned or morphed one's to the deformable buffer. Returns
    /// the handle and the buffers it replaced.
    pub(crate) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &ArchivedMesh,
        materials: &[SubmeshMaterial],
    ) -> Result<(MeshHandle, PoolMoves), RenderError> {
        if self.free_records.is_empty() && u32::try_from(self.records.len()).is_err() {
            return Err(RenderError::MeshPoolFull("meshes"));
        }
        let (record, moves) = self.place(device, queue, mesh, materials)?;
        let index = self.free_records.pop().unwrap_or_else(|| {
            self.records.push(MeshRecord::gone());
            self.generations.push(self.epoch);
            (self.records.len() - 1) as u32
        });
        self.records[index as usize] = record;
        Ok((MeshHandle { index, generation: self.generations[index as usize] }, moves))
    }

    /// Whether `handle` names a mesh of this pool: one uploaded and not yet retired.
    pub(crate) fn is_current(&self, handle: MeshHandle) -> bool {
        self.generations.get(handle.index()).is_some_and(|&generation| generation == handle.generation)
    }

    /// Both handles were checked current and distinct by the renderer's adoption preflight.
    pub(crate) fn swap_records(&mut self, existing: MeshHandle, prepared: MeshHandle) {
        self.records.swap(existing.index(), prepared.index());
    }

    /// Asks for mesh `handle` to be given back, once nothing draws it. Returns whether that's news: not
    /// for a handle already freed, or one of another renderer's.
    pub(crate) fn free(&mut self, handle: MeshHandle) -> bool {
        if !self.is_current(handle) || self.dying.contains(&handle) {
            return false;
        }
        self.dying.push(handle);
        true
    }

    /// Retires each dying mesh no instance in `live` draws: its handle stops being current, its record
    /// and index go to later uploads, and `gone` hears of the material slots it drew with. Its ranges
    /// wait for frame `last_read`, the last that may read them. Call it after a packet's deltas and
    /// before anything is drawn from the frame.
    pub(crate) fn retire_unused(
        &mut self,
        live: impl Iterator<Item = MeshHandle>,
        last_read: u64,
        mut gone: impl FnMut(&[SlotRecord]),
    ) {
        if self.dying.is_empty() {
            return;
        }
        self.dying.sort_unstable_by_key(|handle| handle.index);
        let mut drawn = vec![false; self.dying.len()];
        for mesh in live {
            if let Ok(at) = self.dying.binary_search_by_key(&mesh.index, |handle| handle.index)
                && self.dying[at] == mesh
            {
                drawn[at] = true;
            }
        }
        let mut drawn = drawn.into_iter();
        let mut dying = std::mem::take(&mut self.dying);
        dying.retain(|&handle| {
            if drawn.next().expect("one per dying mesh") {
                return true;
            }
            let record = std::mem::replace(&mut self.records[handle.index()], MeshRecord::gone());
            self.generations[handle.index()] = new_generation();
            self.free_records.push(handle.index);
            self.retired.push(last_read, RetiredRanges::of(&record));
            self.freed += 1;
            gone(&record.slots);
            false
        });
        self.dying = dying;
    }

    /// Hands back the ranges of what no frame up to `finished` was the last to read. Returns whether
    /// any went.
    pub(crate) fn release_finished(&mut self, finished: u64) -> bool {
        let mut released = false;
        while let Some(ranges) = self.retired.pop_finished(finished) {
            self.release(ranges);
            released = true;
        }
        released
    }

    /// Dying meshes and retired ranges still to go.
    pub(crate) fn retiring(&self) -> usize {
        self.dying.len() + self.retired.len()
    }

    /// Meshes with a current handle.
    pub(crate) fn live(&self) -> usize {
        self.records.len() - self.free_records.len()
    }

    /// Meshes given back since the pool was created.
    pub(crate) fn freed(&self) -> u64 {
        self.freed
    }

    /// Replaces mesh `handle` with `mesh`, whose material slots draw with `materials`, under the
    /// same handle: the new streams and indices are written, then the old ranges retired, as frame
    /// `last_read`, the one being prepared or recorded, may still read them. Nothing changes if it
    /// fails. Returns the buffers it replaced and the old record.
    pub(crate) fn replace(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        handle: MeshHandle,
        mesh: &ArchivedMesh,
        materials: &[SubmeshMaterial],
        last_read: u64,
    ) -> Result<(PoolMoves, MeshRecord), RenderError> {
        let (record, moves) = self.place(device, queue, mesh, materials)?;
        let old = std::mem::replace(&mut self.records[handle.index()], record);
        self.retired.push(last_read, RetiredRanges::of(&old));
        Ok((moves, old))
    }

    /// Validates `mesh`, finds room for it and writes its streams and indices, returning its
    /// record. Everything that can fail is checked before anything is written.
    fn place(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &ArchivedMesh,
        materials: &[SubmeshMaterial],
    ) -> Result<(MeshRecord, PoolMoves), RenderError> {
        mesh.validate()?;
        if materials.len() != mesh.materials.len() {
            return Err(RenderError::MaterialCount { slots: mesh.materials.len(), handles: materials.len() });
        }
        let streams = &mesh.streams;
        let vertices = streams.len();
        if vertices > MAX_MESH_VERTICES {
            return Err(RenderError::MeshTooLarge { vertices });
        }
        let deformable = mesh.deforms();
        let (position_offset, position_scale) = streams.position_transform();
        let (uv_offset, uv_scale) = streams.uv_transform();
        let uv_transform = [uv_offset[0], uv_offset[1], uv_scale[0], uv_scale[1]];

        // Room first: the ranges are taken back if any buffer can't grow to hold them.
        let index_len = count(mesh.indices.len(), "indices")?;
        let vertex_len = if deformable { deformable_words(mesh)? } else { count(vertices, "vertices")? };
        // Skinned and morphed meshes draw no foliage (uploads refuse foliage materials on them).
        let foliage_bytes = if deformable { &[][..] } else { streams.foliage_bytes() };
        let foliage_len = count(foliage_bytes.len() / FOLIAGE_BYTES as usize, "foliage")?;
        let format = streams.position_format();
        let slab = format_index(format);
        let vertices_end_before =
            if deformable { self.deformable_words.end() } else { self.slabs[slab].vertices.end() };
        let indices_end_before = self.index_ranges.end();
        let foliage_end_before = self.foliage_ranges.end();
        let lens = (vertex_len, index_len, foliage_len);
        let Some((first_vertex, first_index, first_foliage)) = self.allocate(deformable, slab, lens) else {
            return Err(RenderError::MeshPoolFull("vertices or indices"));
        };

        // Admission precedes regrouping: a valid but oversized morph mesh must not allocate
        // its CPU copy and per-vertex scratch before a guaranteed storage-buffer refusal.
        let region = deformable.then(|| deformable_region(mesh));
        let mut moves = PoolMoves::default();
        let (record_streams, dequantize) = match region {
            Some((bytes, mut source)) => {
                let offset = u64::from(first_vertex) * 4;
                let end = u64::from(self.deformable_words.end()) * 4;
                moves.deformable =
                    self.deformable.reserve(device, queue, end, u64::from(vertices_end_before) * 4);
                write(queue, self.deformable.buffer(), offset, &bytes);
                source.offset_by(first_vertex);
                self.deformable_used += vertex_len;
                let identity =
                    Dequantize { position_offset: [0.0; 3], position_scale: [1.0; 3], uv_transform };
                (MeshStreams::Deformable(source), identity)
            }
            None => {
                let slab = &mut self.slabs[format_index(format)];
                let end = u64::from(slab.vertices.end());
                let streams_bytes = [slab.position_stride(), SHADING_BYTES, SURFACE_BYTES];
                let stream_data =
                    [streams.position_bytes(), streams.shading_bytes(), streams.surface_bytes()];
                for ((buffer, stride), bytes) in [&mut slab.positions, &mut slab.shading, &mut slab.surface]
                    .into_iter()
                    .zip(streams_bytes)
                    .zip(stream_data)
                {
                    buffer.reserve(device, queue, end * stride, u64::from(vertices_end_before) * stride);
                    write(queue, buffer.buffer(), u64::from(first_vertex) * stride, bytes);
                }
                slab.used += vertex_len;
                (
                    MeshStreams::Slab { format, first_vertex },
                    Dequantize { position_offset, position_scale, uv_transform },
                )
            }
        };

        let index_end = u64::from(self.index_ranges.end()) * INDEX_BYTES;
        self.indices.reserve(device, queue, index_end, u64::from(indices_end_before) * INDEX_BYTES);
        write(queue, self.indices.buffer(), u64::from(first_index) * INDEX_BYTES, mesh.index_bytes());
        self.indices_used += index_len;
        if foliage_len > 0 {
            let end = u64::from(self.foliage_ranges.end()) * FOLIAGE_BYTES;
            self.foliage.reserve(device, queue, end, u64::from(foliage_end_before) * FOLIAGE_BYTES);
            write(queue, self.foliage.buffer(), u64::from(first_foliage) * FOLIAGE_BYTES, foliage_bytes);
            self.foliage_used += foliage_len;
        }

        let record = MeshRecord {
            streams: record_streams,
            dequantize,
            lods: mesh
                .lods
                .iter()
                .map(|lod| {
                    lod.submeshes
                        .iter()
                        .map(|s| SubmeshDraw {
                            first_index: first_index + s.first_index.to_native(),
                            index_count: s.index_count.to_native(),
                            material: materials[s.material.to_native() as usize],
                        })
                        .collect()
                })
                .collect(),
            lod_vertices: mesh.lods.iter().map(|lod| lod.vertex_count.to_native()).collect(),
            placement: Placement {
                vertices: (first_vertex, vertex_len),
                indices: (first_index, index_len),
                foliage: (first_foliage, foliage_len),
            },
            slots: mesh
                .materials
                .iter()
                .zip(materials)
                .map(|(slot, &material)| SlotRecord {
                    material,
                    embedded: matches!(slot.material, ArchivedSlotMaterial::Embedded(_)),
                })
                .collect(),
            bounds: {
                let bounds = streams.bounds.to_native();
                Sphere::around([bounds.min, bounds.max])
            },
            palette_reach: palette_reach(mesh),
            morph_reach: crate::bounds::morph_reach(mesh),
            foliage: (foliage_len > 0).then_some(first_foliage),
        };
        Ok((record, moves))
    }

    /// Takes `vertex_len` vertices (words of the deformable buffer if `deformable`, else of slab
    /// `slab`), `index_len` indices and `foliage_len` foliage elements, if the buffers can grow
    /// to hold them. Returns the first of each.
    fn allocate(
        &mut self,
        deformable: bool,
        slab: usize,
        (vertex_len, index_len, foliage_len): (u32, u32, u32),
    ) -> Option<(u32, u32, u32)> {
        let first_index = self.index_ranges.try_allocate(index_len)?;
        let Some(first_foliage) = self.foliage_ranges.try_allocate(foliage_len) else {
            self.index_ranges.free(first_index, index_len);
            return None;
        };
        let first_vertex = if deformable {
            self.deformable_words.try_allocate(vertex_len)
        } else {
            self.slabs[slab].vertices.try_allocate(vertex_len)
        };
        if let Some(first_vertex) = first_vertex {
            if self.fits(deformable, slab) {
                return Some((first_vertex, first_index, first_foliage));
            }
            if deformable {
                self.deformable_words.free(first_vertex, vertex_len);
            } else {
                self.slabs[slab].vertices.free(first_vertex, vertex_len);
            }
        }
        self.foliage_ranges.free(first_foliage, foliage_len);
        self.index_ranges.free(first_index, index_len);
        None
    }

    /// Whether the index and foliage buffers, and the deformable buffer or slab `slab`'s
    /// streams, can grow to hold every range handed out.
    fn fits(&self, deformable: bool, slab: usize) -> bool {
        let indices = u64::from(self.index_ranges.end()) * INDEX_BYTES <= self.indices.max_size()
            && u64::from(self.foliage_ranges.end()) * FOLIAGE_BYTES <= self.foliage.max_size();
        let vertices = if deformable {
            u64::from(self.deformable_words.end()) * 4 <= self.deformable.max_size()
        } else {
            let slab = &self.slabs[slab];
            let end = u64::from(slab.vertices.end());
            [
                (&slab.positions, slab.position_stride()),
                (&slab.shading, SHADING_BYTES),
                (&slab.surface, SURFACE_BYTES),
            ]
            .iter()
            .all(|(buffer, stride)| end * stride <= buffer.max_size())
        };
        indices && vertices
    }

    /// Frees `ranges`.
    fn release(&mut self, ranges: RetiredRanges) {
        let Placement {
            vertices: (first_vertex, vertex_len),
            indices: (first_index, index_len),
            foliage: (first_foliage, foliage_len),
        } = ranges.placement;
        match ranges.slab {
            Some(format) => {
                let slab = &mut self.slabs[format_index(format)];
                slab.vertices.free(first_vertex, vertex_len);
                slab.used -= vertex_len;
            }
            None => {
                self.deformable_words.free(first_vertex, vertex_len);
                self.deformable_used -= vertex_len;
            }
        }
        self.index_ranges.free(first_index, index_len);
        self.indices_used -= index_len;
        self.foliage_ranges.free(first_foliage, foliage_len);
        self.foliage_used -= foliage_len;
    }

    pub(crate) fn records(&self) -> &[MeshRecord] {
        &self.records
    }

    pub(crate) fn slab(&self, format: PositionFormat) -> &Slab {
        &self.slabs[format_index(format)]
    }

    /// The skinned and morphed meshes' streams.
    pub(crate) fn deformable_buffer(&self) -> &wgpu::Buffer {
        self.deformable.buffer()
    }

    pub(crate) fn index_buffer(&self) -> &wgpu::Buffer {
        self.indices.buffer()
    }

    /// The end of the indices in use in [`Self::index_buffer`]: every mesh's lie below it, each
    /// at a multiple of 3 (whole triangles, as every mesh's index count is a multiple of 3).
    pub(crate) fn index_end(&self) -> u32 {
        self.index_ranges.end()
    }

    /// Static foliage meshes' foliage streams, drawn as slot 5.
    pub(crate) fn foliage_buffer(&self) -> &wgpu::Buffer {
        self.foliage.buffer()
    }

    /// Bytes in use: vertex streams (with skin and morph data and foliage streams) and indices.
    pub(crate) fn bytes_used(&self) -> u64 {
        self.slabs.iter().map(Slab::bytes_used).sum::<u64>()
            + u64::from(self.deformable_used) * 4
            + u64::from(self.indices_used) * INDEX_BYTES
            + u64::from(self.foliage_used) * FOLIAGE_BYTES
    }

    /// Bytes the pool's buffers hold, in use or not: they never shrink, so this is the most any walk
    /// has needed, with what growing by doubling leaves over.
    pub(crate) fn capacity_bytes(&self) -> u64 {
        self.slabs.iter().map(Slab::capacity_bytes).sum::<u64>()
            + self.deformable.buffer().size()
            + self.indices.buffer().size()
            + self.foliage.buffer().size()
    }

    /// Times any pool buffer grew.
    pub(crate) fn growths(&self) -> u32 {
        self.slabs.iter().map(Slab::growths).sum::<u32>()
            + self.deformable.growths()
            + self.indices.growths()
            + self.foliage.growths()
    }
}

impl DeformSource {
    /// Moves word offsets relative to the region's start to offsets into the buffer, for a
    /// region placed at word `base`.
    fn offset_by(&mut self, base: u32) {
        self.positions += base;
        self.shading += base;
        self.surface += base;
        if let Some(skin) = &mut self.skin {
            skin.influences += base;
        }
        if let Some(morphs) = &mut self.morphs {
            morphs.offsets += base;
            morphs.deltas += base;
        }
    }
}

/// A skinned or morphed mesh's region of the deformable buffer (see the module docs), with its
/// offsets relative to the region's start.
fn deformable_region(mesh: &ArchivedMesh) -> (Vec<u8>, DeformSource) {
    let streams = &mesh.streams;
    let mut bytes = Vec::new();
    let mut push = |data: &[u8]| {
        debug_assert!(data.len().is_multiple_of(4), "every region is whole words");
        let word = u32::try_from(bytes.len() / 4).expect("checked against the buffer's size");
        bytes.extend_from_slice(data);
        word
    };
    let (position_offset, position_scale) = streams.position_transform();
    let positions = push(streams.position_bytes());
    let shading = push(streams.shading_bytes());
    let surface = push(streams.surface_bytes());
    let skin = mesh.skin.as_ref().map(|skin| SkinSource {
        influences: push(skin.influences.as_bytes()),
        wide: matches!(skin.influences, ArchivedInfluences::U16(_)),
        palette_len: skin.binds.len() as u32,
    });
    let morphs = (!mesh.morph_targets.is_empty()).then(|| {
        let targets = mesh.morph_targets.iter().map(|target| {
            let first = target.first_delta.to_native() as usize;
            first..first + target.delta_count.to_native() as usize
        });
        let (offsets, deltas) = morphs_by_vertex(streams.len(), targets, &mesh.morph_deltas);
        MorphSource {
            offsets: push(bytemuck::cast_slice(&offsets)),
            deltas: push(bytemuck::cast_slice(&deltas)),
            targets: mesh.morph_targets.len() as u32,
        }
    });
    let source = DeformSource {
        vertex_count: streams.len() as u32,
        format: streams.position_format(),
        position_offset,
        position_scale,
        positions,
        shading,
        surface,
        skin,
        morphs,
    };
    (bytes, source)
}

/// Exact source-region size, before allocating morph regrouping scratch or copying streams.
fn deformable_words(mesh: &ArchivedMesh) -> Result<u32, RenderError> {
    let streams = &mesh.streams;
    let skin = mesh.skin.as_ref().map_or(0, |skin| skin.influences.as_bytes().len() as u64);
    let morphs = if mesh.morph_targets.is_empty() {
        0
    } else {
        (streams.len() as u64 + 1) * 4 + mesh.morph_deltas.len() as u64 * 16
    };
    let bytes = [
        streams.position_bytes().len() as u64,
        streams.shading_bytes().len() as u64,
        streams.surface_bytes().len() as u64,
        skin,
        morphs,
    ]
    .into_iter()
    .try_fold(0u64, u64::checked_add)
    .ok_or(RenderError::MeshPoolFull("deformable mesh sources"))?;
    u32::try_from(bytes / 4).map_err(|_| RenderError::MeshPoolFull("deformable mesh sources"))
}

/// Regroups morph deltas, cooked target by target (`targets` are their ranges of `deltas`), by
/// vertex: vertex `v`'s deltas are `offsets[v]..offsets[v + 1]` of the result, in target order,
/// each with its target index where the cooked delta holds its vertex. A counting sort, so
/// linear in vertices and deltas.
pub(crate) fn morphs_by_vertex(
    vertex_count: usize,
    targets: impl Iterator<Item = std::ops::Range<usize>> + Clone,
    deltas: &[MorphDelta],
) -> (Vec<u32>, Vec<[u8; 16]>) {
    let mut offsets = vec![0u32; vertex_count + 1];
    for range in targets.clone() {
        for delta in &deltas[range] {
            offsets[delta.vertex() as usize + 1] += 1;
        }
    }
    for vertex in 0..vertex_count {
        offsets[vertex + 1] += offsets[vertex];
    }
    let mut next = offsets.clone();
    let mut regrouped = vec![[0; 16]; offsets[vertex_count] as usize];
    for (target, range) in targets.enumerate() {
        for delta in &deltas[range] {
            let slot = &mut next[delta.vertex() as usize];
            let mut bytes = delta.to_bytes();
            bytes[..4].copy_from_slice(&(target as u32).to_le_bytes());
            regrouped[*slot as usize] = bytes;
            *slot += 1;
        }
    }
    (offsets, regrouped)
}

/// `len` as the pool's u32 counts.
fn count(len: usize, what: &'static str) -> Result<u32, RenderError> {
    u32::try_from(len).map_err(|_| RenderError::MeshPoolFull(what))
}

fn write(queue: &wgpu::Queue, buffer: &wgpu::Buffer, offset: u64, bytes: &[u8]) {
    if !bytes.is_empty() {
        queue.write_buffer(buffer, offset, bytes);
    }
}

#[cfg(test)]
mod gpu_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morph_deltas_regroup_by_vertex_in_target_order() {
        let delta = |vertex: u32, x: f32| MorphDelta::new(vertex, [x, 0.0, 0.0], [0.0, 0.0, x]);
        // Target 0 moves vertices 1 and 3, target 1 vertices 0 and 3, target 2 none.
        let deltas = [delta(1, 0.5), delta(3, 1.0), delta(0, 2.0), delta(3, 4.0)];
        let (offsets, regrouped) = morphs_by_vertex(5, [0..2, 2..4, 4..4].into_iter(), &deltas);
        assert_eq!(offsets, [0, 1, 2, 2, 4, 4]);
        let read = |bytes: [u8; 16]| {
            let delta = MorphDelta::from_bytes(bytes);
            (delta.vertex(), delta.position()[0], delta.normal()[2])
        };
        // The vertex field now holds the target.
        let found: Vec<_> = regrouped.into_iter().map(read).collect();
        assert_eq!(found, [(1, 2.0, 2.0), (0, 0.5, 0.5), (0, 1.0, 1.0), (1, 4.0, 4.0)]);
    }
}
