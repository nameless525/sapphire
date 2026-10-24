//! Cooked mesh format (`.smesh`).
//!
//! Layout, all little-endian (every Sapphire target is little-endian):
//!
//! ```text
//! MeshHeader
//! [MeshVertex; vertex_count]
//! [u32; index_count]
//! [Submesh; submesh_count]
//! [ToonMaterialParams; material_count]
//! ```
//!
//! This is the M0 format. It is read by copying into owned vectors; M1 replaces it
//! with archives that are memory-mapped and read in place (spec §5.9). Materials are
//! embedded per mesh until material assets exist.

use bytemuck::{Pod, Zeroable};

/// File magic: "SPMS" (Sapphire mesh).
pub const MESH_MAGIC: [u8; 4] = *b"SPMS";
/// Bump on any layout change; readers reject other versions.
pub const MESH_VERSION: u32 = 1;
/// Conventional file extension for cooked meshes.
pub const MESH_EXTENSION: &str = "smesh";

/// One vertex of a cooked mesh. 48 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct MeshVertex {
    /// Object-space position.
    pub position: [f32; 3],
    /// Object-space shading normal (may be split at hard edges).
    pub normal: [f32; 3],
    /// Object-space normal averaged over every vertex sharing this position.
    /// Inverted-hull outlines extrude along it so hard edges don't open gaps.
    pub outline_normal: [f32; 3],
    /// Texture coordinates.
    pub uv: [f32; 2],
    /// RGBA8 vertex color. RGB: albedo tint (sRGB). A: outline width scale, 255 = 1.0.
    pub color: [u8; 4],
}

/// A contiguous range of whole triangles drawn with one material.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct Submesh {
    /// First index of the range; a multiple of 3.
    pub first_index: u32,
    /// Number of indices; a multiple of 3.
    pub index_count: u32,
    /// Index into [`MeshAsset::materials`].
    pub material_index: u32,
    /// Must be zero.
    pub _reserved: u32,
}

/// Toon material parameters. Colors are linear RGBA. 64 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct ToonMaterialParams {
    /// Lit color.
    pub base_color: [f32; 4],
    /// Multiplier applied to `base_color` on the shadow side of the light split.
    pub shade_color: [f32; 4],
    /// Inverted-hull outline color.
    pub outline_color: [f32; 4],
    /// x: shadow threshold on N·L in [-1, 1]; y: split softness;
    /// z: rim-light strength; w: outline width in view-space units at 1 m.
    pub params: [f32; 4],
}

impl Default for ToonMaterialParams {
    fn default() -> Self {
        Self {
            base_color: [0.8, 0.8, 0.8, 1.0],
            shade_color: [0.55, 0.5, 0.65, 1.0],
            outline_color: [0.08, 0.06, 0.1, 1.0],
            params: [0.0, 0.02, 0.35, 0.004],
        }
    }
}

/// Fixed-size file header. 64 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct MeshHeader {
    /// [`MESH_MAGIC`].
    pub magic: [u8; 4],
    /// [`MESH_VERSION`].
    pub version: u32,
    /// Element counts of the arrays that follow the header, in file order.
    pub vertex_count: u32,
    pub index_count: u32,
    pub submesh_count: u32,
    pub material_count: u32,
    /// Must be zero, as must the two padding fields.
    pub _reserved: [u32; 2],
    /// Object-space bounds, as in [`MeshAsset::bounds`].
    pub bounds_min: [f32; 3],
    pub _pad0: f32,
    pub bounds_max: [f32; 3],
    pub _pad1: f32,
}

/// A cooked mesh held in memory.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshAsset {
    pub vertices: Vec<MeshVertex>,
    /// Triangle list indexing [`Self::vertices`].
    pub indices: Vec<u32>,
    pub submeshes: Vec<Submesh>,
    pub materials: Vec<ToonMaterialParams>,
    /// Object-space bounds in meters, `[min, max]`: finite, and `min <= max` on every axis.
    pub bounds: [[f32; 3]; 2],
}

// The file is read and written as raw little-endian bytes.
const _: () = assert!(cfg!(target_endian = "little"), "the .smesh format assumes a little-endian target");

/// Errors from reading or validating a cooked mesh.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum MeshFormatError {
    #[error("not a Sapphire mesh (bad magic)")]
    BadMagic,
    #[error("unsupported mesh version {found} (expected {MESH_VERSION})")]
    UnsupportedVersion { found: u32 },
    #[error("mesh data truncated: need {needed} bytes, have {available}")]
    Truncated { needed: usize, available: usize },
    #[error("{trailing} trailing bytes after mesh data")]
    TrailingBytes { trailing: usize },
    #[error("index {index} at position {position} is out of range for {vertex_count} vertices")]
    IndexOutOfRange { position: usize, index: u32, vertex_count: usize },
    #[error("index count {0} is not a multiple of 3")]
    NotTriangles(usize),
    #[error("submesh {submesh} covers indices outside 0..{index_count}")]
    SubmeshOutOfRange { submesh: usize, index_count: usize },
    #[error("submesh {submesh} references missing material {material}")]
    MissingMaterial { submesh: usize, material: u32 },
    #[error("submesh {submesh} does not cover whole triangles")]
    SubmeshNotTriangles { submesh: usize },
    #[error("vertex {vertex} has a non-finite attribute")]
    NonFiniteVertex { vertex: usize },
    #[error("material {material} has a non-finite parameter")]
    NonFiniteMaterial { material: usize },
    #[error("bounds are not finite or not ordered (min <= max)")]
    InvalidBounds,
    #[error("a reserved field is not zero")]
    ReservedNotZero,
}

impl MeshAsset {
    /// Checks every structural invariant the renderer relies on.
    pub fn validate(&self) -> Result<(), MeshFormatError> {
        if !self.indices.len().is_multiple_of(3) {
            return Err(MeshFormatError::NotTriangles(self.indices.len()));
        }
        let vertex_count = self.vertices.len();
        if let Some((position, &index)) =
            self.indices.iter().enumerate().find(|&(_, &i)| i as usize >= vertex_count)
        {
            return Err(MeshFormatError::IndexOutOfRange { position, index, vertex_count });
        }
        if let Some(vertex) = self.vertices.iter().position(|v| {
            !all_finite(&v.position)
                || !all_finite(&v.normal)
                || !all_finite(&v.outline_normal)
                || !all_finite(&v.uv)
        }) {
            return Err(MeshFormatError::NonFiniteVertex { vertex });
        }
        if let Some(material) = self.materials.iter().position(|m| {
            ![m.base_color, m.shade_color, m.outline_color, m.params].iter().all(|v| all_finite(v))
        }) {
            return Err(MeshFormatError::NonFiniteMaterial { material });
        }
        let [min, max] = self.bounds;
        if !all_finite(&min) || !all_finite(&max) || min.iter().zip(&max).any(|(lo, hi)| lo > hi) {
            return Err(MeshFormatError::InvalidBounds);
        }
        for (submesh, s) in self.submeshes.iter().enumerate() {
            if !s.first_index.is_multiple_of(3) || !s.index_count.is_multiple_of(3) {
                return Err(MeshFormatError::SubmeshNotTriangles { submesh });
            }
            let end = s.first_index as u64 + s.index_count as u64;
            if end > self.indices.len() as u64 {
                return Err(MeshFormatError::SubmeshOutOfRange { submesh, index_count: self.indices.len() });
            }
            if s.material_index as usize >= self.materials.len() {
                return Err(MeshFormatError::MissingMaterial { submesh, material: s.material_index });
            }
            if s._reserved != 0 {
                return Err(MeshFormatError::ReservedNotZero);
            }
        }
        Ok(())
    }

    /// Serializes to the `.smesh` byte layout.
    pub fn to_bytes(&self) -> Vec<u8> {
        let header = MeshHeader {
            magic: MESH_MAGIC,
            version: MESH_VERSION,
            vertex_count: to_u32(self.vertices.len()),
            index_count: to_u32(self.indices.len()),
            submesh_count: to_u32(self.submeshes.len()),
            material_count: to_u32(self.materials.len()),
            _reserved: [0; 2],
            bounds_min: self.bounds[0],
            _pad0: 0.0,
            bounds_max: self.bounds[1],
            _pad1: 0.0,
        };
        let mut out = Vec::with_capacity(
            size_of::<MeshHeader>()
                + size_of_val(self.vertices.as_slice())
                + size_of_val(self.indices.as_slice())
                + size_of_val(self.submeshes.as_slice())
                + size_of_val(self.materials.as_slice()),
        );
        out.extend_from_slice(bytemuck::bytes_of(&header));
        out.extend_from_slice(bytemuck::cast_slice(&self.vertices));
        out.extend_from_slice(bytemuck::cast_slice(&self.indices));
        out.extend_from_slice(bytemuck::cast_slice(&self.submeshes));
        out.extend_from_slice(bytemuck::cast_slice(&self.materials));
        out
    }

    /// Parses and validates a `.smesh` byte buffer. The input needs no particular alignment.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, MeshFormatError> {
        let mut reader = Reader { bytes, offset: 0 };
        let header: MeshHeader = reader.read_one()?;
        if header.magic != MESH_MAGIC {
            return Err(MeshFormatError::BadMagic);
        }
        if header.version != MESH_VERSION {
            return Err(MeshFormatError::UnsupportedVersion { found: header.version });
        }
        if header._reserved != [0; 2] || header._pad0.to_bits() != 0 || header._pad1.to_bits() != 0 {
            return Err(MeshFormatError::ReservedNotZero);
        }
        let mesh = Self {
            vertices: reader.read_vec(header.vertex_count)?,
            indices: reader.read_vec(header.index_count)?,
            submeshes: reader.read_vec(header.submesh_count)?,
            materials: reader.read_vec(header.material_count)?,
            bounds: [header.bounds_min, header.bounds_max],
        };
        let trailing = bytes.len() - reader.offset;
        if trailing != 0 {
            return Err(MeshFormatError::TrailingBytes { trailing });
        }
        mesh.validate()?;
        Ok(mesh)
    }
}

fn all_finite(values: &[f32]) -> bool {
    values.iter().all(|v| v.is_finite())
}

fn to_u32(len: usize) -> u32 {
    u32::try_from(len).expect("mesh element count exceeds u32::MAX")
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Reader<'_> {
    fn take(&mut self, len: usize) -> Result<&[u8], MeshFormatError> {
        let needed = self.offset.saturating_add(len);
        let slice = self
            .bytes
            .get(self.offset..needed)
            .ok_or(MeshFormatError::Truncated { needed, available: self.bytes.len() })?;
        self.offset = needed;
        Ok(slice)
    }

    fn read_one<T: Pod>(&mut self) -> Result<T, MeshFormatError> {
        Ok(bytemuck::pod_read_unaligned(self.take(size_of::<T>())?))
    }

    fn read_vec<T: Pod>(&mut self, count: u32) -> Result<Vec<T>, MeshFormatError> {
        let len = (count as usize).saturating_mul(size_of::<T>());
        Ok(bytemuck::pod_collect_to_vec(self.take(len)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle() -> MeshAsset {
        let v = |x: f32| MeshVertex {
            position: [x, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            outline_normal: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            color: [255; 4],
        };
        MeshAsset {
            vertices: vec![v(0.0), v(1.0), v(2.0)],
            indices: vec![0, 1, 2],
            submeshes: vec![Submesh { first_index: 0, index_count: 3, material_index: 0, _reserved: 0 }],
            materials: vec![ToonMaterialParams::default()],
            bounds: [[0.0; 3], [2.0, 0.0, 0.0]],
        }
    }

    #[test]
    fn layout_sizes_are_stable() {
        assert_eq!(size_of::<MeshVertex>(), 48);
        assert_eq!(size_of::<Submesh>(), 16);
        assert_eq!(size_of::<ToonMaterialParams>(), 64);
        assert_eq!(size_of::<MeshHeader>(), 64);
    }

    #[test]
    fn round_trips() {
        let mesh = triangle();
        assert_eq!(MeshAsset::from_bytes(&mesh.to_bytes()), Ok(mesh));
    }

    #[test]
    fn reads_unaligned_input() {
        let mut bytes = vec![0u8];
        bytes.extend(triangle().to_bytes());
        assert_eq!(MeshAsset::from_bytes(&bytes[1..]), Ok(triangle()));
    }

    #[test]
    fn rejects_corrupt_input() {
        let bytes = triangle().to_bytes();
        assert_eq!(
            MeshAsset::from_bytes(&bytes[..bytes.len() - 1]),
            Err(MeshFormatError::Truncated { needed: bytes.len(), available: bytes.len() - 1 })
        );

        let mut bad_magic = bytes.clone();
        bad_magic[0] = b'X';
        assert_eq!(MeshAsset::from_bytes(&bad_magic), Err(MeshFormatError::BadMagic));

        let mut bad_index = triangle();
        bad_index.indices[2] = 9;
        assert!(matches!(
            MeshAsset::from_bytes(&bad_index.to_bytes()),
            Err(MeshFormatError::IndexOutOfRange { index: 9, .. })
        ));

        let mut reserved = bytes.clone();
        reserved[24] = 1; // MeshHeader::_reserved[0]
        assert_eq!(MeshAsset::from_bytes(&reserved), Err(MeshFormatError::ReservedNotZero));
    }

    #[test]
    fn rejects_values_the_renderer_cannot_use() {
        let check = |edit: fn(&mut MeshAsset), expected: MeshFormatError| {
            let mut mesh = triangle();
            edit(&mut mesh);
            assert_eq!(mesh.validate(), Err(expected.clone()));
            assert_eq!(MeshAsset::from_bytes(&mesh.to_bytes()), Err(expected));
        };
        check(
            |m| m.submeshes[0] = Submesh { first_index: 1, index_count: 0, ..m.submeshes[0] },
            MeshFormatError::SubmeshNotTriangles { submesh: 0 },
        );
        check(|m| m.vertices[1].position[0] = f32::NAN, MeshFormatError::NonFiniteVertex { vertex: 1 });
        check(|m| m.vertices[2].uv[1] = f32::INFINITY, MeshFormatError::NonFiniteVertex { vertex: 2 });
        check(|m| m.materials[0].params[3] = f32::NAN, MeshFormatError::NonFiniteMaterial { material: 0 });
        check(|m| m.bounds = [[1.0; 3], [0.0; 3]], MeshFormatError::InvalidBounds);
        check(|m| m.bounds[1][2] = f32::NAN, MeshFormatError::InvalidBounds);
        check(|m| m.submeshes[0]._reserved = 7, MeshFormatError::ReservedNotZero);
    }
}
