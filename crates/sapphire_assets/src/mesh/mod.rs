//! Cooked meshes (`.smesh`): rkyv archives in `sapphire_serialize`'s cooked envelope, read in
//! place (spec §5.9).
//!
//! A [`Mesh`] holds quantized vertex streams, the index lists of a LOD chain, its material
//! slots and, for characters, a [`Skin`] and morph targets. The cooker (`sapphire_pipeline`)
//! writes it; the renderer uploads the archived streams byte for byte.
//!
//! # Material slots
//!
//! Submeshes index the mesh's [`MaterialSlot`]s, named after the source's materials. A slot
//! either embeds the material the importer generated from the source, or names a material
//! asset that the mesh's sidecar bound to it ([`SlotMaterial`]; why in
//! [`material`](crate::material)).
//!
//! # Vertex streams
//!
//! Vertices are split into three streams ([`VertexStreams`]), so a pass reads only what it
//! needs: shadow and depth passes read positions alone, and a skinning pass rewrites
//! positions and shading while the surface stream stays shared.
//!
//! | Stream | Bytes per vertex | Encoding | Error |
//! |---|---|---|---|
//! | positions | 8 | xyz unorm16 across [`VertexStreams::bounds`] ([`QuantizedPosition`]) | `extent / 131 070` per axis: 16 µm on a 2.1 m character |
//! | positions (alternative) | 12 | f32 xyz ([`FloatPosition`]) | none |
//! | shading | 8 | normal and outline normal, octahedral snorm16 × 2 each ([`VertexShading`]) | < 0.008° |
//! | surface | 8 | UV unorm16 × 2 across [`VertexStreams::uv_bounds`], vertex color RGB unorm8, [`VertexFlags`] u8 ([`VertexSurface`]) | `range / 131 070`: 0.016 texels of a 2K map |
//!
//! That is 24 bytes a vertex where M0 read 48. The element docs give the reasons for each
//! encoding. [`PositionFormat::Float32`] is for meshes whose extent makes 16 bits too coarse,
//! or whose edges must meet a neighbor's exactly (quantizing two meshes across different
//! bounds can open hairline cracks between them).
//!
//! # Vertex color and flags
//!
//! The anime character contract (M1 C1) gives the vertex color three data channels, linear
//! unorm8, each 1 when a source has no vertex color ([`VERTEX_COLOR_NEUTRAL`]):
//!
//! | Channel | Holds | Read by |
//! |---|---|---|
//! | R | Ambient occlusion: 1 open, 0 occluded. Pushes the light split toward shadow, as a cast shadow does, and darkens ambient light | the anime character shader |
//! | G | Outline-width scale on the material's width | the outline hull |
//! | B | Outline Z-offset scale on the material's Z offset | the outline hull |
//!
//! A per-vertex byte of [`VertexFlags`] follows: the vertex's material ID (the region it
//! shades with when its material has no control map), and reserved bits for faces and eyes.
//! Both share the surface stream's second word.
//!
//! # LODs
//!
//! - [`Mesh::lods`] lists LOD0 first, then each coarser level, as submesh index ranges into
//!   the shared [`Mesh::indices`]. Indices are mesh-relative.
//! - Every LOD reads a prefix of the streams, `0..vertex_count`: vertices are ordered by the
//!   coarsest LOD that uses them. A skinning pass for a distant character therefore skins
//!   only its LOD's prefix, and streaming can load prefixes.
//! - [`Lod::error`] is the simplifier's estimate of how far the LOD is from LOD0, as an
//!   object-space distance in meters, for LOD selection to project into pixels. It folds the
//!   weighted shading-normal error in with the geometric one, so it errs on the side of
//!   switching late. It never decreases along the chain. The mobile cook's LOD0 can itself be
//!   simplified (spec §5.27), so its error can be nonzero.
//!
//! # Skins and morph targets
//!
//! A skinned mesh's positions and normals are in bind space; [`Skin`] has the skeleton, the
//! skinning palette and per-vertex influences. Morph targets are sparse deltas per target
//! ([`MorphTarget`], [`MorphDelta`]), all applied before skinning.
//!
//! # Reading
//!
//! [`Mesh::read`] checks the envelope (type, schema version), validates the archive with
//! bytecheck, then checks the invariants a GPU upload relies on ([`MeshError`]): stream
//! lengths, every index inside its LOD's prefix, ranges, weights, finite values, no reserved
//! flag bits. Cooked bytes
//! must start 16-byte aligned, as memory maps and
//! [`AlignedVec`]s do.

mod encoding;
mod morph;
mod skin;
mod validate;

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
/// The buffer cooked meshes are written to, and must be read from: 16-byte aligned.
pub use sapphire_serialize::cooked::AlignedVec;
use sapphire_serialize::cooked::CookedError;
use serde::{Deserialize, Serialize};

pub use encoding::{
    FloatPosition, QuantizedPosition, VERTEX_COLOR_NEUTRAL, VertexFlags, VertexShading, VertexSurface,
    oct_decode, oct_encode,
};
pub use morph::{ArchivedMorphTarget, MorphDelta, MorphTarget};
pub use skin::{
    ArchivedInfluences, ArchivedJoint, ArchivedJointTransform, ArchivedSkin, ArchivedSkinBind, Influence8,
    Influence16, Influences, Joint, JointTransform, MAX_INFLUENCES, MAX_U8_PALETTE, Skin, SkinBind,
    VertexInfluence, quantize_weights,
};
pub use validate::MeshError;

use crate::AssetId;
use crate::material::Material;

/// Conventional file extension for cooked meshes.
pub const MESH_EXTENSION: &str = "smesh";

/// A cooked mesh. See the [module docs](self) for the layout.
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::assets"]
pub struct Mesh {
    pub streams: VertexStreams,
    /// Triangle lists of every LOD, mesh-relative: index 0 is the streams' first vertex.
    pub indices: Vec<u32>,
    /// LOD0 first; at least one.
    pub lods: Vec<Lod>,
    /// Indexed by [`Submesh::material`].
    pub materials: Vec<MaterialSlot>,
    pub skin: Option<Skin>,
    pub morph_targets: Vec<MorphTarget>,
    /// Every target's deltas, target after target.
    pub morph_deltas: Vec<MorphDelta>,
}

impl Versioned for Mesh {
    /// 6: embedded materials of `Material::VERSION` 3 (M1 C3's face settings and expression
    /// slot). 5: embedded materials pack the character light rig's region GI weights (M1 C2,
    /// `Material::VERSION` 2). 4: the anime character vertex contract (M1 C1): vertex color RGB
    /// is linear data (AO, outline width, outline Z offset) instead of an sRGB tint, and the
    /// outline-width scale moved from alpha to G, whose byte holds [`VertexFlags`] now. 3 added
    /// material slots (M1 A3); 2 embedded M0's fixed 64-byte material parameters; 1 was M0's
    /// flat `SPMS` layout of 48-byte float vertices.
    const VERSION: u32 = 6;
}

// Meshes embed generated materials, so a new material block layout or packing is a new mesh
// format too.
const _: () = assert!(
    Mesh::VERSION == 6 && Material::VERSION == 3,
    "Material::VERSION changed: bump Mesh::VERSION with it, then update this pair"
);

/// The packed vertex streams. Each holds one element per vertex.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct VertexStreams {
    /// Object-space bounds of every vertex, in meters. Quantized positions span them.
    pub bounds: Bounds,
    /// The range quantized UVs span.
    pub uv_bounds: UvBounds,
    pub positions: Positions,
    pub shading: Vec<VertexShading>,
    pub surface: Vec<VertexSurface>,
}

/// The position stream, in either [`PositionFormat`].
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum Positions {
    Quantized(Vec<QuantizedPosition>),
    Float(Vec<FloatPosition>),
}

/// How a mesh stores positions; an import setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PositionFormat {
    /// [`QuantizedPosition`]: 8 bytes, unorm16 across the bounds.
    #[default]
    Unorm16,
    /// [`FloatPosition`]: 12 bytes, f32.
    Float32,
}

impl PositionFormat {
    /// Every format, in a stable order.
    pub const ALL: [Self; 2] = [Self::Unorm16, Self::Float32];

    /// Bytes per vertex.
    pub const fn stride(self) -> usize {
        match self {
            Self::Unorm16 => size_of::<QuantizedPosition>(),
            Self::Float32 => size_of::<FloatPosition>(),
        }
    }
}

/// An axis-aligned box in object space, in meters.
#[derive(Clone, Copy, Debug, Default, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    /// The smallest box holding every point; all zero for none.
    pub fn enclosing(points: impl IntoIterator<Item = [f32; 3]>) -> Self {
        enclosing(points).map_or_else(Self::default, |(min, max)| Self { min, max })
    }

    /// Finite, `min <= max` on every axis, and every extent finite.
    pub fn is_valid(&self) -> bool {
        ordered(&self.min, &self.max)
    }
}

impl ArchivedBounds {
    pub fn to_native(&self) -> Bounds {
        Bounds { min: self.min.map(|v| v.to_native()), max: self.max.map(|v| v.to_native()) }
    }
}

/// The range of a mesh's texture coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct UvBounds {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl UvBounds {
    /// The smallest range holding every UV; all zero for none.
    pub fn enclosing(uvs: impl IntoIterator<Item = [f32; 2]>) -> Self {
        enclosing(uvs).map_or_else(Self::default, |(min, max)| Self { min, max })
    }

    /// Finite, `min <= max` on both axes, and both extents finite.
    pub fn is_valid(&self) -> bool {
        ordered(&self.min, &self.max)
    }
}

impl ArchivedUvBounds {
    pub fn to_native(&self) -> UvBounds {
        UvBounds { min: self.min.map(|v| v.to_native()), max: self.max.map(|v| v.to_native()) }
    }
}

/// One level of detail.
#[derive(Clone, Debug, Default, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Lod {
    /// The vertices this LOD reads: the prefix `0..vertex_count` of every stream. LOD0 reads
    /// them all.
    pub vertex_count: u32,
    /// How far this LOD may be from LOD0, in object-space meters (see the module docs).
    pub error: f32,
    /// Index ranges, at most one per material.
    pub submeshes: Vec<Submesh>,
}

/// A contiguous range of whole triangles drawn with one material.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Submesh {
    /// First index of the range; a multiple of 3.
    pub first_index: u32,
    /// Number of indices; a multiple of 3.
    pub index_count: u32,
    /// Index into [`Mesh::materials`].
    pub material: u32,
}

/// One of a mesh's materials, which its submeshes index.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct MaterialSlot {
    /// The source's name for the material (a glTF material's name, `#<index>` for an unnamed
    /// one, `#default` for glTF's default material). Sidecar bindings name slots by it.
    pub name: String,
    pub material: SlotMaterial,
}

/// Where a slot's material comes from.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum SlotMaterial {
    /// Generated by the importer from the source's own material, and cooked into the mesh.
    Embedded(Material),
    /// A material asset the mesh's sidecar bound to this slot.
    Asset(AssetId),
}

impl MaterialSlot {
    /// A slot embedding `material`.
    pub fn embedded(name: impl Into<String>, material: Material) -> Self {
        Self { name: name.into(), material: SlotMaterial::Embedded(material) }
    }
}

/// One vertex at full precision: what importers and procedural meshes produce, and what the
/// streams decode to. The default is all zero but for a neutral color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    /// Object-space position (bind space, for a skinned mesh).
    pub position: [f32; 3],
    /// Unit shading normal (may be split at hard edges).
    pub normal: [f32; 3],
    /// Unit normal averaged over every vertex sharing this position. Inverted-hull outlines
    /// extrude along it so hard edges don't open gaps.
    pub outline_normal: [f32; 3],
    pub uv: [f32; 2],
    /// Character data, linear (255 = 1.0): R ambient occlusion, G outline-width scale, B
    /// outline Z-offset scale (see the [module docs](self)).
    pub color: [u8; 3],
    pub flags: VertexFlags,
}

impl Default for Vertex {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            normal: [0.0; 3],
            outline_normal: [0.0; 3],
            uv: [0.0; 2],
            color: VERTEX_COLOR_NEUTRAL,
            flags: VertexFlags::NONE,
        }
    }
}

impl VertexStreams {
    /// Packs `vertices`, with bounds that fit them exactly.
    pub fn pack(vertices: &[Vertex], format: PositionFormat) -> Self {
        let bounds = Bounds::enclosing(vertices.iter().map(|v| v.position));
        let uv_bounds = UvBounds::enclosing(vertices.iter().map(|v| v.uv));
        let positions = match format {
            PositionFormat::Unorm16 => Positions::Quantized(
                vertices.iter().map(|v| QuantizedPosition::encode(v.position, &bounds)).collect(),
            ),
            PositionFormat::Float32 => {
                Positions::Float(vertices.iter().map(|v| FloatPosition::new(v.position)).collect())
            }
        };
        Self {
            bounds,
            uv_bounds,
            positions,
            shading: vertices.iter().map(|v| VertexShading::new(v.normal, v.outline_normal)).collect(),
            surface: vertices
                .iter()
                .map(|v| VertexSurface::new(v.uv, &uv_bounds, v.color, v.flags))
                .collect(),
        }
    }

    /// Vertices in the streams.
    pub fn len(&self) -> usize {
        match &self.positions {
            Positions::Quantized(positions) => positions.len(),
            Positions::Float(positions) => positions.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn position_format(&self) -> PositionFormat {
        match self.positions {
            Positions::Quantized(_) => PositionFormat::Unorm16,
            Positions::Float(_) => PositionFormat::Float32,
        }
    }

    /// Vertex `index`, decoded as the vertex shaders decode it.
    pub fn vertex(&self, index: usize) -> Option<Vertex> {
        let position = match &self.positions {
            Positions::Quantized(positions) => positions.get(index)?.decode(&self.bounds),
            Positions::Float(positions) => positions.get(index)?.get(),
        };
        let (shading, surface) = (self.shading.get(index)?, self.surface.get(index)?);
        Some(Vertex {
            position,
            normal: shading.normal(),
            outline_normal: shading.outline_normal(),
            uv: surface.uv(&self.uv_bounds),
            color: surface.color(),
            flags: surface.flags(),
        })
    }
}

impl ArchivedVertexStreams {
    /// Vertices in the streams.
    pub fn len(&self) -> usize {
        match &self.positions {
            ArchivedPositions::Quantized(positions) => positions.len(),
            ArchivedPositions::Float(positions) => positions.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn position_format(&self) -> PositionFormat {
        match self.positions {
            ArchivedPositions::Quantized(_) => PositionFormat::Unorm16,
            ArchivedPositions::Float(_) => PositionFormat::Float32,
        }
    }

    /// The position stream as the GPU reads it, [`PositionFormat::stride`] bytes a vertex.
    pub fn position_bytes(&self) -> &[u8] {
        match &self.positions {
            ArchivedPositions::Quantized(positions) => bytemuck::cast_slice(positions.as_slice()),
            ArchivedPositions::Float(positions) => bytemuck::cast_slice(positions.as_slice()),
        }
    }

    pub fn shading_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(self.shading.as_slice())
    }

    pub fn surface_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(self.surface.as_slice())
    }

    /// `(offset, scale)` such that an object-space position is `offset + scale × p`, with `p`
    /// the position as the GPU reads it: unorm in [0, 1] for [`PositionFormat::Unorm16`] (so
    /// the bounds' minimum and extent), the value itself for [`PositionFormat::Float32`].
    pub fn position_transform(&self) -> ([f32; 3], [f32; 3]) {
        match self.positions {
            ArchivedPositions::Quantized(_) => {
                let Bounds { min, max } = self.bounds.to_native();
                (min, std::array::from_fn(|axis| max[axis] - min[axis]))
            }
            ArchivedPositions::Float(_) => ([0.0; 3], [1.0; 3]),
        }
    }

    /// `(offset, scale)` such that a UV is `offset + scale × uv`, with `uv` the unorm values
    /// the GPU reads.
    pub fn uv_transform(&self) -> ([f32; 2], [f32; 2]) {
        let UvBounds { min, max } = self.uv_bounds.to_native();
        (min, std::array::from_fn(|axis| max[axis] - min[axis]))
    }
}

impl Mesh {
    /// A static mesh with a single LOD, packed from full-precision vertices: what procedural
    /// meshes and tests build. The cooker adds LODs, skins and morph targets.
    pub fn from_vertices(
        vertices: &[Vertex],
        indices: Vec<u32>,
        submeshes: Vec<Submesh>,
        materials: Vec<MaterialSlot>,
        format: PositionFormat,
    ) -> Self {
        let vertex_count = u32::try_from(vertices.len()).expect("a mesh's vertex indices fit u32");
        Self {
            streams: VertexStreams::pack(vertices, format),
            indices,
            lods: vec![Lod { vertex_count, error: 0.0, submeshes }],
            materials,
            skin: None,
            morph_targets: Vec::new(),
            morph_deltas: Vec::new(),
        }
    }

    /// The mesh as cooked bytes: the envelope header, then the archive.
    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked mesh bytes in place and checks them (see the [module docs](self)).
    pub fn read(bytes: &[u8]) -> Result<&ArchivedMesh, MeshError> {
        let mesh = sapphire_serialize::access::<Self>(bytes)?;
        mesh.validate()?;
        Ok(mesh)
    }

    /// [`Self::read`], then a copy out of the archive, for tools and tests.
    pub fn from_cooked(bytes: &[u8]) -> Result<Self, MeshError> {
        let archived = Self::read(bytes)?;
        rkyv::deserialize::<Self, rkyv::rancor::Error>(archived)
            .map_err(|error| MeshError::Cooked(error.into()))
    }
}

impl ArchivedMesh {
    /// The index list of every LOD as the GPU reads it: little-endian u32.
    pub fn index_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(self.indices.as_slice())
    }
}

/// The componentwise minimum and maximum of `values`, or `None` for none.
fn enclosing<const N: usize>(values: impl IntoIterator<Item = [f32; N]>) -> Option<([f32; N], [f32; N])> {
    values.into_iter().fold(None, |bounds, value| {
        let (min, max) = bounds.unwrap_or((value, value));
        Some((std::array::from_fn(|i| min[i].min(value[i])), std::array::from_fn(|i| max[i].max(value[i]))))
    })
}

/// Finite, ordered, and with a finite extent on every axis (the decode scales by it).
fn ordered<const N: usize>(min: &[f32; N], max: &[f32; N]) -> bool {
    min.iter().zip(max).all(|(lo, hi)| lo <= hi && (hi - lo).is_finite() && lo.is_finite())
}

#[cfg(test)]
mod tests;
