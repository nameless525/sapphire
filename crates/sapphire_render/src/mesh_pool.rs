//! The mesh pool (spec §5.12): every mesh lives in one shared vertex buffer and one shared
//! index buffer, and every material in one storage buffer indexed by material ID. Draws never
//! rebind geometry, which is also what multi-draw indirect needs later.
//!
//! Indices are rebased to absolute pool positions at upload, so every draw uses
//! `base_vertex = 0`. That keeps the CPU-driven path portable to devices without base-vertex
//! drawing, notably the iOS Simulator (see the crate docs).

use sapphire_assets::mesh::{MeshAsset, MeshVertex, ToonMaterialParams};
use sapphire_rhi::wgpu;

use crate::RenderError;
use crate::buffer::GrowableBuffer;

const VERTEX_BYTES: u64 = size_of::<MeshVertex>() as u64;
const INDEX_BYTES: u64 = size_of::<u32>() as u64;
const MATERIAL_BYTES: u64 = size_of::<ToonMaterialParams>() as u64;

/// Rebased indices are absolute, so the pool stays below 2^24 vertices: Vulkan devices without
/// `fullDrawIndexUint32` (some Vulkan 1.1 phones) only guarantee index values up to 2^24 − 1.
/// The 256 MiB buffer-size floor already caps the pool at about 5.6 M vertices.
const MAX_POOL_VERTICES: u32 = 1 << 24;

/// A mesh uploaded with [`crate::Renderer::upload_mesh`]. Only meaningful to the renderer
/// that returned it. Meshes are never unloaded in M0; residency arrives with the asset
/// runtime in M1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshHandle(u32);

impl MeshHandle {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }

    #[cfg(test)]
    pub(crate) fn from_index(index: u32) -> Self {
        Self(index)
    }
}

/// A submesh with its indices and material resolved to pool-global positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SubmeshDraw {
    pub first_index: u32,
    pub index_count: u32,
    pub material: u32,
}

/// Where a mesh lives in the pool. Its indices are already absolute, so no base vertex.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MeshRecord {
    pub submeshes: Vec<SubmeshDraw>,
}

#[derive(Debug)]
pub(crate) struct MeshPool {
    vertices: GrowableBuffer,
    indices: GrowableBuffer,
    materials: GrowableBuffer,
    vertex_count: u32,
    index_count: u32,
    material_count: u32,
    records: Vec<MeshRecord>,
}

impl MeshPool {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let limits = device.limits();
        Self {
            vertices: GrowableBuffer::new(
                device,
                "mesh pool vertices",
                wgpu::BufferUsages::VERTEX,
                1 << 20,
                limits.max_buffer_size,
            ),
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
            vertex_count: 0,
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
        mesh: &MeshAsset,
    ) -> Result<(MeshHandle, bool), RenderError> {
        mesh.validate()?;
        let grow = |current: u32, added: usize, what| {
            u32::try_from(added)
                .ok()
                .and_then(|added| current.checked_add(added))
                .ok_or(RenderError::MeshPoolFull(what))
        };
        let vertex_count = grow(self.vertex_count, mesh.vertices.len(), "vertices")?;
        if vertex_count > MAX_POOL_VERTICES {
            return Err(RenderError::MeshPoolFull("vertices"));
        }
        let index_count = grow(self.index_count, mesh.indices.len(), "indices")?;
        let material_count = grow(self.material_count, mesh.materials.len(), "materials")?;
        let handle =
            MeshHandle(u32::try_from(self.records.len()).map_err(|_| RenderError::MeshPoolFull("meshes"))?);

        let fits = |buffer: &GrowableBuffer, bytes: u64, what| {
            if bytes <= buffer.max_size() { Ok(()) } else { Err(RenderError::MeshPoolFull(what)) }
        };
        fits(&self.vertices, u64::from(vertex_count) * VERTEX_BYTES, "vertices")?;
        fits(&self.indices, u64::from(index_count) * INDEX_BYTES, "indices")?;
        fits(&self.materials, u64::from(material_count) * MATERIAL_BYTES, "materials")?;

        let vertex_offset = u64::from(self.vertex_count) * VERTEX_BYTES;
        let index_offset = u64::from(self.index_count) * INDEX_BYTES;
        let material_offset = u64::from(self.material_count) * MATERIAL_BYTES;
        self.vertices.reserve(device, queue, u64::from(vertex_count) * VERTEX_BYTES, vertex_offset);
        self.indices.reserve(device, queue, u64::from(index_count) * INDEX_BYTES, index_offset);
        let materials_moved = self.materials.reserve(
            device,
            queue,
            u64::from(material_count) * MATERIAL_BYTES,
            material_offset,
        );

        write(queue, self.vertices.buffer(), vertex_offset, bytemuck::cast_slice(&mesh.vertices));
        // Validation guarantees every index is below the mesh's vertex count, and the pool
        // total is below MAX_POOL_VERTICES, so the addition can't overflow.
        let first_vertex = self.vertex_count;
        let rebased: Vec<u32> = mesh.indices.iter().map(|&index| index + first_vertex).collect();
        write(queue, self.indices.buffer(), index_offset, bytemuck::cast_slice(&rebased));
        write(queue, self.materials.buffer(), material_offset, bytemuck::cast_slice(&mesh.materials));

        self.records.push(MeshRecord {
            submeshes: mesh
                .submeshes
                .iter()
                .map(|s| SubmeshDraw {
                    first_index: self.index_count + s.first_index,
                    index_count: s.index_count,
                    material: self.material_count + s.material_index,
                })
                .collect(),
        });
        self.vertex_count = vertex_count;
        self.index_count = index_count;
        self.material_count = material_count;
        Ok((handle, materials_moved))
    }

    pub(crate) fn records(&self) -> &[MeshRecord] {
        &self.records
    }

    pub(crate) fn vertex_buffer(&self) -> &wgpu::Buffer {
        self.vertices.buffer()
    }

    pub(crate) fn index_buffer(&self) -> &wgpu::Buffer {
        self.indices.buffer()
    }

    pub(crate) fn material_buffer(&self) -> &wgpu::Buffer {
        self.materials.buffer()
    }
}

fn write(queue: &wgpu::Queue, buffer: &wgpu::Buffer, offset: u64, bytes: &[u8]) {
    if !bytes.is_empty() {
        queue.write_buffer(buffer, offset, bytes);
    }
}
