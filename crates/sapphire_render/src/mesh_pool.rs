//! The mesh pool (spec §5.12): every mesh's vertex streams live in shared stream buffers, its
//! indices in one shared index buffer, and its materials in one storage buffer indexed by
//! material ID.
//!
//! Uploads copy the cooked streams byte for byte: the renderer reads the quantized encodings
//! of `sapphire_assets::mesh` directly, and its vertex shaders decode them with the per-mesh
//! dequantization each draw record carries. The streams are kept apart (positions, shading,
//! surface), so a pass binds only the ones it reads.
//!
//! Meshes are grouped into one slab per position format, each a set of stream buffers with its
//! own vertex numbering. Within a slab every stream has one element per vertex, which is what
//! the GPU-driven path (later) needs: one `base_vertex` addresses all of a draw's streams.
//!
//! Indices stay mesh-relative, exactly as cooked. A draw locates its mesh's vertices by
//! binding the stream buffers at the mesh's first vertex (the CPU-driven path, which needs no
//! base-vertex support and so runs on the iOS Simulator) or, on the future GPU-driven path,
//! through `base_vertex`. Relative indices also let compute skinning (M1 R2) write each
//! instance's vertices to buffers of its own, and the pool compact, without rewriting indices.
//!
//! Every LOD's indices are uploaded with the mesh and its index ranges recorded; draws use
//! LOD0 until LOD selection lands with culling. Skin and morph data are not uploaded: skinned
//! meshes draw in bind pose until compute skinning (M1 R2) uploads them (see the crate docs).

use sapphire_assets::mesh::{ArchivedMesh, PositionFormat, ToonMaterialParams, VertexShading, VertexSurface};
use sapphire_rhi::wgpu;

use crate::RenderError;
use crate::buffer::GrowableBuffer;
use crate::gpu_types::Dequantize;
use crate::pipelines::format_index;

const SHADING_BYTES: u64 = size_of::<VertexShading>() as u64;
const SURFACE_BYTES: u64 = size_of::<VertexSurface>() as u64;
const INDEX_BYTES: u64 = size_of::<u32>() as u64;
const MATERIAL_BYTES: u64 = size_of::<ToonMaterialParams>() as u64;

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

/// A submesh with its indices and material resolved to pool-global positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SubmeshDraw {
    pub first_index: u32,
    pub index_count: u32,
    pub material: u32,
}

/// Where a mesh lives in the pool.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MeshRecord {
    /// The slab holding its streams.
    pub format: PositionFormat,
    /// Slab position of the mesh's vertex 0, which its indices are relative to.
    pub first_vertex: u32,
    pub dequantize: Dequantize,
    /// Per LOD, LOD0 first: its submeshes.
    pub lods: Vec<Vec<SubmeshDraw>>,
}

/// The stream buffers of one position format.
#[derive(Debug)]
pub(crate) struct Slab {
    format: PositionFormat,
    positions: GrowableBuffer,
    shading: GrowableBuffer,
    surface: GrowableBuffer,
    vertex_count: u32,
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
            vertex_count: 0,
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
        u64::from(self.vertex_count) * (self.position_stride() + SHADING_BYTES + SURFACE_BYTES)
    }

    fn growths(&self) -> u32 {
        self.positions.growths() + self.shading.growths() + self.surface.growths()
    }
}

#[derive(Debug)]
pub(crate) struct MeshPool {
    /// Per [`PositionFormat::ALL`] entry.
    slabs: [Slab; 2],
    indices: GrowableBuffer,
    materials: GrowableBuffer,
    index_count: u32,
    material_count: u32,
    records: Vec<MeshRecord>,
}

impl MeshPool {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let limits = device.limits();
        Self {
            slabs: PositionFormat::ALL.map(|format| {
                let vertices = if format == PositionFormat::Unorm16 { INITIAL_QUANTIZED_VERTICES } else { 0 };
                Slab::new(device, format, vertices)
            }),
            indices: GrowableBuffer::new(
                device,
                "mesh pool indices",
                wgpu::BufferUsages::INDEX,
                1 << 19,
                limits.max_buffer_size,
            ),
            materials: GrowableBuffer::new(
                device,
                "materials",
                wgpu::BufferUsages::STORAGE,
                64 * MATERIAL_BYTES,
                limits.max_storage_buffer_binding_size.min(limits.max_buffer_size),
            ),
            index_count: 0,
            material_count: 0,
            records: Vec::new(),
        }
    }

    /// Appends a mesh and its materials. Returns the handle and whether the material buffer
    /// was replaced (bind groups that reference it must be rebuilt).
    pub(crate) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &ArchivedMesh,
    ) -> Result<(MeshHandle, bool), RenderError> {
        mesh.validate()?;
        let streams = &mesh.streams;
        let vertices = streams.len();
        if vertices > MAX_MESH_VERTICES {
            return Err(RenderError::MeshTooLarge { vertices });
        }
        let grow = |current: u32, added: usize, what| {
            u32::try_from(added)
                .ok()
                .and_then(|added| current.checked_add(added))
                .ok_or(RenderError::MeshPoolFull(what))
        };
        let slab = &mut self.slabs[format_index(streams.position_format())];
        let vertex_count = grow(slab.vertex_count, vertices, "vertices")?;
        let index_count = grow(self.index_count, mesh.indices.len(), "indices")?;
        let material_count = grow(self.material_count, mesh.materials.len(), "materials")?;
        let handle =
            MeshHandle(u32::try_from(self.records.len()).map_err(|_| RenderError::MeshPoolFull("meshes"))?);

        let fits = |buffer: &GrowableBuffer, bytes: u64, what| {
            if bytes <= buffer.max_size() { Ok(()) } else { Err(RenderError::MeshPoolFull(what)) }
        };
        let streams_bytes = [slab.position_stride(), SHADING_BYTES, SURFACE_BYTES];
        let stream_buffers = [&slab.positions, &slab.shading, &slab.surface];
        for (buffer, stride) in stream_buffers.into_iter().zip(streams_bytes) {
            fits(buffer, u64::from(vertex_count) * stride, "vertices")?;
        }
        fits(&self.indices, u64::from(index_count) * INDEX_BYTES, "indices")?;
        fits(&self.materials, u64::from(material_count) * MATERIAL_BYTES, "materials")?;

        let first_vertex = u64::from(slab.vertex_count);
        let stream_data = [streams.position_bytes(), streams.shading_bytes(), streams.surface_bytes()];
        for ((buffer, stride), bytes) in [&mut slab.positions, &mut slab.shading, &mut slab.surface]
            .into_iter()
            .zip(streams_bytes)
            .zip(stream_data)
        {
            let offset = first_vertex * stride;
            buffer.reserve(device, queue, u64::from(vertex_count) * stride, offset);
            write(queue, buffer.buffer(), offset, bytes);
        }

        let index_offset = u64::from(self.index_count) * INDEX_BYTES;
        self.indices.reserve(device, queue, u64::from(index_count) * INDEX_BYTES, index_offset);
        write(queue, self.indices.buffer(), index_offset, mesh.index_bytes());

        let material_offset = u64::from(self.material_count) * MATERIAL_BYTES;
        let materials_moved = self.materials.reserve(
            device,
            queue,
            u64::from(material_count) * MATERIAL_BYTES,
            material_offset,
        );
        // Materials are a few 64-byte entries; converting them out of the archive (whose floats
        // are little-endian wrappers) costs nothing worth avoiding.
        let materials: Vec<ToonMaterialParams> = mesh.materials.iter().map(|m| m.to_native()).collect();
        write(queue, self.materials.buffer(), material_offset, bytemuck::cast_slice(&materials));

        let (position_offset, position_scale) = streams.position_transform();
        let (uv_offset, uv_scale) = streams.uv_transform();
        self.records.push(MeshRecord {
            format: streams.position_format(),
            first_vertex: slab.vertex_count,
            dequantize: Dequantize {
                position_offset,
                position_scale,
                uv_transform: [uv_offset[0], uv_offset[1], uv_scale[0], uv_scale[1]],
            },
            lods: mesh
                .lods
                .iter()
                .map(|lod| {
                    lod.submeshes
                        .iter()
                        .map(|s| SubmeshDraw {
                            first_index: self.index_count + s.first_index.to_native(),
                            index_count: s.index_count.to_native(),
                            material: self.material_count + s.material.to_native(),
                        })
                        .collect()
                })
                .collect(),
        });
        slab.vertex_count = vertex_count;
        self.index_count = index_count;
        self.material_count = material_count;
        Ok((handle, materials_moved))
    }

    pub(crate) fn records(&self) -> &[MeshRecord] {
        &self.records
    }

    pub(crate) fn slab(&self, format: PositionFormat) -> &Slab {
        &self.slabs[format_index(format)]
    }

    pub(crate) fn index_buffer(&self) -> &wgpu::Buffer {
        self.indices.buffer()
    }

    pub(crate) fn material_buffer(&self) -> &wgpu::Buffer {
        self.materials.buffer()
    }

    /// Bytes in use: vertex streams, indices and materials.
    pub(crate) fn bytes_used(&self) -> u64 {
        self.slabs.iter().map(Slab::bytes_used).sum::<u64>()
            + u64::from(self.index_count) * INDEX_BYTES
            + u64::from(self.material_count) * MATERIAL_BYTES
    }

    /// Times any pool buffer grew.
    pub(crate) fn growths(&self) -> u32 {
        self.slabs.iter().map(Slab::growths).sum::<u32>() + self.indices.growths() + self.materials.growths()
    }
}

fn write(queue: &wgpu::Queue, buffer: &wgpu::Buffer, offset: u64, bytes: &[u8]) {
    if !bytes.is_empty() {
        queue.write_buffer(buffer, offset, bytes);
    }
}
