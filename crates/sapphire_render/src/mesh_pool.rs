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
//! Each buffer hands out ranges first fit ([`RangeAllocator`]): a mesh replaced in place (hot
//! reload, live link: [`MeshPool::replace`]) frees its old ranges for later uploads, under the
//! same handle. Meshes are not unloaded yet (see the crate docs' later milestones).
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
    ArchivedInfluences, ArchivedMesh, ArchivedSlotMaterial, MorphDelta, PositionFormat, VertexShading,
    VertexSurface,
};
use sapphire_rhi::wgpu;

use crate::RenderError;
use crate::buffer::{GrowableBuffer, RangeAllocator};
use crate::gpu_types::Dequantize;
use crate::materials::MaterialHandle;
use crate::pipelines::format_index;

const SHADING_BYTES: u64 = size_of::<VertexShading>() as u64;
const SURFACE_BYTES: u64 = size_of::<VertexSurface>() as u64;
const INDEX_BYTES: u64 = size_of::<u32>() as u64;

/// Vulkan devices without `fullDrawIndexUint32` (some Vulkan 1.1 phones) only guarantee
/// index values up to 2^24 − 1, which caps a single mesh (not the pool).
const MAX_MESH_VERTICES: usize = 1 << 24;

/// Initial room in the quantized slab, in vertices: 768 KiB over its three streams. f32 meshes
/// are the exception, so their slab starts empty and grows on first use.
const INITIAL_QUANTIZED_VERTICES: u64 = 1 << 15;

/// A mesh uploaded with [`crate::Renderer::upload_mesh`]. Only meaningful to the renderer
/// that returned it. Meshes are never unloaded in M0; residency arrives with the asset
/// runtime in M1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshHandle(u32);

impl MeshHandle {
    /// A handle from its raw index (the order meshes were uploaded in), for code that builds
    /// packets without a renderer, such as extraction tests. Drawing a handle the renderer
    /// never returned is a logic error: debug builds assert, release builds skip the instance.
    pub const fn from_raw(index: u32) -> Self {
        Self(index)
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
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
}

/// Where a mesh's vertex streams are.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MeshStreams {
    /// A static mesh, in the slab of `format`: vertex 0 is the slab's `first_vertex`.
    Slab { format: PositionFormat, first_vertex: u32 },
    /// A skinned or morphed mesh, in the deformable buffer (see the module docs).
    Deformable(DeformSource),
}

impl MeshRecord {
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
    records: Vec<MeshRecord>,
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
            records: Vec::new(),
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
        let handle =
            MeshHandle(u32::try_from(self.records.len()).map_err(|_| RenderError::MeshPoolFull("meshes"))?);
        let (record, moves) = self.place(device, queue, mesh, materials)?;
        self.records.push(record);
        Ok((handle, moves))
    }

    /// Replaces mesh `handle` with `mesh`, whose material slots draw with `materials`, under the
    /// same handle: the new streams and indices are written, then the old ranges freed for later
    /// uploads. Nothing changes if it fails. GPU work already submitted read the old ranges
    /// first, since queue writes land before the next submission.
    pub(crate) fn replace(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        handle: MeshHandle,
        mesh: &ArchivedMesh,
        materials: &[SubmeshMaterial],
    ) -> Result<PoolMoves, RenderError> {
        let (record, moves) = self.place(device, queue, mesh, materials)?;
        let old = std::mem::replace(&mut self.records[handle.index()], record);
        self.release(&old);
        Ok(moves)
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
        let deformable = mesh.skin.is_some() || !mesh.morph_targets.is_empty();
        let (position_offset, position_scale) = streams.position_transform();
        let (uv_offset, uv_scale) = streams.uv_transform();
        let uv_transform = [uv_offset[0], uv_offset[1], uv_scale[0], uv_scale[1]];
        let region = deformable.then(|| deformable_region(mesh));

        // Room first: the ranges are taken back if any buffer can't grow to hold them.
        let index_len = count(mesh.indices.len(), "indices")?;
        let vertex_len = match &region {
            Some((bytes, _)) => count(bytes.len() / 4, "deformable mesh sources")?,
            None => count(vertices, "vertices")?,
        };
        let format = streams.position_format();
        let slab = format_index(format);
        let vertices_end_before = match region {
            Some(_) => self.deformable_words.end(),
            None => self.slabs[slab].vertices.end(),
        };
        let indices_end_before = self.index_ranges.end();
        let Some((first_vertex, first_index)) = self.allocate(region.is_some(), slab, vertex_len, index_len)
        else {
            return Err(RenderError::MeshPoolFull("vertices or indices"));
        };

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
            placement: Placement { vertices: (first_vertex, vertex_len), indices: (first_index, index_len) },
            slots: mesh
                .materials
                .iter()
                .zip(materials)
                .map(|(slot, &material)| SlotRecord {
                    material,
                    embedded: matches!(slot.material, ArchivedSlotMaterial::Embedded(_)),
                })
                .collect(),
        };
        Ok((record, moves))
    }

    /// Takes `vertex_len` vertices (words of the deformable buffer if `deformable`, else of slab
    /// `slab`) and `index_len` indices, if the buffers can grow to hold them. Returns the first of
    /// each.
    fn allocate(
        &mut self,
        deformable: bool,
        slab: usize,
        vertex_len: u32,
        index_len: u32,
    ) -> Option<(u32, u32)> {
        let first_index = self.index_ranges.try_allocate(index_len)?;
        let first_vertex = if deformable {
            self.deformable_words.try_allocate(vertex_len)
        } else {
            self.slabs[slab].vertices.try_allocate(vertex_len)
        };
        if let Some(first_vertex) = first_vertex {
            if self.fits(deformable, slab) {
                return Some((first_vertex, first_index));
            }
            if deformable {
                self.deformable_words.free(first_vertex, vertex_len);
            } else {
                self.slabs[slab].vertices.free(first_vertex, vertex_len);
            }
        }
        self.index_ranges.free(first_index, index_len);
        None
    }

    /// Whether the index buffer, and the deformable buffer or slab `slab`'s streams, can grow to
    /// hold every range handed out.
    fn fits(&self, deformable: bool, slab: usize) -> bool {
        let indices = u64::from(self.index_ranges.end()) * INDEX_BYTES <= self.indices.max_size();
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

    /// Frees the ranges `record` occupied.
    fn release(&mut self, record: &MeshRecord) {
        let Placement { vertices: (first_vertex, vertex_len), indices: (first_index, index_len) } =
            record.placement;
        match record.streams {
            MeshStreams::Slab { format, .. } => {
                let slab = &mut self.slabs[format_index(format)];
                slab.vertices.free(first_vertex, vertex_len);
                slab.used -= vertex_len;
            }
            MeshStreams::Deformable(_) => {
                self.deformable_words.free(first_vertex, vertex_len);
                self.deformable_used -= vertex_len;
            }
        }
        self.index_ranges.free(first_index, index_len);
        self.indices_used -= index_len;
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

    /// Bytes in use: vertex streams (with skin and morph data) and indices.
    pub(crate) fn bytes_used(&self) -> u64 {
        self.slabs.iter().map(Slab::bytes_used).sum::<u64>()
            + u64::from(self.deformable_used) * 4
            + u64::from(self.indices_used) * INDEX_BYTES
    }

    /// Times any pool buffer grew.
    pub(crate) fn growths(&self) -> u32 {
        self.slabs.iter().map(Slab::growths).sum::<u32>() + self.deformable.growths() + self.indices.growths()
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
